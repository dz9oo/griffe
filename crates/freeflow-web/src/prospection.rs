//! Routes de mutation `prospection` — même patron et même convention de réponse que
//! `crate::clients` (lot 15) : succès → `200` corps vide + `HX-Trigger: freeflow:saved` ; échec →
//! `200` avec le panneau re-rendu. Voir le doc-comment de tête de `crate::clients` pour le détail
//! complet de la convention, non répété ici.

use axum::Form;
use axum::extract::{Path, Query, State};
use axum::http::HeaderValue;
use axum::response::{Html, IntoResponse, Response};
use freeflow_core::app::{AppError, Executor, Outcome};
use freeflow_core::domain::{InteractionId, Money, OpportunityId, OpportunityStage, Probability};
use freeflow_core::prospection::{self, OpportunityFilter};
use maud::html;
use serde::Deserialize;

use crate::state::AppState;
use crate::views;
use crate::views::prospection::{
    InteractionFormErrors, InteractionFormValues, OpportunityFormErrors, OpportunityFormValues,
    TransitionErrors,
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

fn parse_id<T: std::str::FromStr>(raw: &str) -> Option<T> {
    raw.parse().ok()
}

fn opportunity_error_banner(e: AppError, reload_hx_get: &str) -> OpportunityFormErrors {
    match e {
        AppError::Conflict { .. } => OpportunityFormErrors {
            conflict: Some((e.to_string(), reload_hx_get.to_string())),
            ..Default::default()
        },
        other => OpportunityFormErrors {
            banner: Some(other.to_string()),
            ..Default::default()
        },
    }
}

fn interaction_error_banner(e: AppError, reload_hx_get: &str) -> InteractionFormErrors {
    match e {
        AppError::Conflict { .. } => InteractionFormErrors {
            conflict: Some((e.to_string(), reload_hx_get.to_string())),
            ..Default::default()
        },
        other => InteractionFormErrors {
            banner: Some(other.to_string()),
            ..Default::default()
        },
    }
}

async fn current_opportunity(
    state: &AppState,
    id: OpportunityId,
) -> Option<Result<Option<freeflow_core::domain::Opportunity>, AppError>> {
    state
        .with_store(|store| prospection::opportunity_by_id(store.connection(), id))
        .await
}

// -- Liste ---------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct TableQuery {
    #[serde(default)]
    closed: bool,
    #[serde(default)]
    archived: bool,
}

pub async fn table(State(state): State<AppState>, Query(q): Query<TableQuery>) -> Html<String> {
    let filter = OpportunityFilter {
        include_closed: q.closed,
        include_archived: q.archived,
    };
    match state
        .with_store(|store| views::prospection::list_fragment(store, filter))
        .await
    {
        None => locked_fragment(),
        Some(Ok(markup)) => Html(markup.into_string()),
        Some(Err(e)) => message_fragment(&e.to_string()),
    }
}

// -- Opportunité : créer / afficher / modifier / archiver / supprimer -----------------------

pub async fn new_panel() -> Html<String> {
    Html(
        views::prospection::new_panel(
            &OpportunityFormValues::default(),
            &OpportunityFormErrors::default(),
        )
        .into_string(),
    )
}

#[derive(Debug, Deserialize)]
pub struct OpportunityForm {
    #[serde(default)]
    revision: Option<String>,
    #[serde(default)]
    client: String,
    name: String,
    amount: String,
    probability: String,
    #[serde(default)]
    next_action: String,
    #[serde(default)]
    source: String,
}

