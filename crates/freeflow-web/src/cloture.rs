//! Routes de l'écran `cloture` (lot 20) : clore un exercice, réviser l'affectation d'un
//! projet, approuver, supprimer, et servir les documents de clôture. Même convention de
//! réponse que les écrans des lots 15/16 (`200` vide + `HX-Trigger: freeflow:saved` en succès,
//! panneau re-rendu en échec) — voir le commentaire de module de `crate::clients`.
//!
//! Les documents (`GET /cloture/{id}/doc/{kind}`) sont la seule famille de routes du crate qui
//! renvoie autre chose que du HTML : les octets du PDF (ou le JSON de liasse) rendus par
//! `freeflow-docs`, comme `freeflow year render` côté CLI — l'IO documentaire vit dans
//! l'adaptateur, jamais dans le cœur.

use axum::Form;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, header};
use axum::response::{Html, IntoResponse, Response};
use freeflow_core::app::{AppError, Executor, Outcome};
use freeflow_core::domain::{FiscalYearId, Money, OpeningBalanceLine, format_date};
use freeflow_core::fiscal_year::{
    self, ApproveFiscalYear, CloseFiscalYear, DeleteFiscalYear, FiscalYearRecord,
    UpdateFiscalYearAppropriation,
};
use freeflow_core::opening_balance::{
    self, DeleteOpeningBalance, OpeningBalanceRecord, RecordOpeningBalance, UpdateOpeningBalance,
};
use maud::html;
use serde::Deserialize;

use crate::state::AppState;
use crate::views;
use crate::views::cloture::{
    AmendFormValues, CloseFormErrors, CloseFormValues, OpeningFormErrors, OpeningFormValues,
};

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

fn error_banner(e: &AppError, reload_hx_get: &str) -> CloseFormErrors {
    match e {
        AppError::Conflict { .. } => CloseFormErrors {
            conflict: Some((e.to_string(), reload_hx_get.to_string())),
            ..Default::default()
        },
        other => CloseFormErrors {
            banner: Some(views::errors::message(other)),
            ..Default::default()
        },
    }
}

async fn current_record(
    state: &AppState,
    id: FiscalYearId,
) -> Option<Result<Option<FiscalYearRecord>, AppError>> {
    state
        .with_store(|store| fiscal_year::fiscal_year_by_id(store.connection(), id))
        .await
}

/// Un exercice est éditable s'il est en projet **et** sans successeur — la même règle que le
/// cœur applique (`fiscal_year::require_editable`), relue ici pour piloter l'affichage des
/// actions du panneau, jamais pour se substituer à la garde du cœur.
fn is_editable(store: &freeflow_core::store::Store, record: &FiscalYearRecord) -> bool {
    if record.is_approved() {
        return false;
    }
    fiscal_year::list_fiscal_years(store.connection())
        .map(|years| !years.iter().any(|y| y.ends_on > record.ends_on))
        .unwrap_or(false)
}

// -- Liste ---------------------------------------------------------------------------------

pub async fn table(State(state): State<AppState>) -> Html<String> {
    let today = state.today();
    match state
        .with_store(|store| views::cloture::list_fragment(store, today))
        .await
    {
        None => locked_fragment(),
        Some(Ok(markup)) => Html(markup.into_string()),
        Some(Err(e)) => message_fragment(&e.to_string()),
    }
}

// -- Clore ---------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CloseForm {
    #[serde(default)]
    starts_on: String,
    #[serde(default)]
    ends_on: String,
    #[serde(default)]
    legal_reserve: String,
    #[serde(default)]
    dividends: String,
    /// Case « report en arrière » : présente (`on`) seulement si cochée.
    #[serde(default)]
    carry_back: Option<String>,
}

impl From<&CloseForm> for CloseFormValues {
    fn from(f: &CloseForm) -> Self {
        Self {
            starts_on: f.starts_on.clone(),
            ends_on: f.ends_on.clone(),
            legal_reserve: f.legal_reserve.clone(),
            dividends: f.dividends.clone(),
            carry_back: f.carry_back.is_some(),
        }
    }
}

