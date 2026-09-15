//! Routes de mutation `depenses` (lot 21) — même patron et même convention de réponse que
//! `clients` (voir le commentaire de tête de `crate::clients`) : succès → `200` vide +
//! `HX-Trigger: freeflow:saved`, échec → `200` avec le panneau re-rendu. Toute action ici est un
//! acte humain direct (`AppState::human_ctx()`).
//!
//! Seule différence avec les autres écrans : les formulaires de création et de modification
//! sont en `multipart/form-data` (lot 29), pour le champ fichier du justificatif. Le fichier
//! reçu est archivé chiffré par `freeflow_core::receipts::archive` — la même IO d'adaptateur que
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
use freeflow_core::domain::{
    AccountCode, BankTransactionId, ExpenseCategory, ExpenseId, FixedAssetId, Money, VatRate,
};
use freeflow_core::expenses::{self, ExpenseDetail};
use freeflow_core::fixed_assets::{self, AddFixedAsset, DeleteFixedAsset};
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

fn saved_panel(markup: maud::Markup) -> Response {
    let mut response = Html(markup.into_string()).into_response();
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
    /// Bénéficiaire (fournisseur), lot 41 — vide = aucun.
    supplier: String,
    /// Nom du justificatif actuellement archivé — affiché par le panneau d'édition, jamais lu
    /// pour persister quoi que ce soit (l'état de référence est toujours relu dans le coffre).
    current_receipt: Option<String>,
    /// Case « détacher le justificatif » (`on` quand cochée) — l'équivalent de
    /// `expense edit --clear-receipt` : détache sans toucher au fichier archivé.
    clear_receipt: bool,
    /// Débit du relevé que la dépense créée paie (création seulement, lot 33) — l'équivalent
    /// de `expense record --transaction`.
    bank_transaction_id: Option<String>,
    paid_by: Option<String>,
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
            "supplier" => form.supplier = value,
            "current_receipt" => form.current_receipt = Some(value).filter(|v| !v.is_empty()),
            "clear_receipt" => form.clear_receipt = value == "on" || value == "true",
            "bank_transaction_id" => {
                form.bank_transaction_id = Some(value).filter(|v| !v.is_empty());
            }
            "paid_by" => form.paid_by = Some(value).filter(|v| !v.is_empty()),
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
async fn archive_uploaded(
    state: &AppState,
    receipt: &UploadedReceipt,
) -> Result<(Option<String>, Option<String>), String> {
    // Lot 39 : chiffré sous une clé dérivée de celle du coffre, dans `<coffre>.receipts/` —
    // la même implémentation que la CLI et le serveur MCP (`freeflow_core::receipts`).
    let archived = state
        .with_store(|store| {
            freeflow_core::receipts::archive(store, &receipt.filename, &receipt.content)
                .map_err(|e| e.to_string())
        })
        .await
        .ok_or_else(|| "coffre verrouillé — rechargez la page".to_string())??;
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
            supplier: f.supplier.clone(),
            current_receipt: f.current_receipt.clone(),
            bank_transaction_id: f.bank_transaction_id.clone(),
            bank_transaction_note: None,
            paid_by: f.paid_by.clone().unwrap_or_default(),
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
    supplier: Option<String>,
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
            supplier: Some(form.supplier.trim().to_string()).filter(|s| !s.is_empty()),
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
    let today = state.today();
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
        Some(uploaded) => match archive_uploaded(&state, uploaded).await {
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
    let captured_receipt = receipt_filename
        .as_deref()
        .zip(receipt_hash.as_deref())
        .map(|(f, h)| (f.to_string(), h.to_string()));
    let paid_by = if bank_transaction_id.is_some() {
        freeflow_core::domain::ExpensePaidBy::Company
    } else {
        match form.paid_by.as_deref() {
            Some("me") => freeflow_core::domain::ExpensePaidBy::Associate,
            Some("company") => freeflow_core::domain::ExpensePaidBy::Company,
            _ => {
                let errors = ExpenseFormErrors {
                    paid_by: Some(
                        "Dis qui a payé. Si ça n'est pas sur le relevé de la société, c'est probablement toi."
                            .into(),
                    ),
                    ..Default::default()
                };
                let values = create_form_values(&state, &form).await;
                return Html(views::depenses::new_panel(&values, &errors).into_string())
                    .into_response();
            }
        }
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
        supplier: parsed.supplier,
        paid_by,
    };
    match execute(&state, cmd).await {
        None => locked_fragment().into_response(),
        Some(Ok(Outcome::Applied(id))) => {
            if let Some((filename, hash)) = captured_receipt.as_ref() {
                let _ = state
                    .with_store_mut(|store| {
                        freeflow_cli::capture_expense_receipt(
                            store,
                            &AppState::human_ctx(),
                            id,
                            filename,
                            hash,
                        )
                    })
                    .await;
            }
            if paid_by == freeflow_core::domain::ExpensePaidBy::Associate {
                saved_panel(views::depenses::advanced_panel(parsed.amount))
            } else {
                saved()
            }
        }
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
        Some(Ok(Some(detail))) => {
            let asset = expense_asset(&state, id).await;
            Html(views::depenses::detail_panel(&detail, asset.as_ref(), None).into_string())
        }
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
        match archive_uploaded(&state, uploaded).await {
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
    let new_receipt = receipt.is_some();
    let captured_receipt = receipt_filename
        .as_deref()
        .zip(receipt_hash.as_deref())
        .map(|(f, h)| (f.to_string(), h.to_string()));
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
        supplier: parsed.supplier,
        paid_by: match form.paid_by.as_deref() {
            Some("me") => freeflow_core::domain::ExpensePaidBy::Associate,
            _ => freeflow_core::domain::ExpensePaidBy::Company,
        },
    };
    match execute(&state, cmd).await {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => {
            if new_receipt && let Some((filename, hash)) = captured_receipt.as_ref() {
                let _ = state
                    .with_store_mut(|store| {
                        freeflow_cli::capture_expense_receipt(
                            store,
                            &AppState::human_ctx(),
                            id,
                            filename.as_str(),
                            hash.as_str(),
                        )
                    })
                    .await;
            }
            saved()
        }
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
        Some(Err(e)) => detail_with_error(&state, id, &views::errors::message(&e)).await,
    }
}

/// Re-rend la fiche avec un bandeau d'erreur — pour les actions déclenchées depuis la fiche.
async fn detail_with_error(state: &AppState, id: ExpenseId, error: &str) -> Response {
    match current_expense(state, id).await {
        Some(Ok(Some(detail))) => {
            let asset = expense_asset(state, id).await;
            Html(views::depenses::detail_panel(&detail, asset.as_ref(), Some(error)).into_string())
                .into_response()
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
            let candidates = views::depenses::reconcile_candidates(store, &detail.expense)?;
            Ok::<_, AppError>(Some((detail, candidates)))
        })
        .await;
    match loaded {
        None => locked_fragment(),
        Some(Err(e)) => message_fragment(&e.to_string()),
        Some(Ok(None)) => message_fragment("dépense introuvable"),
        Some(Ok(Some((detail, _)))) if detail.bank_transaction.is_some() => Html(
            views::depenses::detail_panel(&detail, None, Some("déjà rapprochée")).into_string(),
        ),
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
        Some(Err(e)) => detail_with_error(&state, id, &views::errors::message(&e)).await,
    }
}

// -- Règlement d'un compte de bilan depuis le relevé (lot 37) ---------------------------------

async fn current_transaction(
    state: &AppState,
    id: freeflow_core::domain::BankTransactionId,
) -> Option<Result<Option<freeflow_core::domain::BankTransaction>, AppError>> {
    state
        .with_store(|store| billing::bank_transaction_by_id(store.connection(), id))
        .await
}

/// `GET /depenses/transaction/{id}/settle` : le panneau de règlement d'un mouvement.
pub async fn settle_panel(State(state): State<AppState>, Path(id): Path<String>) -> Html<String> {
    let Some(id) = parse_id::<freeflow_core::domain::BankTransactionId>(&id) else {
        return message_fragment("identifiant de transaction invalide");
    };
    match current_transaction(&state, id).await {
        None => locked_fragment(),
        Some(Err(e)) => message_fragment(&e.to_string()),
        Some(Ok(None)) => message_fragment("transaction introuvable"),
        Some(Ok(Some(tx))) => Html(
            views::depenses::settle_panel(
                &tx,
                &views::depenses::SettleFormValues {
                    account: "401000".to_string(),
                    ..Default::default()
                },
                None,
            )
            .into_string(),
        ),
    }
}

#[derive(Debug, Deserialize)]
pub struct SettleForm {
    #[serde(default)]
    account: String,
    #[serde(default)]
    other_account: String,
    #[serde(default)]
    label: String,
}

/// `POST /depenses/transaction/{id}/settle` : `billing::SettleBankTransaction` — les gardes
/// (compte de bilan hors 512, transaction libre, libellé requis hors plan) sont celles du
/// cœur, re-rendues dans le panneau en cas de refus.
pub async fn settle(
    State(state): State<AppState>,
    Path(id): Path<String>,
    axum::Form(form): axum::Form<SettleForm>,
) -> Response {
    let Some(id) = parse_id::<freeflow_core::domain::BankTransactionId>(&id) else {
        return message_fragment("identifiant de transaction invalide").into_response();
    };
    let tx = match current_transaction(&state, id).await {
        None => return locked_fragment().into_response(),
        Some(Err(e)) => return message_fragment(&e.to_string()).into_response(),
        Some(Ok(None)) => return message_fragment("transaction introuvable").into_response(),
        Some(Ok(Some(tx))) => tx,
    };
    let values = views::depenses::SettleFormValues {
        account: form.account.clone(),
        other_account: form.other_account.clone(),
        label: form.label.clone(),
    };
    let chosen = if form.account.trim().is_empty() {
        form.other_account.trim()
    } else {
        form.account.trim()
    };
    let account = match chosen.parse::<freeflow_core::domain::SettlementAccount>() {
        Ok(a) => a,
        Err(e) => {
            return Html(
                views::depenses::settle_panel(&tx, &values, Some(&e.to_string())).into_string(),
            )
            .into_response();
        }
    };
    let label = Some(form.label.trim().to_string()).filter(|l| !l.is_empty());
    let cmd = billing::SettleBankTransaction {
        transaction_id: id,
        account,
        label,
    };
    match execute(&state, cmd).await {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => saved(),
        Some(Err(e)) => {
            Html(views::depenses::settle_panel(&tx, &values, Some(&e.to_string())).into_string())
                .into_response()
        }
    }
}

/// `POST /depenses/transaction/{id}/unsettle` : défait un règlement, la transaction redevient
/// « à rapprocher ».
pub async fn unsettle(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let Some(id) = parse_id::<freeflow_core::domain::BankTransactionId>(&id) else {
        return message_fragment("identifiant de transaction invalide").into_response();
    };
    match execute(
        &state,
        billing::UnsettleBankTransaction { transaction_id: id },
    )
    .await
    {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => saved(),
        Some(Err(e)) => message_fragment(&e.to_string()).into_response(),
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
        Some(Err(e)) => detail_with_error(&state, id, &views::errors::message(&e)).await,
    }
}

/// `GET /depenses/{id}/receipt` : le justificatif déchiffré à la volée (lot 39) — les pièces
/// vivent chiffrées dans `<coffre>.receipts/`, la fenêtre est le seul visualiseur qui n'a pas
/// besoin d'un fichier temporaire.
pub async fn receipt(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let Some(id) = parse_id::<ExpenseId>(&id) else {
        return message_fragment("identifiant de dépense invalide").into_response();
    };
    let loaded = state
        .with_store(|store| -> Result<Option<(String, Vec<u8>)>, String> {
            let Some(detail) = views::depenses::load(store, id).map_err(|e| e.to_string())? else {
                return Ok(None);
            };
            let Some(filename) = detail.expense.receipt_filename else {
                return Ok(None);
            };
            let bytes =
                freeflow_core::receipts::read(store, &filename).map_err(|e| e.to_string())?;
            Ok(Some((filename, bytes)))
        })
        .await;
    match loaded {
        None => locked_fragment().into_response(),
        Some(Err(e)) => message_fragment(&e).into_response(),
        Some(Ok(None)) => message_fragment("aucun justificatif pour cette dépense").into_response(),
        Some(Ok(Some((filename, bytes)))) => {
            let original = filename
                .split_once('-')
                .map_or(filename.as_str(), |(_, n)| n)
                .to_string();
            let content_type = match original.rsplit('.').next().map(str::to_ascii_lowercase) {
                Some(ext) if ext == "pdf" => "application/pdf",
                Some(ext) if ext == "png" => "image/png",
                Some(ext) if ext == "jpg" || ext == "jpeg" => "image/jpeg",
                Some(ext) if ext == "webp" => "image/webp",
                _ => "application/octet-stream",
            };
            let mut response = bytes.into_response();
            response.headers_mut().insert(
                axum::http::header::CONTENT_TYPE,
                HeaderValue::from_static(content_type),
            );
            if let Ok(disposition) = HeaderValue::from_str(&format!(
                "inline; filename=\"{}\"",
                original.replace('"', "")
            )) {
                response
                    .headers_mut()
                    .insert(axum::http::header::CONTENT_DISPOSITION, disposition);
            }
            response
        }
    }
}

async fn expense_asset(
    state: &AppState,
    id: ExpenseId,
) -> Option<freeflow_core::domain::FixedAsset> {
    state
        .with_store(|store| fixed_assets::asset_for_expense(store.connection(), id))
        .await
        .and_then(Result::ok)
        .flatten()
}

#[derive(Debug, Deserialize)]
pub struct ImmobilizeForm {
    account: String,
    duration: String,
}

pub async fn immobilize_panel(
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
            Html(views::depenses::immobilize_panel(&detail.expense, None).into_string())
        }
    }
}

pub async fn immobilize(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(form): Form<ImmobilizeForm>,
) -> Response {
    let Some(id) = parse_id::<ExpenseId>(&id) else {
        return message_fragment("identifiant de dépense invalide").into_response();
    };
    let Some(Ok(Some(detail))) = current_expense(&state, id).await else {
        return message_fragment("dépense introuvable").into_response();
    };
    let expense = &detail.expense;
    let account = match form.account.parse::<AccountCode>() {
        Ok(a) => a,
        Err(e) => {
            return Html(
                views::depenses::immobilize_panel(expense, Some(&e.to_string())).into_string(),
            )
            .into_response();
        }
    };
    let duration = form.duration.parse::<u32>().unwrap_or(36);
    let cmd = AddFixedAsset {
        label: expense.label.clone(),
        account,
        acquired_on: expense.incurred_on,
        base: fixed_assets::expense_net(expense),
        duration_months: duration,
        prior_depreciation: Money::ZERO,
        expense_id: Some(id),
    };
    match execute(&state, cmd).await {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => saved(),
        Some(Err(e)) => Html(
            views::depenses::immobilize_panel(expense, Some(&views::errors::message(&e)))
                .into_string(),
        )
        .into_response(),
    }
}

pub async fn asset_panel(State(state): State<AppState>, Path(id): Path<String>) -> Html<String> {
    let Some(id) = parse_id::<FixedAssetId>(&id) else {
        return message_fragment("identifiant d'immobilisation invalide");
    };
    let today = state.today();
    match state
        .with_store(|store| {
            let asset = fixed_assets::fixed_asset_by_id(store.connection(), id)?;
            let fye = freeflow_core::company::company_profile(store.connection())?
                .and_then(|p| p.fiscal_year_end)
                .unwrap_or(freeflow_core::domain::FiscalYearEnd::CALENDAR);
            let fy = fye.containing(today);
            Ok::<_, AppError>(asset.map(|a| {
                let dep = a.depreciation_for(fy);
                (a, dep)
            }))
        })
        .await
    {
        None => locked_fragment(),
        Some(Err(e)) => message_fragment(&e.to_string()),
        Some(Ok(None)) => message_fragment("immobilisation introuvable"),
        Some(Ok(Some((asset, dep)))) => {
            Html(views::depenses::asset_panel(&asset, dep, None).into_string())
        }
    }
}

pub async fn delete_asset(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let Some(id) = parse_id::<FixedAssetId>(&id) else {
        return message_fragment("identifiant d'immobilisation invalide").into_response();
    };
    let revision = match state
        .with_store(|store| fixed_assets::fixed_asset_by_id(store.connection(), id))
        .await
    {
        None => return locked_fragment().into_response(),
        Some(Err(e)) => return message_fragment(&e.to_string()).into_response(),
        Some(Ok(None)) => return message_fragment("immobilisation introuvable").into_response(),
        Some(Ok(Some(asset))) => asset.revision,
    };
    match execute(&state, DeleteFixedAsset { id, revision }).await {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => saved(),
        Some(Err(e)) => message_fragment(&views::errors::message(&e)).into_response(),
    }
}