fn opt(s: &str) -> Option<String> {
    let trimmed = s.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

struct ParsedOpportunityForm {
    name: String,
    amount: Money,
    probability: Probability,
    next_action_at: Option<time::Date>,
    source: Option<String>,
}

fn parse_opportunity_form(
    form: &OpportunityForm,
) -> Result<ParsedOpportunityForm, Box<OpportunityFormErrors>> {
    let mut errors = OpportunityFormErrors::default();

    if form.name.trim().is_empty() {
        errors.name = Some("le nom est obligatoire".to_string());
    }
    let amount = match Money::parse_decimal(&form.amount) {
        Ok(a) => Some(a),
        Err(e) => {
            errors.amount = Some(e.to_string());
            None
        }
    };
    let probability = match form.probability.trim().parse::<u8>().map(Probability::new) {
        Ok(Ok(p)) => Some(p),
        Ok(Err(e)) => {
            errors.probability = Some(e.to_string());
            None
        }
        Err(_) => {
            errors.probability = Some("probabilité invalide (attendu un entier 0-100)".to_string());
            None
        }
    };
    let next_action_at = opt(&form.next_action)
        .map(|s| freeflow_core::domain::parse_date(&s))
        .transpose()
        .ok()
        .flatten();

    if errors.name.is_some() || errors.amount.is_some() || errors.probability.is_some() {
        return Err(Box::new(errors));
    }
    Ok(ParsedOpportunityForm {
        name: form.name.trim().to_string(),
        amount: amount.expect("validé ci-dessus"),
        probability: probability.expect("validé ci-dessus"),
        next_action_at,
        source: opt(&form.source),
    })
}

pub async fn create(State(state): State<AppState>, Form(form): Form<OpportunityForm>) -> Response {
    let client_id = match state
        .with_store(|store| {
            freeflow_core::reference::resolve_client(store.connection(), &form.client)
        })
        .await
    {
        None => return locked_fragment().into_response(),
        Some(Ok(freeflow_core::reference::RefMatch::Unique(id))) => id,
        Some(Ok(_)) | Some(Err(_)) => {
            let errors = OpportunityFormErrors {
                client: Some("client introuvable ou ambigu".to_string()),
                ..Default::default()
            };
            return Html(views::prospection::new_panel(&(&form).into(), &errors).into_string())
                .into_response();
        }
    };
    let parsed = match parse_opportunity_form(&form) {
        Ok(p) => p,
        Err(errors) => {
            return Html(views::prospection::new_panel(&(&form).into(), &errors).into_string())
                .into_response();
        }
    };
    let Some(next_action_at) = parsed.next_action_at else {
        let errors = OpportunityFormErrors {
            banner: Some("la prochaine action est obligatoire".to_string()),
            ..Default::default()
        };
        return Html(views::prospection::new_panel(&(&form).into(), &errors).into_string())
            .into_response();
    };
    let cmd = prospection::CreateOpportunity {
        client_id,
        name: parsed.name,
        amount: parsed.amount,
        probability: parsed.probability,
        next_action_at,
        source: parsed.source,
    };
    match execute(&state, cmd).await {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => saved(),
        Some(Err(e)) => {
            let errors = opportunity_error_banner(e, "/prospection/new");
            Html(views::prospection::new_panel(&(&form).into(), &errors).into_string())
                .into_response()
        }
    }
}

impl From<&OpportunityForm> for OpportunityFormValues {
    fn from(f: &OpportunityForm) -> Self {
        Self {
            client: f.client.clone(),
            name: f.name.clone(),
            amount: f.amount.clone(),
            probability: f.probability.clone(),
            next_action: f.next_action.clone(),
            source: f.source.clone(),
        }
    }
}

pub async fn show_panel(State(state): State<AppState>, Path(id): Path<String>) -> Html<String> {
    let Some(id) = parse_id::<OpportunityId>(&id) else {
        return message_fragment("identifiant d'opportunité invalide");
    };
    match state
        .with_store(|store| views::prospection::load_detail(store, id))
        .await
    {
        None => locked_fragment(),
        Some(Err(e)) => message_fragment(&e.to_string()),
        Some(Ok(None)) => {
            message_fragment("opportunité introuvable — elle a peut-être été supprimée entre-temps")
        }
        Some(Ok(Some((opportunity, interactions, refs)))) => state
            .with_store(|store| {
                Html(
                    views::prospection::detail_panel(
                        store,
                        &opportunity,
                        &interactions,
                        refs,
                        None,
                    )
                    .into_string(),
                )
            })
            .await
            .unwrap_or_else(locked_fragment),
    }
}

pub async fn edit_panel(State(state): State<AppState>, Path(id): Path<String>) -> Html<String> {
    let Some(id) = parse_id::<OpportunityId>(&id) else {
        return message_fragment("identifiant d'opportunité invalide");
    };
    match current_opportunity(&state, id).await {
        None => locked_fragment(),
        Some(Err(e)) => message_fragment(&e.to_string()),
        Some(Ok(None)) => message_fragment("opportunité introuvable"),
        Some(Ok(Some(o))) => {
            let values = OpportunityFormValues {
                client: String::new(),
                name: o.name.clone(),
                amount: o.amount.to_string(),
                probability: o.probability.percent().to_string(),
                next_action: o
                    .next_action_at
                    .map(freeflow_core::domain::format_date)
                    .unwrap_or_default(),
                source: o.source.clone().unwrap_or_default(),
            };
            Html(
                views::prospection::edit_panel(
                    id,
                    o.revision,
                    &values,
                    &OpportunityFormErrors::default(),
                )
                .into_string(),
            )
        }
    }
}

pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(form): Form<OpportunityForm>,
) -> Response {
    let Some(id) = parse_id::<OpportunityId>(&id) else {
        return message_fragment("identifiant d'opportunité invalide").into_response();
    };
    let revision: i64 = form
        .revision
        .as_deref()
        .and_then(|s| s.parse().ok())
        .unwrap_or_default();

    let parsed = match parse_opportunity_form(&form) {
        Ok(p) => p,
        Err(errors) => {
            return Html(
                views::prospection::edit_panel(id, revision, &(&form).into(), &errors)
                    .into_string(),
            )
            .into_response();
        }
    };
    let cmd = prospection::UpdateOpportunity {
        id,
        revision,
        name: parsed.name,
        amount: parsed.amount,
        probability: parsed.probability,
        next_action_at: parsed.next_action_at,
        source: parsed.source,
    };
    match execute(&state, cmd).await {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => saved(),
        Some(Err(e)) => {
            let errors = opportunity_error_banner(e, &format!("/prospection/{id}/edit"));
            Html(
                views::prospection::edit_panel(id, revision, &(&form).into(), &errors)
                    .into_string(),
            )
            .into_response()
        }
    }
}