fn parse_money_field(raw: &str, errors_slot: &mut Option<String>) -> Money {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Money::ZERO;
    }
    match Money::parse_decimal(trimmed) {
        Ok(m) => m,
        Err(e) => {
            *errors_slot = Some(e.to_string());
            Money::ZERO
        }
    }
}

struct ParsedCloseForm {
    cmd: CloseFiscalYear,
}

fn parse_close_form(form: &CloseForm) -> Result<ParsedCloseForm, Box<CloseFormErrors>> {
    let mut errors = CloseFormErrors::default();
    let starts_on = match freeflow_core::domain::parse_date(form.starts_on.trim()) {
        Ok(d) => Some(d),
        Err(_) => {
            errors.starts_on = Some("date invalide (AAAA-MM-JJ)".to_string());
            None
        }
    };
    let ends_on = match freeflow_core::domain::parse_date(form.ends_on.trim()) {
        Ok(d) => Some(d),
        Err(_) => {
            errors.ends_on = Some("date invalide (AAAA-MM-JJ)".to_string());
            None
        }
    };
    let legal_reserve = parse_money_field(&form.legal_reserve, &mut errors.legal_reserve);
    let dividends = parse_money_field(&form.dividends, &mut errors.dividends);

    match (starts_on, ends_on) {
        (Some(starts_on), Some(ends_on))
            if errors.legal_reserve.is_none() && errors.dividends.is_none() =>
        {
            Ok(ParsedCloseForm {
                cmd: CloseFiscalYear {
                    starts_on,
                    ends_on,
                    legal_reserve,
                    dividends,
                    carry_back: form.carry_back.is_some(),
                    today: None,
                },
            })
        }
        _ => Err(Box::new(errors)),
    }
}

/// Pré-remplissage facultatif du formulaire de clôture — ce que le parcours de clôture
/// (lot 34) propose : la période de l'exercice et la dotation minimale à la réserve légale.
#[derive(Debug, Default, Deserialize)]
pub struct NewCloseQuery {
    #[serde(default)]
    starts_on: Option<String>,
    #[serde(default)]
    ends_on: Option<String>,
    #[serde(default)]
    legal_reserve: Option<String>,
}

pub async fn new_panel(
    State(state): State<AppState>,
    Query(query): Query<NewCloseQuery>,
) -> Html<String> {
    let today = state.today();
    let mut values = state
        .with_store(|store| views::cloture::default_close_values(store, today))
        .await
        .unwrap_or_default();
    if let Some(starts_on) = query.starts_on {
        values.starts_on = starts_on;
    }
    if let Some(ends_on) = query.ends_on {
        values.ends_on = ends_on;
    }
    if let Some(legal_reserve) = query.legal_reserve {
        values.legal_reserve = legal_reserve;
    }
    Html(views::cloture::new_panel(&values, &CloseFormErrors::default()).into_string())
}

pub async fn create(State(state): State<AppState>, Form(form): Form<CloseForm>) -> Response {
    let mut parsed = match parse_close_form(&form) {
        Ok(p) => p,
        Err(errors) => {
            return Html(views::cloture::new_panel(&(&form).into(), &errors).into_string())
                .into_response();
        }
    };
    // La date du jour vient de cet adaptateur (lot 36) : le cœur refuse un exercice pas encore
    // écoulé, et le refus s'affiche en bandeau comme n'importe quelle autre règle.
    parsed.cmd.today = Some(state.today());
    match execute(&state, parsed.cmd).await {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => saved(),
        Some(Err(e)) => {
            let errors = error_banner(&e, "/cloture/new");
            Html(views::cloture::new_panel(&(&form).into(), &errors).into_string()).into_response()
        }
    }
}

// -- Détail / révision d'affectation -------------------------------------------------------

fn parse_id(raw: &str) -> Option<FiscalYearId> {
    raw.parse().ok()
}

