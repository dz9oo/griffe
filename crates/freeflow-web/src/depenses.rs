//! Routes de mutation `depenses` (lot 21) — même patron et même convention de réponse que
//! `clients` (voir le commentaire de tête de `crate::clients`) : succès → `200` vide +
//! `HX-Trigger: freeflow:saved`, échec → `200` avec le panneau re-rendu. Toute action ici est un
//! acte humain direct (`AppState::human_ctx()`).
//!
//! Seule différence avec les autres écrans : les formulaires de création et de modification
//! sont en `multipart/form-data` (lot 29), pour le champ fichier du justificatif. Le fichier
//! reçu est archivé par [`freeflow_cli::archive_receipt_bytes`] — la même IO d'adaptateur que
//! `freeflow expense record --receipt`, jamais réimplémentée ici — *avant* de construire la
//! commande du cœur, qui ne reçoit que le hash et le nom archivé (voir `CLAUDE.md`, « les
//! `Command` ne touchent que `&Connection` »). Le corps du protocole `freeflow://` arrive
//! entier en mémoire (voir `freeflow-desktop/src/main.rs`) : le multipart y passe comme
//! n'importe quel `POST`, sans socket ni fichier temporaire.

use axum::Form;
use axum::extract::{DefaultBodyLimit, Multipart, Path, Query, State};
use axum::http::HeaderValue;
use axum::response::{Html, IntoResponse, Response};
use freeflow_core::app::{AppError, Executor, Outcome};
use freeflow_core::billing::{self, bank_transaction_by_id};
use freeflow_core::domain::{BankTransactionId, ExpenseCategory, ExpenseId, Money, VatRate};
use freeflow_core::expenses::{self, ExpenseDetail};
use maud::html;
use serde::Deserialize;

use crate::state::AppState;
use crate::views;
use crate::views::depenses::{ExpenseFormErrors, ExpenseFormValues};

fn locked_fragment() -> Html<String> {
    Html(
        html! { div class="empty-state" { "coffre verrouillé — rechargez la page" } }.into_string(),
    )
}

fn message_fragment(message: &str) -> Html<String> {
    Html(html! { div class="empty-state" { (message) } }.into_string())
}

fn saved() -> Response {
    let mut response = Html(String::new()).into_response();
    response
        .headers_mut()
        .insert("HX-Trigger", HeaderValue::from_static("freeflow:saved"));
    response
}

async fn execute<C: freeflow_core::app::Command>(
    state: &AppState,
    cmd: C,
) -> Option<Result<Outcome<C::Output>, AppError>> {
    state
        .with_store_mut(|store| Executor::new(store).execute(&cmd, &AppState::human_ctx()))
        .await
}

fn parse_id<T: std::str::FromStr>(raw: &str) -> Option<T> {
    raw.parse().ok()
}

/// Taille maximale d'un corps `multipart` sur les routes de saisie de dépense : au-delà de la
/// limite par défaut d'axum (2 Mio), trop juste pour une facture fournisseur scannée. Une
/// borne reste nécessaire — le corps entier est tenu en mémoire — et 32 Mio couvre largement
/// un PDF ou une photo de note de frais sans permettre d'épuiser la mémoire de la fenêtre.
const EXPENSE_FORM_BODY_LIMIT: usize = 32 * 1024 * 1024;

/// Pose la limite de corps ci-dessus sur une route de saisie de dépense.
pub fn body_limit() -> DefaultBodyLimit {
    DefaultBodyLimit::max(EXPENSE_FORM_BODY_LIMIT)
}

/// Champs texte du formulaire de dépense, tels que lus par [`read_multipart_form`] — plus de
/// `axum::Form`/`serde_urlencoded` ici depuis le passage en multipart.
#[derive(Debug, Default)]
pub struct ExpenseForm {
    revision: Option<String>,
    label: String,
    category: String,
    amount: String,
    vat_rate: String,
    vat_deductible: String,
    incurred_on: String,
    /// Nom du justificatif actuellement archivé — affiché par le panneau d'édition, jamais lu
    /// pour persister quoi que ce soit (l'état de référence est toujours relu dans le coffre).
    current_receipt: Option<String>,
    /// Case « détacher le justificatif » (`on` quand cochée) — l'équivalent de
    /// `expense edit --clear-receipt` : détache sans toucher au fichier archivé.
    clear_receipt: bool,
    /// Débit du relevé que la dépense créée paie (création seulement, lot 33) — l'équivalent
    /// de `expense record --transaction`.
    bank_transaction_id: Option<String>,
}