pub async fn archive(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    set_archived(state, id, true).await
}

pub async fn unarchive(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    set_archived(state, id, false).await
}

async fn set_archived(state: AppState, id: String, archived: bool) -> Response {
    let Some(id) = parse_id::<OpportunityId>(&id) else {
        return message_fragment("identifiant d'opportunité invalide").into_response();
    };
    let revision = match current_opportunity(&state, id).await {
        None => return locked_fragment().into_response(),
        Some(Err(e)) => return message_fragment(&e.to_string()).into_response(),
        Some(Ok(None)) => return message_fragment("opportunité introuvable").into_response(),
        Some(Ok(Some(o))) => o.revision,
    };
    let outcome = if archived {
        execute(&state, prospection::ArchiveOpportunity { id, revision }).await
    } else {
        execute(&state, prospection::UnarchiveOpportunity { id, revision }).await
    };
    match outcome {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => saved(),
        Some(Err(e)) => reload_detail_with_error(&state, id, &e.to_string()).await,
    }
}

async fn reload_detail_with_error(state: &AppState, id: OpportunityId, message: &str) -> Response {
    match state
        .with_store(|store| views::prospection::load_detail(store, id))
        .await
    {
        Some(Ok(Some((opportunity, interactions, refs)))) => state
            .with_store(|store| {
                Html(
                    views::prospection::detail_panel(
                        store,
                        &opportunity,
                        &interactions,
                        refs,
                        Some(message),
                    )
                    .into_string(),
                )
                .into_response()
            })
            .await
            .unwrap_or_else(|| message_fragment(message).into_response()),
        _ => message_fragment(message).into_response(),
    }
}