pub async fn show_panel(State(state): State<AppState>, Path(id): Path<String>) -> Html<String> {
    let Some(id) = parse_id(&id) else {
        return message_fragment("identifiant d'exercice invalide");
    };
    match state
        .with_store(|store| {
            fiscal_year::fiscal_year_by_id(store.connection(), id)
                .map(|record| record.map(|r| (is_editable(store, &r), r)))
        })
        .await
    {
        None => locked_fragment(),
        Some(Err(e)) => message_fragment(&e.to_string()),
        Some(Ok(None)) => {
            message_fragment("exercice introuvable — il a peut-être été supprimé entre-temps")
        }
        Some(Ok(Some((editable, record)))) => {
            Html(views::cloture::detail_panel(&record, editable, None).into_string())
        }
    }
}

pub async fn edit_panel(State(state): State<AppState>, Path(id): Path<String>) -> Html<String> {
    let Some(id) = parse_id(&id) else {
        return message_fragment("identifiant d'exercice invalide");
    };
    match current_record(&state, id).await {
        None => locked_fragment(),
        Some(Err(e)) => message_fragment(&e.to_string()),
        Some(Ok(None)) => message_fragment("exercice introuvable"),
        Some(Ok(Some(record))) => Html(
            views::cloture::edit_panel(
                &record,
                &AmendFormValues::from(&record),
                &CloseFormErrors::default(),
            )
            .into_string(),
        ),
    }
}

#[derive(Debug, Deserialize)]
pub struct AmendForm {
    #[serde(default)]
    revision: Option<String>,
    #[serde(default)]
    legal_reserve: String,
    #[serde(default)]
    dividends: String,
}

pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(form): Form<AmendForm>,
) -> Response {
    let Some(id) = parse_id(&id) else {
        return message_fragment("identifiant d'exercice invalide").into_response();
    };
    let record = match current_record(&state, id).await {
        None => return locked_fragment().into_response(),
        Some(Err(e)) => return message_fragment(&e.to_string()).into_response(),
        Some(Ok(None)) => return message_fragment("exercice introuvable").into_response(),
        Some(Ok(Some(record))) => record,
    };
    let revision: i64 = form
        .revision
        .as_deref()
        .and_then(|s| s.parse().ok())
        .unwrap_or_default();

    let mut errors = CloseFormErrors::default();
    let legal_reserve = parse_money_field(&form.legal_reserve, &mut errors.legal_reserve);
    let dividends = parse_money_field(&form.dividends, &mut errors.dividends);
    let values = AmendFormValues {
        legal_reserve: form.legal_reserve.clone(),
        dividends: form.dividends.clone(),
    };
    if errors.legal_reserve.is_some() || errors.dividends.is_some() {
        return Html(views::cloture::edit_panel(&record, &values, &errors).into_string())
            .into_response();
    }

    let cmd = UpdateFiscalYearAppropriation {
        id,
        revision,
        legal_reserve,
        dividends,
    };
    match execute(&state, cmd).await {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => saved(),
        Some(Err(e)) => {
            let errors = error_banner(&e, &format!("/cloture/{id}/edit"));
            Html(views::cloture::edit_panel(&record, &values, &errors).into_string())
                .into_response()
        }
    }
}

// -- Approbation / suppression -------------------------------------------------------------