/// Fichier reçu dans le champ `receipt` du formulaire, tel quel, avant archivage.
struct UploadedReceipt {
    filename: String,
    content: Vec<u8>,
}

/// Lit un formulaire `multipart/form-data` : les champs texte remplissent un [`ExpenseForm`],
/// le champ fichier `receipt` (s'il porte un nom et un contenu — un `<input type="file">` laissé
/// vide arrive comme une partie vide) devient un [`UploadedReceipt`].
async fn read_multipart_form(
    mut multipart: Multipart,
) -> Result<(ExpenseForm, Option<UploadedReceipt>), String> {
    let mut form = ExpenseForm::default();
    let mut receipt = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| format!("formulaire illisible : {e}"))?
    {
        let name = field.name().unwrap_or_default().to_string();
        if name == "receipt" {
            let filename = field.file_name().unwrap_or_default().to_string();
            let content = field
                .bytes()
                .await
                .map_err(|e| format!("justificatif illisible : {e}"))?;
            if !filename.is_empty() && !content.is_empty() {
                receipt = Some(UploadedReceipt {
                    filename,
                    content: content.to_vec(),
                });
            }
            continue;
        }
        let value = field
            .text()
            .await
            .map_err(|e| format!("champ « {name} » illisible : {e}"))?;
        match name.as_str() {
            "revision" => form.revision = Some(value),
            "label" => form.label = value,
            "category" => form.category = value,
            "amount" => form.amount = value,
            "vat_rate" => form.vat_rate = value,
            "vat_deductible" => form.vat_deductible = value,
            "incurred_on" => form.incurred_on = value,
            "current_receipt" => form.current_receipt = Some(value).filter(|v| !v.is_empty()),
            "clear_receipt" => form.clear_receipt = value == "on" || value == "true",
            "bank_transaction_id" => {
                form.bank_transaction_id = Some(value).filter(|v| !v.is_empty());
            }
            // Un champ inconnu est une soumission forgée ou un formulaire d'une autre version :
            // ignoré, les validations de champ feront le reste.
            _ => {}
        }
    }
    Ok((form, receipt))
}

/// Archive le justificatif reçu à côté du coffre et renvoie `(hash, nom archivé)` — ou une
/// erreur de bandeau si le disque refuse. IO bloquante, comme tout accès au `Store` dans ces
/// handlers (transport en mémoire, une requête à la fois).
fn archive_uploaded(
    state: &AppState,
    receipt: &UploadedReceipt,
) -> Result<(Option<String>, Option<String>), String> {
    let archived =
        freeflow_cli::archive_receipt_bytes(state.db_path(), &receipt.filename, &receipt.content)?;
    Ok((Some(archived.hash), Some(archived.filename)))
}

impl From<&ExpenseForm> for ExpenseFormValues {
    fn from(f: &ExpenseForm) -> Self {
        Self {
            label: f.label.clone(),
            category: f.category.clone(),
            amount: f.amount.clone(),
            vat_rate: f.vat_rate.clone(),
            vat_deductible: f.vat_deductible.clone(),
            incurred_on: f.incurred_on.clone(),
            current_receipt: f.current_receipt.clone(),
            bank_transaction_id: f.bank_transaction_id.clone(),
            bank_transaction_note: None,
        }
    }
}

/// Les valeurs d'un formulaire de création à re-rendre, avec le résumé du débit rapproché relu
/// dans le coffre (la note n'est qu'un affichage, jamais un champ soumis).
async fn create_form_values(state: &AppState, form: &ExpenseForm) -> ExpenseFormValues {
    let mut values: ExpenseFormValues = form.into();
    if let Some(id) = form
        .bank_transaction_id
        .as_deref()
        .and_then(parse_id::<BankTransactionId>)
    {
        let debit = state
            .with_store(|store| bank_transaction_by_id(store.connection(), id))
            .await;
        if let Some(Ok(Some(tx))) = debit {
            values.bank_transaction_note =
                views::depenses::form_values_from_debit(&tx).bank_transaction_note;
        }
    }
    values
}

