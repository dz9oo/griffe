//! Routes `GET /view/*` : chaque écran rend son contenu ; si la requête vient d'une navigation
//! htmx boostée (`HX-Request`), seul ce contenu est renvoyé — sinon la page complète, coque
//! comprise, pour un chargement direct ou un rechargement.
//!
//! Ces handlers ne sont atteints que le coffre déverrouillé : [`crate::unlock::require_unlocked`]
//! redirige toute autre requête vers l'écran de déverrouillage avant d'y arriver. Ils restent
//! défensifs (`with_store` renvoie `None` plutôt que de paniquer) au cas où l'état basculerait
//! entre le passage du middleware et l'exécution du handler.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use maud::{Markup, html};

use crate::layout::{self, ViewId};
use crate::state::AppState;
use crate::views;
use freeflow_core::fiscal::FiscalDeadlineKind;
use freeflow_core::society::duty_briefing;

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
        &freeflow_core::store::Store,
        time::Date,
    ) -> Result<Markup, freeflow_core::app::AppError>,
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
    let Some(kind) = parse_external_kind(&kind) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    letter(&state, headers, ViewId::Societe, move |store, today| {
        views::societe::duty(store, today, kind)
    })
    .await
    .into_response()
}

pub async fn societe_duty_open(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(kind): Path<String>,
) -> Response {
    let Some(kind) = parse_external_kind(&kind) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let today = state.today();
    state
        .with_store(|store| {
            if let Ok(briefing) = duty_briefing(store.connection(), kind, today) {
                open_allowed_url(briefing.url);
            }
        })
        .await;
    letter(&state, headers, ViewId::Societe, move |store, today| {
        views::societe::duty(store, today, kind)
    })
    .await
    .into_response()
}

fn parse_external_kind(raw: &str) -> Option<FiscalDeadlineKind> {
    let kind = FiscalDeadlineKind::parse(raw)?;
    (kind != FiscalDeadlineKind::ApprovalMeeting).then_some(kind)
}

fn open_allowed_url(url: &str) {
    const ALLOWED: &[&str] = &["https://www.impots.gouv.fr/", "https://procedures.inpi.fr/"];
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

pub async fn societe_statement(State(state): State<AppState>, headers: HeaderMap) -> Html<String> {
    letter(&state, headers, ViewId::Societe, views::societe::statement).await
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
    letter(&state, headers, ViewId::Gens, views::gens::render).await
}

pub async fn clients(State(state): State<AppState>, headers: HeaderMap) -> Html<String> {
    letter(&state, headers, ViewId::Gens, views::gens::render).await
}

pub async fn devis(State(state): State<AppState>, headers: HeaderMap) -> Html<String> {
    letter(&state, headers, ViewId::Gens, views::gens::render).await
}

pub async fn depenses(State(state): State<AppState>, headers: HeaderMap) -> Html<String> {
    letter(&state, headers, ViewId::Depenses, views::societe::statement).await
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

pub async fn lexique() -> Html<String> {
    Html(views::lexique::panel().into_string())
}

pub async fn console(headers: HeaderMap) -> Html<String> {
    respond(headers, ViewId::Console, views::console::render()).await
}

pub async fn index(State(state): State<AppState>, headers: HeaderMap) -> Html<String> {
    jour(State(state), headers).await
}