pub async fn approve_panel(State(state): State<AppState>, Path(id): Path<String>) -> Html<String> {
    let Some(id) = parse_id(&id) else {
        return message_fragment("identifiant d'exercice invalide");
    };
    match current_record(&state, id).await {
        None => locked_fragment(),
        Some(Err(e)) => message_fragment(&e.to_string()),
        Some(Ok(None)) => message_fragment("exercice introuvable"),
        Some(Ok(Some(record))) => {
            Html(views::cloture::approve_panel(&record, state.today()).into_string())
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ApproveForm {
    #[serde(default)]
    approved_on: String,
}

pub async fn approve(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(form): Form<ApproveForm>,
) -> Response {
    let Some(id) = parse_id(&id) else {
        return message_fragment("identifiant d'exercice invalide").into_response();
    };
    let Ok(approved_on) = freeflow_core::domain::parse_date(form.approved_on.trim()) else {
        return message_fragment("date d'AG invalide (AAAA-MM-JJ)").into_response();
    };
    // Révision relue au moment du clic, comme les transitions des lots 15/16.
    let record = match current_record(&state, id).await {
        None => return locked_fragment().into_response(),
        Some(Err(e)) => return message_fragment(&e.to_string()).into_response(),
        Some(Ok(None)) => return message_fragment("exercice introuvable").into_response(),
        Some(Ok(Some(record))) => record,
    };
    let cmd = ApproveFiscalYear {
        id,
        revision: record.revision,
        approved_on,
        today: Some(state.today()),
    };
    // Sauvegarde préalable obligatoire (lot 36), comme `freeflow year approve` : l'approbation
    // rend l'exercice immuable, la sauvegarde est le seul retour en arrière. Son échec abandonne
    // l'approbation. IO d'adaptateur, jamais dans la commande.
    let backup = match state
        .with_store(|store| pre_approve_backup(store, record.ends_on.year()))
        .await
    {
        None => return locked_fragment().into_response(),
        Some(Err(message)) => {
            return Html(views::cloture::detail_panel(&record, true, Some(&message)).into_string())
                .into_response();
        }
        Some(Ok(path)) => path,
    };
    match execute(&state, cmd).await {
        None => locked_fragment().into_response(),
        Some(Ok(Outcome::Applied(approval))) => {
            // Succès, mais avec quelque chose à dire (sauvegarde, retard) : le panneau reste
            // ouvert sur ce compte rendu, et la liste se rafraîchit quand même.
            let mut response = Html(
                views::cloture::approved_panel(&record, &backup, approval.late_by_days)
                    .into_string(),
            )
            .into_response();
            response
                .headers_mut()
                .insert("HX-Trigger", HeaderValue::from_static("freeflow:saved"));
            response
        }
        Some(Ok(_)) => saved(),
        Some(Err(e)) => {
            Html(views::cloture::detail_panel(&record, true, Some(&e.to_string())).into_string())
                .into_response()
        }
    }
}

/// `backups/pre-approve-<période>-<horodatage>.db` à côté du coffre — le même nom que la CLI,
/// pour qu'un utilisateur retrouve ses sauvegardes au même endroit quel que soit l'adaptateur.
fn pre_approve_backup(
    store: &freeflow_core::store::Store,
    period: i32,
) -> Result<std::path::PathBuf, String> {
    let stamp = time::OffsetDateTime::now_utc()
        .format(&time::macros::format_description!(
            "[year][month][day]T[hour][minute][second]Z"
        ))
        .map_err(|e| e.to_string())?;
    let backups = store.db_path().with_file_name("backups");
    // Un nom neuf (même règle que la CLI) : `backup_to` refuse d'écraser.
    let dest = (0u32..)
        .map(|n| {
            let suffix = if n == 0 {
                String::new()
            } else {
                format!("-{n}")
            };
            backups.join(format!("pre-approve-{period}-{stamp}{suffix}.db"))
        })
        .find(|p| !p.exists() && !p.with_extension("db.kdf").exists())
        .expect("un suffixe libre finit toujours par exister");
    let failed = |e: &dyn std::fmt::Display| {
        format!(
            "sauvegarde préalable impossible ({}) : {e} — approbation abandonnée",
            dest.display()
        )
    };
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| failed(&e))?;
    }
    store.backup_to(&dest).map_err(|e| failed(&e))?;
    Ok(dest)
}

pub async fn delete_confirm_panel(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Html<String> {
    let Some(id) = parse_id(&id) else {
        return message_fragment("identifiant d'exercice invalide");
    };
    match current_record(&state, id).await {
        None => locked_fragment(),
        Some(Err(e)) => message_fragment(&e.to_string()),
        Some(Ok(None)) => message_fragment("exercice introuvable"),
        Some(Ok(Some(record))) => Html(views::cloture::delete_confirm_panel(&record).into_string()),
    }
}

pub async fn delete(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let Some(id) = parse_id(&id) else {
        return message_fragment("identifiant d'exercice invalide").into_response();
    };
    let record = match current_record(&state, id).await {
        None => return locked_fragment().into_response(),
        Some(Err(e)) => return message_fragment(&e.to_string()).into_response(),
        Some(Ok(None)) => {
            return message_fragment("exercice introuvable — déjà supprimé").into_response();
        }
        Some(Ok(Some(record))) => record,
    };
    let cmd = DeleteFiscalYear {
        id,
        revision: record.revision,
    };
    match execute(&state, cmd).await {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => saved(),
        Some(Err(e)) => {
            Html(views::cloture::detail_panel(&record, true, Some(&e.to_string())).into_string())
                .into_response()
        }
    }
}

// -- FEC (lot 28) ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct FecQuery {
    /// Année civile de la clôture — même désignation que `year show` ; l'exercice n'a pas
    /// besoin d'être clos.
    pub period: i32,
}

/// `GET /cloture/fec?period=AAAA` : le Fichier des Écritures Comptables de l'exercice, en
/// téléchargement sous son nom réglementaire. Construit et rendu par le cœur
/// (`freeflow_core::fec`), comme en CLI et via MCP.
pub async fn fec(State(state): State<AppState>, Query(query): Query<FecQuery>) -> Response {
    let built = state
        .with_store(|store| freeflow_core::fec::build_fec(store.connection(), query.period))
        .await;
    match built {
        None => locked_fragment().into_response(),
        Some(Err(e)) => message_fragment(&e.to_string()).into_response(),
        Some(Ok(fec)) => document_response(
            fec.render().into_bytes(),
            "text/plain; charset=utf-8",
            &fec.file_name(),
        ),
    }
}

/// `GET /cloture/checklist?period=AAAA` : le parcours de clôture guidé de l'exercice
/// (`freeflow_core::closing`), dans le panneau — les étapes viennent du cœur, les boutons qui y
/// répondent sont propres à cette façade (`views::cloture::checklist_panel`).
pub async fn checklist_panel(
    State(state): State<AppState>,
    Query(query): Query<FecQuery>,
) -> Html<String> {
    let today = state.today();
    let built = state
        .with_store(|store| {
            freeflow_core::closing::closing_checklist(store.connection(), query.period, today)
        })
        .await;
    match built {
        None => locked_fragment(),
        Some(Err(e)) => message_fragment(&e.to_string()),
        Some(Ok(checklist)) => Html(views::cloture::checklist_panel(&checklist).into_string()),
    }
}

/// `GET /cloture/balance?period=AAAA` : balance des comptes et bilan 2033-A dérivés du grand
/// livre (`freeflow_core::ledger`), dans le panneau — exercice clos ou non, comme le FEC.
pub async fn balance_panel(
    State(state): State<AppState>,
    Query(query): Query<FecQuery>,
) -> Html<String> {
    let built = state
        .with_store(|store| {
            freeflow_core::ledger::ledger_ending_in(store.connection(), query.period)
        })
        .await;
    match built {
        None => locked_fragment(),
        Some(Err(e)) => message_fragment(&e.to_string()),
        Some(Ok((_, ledger))) => Html(
            views::cloture::balance_panel(
                query.period,
                &ledger.trial_balance(),
                &ledger.balance_sheet(),
            )
            .into_string(),
        ),
    }
}

/// `GET /cloture/balance.pdf?period=AAAA` : le même bilan et la même balance en PDF
/// (`freeflow_docs::render_balance_sheet`), en téléchargement.
pub async fn balance_pdf(State(state): State<AppState>, Query(query): Query<FecQuery>) -> Response {
    let built = state
        .with_store(|store| {
            freeflow_core::ledger::ledger_ending_in(store.connection(), query.period)
        })
        .await;
    let (profile, ledger) = match built {
        None => return locked_fragment().into_response(),
        Some(Err(e)) => return message_fragment(&e.to_string()).into_response(),
        Some(Ok(pair)) => pair,
    };
    match freeflow_docs::render_balance_sheet(
        &profile,
        &ledger.balance_sheet(),
        &ledger.trial_balance(),
    ) {
        Ok(pdf) => document_response(
            pdf,
            "application/pdf",
            &format!("bilan-{}.pdf", ledger.exercise.end().year()),
        ),
        Err(e) => message_fragment(&e.to_string()).into_response(),
    }
}

// -- Documents -----------------------------------------------------------------------------

fn document_response(bytes: Vec<u8>, content_type: &'static str, filename: &str) -> Response {
    let disposition = format!("attachment; filename=\"{filename}\"");
    (
        [
            (header::CONTENT_TYPE, HeaderValue::from_static(content_type)),
            (
                header::CONTENT_DISPOSITION,
                HeaderValue::from_str(&disposition)
                    .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
            ),
        ],
        bytes,
    )
        .into_response()
}

pub async fn document(
    State(state): State<AppState>,
    Path((id, kind)): Path<(String, String)>,
) -> Response {
    let Some(id) = parse_id(&id) else {
        return message_fragment("identifiant d'exercice invalide").into_response();
    };
    let loaded = state
        .with_store(|store| -> Result<_, AppError> {
            let record = fiscal_year::fiscal_year_by_id(store.connection(), id)?;
            let profile = freeflow_core::company::company_profile(store.connection())?;
            let years = fiscal_year::list_fiscal_years(store.connection())?;
            // Le bilan 2033-A de la liasse est dérivé du grand livre (lot 31).
            let sheet = match (&record, &profile) {
                (Some(r), Some(_)) => Some(freeflow_core::ledger::build_ledger(
                    store.connection(),
                    r.period(),
                )?),
                _ => None,
            };
            Ok((record, profile, years, sheet))
        })
        .await;
    let (record, profile, years, sheet) = match loaded {
        None => return locked_fragment().into_response(),
        Some(Err(e)) => return message_fragment(&e.to_string()).into_response(),
        Some(Ok((None, _, _, _))) => {
            return message_fragment("exercice introuvable").into_response();
        }
        Some(Ok((_, None, _, _))) => {
            return message_fragment(
                "aucun profil d'entreprise défini — configurez-le d'abord (console : `company \
                 set-profile`)",
            )
            .into_response();
        }
        Some(Ok((Some(record), Some(profile), years, sheet))) => (record, profile, years, sheet),
    };
    let year_label = record.ends_on.year();
    let result = match kind.as_str() {
        "minutes" => {
            let today = state.today();
            freeflow_docs::render_approval_minutes(&profile, &record, today).map(|pdf| {
                document_response(pdf, "application/pdf", &format!("pv-{year_label}.pdf"))
            })
        }
        "appropriation" => {
            freeflow_docs::render_appropriation_decision(&profile, &record).map(|pdf| {
                document_response(
                    pdf,
                    "application/pdf",
                    &format!("affectation-{year_label}.pdf"),
                )
            })
        }
        "synthesis" => {
            let result = record.accounting_result();
            let prior = years
                .iter()
                .filter(|y| y.ends_on < record.starts_on)
                .max_by_key(|y| y.ends_on);
            freeflow_docs::render_synthesis(&profile, &result, prior).map(|pdf| {
                document_response(
                    pdf,
                    "application/pdf",
                    &format!("resultat-{year_label}.pdf"),
                )
            })
        }
        "liasse" => {
            let export = freeflow_docs::liasse_export(&profile, &record, sheet.as_ref());
            return match serde_json::to_vec_pretty(&export) {
                Ok(json) => document_response(
                    json,
                    "application/json",
                    &format!("liasse-{year_label}.json"),
                ),
                Err(e) => message_fragment(&e.to_string()).into_response(),
            };
        }
        other => {
            return message_fragment(&format!("document inconnu : {other}")).into_response();
        }
    };
    match result {
        Ok(response) => response,
        Err(e) => message_fragment(&e.to_string()).into_response(),
    }
}

// -- Bilan d'ouverture (lot 30) -------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct OpeningForm {
    #[serde(default)]
    revision: Option<String>,
    #[serde(default)]
    opens_on: String,
    #[serde(default)]
    source: String,
    #[serde(default)]
    lines: String,
    #[serde(default)]
    tax_losses: String,
}