struct ParsedExpenseForm {
    label: String,
    category: ExpenseCategory,
    amount: Money,
    vat_rate: VatRate,
    vat_deductible: Money,
    incurred_on: time::Date,
}

/// Validations attribuables à un champ précis (montants, date, libellé) — au-delà (TVA
/// déductible > montant, exercice clôturé, conflit de révision), les erreurs du cœur restent un
/// bandeau, même doctrine que `clients`.
fn parse_expense_form(form: &ExpenseForm) -> Result<ParsedExpenseForm, Box<ExpenseFormErrors>> {
    let mut errors = ExpenseFormErrors::default();

    if form.label.trim().is_empty() {
        errors.label = Some("le libellé est obligatoire".to_string());
    }
    let amount = match Money::parse_decimal(form.amount.trim()) {
        Ok(m) => Some(m),
        Err(e) => {
            errors.amount = Some(e.to_string());
            None
        }
    };
    let vat_deductible = match Money::parse_decimal(form.vat_deductible.trim()) {
        Ok(m) => Some(m),
        Err(e) => {
            errors.vat_deductible = Some(e.to_string());
            None
        }
    };
    let incurred_on = match freeflow_core::domain::parse_date(form.incurred_on.trim()) {
        Ok(d) => Some(d),
        Err(e) => {
            errors.incurred_on = Some(e.to_string());
            None
        }
    };
    // Les `<select>` ne proposent que des valeurs valides : un échec ici est une soumission
    // forgée, un bandeau suffit.
    let category = form.category.parse::<ExpenseCategory>();
    let vat_rate = form.vat_rate.parse::<VatRate>();
    if category.is_err() || vat_rate.is_err() {
        errors.banner = Some("catégorie ou taux de TVA invalide".to_string());
    }

    match (
        amount,
        vat_deductible,
        incurred_on,
        category,
        vat_rate,
        errors.label.is_none(),
    ) {
        (
            Some(amount),
            Some(vat_deductible),
            Some(incurred_on),
            Ok(category),
            Ok(vat_rate),
            true,
        ) if errors.banner.is_none() => Ok(ParsedExpenseForm {
            label: form.label.trim().to_string(),
            category,
            amount,
            vat_rate,
            vat_deductible,
            incurred_on,
        }),
        _ => Err(Box::new(errors)),
    }
}

fn expense_error_banner(e: AppError, reload_hx_get: &str) -> ExpenseFormErrors {
    match e {
        AppError::Conflict { .. } => ExpenseFormErrors {
            conflict: Some((e.to_string(), reload_hx_get.to_string())),
            ..Default::default()
        },
        other => ExpenseFormErrors {
            banner: Some(other.to_string()),
            ..Default::default()
        },
    }
}

async fn current_expense(
    state: &AppState,
    id: ExpenseId,
) -> Option<Result<Option<ExpenseDetail>, AppError>> {
    state
        .with_store(|store| views::depenses::load(store, id))
        .await
}

// -- Liste ---------------------------------------------------------------------------------

pub async fn table(State(state): State<AppState>) -> Html<String> {
    match state.with_store(views::depenses::list_fragment).await {
        None => locked_fragment(),
        Some(Ok(markup)) => Html(markup.into_string()),
        Some(Err(e)) => message_fragment(&e.to_string()),
    }
}

// -- Créer / afficher / modifier / supprimer -----------------------------------------------

#[derive(Debug, Deserialize)]
pub struct NewQuery {
    /// Débit du relevé dont pré-remplir la dépense (lot 33).
    transaction: Option<String>,
}

