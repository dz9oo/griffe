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
use freeflow_core::domain::{FiscalYearId, Money};
use freeflow_core::fiscal_year::{
    self, ApproveFiscalYear, CloseFiscalYear, DeleteFiscalYear, FiscalYearRecord,
    UpdateFiscalYearAppropriation,
};
use maud::html;
use serde::Deserialize;
use time::OffsetDateTime;

use crate::state::AppState;
use crate::views;
use crate::views::cloture::{AmendFormValues, CloseFormErrors, CloseFormValues};

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
            banner: Some(other.to_string()),
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
    match state.with_store(views::cloture::list_fragment).await {
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
}

impl From<&CloseForm> for CloseFormValues {
    fn from(f: &CloseForm) -> Self {
        Self {
            starts_on: f.starts_on.clone(),
            ends_on: f.ends_on.clone(),
            legal_reserve: f.legal_reserve.clone(),
            dividends: f.dividends.clone(),
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
                },
            })
        }
        _ => Err(Box::new(errors)),
    }
}

pub async fn new_panel(State(state): State<AppState>) -> Html<String> {
    let today = OffsetDateTime::now_utc().date();
    let values = state
        .with_store(|store| views::cloture::default_close_values(store, today))
        .await
        .unwrap_or_default();
    Html(views::cloture::new_panel(&values, &CloseFormErrors::default()).into_string())
}

pub async fn create(State(state): State<AppState>, Form(form): Form<CloseForm>) -> Response {
    let parsed = match parse_close_form(&form) {
        Ok(p) => p,
        Err(errors) => {
            return Html(views::cloture::new_panel(&(&form).into(), &errors).into_string())
                .into_response();
        }
    };
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
        Some(Ok(Some(record))) => Html(
            views::cloture::approve_panel(&record, OffsetDateTime::now_utc().date()).into_string(),
        ),
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
            Ok((record, profile, years))
        })
        .await;
    let (record, profile, years) = match loaded {
        None => return locked_fragment().into_response(),
        Some(Err(e)) => return message_fragment(&e.to_string()).into_response(),
        Some(Ok((None, _, _))) => {
            return message_fragment("exercice introuvable").into_response();
        }
        Some(Ok((_, None, _))) => {
            return message_fragment(
                "aucun profil d'entreprise défini — configurez-le d'abord (console : `company \
                 set-profile`)",
            )
            .into_response();
        }
        Some(Ok((Some(record), Some(profile), years))) => (record, profile, years),
    };
    let year_label = record.ends_on.year();
    let result = match kind.as_str() {
        "minutes" => {
            let today = OffsetDateTime::now_utc().date();
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
            let result = freeflow_core::accounting::AccountingResult {
                period: record.period(),
                revenue_ht: record.revenue_ht,
                expenses: record.expenses,
                director_remuneration: record.director_remuneration,
                result_before_tax: record.result_before_tax,
                corporate_tax: record.corporate_tax,
                net_result: record.net_result,
            };
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
            let export = freeflow_docs::liasse_export(&profile, &record);
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