impl From<&OpeningForm> for OpeningFormValues {
    fn from(f: &OpeningForm) -> Self {
        Self {
            opens_on: f.opens_on.clone(),
            source: f.source.clone(),
            lines: f.lines.clone(),
            tax_losses: f.tax_losses.clone(),
        }
    }
}

type OpeningState = Option<Result<Option<OpeningBalanceRecord>, AppError>>;

async fn current_opening(state: &AppState) -> OpeningState {
    state
        .with_store(|store| opening_balance::opening_balance(store.connection()))
        .await
}

/// La période du premier exercice clos, si un exercice existe : le bilan est alors figé — la
/// même règle que le cœur (`opening_balance::require_no_fiscal_year`), relue pour l'affichage.
fn frozen_by(store: &freeflow_core::store::Store) -> Option<String> {
    fiscal_year::list_fiscal_years(store.connection())
        .ok()?
        .first()
        .map(|y| format!("{} → {}", format_date(y.starts_on), format_date(y.ends_on)))
}

pub async fn opening_panel(State(state): State<AppState>) -> Html<String> {
    match state
        .with_store(|store| {
            opening_balance::opening_balance(store.connection())
                .map(|record| record.map(|r| (frozen_by(store), r)))
        })
        .await
    {
        None => locked_fragment(),
        Some(Err(e)) => message_fragment(&e.to_string()),
        Some(Ok(None)) => Html(
            views::cloture::opening_form_panel(
                &OpeningFormValues::default(),
                &OpeningFormErrors::default(),
                None,
            )
            .into_string(),
        ),
        Some(Ok(Some((frozen, record)))) => {
            Html(views::cloture::opening_detail_panel(&record, frozen.as_deref()).into_string())
        }
    }
}