pub async fn new_panel(
    State(state): State<AppState>,
    Query(query): Query<NewQuery>,
) -> Html<String> {
    let today = time::OffsetDateTime::now_utc().date();
    let Some(transaction) = query.transaction.as_deref().filter(|t| !t.is_empty()) else {
        return Html(
            views::depenses::new_panel(
                &views::depenses::default_form_values(today),
                &ExpenseFormErrors::default(),
            )
            .into_string(),
        );
    };
    let Some(id) = parse_id::<BankTransactionId>(transaction) else {
        return message_fragment("identifiant de transaction invalide");
    };
    match state
        .with_store(|store| bank_transaction_by_id(store.connection(), id))
        .await
    {
        None => locked_fragment(),
        Some(Err(e)) => message_fragment(&e.to_string()),
        Some(Ok(None)) => message_fragment("transaction bancaire introuvable"),
        Some(Ok(Some(tx))) if !tx.is_debit() || tx.is_matched() => {
            message_fragment("cette transaction n'est pas un débit à rapprocher")
        }
        Some(Ok(Some(tx))) => Html(
            views::depenses::new_panel(
                &views::depenses::form_values_from_debit(&tx),
                &ExpenseFormErrors::default(),
            )
            .into_string(),
        ),
    }
}

pub async fn create(State(state): State<AppState>, multipart: Multipart) -> Response {
    let (form, receipt) = match read_multipart_form(multipart).await {
        Ok(read) => read,
        Err(message) => {
            let errors = ExpenseFormErrors {
                banner: Some(message),
                ..Default::default()
            };
            return Html(
                views::depenses::new_panel(&ExpenseFormValues::default(), &errors).into_string(),
            )
            .into_response();
        }
    };
    let parsed = match parse_expense_form(&form) {
        Ok(p) => p,
        Err(errors) => {
            let values = create_form_values(&state, &form).await;
            return Html(views::depenses::new_panel(&values, &errors).into_string())
                .into_response();
        }
    };
    // Un id de débit forgé est un bandeau, pas une panique — le cœur revérifie tout de toute
    // façon (débit, montant exact, non rapproché).
    let bank_transaction_id = match form.bank_transaction_id.as_deref() {
        None => None,
        Some(raw) => match parse_id::<BankTransactionId>(raw) {
            Some(id) => Some(id),
            None => {
                let errors = ExpenseFormErrors {
                    banner: Some("identifiant de transaction bancaire invalide".to_string()),
                    ..Default::default()
                };
                return Html(views::depenses::new_panel(&(&form).into(), &errors).into_string())
                    .into_response();
            }
        },
    };
    // Archivage *après* validation des champs (un formulaire refusé ne laisse pas de fichier
    // orphelin dans `receipts/`) et *avant* la commande, comme la CLI.
    let (receipt_hash, receipt_filename) = match &receipt {
        Some(uploaded) => match archive_uploaded(&state, uploaded) {
            Ok(archived) => archived,
            Err(message) => {
                let errors = ExpenseFormErrors {
                    banner: Some(message),
                    ..Default::default()
                };
                let values = create_form_values(&state, &form).await;
                return Html(views::depenses::new_panel(&values, &errors).into_string())
                    .into_response();
            }
        },
        None => (None, None),
    };
    let cmd = expenses::RecordExpense {
        label: parsed.label,
        category: parsed.category,
        amount: parsed.amount,
        vat_rate: parsed.vat_rate,
        vat_deductible: parsed.vat_deductible,
        incurred_on: parsed.incurred_on,
        receipt_hash,
        receipt_filename,
        bank_transaction_id,
    };
    match execute(&state, cmd).await {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => saved(),
        Some(Err(e)) => {
            let errors = expense_error_banner(e, "/depenses/new");
            let values = create_form_values(&state, &form).await;
            Html(views::depenses::new_panel(&values, &errors).into_string()).into_response()
        }
    }
}

pub async fn show_panel(State(state): State<AppState>, Path(id): Path<String>) -> Html<String> {
    let Some(id) = parse_id::<ExpenseId>(&id) else {
        return message_fragment("identifiant de dépense invalide");
    };
    match current_expense(&state, id).await {
        None => locked_fragment(),
        Some(Err(e)) => message_fragment(&e.to_string()),
        Some(Ok(None)) => {
            message_fragment("dépense introuvable — elle a peut-être été supprimée entre-temps")
        }
        Some(Ok(Some(detail))) => Html(views::depenses::detail_panel(&detail, None).into_string()),
    }
}

