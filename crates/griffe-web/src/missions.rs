//! Routes de mutation `missions` — même patron et même convention de réponse que `crate::clients`
//! (lot 15) et `crate::prospection` (lot 16).

use axum::Form;
use axum::extract::{Path, Query, State};
use axum::http::HeaderValue;
use axum::response::{Html, IntoResponse, Response};
use griffe_core::app::{AppError, Executor, Outcome};
use griffe_core::domain::{Mission, MissionId, MissionKind, Money, TimeCategory, TimeEntryId};
use griffe_core::missions::{self, MissionFilter};
use maud::html;
use serde::Deserialize;

use crate::state::AppState;
use crate::views;
use crate::views::missions::{
    MissionFormErrors, MissionFormValues, TimeEntryFormErrors, TimeEntryFormValues,
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
        .insert("HX-Trigger", HeaderValue::from_static("griffe:saved"));
    response
}

async fn execute<C: griffe_core::app::Command>(
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

fn mission_error_banner(e: AppError, reload_hx_get: &str) -> MissionFormErrors {
    match e {
        AppError::Conflict { .. } => MissionFormErrors {
            conflict: Some((e.to_string(), reload_hx_get.to_string())),
            ..Default::default()
        },
        other => MissionFormErrors {
            banner: Some(other.to_string()),
            ..Default::default()
        },
    }
}

fn time_entry_error_banner(e: AppError, reload_hx_get: &str) -> TimeEntryFormErrors {
    match e {
        AppError::Conflict { .. } => TimeEntryFormErrors {
            conflict: Some((e.to_string(), reload_hx_get.to_string())),
            ..Default::default()
        },
        other => TimeEntryFormErrors {
            banner: Some(other.to_string()),
            ..Default::default()
        },
    }
}

async fn current_mission(
    state: &AppState,
    id: MissionId,
) -> Option<Result<Option<Mission>, AppError>> {
    state
        .with_store(|store| missions::mission_by_id(store.connection(), id))
        .await
}

fn parse_mission_kind(kind: &str, amount: &str) -> Result<MissionKind, String> {
    let amount = Money::parse_decimal(amount).map_err(|e| e.to_string())?;
    match kind {
        "regie" => Ok(MissionKind::Regie { daily_rate: amount }),
        "forfait" => Ok(MissionKind::Forfait { budget: amount }),
        "recurrent" => Ok(MissionKind::Recurrent {
            monthly_amount: amount,
        }),
        other => Err(format!("type invalide : {other}")),
    }
}

fn opt(s: &str) -> Option<String> {
    let trimmed = s.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

// -- Liste ---------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct TableQuery {
    #[serde(default)]
    ended: bool,
    #[serde(default)]
    archived: bool,
}

pub async fn table(State(state): State<AppState>, Query(q): Query<TableQuery>) -> Html<String> {
    let filter = MissionFilter {
        include_ended: q.ended,
        include_archived: q.archived,
    };
    match state
        .with_store(|store| views::missions::list_fragment(store, filter))
        .await
    {
        None => locked_fragment(),
        Some(Ok(markup)) => Html(markup.into_string()),
        Some(Err(e)) => message_fragment(&e.to_string()),
    }
}

// -- Mission : créer / afficher / modifier / clôturer / archiver / supprimer -----------------

pub async fn new_panel() -> Html<String> {
    Html(
        views::missions::new_panel(&MissionFormValues::default(), &MissionFormErrors::default())
            .into_string(),
    )
}

#[derive(Debug, Deserialize)]
pub struct MissionForm {
    #[serde(default)]
    revision: Option<String>,
    #[serde(default)]
    client: String,
    name: String,
    kind: String,
    amount: String,
    #[serde(default)]
    started_on: String,
    #[serde(default)]
    milestones: String,
}

impl From<&MissionForm> for MissionFormValues {
    fn from(f: &MissionForm) -> Self {
        Self {
            client: f.client.clone(),
            name: f.name.clone(),
            kind: f.kind.clone(),
            amount: f.amount.clone(),
            started_on: f.started_on.clone(),
            milestones: f.milestones.clone(),
        }
    }
}

pub async fn create(State(state): State<AppState>, Form(form): Form<MissionForm>) -> Response {
    let client_id = match state
        .with_store(|store| {
            griffe_core::reference::resolve_client(store.connection(), &form.client)
        })
        .await
    {
        None => return locked_fragment().into_response(),
        Some(Ok(griffe_core::reference::RefMatch::Unique(id))) => id,
        Some(Ok(_)) | Some(Err(_)) => {
            let errors = MissionFormErrors {
                client: Some("client introuvable ou ambigu".to_string()),
                ..Default::default()
            };
            return Html(views::missions::new_panel(&(&form).into(), &errors).into_string())
                .into_response();
        }
    };
    if form.name.trim().is_empty() {
        let errors = MissionFormErrors {
            name: Some("le nom est obligatoire".to_string()),
            ..Default::default()
        };
        return Html(views::missions::new_panel(&(&form).into(), &errors).into_string())
            .into_response();
    }
    let kind = match parse_mission_kind(&form.kind, &form.amount) {
        Ok(k) => k,
        Err(msg) => {
            let errors = MissionFormErrors {
                amount: Some(msg),
                ..Default::default()
            };
            return Html(views::missions::new_panel(&(&form).into(), &errors).into_string())
                .into_response();
        }
    };
    let Ok(started_on) = griffe_core::domain::parse_date(&form.started_on) else {
        let errors = MissionFormErrors {
            banner: Some("date de début invalide".to_string()),
            ..Default::default()
        };
        return Html(views::missions::new_panel(&(&form).into(), &errors).into_string())
            .into_response();
    };
    let milestones = match views::missions::parse_milestones(&form.milestones) {
        Ok(m) => m,
        Err(msg) => {
            let errors = MissionFormErrors {
                milestones: Some(msg),
                ..Default::default()
            };
            return Html(views::missions::new_panel(&(&form).into(), &errors).into_string())
                .into_response();
        }
    };
    let cmd = missions::CreateMission {
        client_id,
        quote_id: None,
        name: form.name.trim().to_string(),
        kind,
        milestones,
        started_on,
    };
    match execute(&state, cmd).await {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => saved(),
        Some(Err(e)) => {
            let errors = mission_error_banner(e, "/missions/new");
            Html(views::missions::new_panel(&(&form).into(), &errors).into_string()).into_response()
        }
    }
}

pub async fn show_panel(State(state): State<AppState>, Path(id): Path<String>) -> Html<String> {
    let Some(id) = parse_id::<MissionId>(&id) else {
        return message_fragment("identifiant de mission invalide");
    };
    match state
        .with_store(|store| views::missions::load_detail(store, id))
        .await
    {
        None => locked_fragment(),
        Some(Err(e)) => message_fragment(&e.to_string()),
        Some(Ok(None)) => {
            message_fragment("mission introuvable — elle a peut-être été supprimée entre-temps")
        }
        Some(Ok(Some((mission, time_entries, refs)))) => state
            .with_store(|store| {
                match views::missions::detail_panel(store, &mission, &time_entries, refs, None) {
                    Ok(markup) => Html(markup.into_string()),
                    Err(e) => message_fragment(&e.to_string()),
                }
            })
            .await
            .unwrap_or_else(locked_fragment),
    }
}

pub async fn edit_panel(State(state): State<AppState>, Path(id): Path<String>) -> Html<String> {
    let Some(id) = parse_id::<MissionId>(&id) else {
        return message_fragment("identifiant de mission invalide");
    };
    match state
        .with_store(|store| {
            missions::mission_by_id(store.connection(), id)
                .map(|opt| opt.map(|m| (MissionFormValues::from_mission(store, &m), m.revision)))
        })
        .await
    {
        None => locked_fragment(),
        Some(Err(e)) => message_fragment(&e.to_string()),
        Some(Ok(None)) => message_fragment("mission introuvable"),
        Some(Ok(Some((values, revision)))) => Html(
            views::missions::edit_panel(id, revision, &values, &MissionFormErrors::default())
                .into_string(),
        ),
    }
}

pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(form): Form<MissionForm>,
) -> Response {
    let Some(id) = parse_id::<MissionId>(&id) else {
        return message_fragment("identifiant de mission invalide").into_response();
    };
    let revision: i64 = form
        .revision
        .as_deref()
        .and_then(|s| s.parse().ok())
        .unwrap_or_default();
    let current = match current_mission(&state, id).await {
        None => return locked_fragment().into_response(),
        Some(Err(e)) => return message_fragment(&e.to_string()).into_response(),
        Some(Ok(None)) => return message_fragment("mission introuvable").into_response(),
        Some(Ok(Some(m))) => m,
    };
    if form.name.trim().is_empty() {
        let errors = MissionFormErrors {
            name: Some("le nom est obligatoire".to_string()),
            ..Default::default()
        };
        return Html(
            views::missions::edit_panel(id, revision, &(&form).into(), &errors).into_string(),
        )
        .into_response();
    }
    let kind = match parse_mission_kind(&form.kind, &form.amount) {
        Ok(k) => k,
        Err(msg) => {
            let errors = MissionFormErrors {
                amount: Some(msg),
                ..Default::default()
            };
            return Html(
                views::missions::edit_panel(id, revision, &(&form).into(), &errors).into_string(),
            )
            .into_response();
        }
    };
    let Ok(started_on) = griffe_core::domain::parse_date(&form.started_on) else {
        let errors = MissionFormErrors {
            banner: Some("date de début invalide".to_string()),
            ..Default::default()
        };
        return Html(
            views::missions::edit_panel(id, revision, &(&form).into(), &errors).into_string(),
        )
        .into_response();
    };
    let milestones = match views::missions::parse_milestones(&form.milestones) {
        Ok(m) => m,
        Err(msg) => {
            let errors = MissionFormErrors {
                milestones: Some(msg),
                ..Default::default()
            };
            return Html(
                views::missions::edit_panel(id, revision, &(&form).into(), &errors).into_string(),
            )
            .into_response();
        }
    };
    let cmd = missions::UpdateMission {
        id,
        revision,
        name: form.name.trim().to_string(),
        kind,
        milestones,
        started_on,
        quote_id: current.quote_id,
    };
    match execute(&state, cmd).await {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => saved(),
        Some(Err(e)) => {
            let errors = mission_error_banner(e, &format!("/missions/{id}/edit"));
            Html(views::missions::edit_panel(id, revision, &(&form).into(), &errors).into_string())
                .into_response()
        }
    }
}