pub async fn delete_confirm_panel(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Html<String> {
    let Some(id) = parse_id::<OpportunityId>(&id) else {
        return message_fragment("identifiant d'opportunité invalide");
    };
    match current_opportunity(&state, id).await {
        None => locked_fragment(),
        Some(Err(e)) => message_fragment(&e.to_string()),
        Some(Ok(None)) => message_fragment("opportunité introuvable"),
        Some(Ok(Some(o))) => Html(views::prospection::delete_confirm_panel(&o).into_string()),
    }
}

pub async fn delete(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let Some(id) = parse_id::<OpportunityId>(&id) else {
        return message_fragment("identifiant d'opportunité invalide").into_response();
    };
    let revision = match current_opportunity(&state, id).await {
        None => return locked_fragment().into_response(),
        Some(Err(e)) => return message_fragment(&e.to_string()).into_response(),
        Some(Ok(None)) => {
            return message_fragment("opportunité introuvable — déjà supprimée").into_response();
        }
        Some(Ok(Some(o))) => o.revision,
    };
    match execute(&state, prospection::DeleteOpportunity { id, revision }).await {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => saved(),
        Some(Err(e)) => match current_opportunity(&state, id).await {
            Some(Ok(Some(o))) => {
                let body = html! {
                    div class="form-error" { (e.to_string()) }
                    (views::prospection::delete_confirm_panel(&o))
                };
                Html(body.into_string()).into_response()
            }
            _ => message_fragment(&e.to_string()).into_response(),
        },
    }
}

// -- Transitions : avancer / gagner / perdre -------------------------------------------------

pub async fn advance_panel(State(state): State<AppState>, Path(id): Path<String>) -> Html<String> {
    let Some(id) = parse_id::<OpportunityId>(&id) else {
        return message_fragment("identifiant d'opportunité invalide");
    };
    match current_opportunity(&state, id).await {
        None => locked_fragment(),
        Some(Err(e)) => message_fragment(&e.to_string()),
        Some(Ok(None)) => message_fragment("opportunité introuvable"),
        Some(Ok(Some(o))) => {
            Html(views::prospection::advance_panel(&o, &TransitionErrors::default()).into_string())
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct AdvanceForm {
    to: String,
    next_action: String,
}

pub async fn advance(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(form): Form<AdvanceForm>,
) -> Response {
    let Some(opportunity_id) = parse_id::<OpportunityId>(&id) else {
        return message_fragment("identifiant d'opportunité invalide").into_response();
    };
    let Ok(to) = form.to.parse::<OpportunityStage>() else {
        return message_fragment("étape invalide").into_response();
    };
    let Ok(next_action_at) = freeflow_core::domain::parse_date(&form.next_action) else {
        return match current_opportunity(&state, opportunity_id).await {
            Some(Ok(Some(o))) => {
                let errors = TransitionErrors {
                    banner: Some("date invalide".to_string()),
                };
                Html(views::prospection::advance_panel(&o, &errors).into_string()).into_response()
            }
            _ => message_fragment("date invalide").into_response(),
        };
    };
    let cmd = prospection::AdvanceOpportunity {
        opportunity_id,
        to,
        next_action_at,
    };
    match execute(&state, cmd).await {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => saved(),
        Some(Err(e)) => reload_detail_with_error(&state, opportunity_id, &e.to_string()).await,
    }
}

pub async fn win_panel(State(state): State<AppState>, Path(id): Path<String>) -> Html<String> {
    let Some(id) = parse_id::<OpportunityId>(&id) else {
        return message_fragment("identifiant d'opportunité invalide");
    };
    match current_opportunity(&state, id).await {
        None => locked_fragment(),
        Some(Err(e)) => message_fragment(&e.to_string()),
        Some(Ok(None)) => message_fragment("opportunité introuvable"),
        Some(Ok(Some(o))) => {
            Html(views::prospection::win_panel(&o, &TransitionErrors::default()).into_string())
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct WinForm {
    started_on: String,
}

pub async fn win(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(form): Form<WinForm>,
) -> Response {
    let Some(opportunity_id) = parse_id::<OpportunityId>(&id) else {
        return message_fragment("identifiant d'opportunité invalide").into_response();
    };
    let Ok(started_on) = freeflow_core::domain::parse_date(&form.started_on) else {
        return match current_opportunity(&state, opportunity_id).await {
            Some(Ok(Some(o))) => {
                let errors = TransitionErrors {
                    banner: Some("date invalide".to_string()),
                };
                Html(views::prospection::win_panel(&o, &errors).into_string()).into_response()
            }
            _ => message_fragment("date invalide").into_response(),
        };
    };
    let cmd = prospection::WinOpportunity {
        opportunity_id,
        started_on,
    };
    match execute(&state, cmd).await {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => saved(),
        Some(Err(e)) => reload_detail_with_error(&state, opportunity_id, &e.to_string()).await,
    }
}

pub async fn lose_panel(State(state): State<AppState>, Path(id): Path<String>) -> Html<String> {
    let Some(id) = parse_id::<OpportunityId>(&id) else {
        return message_fragment("identifiant d'opportunité invalide");
    };
    match current_opportunity(&state, id).await {
        None => locked_fragment(),
        Some(Err(e)) => message_fragment(&e.to_string()),
        Some(Ok(None)) => message_fragment("opportunité introuvable"),
        Some(Ok(Some(o))) => {
            Html(views::prospection::lose_panel(&o, &TransitionErrors::default()).into_string())
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct LoseForm {
    reason: String,
    #[serde(default)]
    detail: String,
}

pub async fn lose(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(form): Form<LoseForm>,
) -> Response {
    let Some(opportunity_id) = parse_id::<OpportunityId>(&id) else {
        return message_fragment("identifiant d'opportunité invalide").into_response();
    };
    let reason = match views::prospection::parse_loss_reason(&form.reason, &form.detail) {
        Ok(r) => r,
        Err(msg) => {
            return match current_opportunity(&state, opportunity_id).await {
                Some(Ok(Some(o))) => {
                    let errors = TransitionErrors { banner: Some(msg) };
                    Html(views::prospection::lose_panel(&o, &errors).into_string()).into_response()
                }
                _ => message_fragment(&msg).into_response(),
            };
        }
    };
    let cmd = prospection::LoseOpportunity {
        opportunity_id,
        reason,
    };
    match execute(&state, cmd).await {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => saved(),
        Some(Err(e)) => reload_detail_with_error(&state, opportunity_id, &e.to_string()).await,
    }
}

// -- Interactions --------------------------------------------------------------------------

pub async fn new_interaction_panel(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Html<String> {
    let Some(id) = parse_id::<OpportunityId>(&id) else {
        return message_fragment("identifiant d'opportunité invalide");
    };
    match current_opportunity(&state, id).await {
        None => locked_fragment(),
        Some(Err(e)) => message_fragment(&e.to_string()),
        Some(Ok(None)) => message_fragment("opportunité introuvable"),
        Some(Ok(Some(o))) => Html(
            views::prospection::new_interaction_panel(
                &o,
                &InteractionFormValues::default(),
                &InteractionFormErrors::default(),
            )
            .into_string(),
        ),
    }
}

#[derive(Debug, Deserialize)]
pub struct InteractionForm {
    #[serde(default)]
    revision: Option<String>,
    kind: String,
    #[serde(default)]
    note: String,
    #[serde(default)]
    occurred_on: String,
}

impl From<&InteractionForm> for InteractionFormValues {
    fn from(f: &InteractionForm) -> Self {
        Self {
            kind: f.kind.clone(),
            note: f.note.clone(),
            occurred_on: f.occurred_on.clone(),
        }
    }
}

fn midnight_utc(date: time::Date) -> time::OffsetDateTime {
    date.with_hms(0, 0, 0)
        .expect("minuit est toujours une heure valide")
        .assume_utc()
}

pub async fn create_interaction(
    State(state): State<AppState>,
    Path(opportunity_id): Path<String>,
    Form(form): Form<InteractionForm>,
) -> Response {
    let Some(opportunity_id) = parse_id::<OpportunityId>(&opportunity_id) else {
        return message_fragment("identifiant d'opportunité invalide").into_response();
    };
    let kind = match views::prospection::parse_interaction_kind(&form.kind) {
        Ok(k) => k,
        Err(msg) => return message_fragment(&msg).into_response(),
    };
    let occurred_at = opt(&form.occurred_on)
        .and_then(|s| freeflow_core::domain::parse_date(&s).ok())
        .map(midnight_utc);
    let cmd = prospection::LogInteraction {
        opportunity_id,
        kind,
        note: form.note.trim().to_string(),
        occurred_at,
    };
    match execute(&state, cmd).await {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => reload_detail_or_saved(&state, opportunity_id).await,
        Some(Err(e)) => {
            let opportunity = match current_opportunity(&state, opportunity_id).await {
                Some(Ok(Some(o))) => o,
                _ => return message_fragment(&e.to_string()).into_response(),
            };
            let errors = interaction_error_banner(
                e,
                &format!("/prospection/{opportunity_id}/interactions/new"),
            );
            Html(
                views::prospection::new_interaction_panel(&opportunity, &(&form).into(), &errors)
                    .into_string(),
            )
            .into_response()
        }
    }
}

pub async fn edit_interaction_panel(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Html<String> {
    let Some(id) = parse_id::<InteractionId>(&id) else {
        return message_fragment("identifiant d'interaction invalide");
    };
    match state
        .with_store(|store| prospection::interaction_by_id(store.connection(), id))
        .await
    {
        None => locked_fragment(),
        Some(Err(e)) => message_fragment(&e.to_string()),
        Some(Ok(None)) => message_fragment("interaction introuvable"),
        Some(Ok(Some(i))) => Html(
            views::prospection::edit_interaction_panel(
                &i,
                &(&i).into(),
                &InteractionFormErrors::default(),
            )
            .into_string(),
        ),
    }
}

pub async fn update_interaction(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(form): Form<InteractionForm>,
) -> Response {
    let Some(id) = parse_id::<InteractionId>(&id) else {
        return message_fragment("identifiant d'interaction invalide").into_response();
    };
    let current = match state
        .with_store(|store| prospection::interaction_by_id(store.connection(), id))
        .await
    {
        None => return locked_fragment().into_response(),
        Some(Err(e)) => return message_fragment(&e.to_string()).into_response(),
        Some(Ok(None)) => return message_fragment("interaction introuvable").into_response(),
        Some(Ok(Some(i))) => i,
    };
    let kind = match views::prospection::parse_interaction_kind(&form.kind) {
        Ok(k) => k,
        Err(msg) => {
            let errors = InteractionFormErrors {
                banner: Some(msg),
                ..Default::default()
            };
            return Html(
                views::prospection::edit_interaction_panel(&current, &(&form).into(), &errors)
                    .into_string(),
            )
            .into_response();
        }
    };
    let occurred_at = opt(&form.occurred_on)
        .and_then(|s| freeflow_core::domain::parse_date(&s).ok())
        .map_or(current.occurred_at, midnight_utc);
    let revision: i64 = form
        .revision
        .as_deref()
        .and_then(|s| s.parse().ok())
        .unwrap_or(current.revision);
    let opportunity_id = current.opportunity_id;
    let cmd = prospection::UpdateInteraction {
        id,
        revision,
        kind,
        note: form.note.trim().to_string(),
        occurred_at,
    };
    match execute(&state, cmd).await {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => reload_detail_or_saved(&state, opportunity_id).await,
        Some(Err(e)) => {
            let errors = interaction_error_banner(e, &format!("/interactions/{id}/edit"));
            Html(
                views::prospection::edit_interaction_panel(&current, &(&form).into(), &errors)
                    .into_string(),
            )
            .into_response()
        }
    }
}

pub async fn delete_interaction(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let Some(id) = parse_id::<InteractionId>(&id) else {
        return message_fragment("identifiant d'interaction invalide").into_response();
    };
    let current = match state
        .with_store(|store| prospection::interaction_by_id(store.connection(), id))
        .await
    {
        None => return locked_fragment().into_response(),
        Some(Err(e)) => return message_fragment(&e.to_string()).into_response(),
        Some(Ok(None)) => {
            return message_fragment("interaction introuvable — déjà supprimée").into_response();
        }
        Some(Ok(Some(i))) => i,
    };
    let opportunity_id = current.opportunity_id;
    match execute(
        &state,
        prospection::DeleteInteraction {
            id,
            revision: current.revision,
        },
    )
    .await
    {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => reload_detail_or_saved(&state, opportunity_id).await,
        Some(Err(e)) => message_fragment(&e.to_string()).into_response(),
    }
}

async fn reload_detail_or_saved(state: &AppState, opportunity_id: OpportunityId) -> Response {
    match state
        .with_store(|store| views::prospection::load_detail(store, opportunity_id))
        .await
    {
        Some(Ok(Some((opportunity, interactions, refs)))) => state
            .with_store(|store| {
                Html(
                    views::prospection::detail_panel(
                        store,
                        &opportunity,
                        &interactions,
                        refs,
                        None,
                    )
                    .into_string(),
                )
                .into_response()
            })
            .await
            .unwrap_or_else(saved),
        _ => saved(),
    }
}