pub async fn edit_panel(State(state): State<AppState>, Path(id): Path<String>) -> Html<String> {
    let Some(id) = parse_id::<ExpenseId>(&id) else {
        return message_fragment("identifiant de dépense invalide");
    };
    match current_expense(&state, id).await {
        None => locked_fragment(),
        Some(Err(e)) => message_fragment(&e.to_string()),
        Some(Ok(None)) => message_fragment("dépense introuvable"),
        Some(Ok(Some(detail))) => Html(
            views::depenses::edit_panel(
                id,
                detail.expense.revision,
                &(&detail.expense).into(),
                &ExpenseFormErrors::default(),
            )
            .into_string(),
        ),
    }
}

pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<String>,
    multipart: Multipart,
) -> Response {
    let Some(id) = parse_id::<ExpenseId>(&id) else {
        return message_fragment("identifiant de dépense invalide").into_response();
    };
    let (form, receipt) = match read_multipart_form(multipart).await {
        Ok(read) => read,
        Err(message) => {
            let errors = ExpenseFormErrors {
                banner: Some(message),
                ..Default::default()
            };
            return Html(
                views::depenses::edit_panel(id, 0, &ExpenseFormValues::default(), &errors)
                    .into_string(),
            )
            .into_response();
        }
    };
    let revision: i64 = form
        .revision
        .as_deref()
        .and_then(|s| s.parse().ok())
        .unwrap_or_default();
    let parsed = match parse_expense_form(&form) {
        Ok(p) => p,
        Err(errors) => {
            return Html(
                views::depenses::edit_panel(id, revision, &(&form).into(), &errors).into_string(),
            )
            .into_response();
        }
    };
    // Même logique de justificatif que `expense edit` en CLI : la case « détacher » vide les
    // deux champs sans toucher au fichier archivé ; un fichier reçu remplace l'existant ; sinon
    // le justificatif actuel, relu dans le coffre (jamais depuis le formulaire), voyage tel quel
    // dans l'état complet.
    let current = match current_expense(&state, id).await {
        None => return locked_fragment().into_response(),
        Some(Err(e)) => return message_fragment(&e.to_string()).into_response(),
        Some(Ok(None)) => return message_fragment("dépense introuvable").into_response(),
        Some(Ok(Some(detail))) => detail.expense,
    };
    let (receipt_hash, receipt_filename) = if form.clear_receipt {
        (None, None)
    } else if let Some(uploaded) = &receipt {
        match archive_uploaded(&state, uploaded) {
            Ok(archived) => archived,
            Err(message) => {
                let errors = ExpenseFormErrors {
                    banner: Some(message),
                    ..Default::default()
                };
                return Html(
                    views::depenses::edit_panel(id, revision, &(&form).into(), &errors)
                        .into_string(),
                )
                .into_response();
            }
        }
    } else {
        (current.receipt_hash, current.receipt_filename)
    };
    let cmd = expenses::UpdateExpense {
        id,
        revision,
        label: parsed.label,
        category: parsed.category,
        amount: parsed.amount,
        vat_rate: parsed.vat_rate,
        vat_deductible: parsed.vat_deductible,
        incurred_on: parsed.incurred_on,
        receipt_hash,
        receipt_filename,
    };
    match execute(&state, cmd).await {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => saved(),
        Some(Err(e)) => {
            let errors = expense_error_banner(e, &format!("/depenses/{id}/edit"));
            Html(views::depenses::edit_panel(id, revision, &(&form).into(), &errors).into_string())
                .into_response()
        }
    }
}