async fn reload_detail_with_error(state: &AppState, id: MissionId, message: &str) -> Response {
    match state
        .with_store(|store| views::missions::load_detail(store, id))
        .await
    {
        Some(Ok(Some((mission, time_entries, refs)))) => state
            .with_store(|store| {
                match views::missions::detail_panel(
                    store,
                    &mission,
                    &time_entries,
                    refs,
                    Some(message),
                ) {
                    Ok(markup) => Html(markup.into_string()).into_response(),
                    Err(e) => message_fragment(&e.to_string()).into_response(),
                }
            })
            .await
            .unwrap_or_else(|| message_fragment(message).into_response()),
        _ => message_fragment(message).into_response(),
    }
}

pub async fn close_panel(State(state): State<AppState>, Path(id): Path<String>) -> Html<String> {
    let Some(id) = parse_id::<MissionId>(&id) else {
        return message_fragment("identifiant de mission invalide");
    };
    match current_mission(&state, id).await {
        None => locked_fragment(),
        Some(Err(e)) => message_fragment(&e.to_string()),
        Some(Ok(None)) => message_fragment("mission introuvable"),
        Some(Ok(Some(m))) => {
            Html(views::missions::close_panel(&m, &TransitionErrors::default()).into_string())
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct CloseForm {
    ended_on: String,
}

pub async fn close(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(form): Form<CloseForm>,
) -> Response {
    let Some(id) = parse_id::<MissionId>(&id) else {
        return message_fragment("identifiant de mission invalide").into_response();
    };
    let revision = match current_mission(&state, id).await {
        None => return locked_fragment().into_response(),
        Some(Err(e)) => return message_fragment(&e.to_string()).into_response(),
        Some(Ok(None)) => return message_fragment("mission introuvable").into_response(),
        Some(Ok(Some(m))) => m.revision,
    };
    let Ok(ended_on) = griffe_core::domain::parse_date(&form.ended_on) else {
        return match current_mission(&state, id).await {
            Some(Ok(Some(m))) => {
                let errors = TransitionErrors {
                    banner: Some("date invalide".to_string()),
                };
                Html(views::missions::close_panel(&m, &errors).into_string()).into_response()
            }
            _ => message_fragment("date invalide").into_response(),
        };
    };
    match execute(
        &state,
        missions::CloseMission {
            id,
            revision,
            ended_on,
        },
    )
    .await
    {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => saved(),
        Some(Err(e)) => reload_detail_with_error(&state, id, &e.to_string()).await,
    }
}

pub async fn reopen(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let Some(id) = parse_id::<MissionId>(&id) else {
        return message_fragment("identifiant de mission invalide").into_response();
    };
    let revision = match current_mission(&state, id).await {
        None => return locked_fragment().into_response(),
        Some(Err(e)) => return message_fragment(&e.to_string()).into_response(),
        Some(Ok(None)) => return message_fragment("mission introuvable").into_response(),
        Some(Ok(Some(m))) => m.revision,
    };
    match execute(&state, missions::ReopenMission { id, revision }).await {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => saved(),
        Some(Err(e)) => reload_detail_with_error(&state, id, &e.to_string()).await,
    }
}

pub async fn archive(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    set_archived(state, id, true).await
}

pub async fn unarchive(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    set_archived(state, id, false).await
}

async fn set_archived(state: AppState, id: String, archived: bool) -> Response {
    let Some(id) = parse_id::<MissionId>(&id) else {
        return message_fragment("identifiant de mission invalide").into_response();
    };
    let revision = match current_mission(&state, id).await {
        None => return locked_fragment().into_response(),
        Some(Err(e)) => return message_fragment(&e.to_string()).into_response(),
        Some(Ok(None)) => return message_fragment("mission introuvable").into_response(),
        Some(Ok(Some(m))) => m.revision,
    };
    let outcome = if archived {
        execute(&state, missions::ArchiveMission { id, revision }).await
    } else {
        execute(&state, missions::UnarchiveMission { id, revision }).await
    };
    match outcome {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => saved(),
        Some(Err(e)) => reload_detail_with_error(&state, id, &e.to_string()).await,
    }
}

pub async fn delete_confirm_panel(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Html<String> {
    let Some(id) = parse_id::<MissionId>(&id) else {
        return message_fragment("identifiant de mission invalide");
    };
    match current_mission(&state, id).await {
        None => locked_fragment(),
        Some(Err(e)) => message_fragment(&e.to_string()),
        Some(Ok(None)) => message_fragment("mission introuvable"),
        Some(Ok(Some(m))) => Html(views::missions::delete_confirm_panel(&m).into_string()),
    }
}

pub async fn delete(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let Some(id) = parse_id::<MissionId>(&id) else {
        return message_fragment("identifiant de mission invalide").into_response();
    };
    let revision = match current_mission(&state, id).await {
        None => return locked_fragment().into_response(),
        Some(Err(e)) => return message_fragment(&e.to_string()).into_response(),
        Some(Ok(None)) => {
            return message_fragment("mission introuvable — déjà supprimée").into_response();
        }
        Some(Ok(Some(m))) => m.revision,
    };
    match execute(&state, missions::DeleteMission { id, revision }).await {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => saved(),
        Some(Err(e)) => match current_mission(&state, id).await {
            Some(Ok(Some(m))) => {
                let body = html! {
                    div class="form-error" { (e.to_string()) }
                    (views::missions::delete_confirm_panel(&m))
                };
                Html(body.into_string()).into_response()
            }
            _ => message_fragment(&e.to_string()).into_response(),
        },
    }
}

// -- Temps -----------------------------------------------------------------------------------

pub async fn new_time_entry_panel(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Html<String> {
    let Some(id) = parse_id::<MissionId>(&id) else {
        return message_fragment("identifiant de mission invalide");
    };
    match current_mission(&state, id).await {
        None => locked_fragment(),
        Some(Err(e)) => message_fragment(&e.to_string()),
        Some(Ok(None)) => message_fragment("mission introuvable"),
        Some(Ok(Some(m))) => Html(
            views::missions::new_time_entry_panel(
                &m,
                &TimeEntryFormValues::default(),
                &TimeEntryFormErrors::default(),
            )
            .into_string(),
        ),
    }
}

#[derive(Debug, Deserialize)]
pub struct TimeEntryForm {
    #[serde(default)]
    revision: Option<String>,
    #[serde(default)]
    worked_on: String,
    days: String,
    category: String,
    #[serde(default)]
    note: String,
}

impl From<&TimeEntryForm> for TimeEntryFormValues {
    fn from(f: &TimeEntryForm) -> Self {
        Self {
            worked_on: f.worked_on.clone(),
            days: f.days.clone(),
            category: f.category.clone(),
            note: f.note.clone(),
        }
    }
}

pub async fn create_time_entry(
    State(state): State<AppState>,
    Path(mission_id): Path<String>,
    Form(form): Form<TimeEntryForm>,
) -> Response {
    let Some(mission_id) = parse_id::<MissionId>(&mission_id) else {
        return message_fragment("identifiant de mission invalide").into_response();
    };
    let mission = match current_mission(&state, mission_id).await {
        Some(Ok(Some(m))) => m,
        None => return locked_fragment().into_response(),
        _ => return message_fragment("mission introuvable").into_response(),
    };
    let days: f64 = match form.days.trim().parse::<f64>() {
        Ok(d) if d.is_finite() && d > 0.0 => d,
        _ => {
            let errors = TimeEntryFormErrors {
                days: Some("nombre de jours invalide (doit être positif)".to_string()),
                ..Default::default()
            };
            return Html(
                views::missions::new_time_entry_panel(&mission, &(&form).into(), &errors)
                    .into_string(),
            )
            .into_response();
        }
    };
    let Ok(category) = form.category.parse::<TimeCategory>() else {
        return message_fragment("catégorie invalide").into_response();
    };
    let Ok(worked_on) = griffe_core::domain::parse_date(&form.worked_on) else {
        let errors = TimeEntryFormErrors {
            banner: Some("date invalide".to_string()),
            ..Default::default()
        };
        return Html(
            views::missions::new_time_entry_panel(&mission, &(&form).into(), &errors).into_string(),
        )
        .into_response();
    };
    let cmd = missions::LogTime {
        mission_id,
        worked_on,
        days,
        category,
        note: opt(&form.note),
    };
    match execute(&state, cmd).await {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => reload_detail_or_saved(&state, mission_id).await,
        Some(Err(e)) => {
            let errors = time_entry_error_banner(e, &format!("/missions/{mission_id}/time/new"));
            Html(
                views::missions::new_time_entry_panel(&mission, &(&form).into(), &errors)
                    .into_string(),
            )
            .into_response()
        }
    }
}

pub async fn edit_time_entry_panel(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Html<String> {
    let Some(id) = parse_id::<TimeEntryId>(&id) else {
        return message_fragment("identifiant de saisie de temps invalide");
    };
    match state
        .with_store(|store| missions::time_entry_by_id(store.connection(), id))
        .await
    {
        None => locked_fragment(),
        Some(Err(e)) => message_fragment(&e.to_string()),
        Some(Ok(None)) => message_fragment("saisie de temps introuvable"),
        Some(Ok(Some(entry))) => Html(
            views::missions::edit_time_entry_panel(
                &entry,
                &(&entry).into(),
                &TimeEntryFormErrors::default(),
            )
            .into_string(),
        ),
    }
}

pub async fn update_time_entry(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(form): Form<TimeEntryForm>,
) -> Response {
    let Some(id) = parse_id::<TimeEntryId>(&id) else {
        return message_fragment("identifiant de saisie de temps invalide").into_response();
    };
    let current = match state
        .with_store(|store| missions::time_entry_by_id(store.connection(), id))
        .await
    {
        None => return locked_fragment().into_response(),
        Some(Err(e)) => return message_fragment(&e.to_string()).into_response(),
        Some(Ok(None)) => return message_fragment("saisie de temps introuvable").into_response(),
        Some(Ok(Some(e))) => e,
    };
    let days: f64 = match form.days.trim().parse::<f64>() {
        Ok(d) if d.is_finite() && d > 0.0 => d,
        _ => {
            let errors = TimeEntryFormErrors {
                days: Some("nombre de jours invalide (doit être positif)".to_string()),
                ..Default::default()
            };
            return Html(
                views::missions::edit_time_entry_panel(&current, &(&form).into(), &errors)
                    .into_string(),
            )
            .into_response();
        }
    };
    let Ok(category) = form.category.parse::<TimeCategory>() else {
        return message_fragment("catégorie invalide").into_response();
    };
    let worked_on = griffe_core::domain::parse_date(&form.worked_on).unwrap_or(current.worked_on);
    let revision: i64 = form
        .revision
        .as_deref()
        .and_then(|s| s.parse().ok())
        .unwrap_or(current.revision);
    let mission_id = current.mission_id;
    let cmd = missions::UpdateTimeEntry {
        id,
        revision,
        worked_on,
        days,
        category,
        note: opt(&form.note),
    };
    match execute(&state, cmd).await {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => reload_detail_or_saved(&state, mission_id).await,
        Some(Err(e)) => {
            let errors = time_entry_error_banner(e, &format!("/time-entries/{id}/edit"));
            Html(
                views::missions::edit_time_entry_panel(&current, &(&form).into(), &errors)
                    .into_string(),
            )
            .into_response()
        }
    }
}

pub async fn delete_time_entry(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let Some(id) = parse_id::<TimeEntryId>(&id) else {
        return message_fragment("identifiant de saisie de temps invalide").into_response();
    };
    let current = match state
        .with_store(|store| missions::time_entry_by_id(store.connection(), id))
        .await
    {
        None => return locked_fragment().into_response(),
        Some(Err(e)) => return message_fragment(&e.to_string()).into_response(),
        Some(Ok(None)) => {
            return message_fragment("saisie de temps introuvable — déjà supprimée")
                .into_response();
        }
        Some(Ok(Some(e))) => e,
    };
    let mission_id = current.mission_id;
    match execute(
        &state,
        missions::DeleteTimeEntry {
            id,
            revision: current.revision,
        },
    )
    .await
    {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => reload_detail_or_saved(&state, mission_id).await,
        Some(Err(e)) => message_fragment(&e.to_string()).into_response(),
    }
}

async fn reload_detail_or_saved(state: &AppState, mission_id: MissionId) -> Response {
    match state
        .with_store(|store| views::missions::load_detail(store, mission_id))
        .await
    {
        Some(Ok(Some((mission, time_entries, refs)))) => state
            .with_store(|store| {
                match views::missions::detail_panel(store, &mission, &time_entries, refs, None) {
                    Ok(markup) => Html(markup.into_string()).into_response(),
                    Err(_) => saved(),
                }
            })
            .await
            .unwrap_or_else(saved),
        _ => saved(),
    }
}