pub async fn opening_edit_panel(State(state): State<AppState>) -> Html<String> {
    match current_opening(&state).await {
        None => locked_fragment(),
        Some(Err(e)) => message_fragment(&e.to_string()),
        Some(Ok(None)) => message_fragment("aucun bilan d'ouverture enregistré"),
        Some(Ok(Some(record))) => Html(
            views::cloture::opening_form_panel(
                &OpeningFormValues::from(&record),
                &OpeningFormErrors::default(),
                Some(record.revision),
            )
            .into_string(),
        ),
    }
}

struct ParsedOpeningForm {
    opens_on: time::Date,
    source: Option<String>,
    lines: Vec<OpeningBalanceLine>,
    tax_losses: Money,
}

fn parse_opening_form(form: &OpeningForm) -> Result<ParsedOpeningForm, Box<OpeningFormErrors>> {
    let mut errors = OpeningFormErrors::default();
    let opens_on = match freeflow_core::domain::parse_date(form.opens_on.trim()) {
        Ok(d) => Some(d),
        Err(_) => {
            errors.opens_on = Some("date invalide (AAAA-MM-JJ)".to_string());
            None
        }
    };
    let mut lines = Vec::new();
    for raw in form
        .lines
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
    {
        match raw.parse::<OpeningBalanceLine>() {
            Ok(line) => lines.push(line),
            Err(e) => {
                errors.lines = Some(e.to_string());
                break;
            }
        }
    }
    let source = form.source.trim();
    let tax_losses = parse_money_field(&form.tax_losses, &mut errors.tax_losses);
    match opens_on {
        Some(opens_on) if errors.lines.is_none() && errors.tax_losses.is_none() => {
            Ok(ParsedOpeningForm {
                opens_on,
                source: (!source.is_empty()).then(|| source.to_string()),
                lines,
                tax_losses,
            })
        }
        _ => Err(Box::new(errors)),
    }
}