pub async fn delete_confirm_panel(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Html<String> {
    let Some(id) = parse_id::<ExpenseId>(&id) else {
        return message_fragment("identifiant de dépense invalide");
    };
    match current_expense(&state, id).await {
        None => locked_fragment(),
        Some(Err(e)) => message_fragment(&e.to_string()),
        Some(Ok(None)) => message_fragment("dépense introuvable"),
        Some(Ok(Some(detail))) => {
            Html(views::depenses::delete_confirm_panel(&detail.expense).into_string())
        }
    }
}

pub async fn delete(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let Some(id) = parse_id::<ExpenseId>(&id) else {
        return message_fragment("identifiant de dépense invalide").into_response();
    };
    let revision = match current_expense(&state, id).await {
        None => return locked_fragment().into_response(),
        Some(Err(e)) => return message_fragment(&e.to_string()).into_response(),
        Some(Ok(None)) => {
            return message_fragment("dépense introuvable — déjà supprimée").into_response();
        }
        Some(Ok(Some(detail))) => detail.expense.revision,
    };
    match execute(&state, expenses::DeleteExpense { id, revision }).await {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => saved(),
        Some(Err(e)) => detail_with_error(&state, id, &e.to_string()).await,
    }
}

/// Re-rend la fiche avec un bandeau d'erreur — pour les actions déclenchées depuis la fiche.
async fn detail_with_error(state: &AppState, id: ExpenseId, error: &str) -> Response {
    match current_expense(state, id).await {
        Some(Ok(Some(detail))) => {
            Html(views::depenses::detail_panel(&detail, Some(error)).into_string()).into_response()
        }
        _ => message_fragment(error).into_response(),
    }
}

// -- Rapprochement bancaire (lot 33) ---------------------------------------------------------

pub async fn reconcile_panel(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Html<String> {
    let Some(id) = parse_id::<ExpenseId>(&id) else {
        return message_fragment("identifiant de dépense invalide");
    };
    let loaded = state
        .with_store(|store| {
            let Some(detail) = views::depenses::load(store, id)? else {
                return Ok(None);
            };
            let candidates = views::depenses::reconcile_candidates(store, detail.expense.amount)?;
            Ok::<_, AppError>(Some((detail, candidates)))
        })
        .await;
    match loaded {
        None => locked_fragment(),
        Some(Err(e)) => message_fragment(&e.to_string()),
        Some(Ok(None)) => message_fragment("dépense introuvable"),
        Some(Ok(Some((detail, _)))) if detail.bank_transaction.is_some() => {
            Html(views::depenses::detail_panel(&detail, Some("déjà rapprochée")).into_string())
        }
        Some(Ok(Some((detail, candidates)))) => {
            Html(views::depenses::reconcile_panel(&detail.expense, &candidates, None).into_string())
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ReconcileForm {
    transaction: String,
}

pub async fn reconcile(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(form): Form<ReconcileForm>,
) -> Response {
    let Some(id) = parse_id::<ExpenseId>(&id) else {
        return message_fragment("identifiant de dépense invalide").into_response();
    };
    let Some(transaction_id) = parse_id::<BankTransactionId>(&form.transaction) else {
        return message_fragment("identifiant de transaction invalide").into_response();
    };
    let cmd = expenses::ReconcileExpense {
        transaction_id,
        expense_id: id,
    };
    match execute(&state, cmd).await {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => saved(),
        Some(Err(e)) => detail_with_error(&state, id, &e.to_string()).await,
    }
}

/// Défait le rapprochement de la dépense — `billing::UnreconcileTransaction` sur son débit :
/// la dépense reste, le débit redevient « à rapprocher ».
pub async fn unreconcile(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let Some(id) = parse_id::<ExpenseId>(&id) else {
        return message_fragment("identifiant de dépense invalide").into_response();
    };
    let transaction_id = match current_expense(&state, id).await {
        None => return locked_fragment().into_response(),
        Some(Err(e)) => return message_fragment(&e.to_string()).into_response(),
        Some(Ok(None)) => return message_fragment("dépense introuvable").into_response(),
        Some(Ok(Some(detail))) => match detail.bank_transaction {
            Some(tx) => tx.id,
            None => {
                return detail_with_error(&state, id, "cette dépense n'est pas rapprochée").await;
            }
        },
    };
    match execute(&state, billing::UnreconcileTransaction { transaction_id }).await {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => saved(),
        Some(Err(e)) => detail_with_error(&state, id, &e.to_string()).await,
    }
}
