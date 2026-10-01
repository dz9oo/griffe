//! Routes `GET /view/*` : chaque écran rend son contenu ; si la requête vient d'une navigation
//! htmx boostée (`HX-Request`), seul ce contenu est renvoyé — sinon la page complète, coque
//! comprise, pour un chargement direct ou un rechargement.
//!
//! Ces handlers ne sont atteints que le coffre déverrouillé : [`crate::unlock::require_unlocked`]
//! redirige toute autre requête vers l'écran de déverrouillage avant d'y arriver. Ils restent
//! défensifs (`with_store` renvoie `None` plutôt que de paniquer) au cas où l'état basculerait
//! entre le passage du middleware et l'exécution du handler.

use axum::extract::rejection::FormRejection;
use axum::extract::{Form, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use maud::{Markup, html};
use serde::Deserialize;

use crate::layout::{self, ViewId};
use crate::state::AppState;
use crate::views;
use griffe_core::app::Executor;
use griffe_core::domain::{Money, parse_date};
use griffe_core::fiscal::FiscalDeadlineKind;
use griffe_core::setup::vault_started_on;
use griffe_core::society::{
    MarkCatchUpFiled, MarkDutyFiled, RecordVatCarryIn, RecordVatReversal, RequestVatRefund,
    RetractDutyFiled, RetractVatRefund, RetractVatReversal, VatRefundStatus, duty_briefing,
};

#[derive(Debug, Default, Deserialize)]
pub(crate) struct FiledForm {
    #[serde(default)]
    filed_on: Option<String>,
    #[serde(default)]
    at_due: Option<String>,
}

fn is_htmx_request(headers: &HeaderMap) -> bool {
    headers.contains_key("hx-request")
}

fn error_markup(active: ViewId, err: impl std::fmt::Display) -> Markup {
    html! {
        div class="empty-state" { "erreur de lecture : " (err.to_string()) " (" (active.label()) ")" }
    }
}

fn locked_markup(active: ViewId) -> Markup {
    html! {
        div class="empty-state" { "coffre verrouillé (" (active.label()) ") — rechargez la page" }
    }
}

async fn respond(headers: HeaderMap, active: ViewId, content: Markup) -> Html<String> {
    if is_htmx_request(&headers) {
        Html(content.into_string())
    } else {
        Html(layout::page(active, "déverrouillé", content).into_string())
    }
}

/// Le jour complet (coque comprise), en dehors de tout contexte de requête HTTP — utilisé par
/// [`crate::unlock`] pour atterrir directement dessus après un déverrouillage, sans passer par
/// une redirection `Location` : le protocole URI custom de la coque Tauri ne la suit pas de
/// façon fiable pour une navigation de premier niveau (WebKitGTK), contrairement à
/// `HX-Redirect`, qui est piloté par le JavaScript d'htmx plutôt que par le moteur de rendu.
pub async fn jour_page(state: &AppState) -> String {
    let today = state.today();
    let content = state
        .with_store(|store| {
            views::jour::render(store, today).unwrap_or_else(|e| error_markup(ViewId::Jour, e))
        })
        .await
        .unwrap_or_else(|| locked_markup(ViewId::Jour));
    layout::page(ViewId::Jour, "déverrouillé", content).into_string()
}

/// Alias conservé : le déverrouillage historique atterrissait sur le tableau de bord.
pub async fn dashboard_page(state: &AppState) -> String {
    jour_page(state).await
}

async fn letter(
    state: &AppState,
    headers: HeaderMap,
    active: ViewId,
    render: impl FnOnce(
        &griffe_core::store::Store,
        time::Date,
    ) -> Result<Markup, griffe_core::app::AppError>,
) -> Html<String> {
    let today = state.today();
    let content = state
        .with_store(|store| render(store, today).unwrap_or_else(|e| error_markup(active, e)))
        .await
        .unwrap_or_else(|| locked_markup(active));
    respond(headers, active, content).await
}

pub async fn jour(State(state): State<AppState>, headers: HeaderMap) -> Html<String> {
    letter(&state, headers, ViewId::Jour, views::jour::render).await
}

pub async fn dashboard(State(state): State<AppState>, headers: HeaderMap) -> Html<String> {
    letter(&state, headers, ViewId::Dashboard, views::jour::render).await
}

pub async fn gens(State(state): State<AppState>, headers: HeaderMap) -> Html<String> {
    letter(&state, headers, ViewId::Gens, views::gens::render).await
}

pub async fn societe_piece(State(state): State<AppState>, headers: HeaderMap) -> Html<String> {
    letter(&state, headers, ViewId::Societe, views::societe::piece).await
}

pub async fn societe_pay(State(state): State<AppState>, headers: HeaderMap) -> Html<String> {
    letter(&state, headers, ViewId::Societe, views::societe::pay).await
}

pub async fn societe_duties(State(state): State<AppState>, headers: HeaderMap) -> Html<String> {
    letter(&state, headers, ViewId::Societe, views::societe::duties).await
}

pub async fn societe_duty(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(kind): Path<String>,
) -> Response {
    duty_letter(&state, headers, kind, None).await
}

pub async fn societe_duty_at(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((kind, period)): Path<(String, String)>,
) -> Response {
    duty_letter(&state, headers, kind, Some(period)).await
}

async fn duty_letter(
    state: &AppState,
    headers: HeaderMap,
    kind: String,
    period: Option<String>,
) -> Response {
    let Some(kind) = parse_external_kind(&kind) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    letter(state, headers, ViewId::Societe, move |store, today| {
        views::societe::duty(store, today, kind, period.as_deref())
    })
    .await
    .into_response()
}

pub async fn societe_duty_open(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(kind): Path<String>,
) -> Response {
    duty_open(&state, headers, kind, None).await
}

pub async fn societe_duty_open_at(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((kind, period)): Path<(String, String)>,
) -> Response {
    duty_open(&state, headers, kind, Some(period)).await
}

async fn duty_open(
    state: &AppState,
    headers: HeaderMap,
    kind: String,
    period: Option<String>,
) -> Response {
    let Some(kind) = parse_external_kind(&kind) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let today = state.today();
    let period_ref = period.as_deref();
    state
        .with_store(|store| {
            if let Ok(briefing) = duty_briefing(store.connection(), kind, today, period_ref) {
                open_allowed_url(briefing.url);
            }
        })
        .await;
    letter(state, headers, ViewId::Societe, move |store, today| {
        views::societe::duty(store, today, kind, period.as_deref())
    })
    .await
    .into_response()
}

pub async fn societe_duty_filed(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(kind): Path<String>,
    form: Result<Form<FiledForm>, FormRejection>,
) -> Response {
    mark_duty_filed(&state, headers, kind, None, form.ok().map(|Form(f)| f)).await
}

pub async fn societe_duty_filed_at(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((kind, period)): Path<(String, String)>,
    form: Result<Form<FiledForm>, FormRejection>,
) -> Response {
    mark_duty_filed(
        &state,
        headers,
        kind,
        Some(period),
        form.ok().map(|Form(f)| f),
    )
    .await
}

pub async fn societe_duties_catch_up(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    let today = state.today();
    let result = state
        .with_store_mut(|store| {
            let before = vault_started_on(store.connection(), today)?;
            Executor::new(store).execute(&MarkCatchUpFiled { before }, &AppState::human_ctx())?;
            Ok::<_, griffe_core::app::AppError>(())
        })
        .await;
    if let Some(Err(e)) = result {
        return respond(headers, ViewId::Societe, error_markup(ViewId::Societe, e))
            .await
            .into_response();
    }
    letter(&state, headers, ViewId::Societe, views::societe::duties)
        .await
        .into_response()
}

async fn mark_duty_filed(
    state: &AppState,
    headers: HeaderMap,
    kind: String,
    period: Option<String>,
    form: Option<FiledForm>,
) -> Response {
    let Some(kind) = parse_external_kind(&kind) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let today = state.today();
    let period_owned = period.clone();
    let form = form.unwrap_or_default();
    let result = state
        .with_store_mut(|store| {
            let briefing = duty_briefing(store.connection(), kind, today, period_owned.as_deref())?;
            let filed_on = if form.at_due.as_deref().is_some_and(|v| !v.is_empty()) {
                briefing.due_on
            } else {
                form.filed_on
                    .as_deref()
                    .and_then(|s| parse_date(s).ok())
                    .unwrap_or(today)
            };
            Executor::new(store).execute(
                &MarkDutyFiled {
                    kind,
                    period_key: briefing.period_key,
                    due_on: briefing.due_on,
                    filed_on,
                },
                &AppState::human_ctx(),
            )?;
            Ok::<_, griffe_core::app::AppError>(())
        })
        .await;
    if let Some(Err(e)) = result {
        return respond(headers, ViewId::Societe, error_markup(ViewId::Societe, e))
            .await
            .into_response();
    }
    let mut response = letter(state, headers, ViewId::Societe, move |store, today| {
        views::societe::duty(store, today, kind, period.as_deref())
    })
    .await
    .into_response();
    response
        .headers_mut()
        .insert("HX-Trigger", HeaderValue::from_static("griffe:saved"));
    response
}

pub async fn societe_duty_unfiled(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(kind): Path<String>,
) -> Response {
    retract_duty_filed(&state, headers, kind, None).await
}

pub async fn societe_duty_unfiled_at(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((kind, period)): Path<(String, String)>,
) -> Response {
    retract_duty_filed(&state, headers, kind, Some(period)).await
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct VatCreditPosted {
    #[serde(default)]
    after_period: String,
    #[serde(default)]
    credit: String,
    #[serde(default)]
    revision: String,
}

pub async fn societe_vat_credit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(kind): Path<String>,
    form: Result<Form<VatCreditPosted>, FormRejection>,
) -> Response {
    save_vat_credit(&state, headers, kind, None, form).await
}

pub async fn societe_vat_credit_at(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((kind, period)): Path<(String, String)>,
    form: Result<Form<VatCreditPosted>, FormRejection>,
) -> Response {
    save_vat_credit(&state, headers, kind, Some(period), form).await
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct VatRefundPosted {
    #[serde(default)]
    amount: String,
}

pub async fn societe_vat_refund(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(kind): Path<String>,
    form: Result<Form<VatRefundPosted>, FormRejection>,
) -> Response {
    request_vat_refund(&state, headers, kind, None, form).await
}

pub async fn societe_vat_refund_at(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((kind, period)): Path<(String, String)>,
    form: Result<Form<VatRefundPosted>, FormRejection>,
) -> Response {
    request_vat_refund(&state, headers, kind, Some(period), form).await
}

pub async fn societe_vat_refund_retract(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(kind): Path<String>,
) -> Response {
    retract_vat_refund(&state, headers, kind, None).await
}

pub async fn societe_vat_refund_retract_at(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((kind, period)): Path<(String, String)>,
) -> Response {
    retract_vat_refund(&state, headers, kind, Some(period)).await
}

async fn request_vat_refund(
    state: &AppState,
    headers: HeaderMap,
    kind: String,
    period: Option<String>,
    form: Result<Form<VatRefundPosted>, FormRejection>,
) -> Response {
    let Some(kind) = parse_external_kind(&kind) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let posted = form.map_or_else(|_| VatRefundPosted::default(), |Form(f)| f);
    let today = state.today();
    let period_owned = period.clone();
    let result = state
        .with_store_mut(|store| {
            let briefing = duty_briefing(store.connection(), kind, today, period_owned.as_deref())?;
            let trimmed = posted.amount.trim();
            let amount = if trimmed.is_empty() {
                match briefing.vat_refund {
                    Some(VatRefundStatus::Offered { credit, .. }) => credit,
                    _ => {
                        return Err(griffe_core::app::AppError::Domain(
                            "pas de crédit à récupérer".into(),
                        ));
                    }
                }
            } else {
                Money::parse_decimal(trimmed)
                    .map_err(|e| griffe_core::app::AppError::Domain(e.to_string()))?
            };
            Executor::new(store).execute(
                &RequestVatRefund {
                    period_key: briefing.period_key,
                    amount,
                    requested_on: today,
                },
                &AppState::human_ctx(),
            )?;
            Ok::<_, griffe_core::app::AppError>(())
        })
        .await;
    if let Some(Err(e)) = result {
        return respond(headers, ViewId::Societe, error_markup(ViewId::Societe, e))
            .await
            .into_response();
    }
    let mut response = letter(state, headers, ViewId::Societe, move |store, today| {
        views::societe::duty(store, today, kind, period.as_deref())
    })
    .await
    .into_response();
    response
        .headers_mut()
        .insert("HX-Trigger", HeaderValue::from_static("griffe:saved"));
    response
}

async fn retract_vat_refund(
    state: &AppState,
    headers: HeaderMap,
    kind: String,
    period: Option<String>,
) -> Response {
    let Some(kind) = parse_external_kind(&kind) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let today = state.today();
    let period_owned = period.clone();
    let result = state
        .with_store_mut(|store| {
            let briefing = duty_briefing(store.connection(), kind, today, period_owned.as_deref())?;
            Executor::new(store).execute(
                &RetractVatRefund {
                    period_key: briefing.period_key,
                },
                &AppState::human_ctx(),
            )?;
            Ok::<_, griffe_core::app::AppError>(())
        })
        .await;
    if let Some(Err(e)) = result {
        return respond(headers, ViewId::Societe, error_markup(ViewId::Societe, e))
            .await
            .into_response();
    }
    let mut response = letter(state, headers, ViewId::Societe, move |store, today| {
        views::societe::duty(store, today, kind, period.as_deref())
    })
    .await
    .into_response();
    response
        .headers_mut()
        .insert("HX-Trigger", HeaderValue::from_static("griffe:saved"));
    response
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct VatReversalPosted {
    #[serde(default)]
    amount: String,
}

pub async fn societe_vat_reversal(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(kind): Path<String>,
    form: Result<Form<VatReversalPosted>, FormRejection>,
) -> Response {
    record_vat_reversal(&state, headers, kind, None, form).await
}

pub async fn societe_vat_reversal_at(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((kind, period)): Path<(String, String)>,
    form: Result<Form<VatReversalPosted>, FormRejection>,
) -> Response {
    record_vat_reversal(&state, headers, kind, Some(period), form).await
}

pub async fn societe_vat_reversal_retract(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(kind): Path<String>,
) -> Response {
    retract_vat_reversal(&state, headers, kind, None).await
}

pub async fn societe_vat_reversal_retract_at(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((kind, period)): Path<(String, String)>,
) -> Response {
    retract_vat_reversal(&state, headers, kind, Some(period)).await
}

async fn record_vat_reversal(
    state: &AppState,
    headers: HeaderMap,
    kind: String,
    period: Option<String>,
    form: Result<Form<VatReversalPosted>, FormRejection>,
) -> Response {
    let Some(kind) = parse_external_kind(&kind) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let posted = form.map_or_else(|_| VatReversalPosted::default(), |Form(f)| f);
    let today = state.today();
    let period_owned = period.clone();
    let result = state
        .with_store_mut(|store| {
            let briefing = duty_briefing(store.connection(), kind, today, period_owned.as_deref())?;
            let amount = Money::parse_decimal(posted.amount.trim())
                .map_err(|e| griffe_core::app::AppError::Domain(e.to_string()))?;
            Executor::new(store).execute(
                &RecordVatReversal {
                    period_key: briefing.period_key,
                    amount,
                    recorded_on: today,
                },
                &AppState::human_ctx(),
            )?;
            Ok::<_, griffe_core::app::AppError>(())
        })
        .await;
    if let Some(Err(e)) = result {
        return respond(headers, ViewId::Societe, error_markup(ViewId::Societe, e))
            .await
            .into_response();
    }
    let mut response = letter(state, headers, ViewId::Societe, move |store, today| {
        views::societe::duty(store, today, kind, period.as_deref())
    })
    .await
    .into_response();
    response
        .headers_mut()
        .insert("HX-Trigger", HeaderValue::from_static("griffe:saved"));
    response
}

async fn retract_vat_reversal(
    state: &AppState,
    headers: HeaderMap,
    kind: String,
    period: Option<String>,
) -> Response {
    let Some(kind) = parse_external_kind(&kind) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let today = state.today();
    let period_owned = period.clone();
    let result = state
        .with_store_mut(|store| {
            let briefing = duty_briefing(store.connection(), kind, today, period_owned.as_deref())?;
            Executor::new(store).execute(
                &RetractVatReversal {
                    period_key: briefing.period_key,
                },
                &AppState::human_ctx(),
            )?;
            Ok::<_, griffe_core::app::AppError>(())
        })
        .await;
    if let Some(Err(e)) = result {
        return respond(headers, ViewId::Societe, error_markup(ViewId::Societe, e))
            .await
            .into_response();
    }
    let mut response = letter(state, headers, ViewId::Societe, move |store, today| {
        views::societe::duty(store, today, kind, period.as_deref())
    })
    .await
    .into_response();
    response
        .headers_mut()
        .insert("HX-Trigger", HeaderValue::from_static("griffe:saved"));
    response
}

async fn save_vat_credit(
    state: &AppState,
    headers: HeaderMap,
    kind: String,
    period: Option<String>,
    form: Result<Form<VatCreditPosted>, FormRejection>,
) -> Response {
    let Some(kind) = parse_external_kind(&kind) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let posted = form.map_or_else(|_| VatCreditPosted::default(), |Form(f)| f);
    let mut vat_form = views::societe::VatCreditForm {
        after_period: posted.after_period.clone(),
        credit: posted.credit.clone(),
        revision: posted.revision.clone(),
        after_error: None,
        credit_error: None,
        banner: None,
    };
    let credit = match Money::parse_decimal(posted.credit.trim()) {
        Ok(c) => c,
        Err(e) => {
            vat_form.credit_error = Some(e.to_string());
            return letter(state, headers, ViewId::Societe, {
                let period = period.clone();
                let vat_form = vat_form.clone();
                move |store, today| {
                    views::societe::duty_with_vat_form(
                        store,
                        today,
                        kind,
                        period.as_deref(),
                        Some(vat_form),
                    )
                }
            })
            .await
            .into_response();
        }
    };
    let period_owned = period.clone();
    let after = posted.after_period.clone();
    let source = Some("lettre de TVA".to_string());
    let result = state
        .with_store_mut(|store| {
            let outcome = Executor::new(store).execute(
                &RecordVatCarryIn {
                    after_period: after,
                    credit,
                    source,
                },
                &AppState::human_ctx(),
            )?;
            Ok::<_, griffe_core::app::AppError>(outcome)
        })
        .await;
    match result {
        Some(Err(e)) => {
            let msg = e.to_string();
            if msg.contains("figé") || msg.contains("existe déjà") {
                letter(state, headers, ViewId::Societe, move |store, today| {
                    views::societe::duty(store, today, kind, period.as_deref())
                })
                .await
                .into_response()
            } else {
                vat_form.banner = Some(msg);
                letter(state, headers, ViewId::Societe, {
                    let vat_form = vat_form.clone();
                    move |store, today| {
                        views::societe::duty_with_vat_form(
                            store,
                            today,
                            kind,
                            period.as_deref(),
                            Some(vat_form),
                        )
                    }
                })
                .await
                .into_response()
            }
        }
        Some(Ok(_)) | None => {
            let mut response = letter(state, headers, ViewId::Societe, move |store, today| {
                views::societe::duty(store, today, kind, period_owned.as_deref())
            })
            .await
            .into_response();
            if result.is_some() {
                response
                    .headers_mut()
                    .insert("HX-Trigger", HeaderValue::from_static("griffe:saved"));
            }
            response
        }
    }
}

async fn retract_duty_filed(
    state: &AppState,
    headers: HeaderMap,
    kind: String,
    period: Option<String>,
) -> Response {
    let Some(kind) = parse_external_kind(&kind) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let today = state.today();
    let period_owned = period.clone();
    let result = state
        .with_store_mut(|store| {
            let briefing = duty_briefing(store.connection(), kind, today, period_owned.as_deref())?;
            Executor::new(store).execute(
                &RetractDutyFiled {
                    kind,
                    period_key: briefing.period_key,
                },
                &AppState::human_ctx(),
            )?;
            Ok::<_, griffe_core::app::AppError>(())
        })
        .await;
    if let Some(Err(e)) = result {
        return respond(headers, ViewId::Societe, error_markup(ViewId::Societe, e))
            .await
            .into_response();
    }
    let mut response = letter(state, headers, ViewId::Societe, move |store, today| {
        views::societe::duty(store, today, kind, period.as_deref())
    })
    .await
    .into_response();
    response
        .headers_mut()
        .insert("HX-Trigger", HeaderValue::from_static("griffe:saved"));
    response
}

fn parse_external_kind(raw: &str) -> Option<FiscalDeadlineKind> {
    let kind = FiscalDeadlineKind::parse(raw)?;
    (kind != FiscalDeadlineKind::ApprovalMeeting).then_some(kind)
}

fn open_allowed_url(url: &str) {
    const ALLOWED: &[&str] = &["https://www.impots.gouv.fr/", "https://procedures.inpi.fr/"];
    if !griffe_cli::should_open_externally() {
        return;
    }
    if !ALLOWED.iter().any(|prefix| url.starts_with(prefix)) {
        return;
    }
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(opener).arg(url).spawn();
}

pub async fn societe_closing(State(state): State<AppState>, headers: HeaderMap) -> Html<String> {
    letter(&state, headers, ViewId::Societe, views::societe::closing).await
}

#[derive(Debug, Default, Deserialize)]
pub struct StatementQuery {
    #[serde(default)]
    q: String,
    #[serde(default)]
    etat: String,
    #[serde(default)]
    avant: String,
    #[serde(default)]
    avant_id: String,
}

pub async fn societe_statement(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(form): Query<StatementQuery>,
) -> Html<String> {
    use griffe_core::domain::BankTransactionId;
    use griffe_core::society::StatementCursor;
    use std::str::FromStr;

    let before = match (
        parse_date(form.avant.trim()),
        BankTransactionId::from_str(form.avant_id.trim()),
    ) {
        (Ok(occurred_on), Ok(id)) => Some(StatementCursor { occurred_on, id }),
        _ => None,
    };
    let target = headers
        .get("hx-target")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let fragment = if target == "releve-more" || target == "#releve-more" {
        views::societe::ReleveFragment::More
    } else if target == "releve-body" || target == "#releve-body" {
        views::societe::ReleveFragment::Body
    } else {
        views::societe::ReleveFragment::Letter
    };
    let query = views::societe::ReleveQuery {
        search: form.q,
        unread_only: form.etat == "non-traites",
        before,
        fragment,
    };
    let today = state.today();
    let content = state
        .with_store(|store| {
            views::societe::statement(store, today, &query)
                .unwrap_or_else(|e| error_markup(ViewId::Societe, e))
        })
        .await
        .unwrap_or_else(|| locked_markup(ViewId::Societe));
    respond(headers, ViewId::Societe, content).await
}

pub async fn societe_identity(State(state): State<AppState>, headers: HeaderMap) -> Html<String> {
    letter(&state, headers, ViewId::Societe, views::societe::identity).await
}

pub async fn prospection(State(state): State<AppState>, headers: HeaderMap) -> Html<String> {
    letter(&state, headers, ViewId::Gens, views::gens::render).await
}

pub async fn missions(State(state): State<AppState>, headers: HeaderMap) -> Html<String> {
    letter(&state, headers, ViewId::Gens, views::gens::render).await
}

pub async fn facturation(State(state): State<AppState>, headers: HeaderMap) -> Html<String> {
    letter(
        &state,
        headers,
        ViewId::Facturation,
        views::facturation::list_fragment,
    )
    .await
}

pub async fn clients(State(state): State<AppState>, headers: HeaderMap) -> Html<String> {
    letter(&state, headers, ViewId::Gens, views::gens::render).await
}

pub async fn devis(State(state): State<AppState>, headers: HeaderMap) -> Html<String> {
    letter(&state, headers, ViewId::Gens, views::gens::render).await
}

pub async fn depenses(State(state): State<AppState>, headers: HeaderMap) -> Html<String> {
    letter(&state, headers, ViewId::Depenses, |store, today| {
        views::societe::statement(
            store,
            today,
            &views::societe::ReleveQuery {
                search: String::new(),
                unread_only: false,
                before: None,
                fragment: views::societe::ReleveFragment::Letter,
            },
        )
    })
    .await
}

pub async fn cloture(State(state): State<AppState>, headers: HeaderMap) -> Html<String> {
    letter(&state, headers, ViewId::Cloture, views::societe::closing).await
}

pub async fn societe(State(state): State<AppState>, headers: HeaderMap) -> Html<String> {
    let content = state
        .with_store(|store| {
            views::societe::render(store).unwrap_or_else(|e| error_markup(ViewId::Societe, e))
        })
        .await
        .unwrap_or_else(|| locked_markup(ViewId::Societe));
    respond(headers, ViewId::Societe, content).await
}

pub async fn aide(headers: HeaderMap) -> Html<String> {
    respond(headers, ViewId::Aide, views::aide::index()).await
}

pub async fn aide_recipe(headers: HeaderMap, Path(slug): Path<String>) -> Html<String> {
    respond(headers, ViewId::Aide, views::aide::page(&slug)).await
}

pub async fn lexique(headers: HeaderMap) -> Html<String> {
    respond(headers, ViewId::Aide, views::aide::page("lexique")).await
}

pub async fn console(headers: HeaderMap) -> Html<String> {
    respond(headers, ViewId::Console, views::console::render()).await
}

pub async fn index(State(state): State<AppState>, headers: HeaderMap) -> Html<String> {
    jour(State(state), headers).await
}