fn opening_error_banner(e: &AppError, reload_hx_get: &str) -> OpeningFormErrors {
    match e {
        AppError::Conflict { .. } => OpeningFormErrors {
            conflict: Some((e.to_string(), reload_hx_get.to_string())),
            ..Default::default()
        },
        other => OpeningFormErrors {
            banner: Some(other.to_string()),
            ..Default::default()
        },
    }
}

/// Enregistre ou remplace : la présence d'un bilan en base décide de la commande (état complet
/// dans les deux cas), le champ caché `revision` protège le remplacement.
pub async fn opening_save(
    State(state): State<AppState>,
    Form(form): Form<OpeningForm>,
) -> Response {
    let revision: Option<i64> = form.revision.as_deref().and_then(|s| s.parse().ok());
    let values = OpeningFormValues::from(&form);
    let parsed = match parse_opening_form(&form) {
        Ok(p) => p,
        Err(errors) => {
            return Html(
                views::cloture::opening_form_panel(&values, &errors, revision).into_string(),
            )
            .into_response();
        }
    };
    let existing = match current_opening(&state).await {
        None => return locked_fragment().into_response(),
        Some(Err(e)) => return message_fragment(&e.to_string()).into_response(),
        Some(Ok(existing)) => existing,
    };
    let result = match existing {
        Some(existing) => {
            execute(
                &state,
                UpdateOpeningBalance {
                    // Sans révision dans le formulaire (soumission « nouveau » alors qu'un bilan
                    // vient d'être créé ailleurs), on force le conflit plutôt que d'écraser.
                    revision: revision.unwrap_or(existing.revision.wrapping_neg()),
                    opens_on: parsed.opens_on,
                    source: parsed.source,
                    lines: parsed.lines,
                    tax_losses: parsed.tax_losses,
                },
            )
            .await
            .map(|r| r.map(|_| ()))
        }
        None => execute(
            &state,
            RecordOpeningBalance {
                opens_on: parsed.opens_on,
                source: parsed.source,
                lines: parsed.lines,
                tax_losses: parsed.tax_losses,
            },
        )
        .await
        .map(|r| r.map(|_| ())),
    };
    match result {
        None => locked_fragment().into_response(),
        Some(Ok(())) => saved(),
        Some(Err(e)) => {
            let errors = opening_error_banner(&e, "/cloture/opening/edit");
            Html(views::cloture::opening_form_panel(&values, &errors, revision).into_string())
                .into_response()
        }
    }
}

pub async fn opening_delete_panel(State(state): State<AppState>) -> Html<String> {
    match current_opening(&state).await {
        None => locked_fragment(),
        Some(Err(e)) => message_fragment(&e.to_string()),
        Some(Ok(None)) => message_fragment("aucun bilan d'ouverture enregistré"),
        Some(Ok(Some(record))) => Html(views::cloture::opening_delete_panel(&record).into_string()),
    }
}

pub async fn opening_delete(State(state): State<AppState>) -> Response {
    // Révision relue au moment du clic — même raison que la suppression d'un exercice.
    let record = match current_opening(&state).await {
        None => return locked_fragment().into_response(),
        Some(Err(e)) => return message_fragment(&e.to_string()).into_response(),
        Some(Ok(None)) => {
            return message_fragment("aucun bilan d'ouverture enregistré").into_response();
        }
        Some(Ok(Some(record))) => record,
    };
    match execute(
        &state,
        DeleteOpeningBalance {
            revision: record.revision,
        },
    )
    .await
    {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => saved(),
        Some(Err(e)) => {
            Html(views::cloture::opening_detail_panel(&record, Some(&e.to_string())).into_string())
                .into_response()
        }
    }
}
