//! Routes `GET /view/*` : chaque écran rend son contenu ; si la requête vient d'une navigation
//! htmx boostée (`HX-Request`), seul ce contenu est renvoyé — sinon la page complète, coque
//! comprise, pour un chargement direct ou un rechargement.
//!
//! Ces handlers ne sont atteints que le coffre déverrouillé : [`crate::unlock::require_unlocked`]
//! redirige toute autre requête vers l'écran de déverrouillage avant d'y arriver. Ils restent
//! défensifs (`with_store` renvoie `None` plutôt que de paniquer) au cas où l'état basculerait
//! entre le passage du middleware et l'exécution du handler.

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Html;
use maud::{Markup, html};

use crate::layout::{self, ViewId};
use crate::state::AppState;
use crate::views;

fn is_htmx_request(headers: &HeaderMap) -> bool {
    headers.contains_key("hx-request")
}

fn error_markup(active: ViewId, err: impl std::fmt::Display) -> Markup {
    html! {
        div class="empty-state" { "erreur de lecture : " (err.to_string()) " (" (active.slug()) ")" }
    }
}

fn locked_markup(active: ViewId) -> Markup {
    html! {
        div class="empty-state" { "coffre verrouillé (" (active.slug()) ") — rechargez la page" }
    }
}

async fn respond(headers: HeaderMap, active: ViewId, content: Markup) -> Html<String> {
    if is_htmx_request(&headers) {
        Html(content.into_string())
    } else {
        Html(layout::page(active, "déverrouillé", content).into_string())
    }
}

/// Le tableau de bord complet (coque comprise), en dehors de tout contexte de requête HTTP —
/// utilisé par [`crate::unlock`] pour atterrir directement dessus après un déverrouillage ou une
/// création réussis, sans passer par une redirection `Location` : le protocole URI custom de la
/// coque Tauri ne la suit pas de façon fiable pour une navigation de premier niveau (WebKitGTK),
/// contrairement à `HX-Redirect`, qui est piloté par le JavaScript d'htmx plutôt que par le
/// moteur de rendu lui-même.
pub async fn dashboard_page(state: &AppState) -> String {
    let content = state
        .with_store(|store| {
            views::dashboard::render(store).unwrap_or_else(|e| error_markup(ViewId::Dashboard, e))
        })
        .await
        .unwrap_or_else(|| locked_markup(ViewId::Dashboard));
    layout::page(ViewId::Dashboard, "déverrouillé", content).into_string()
}

pub async fn dashboard(State(state): State<AppState>, headers: HeaderMap) -> Html<String> {
    let content = state
        .with_store(|store| {
            views::dashboard::render(store).unwrap_or_else(|e| error_markup(ViewId::Dashboard, e))
        })
        .await
        .unwrap_or_else(|| locked_markup(ViewId::Dashboard));
    respond(headers, ViewId::Dashboard, content).await
}

pub async fn prospection(State(state): State<AppState>, headers: HeaderMap) -> Html<String> {
    let content = state
        .with_store(|store| {
            views::prospection::render(store, freeflow_core::prospection::OpportunityFilter::OPEN)
                .unwrap_or_else(|e| error_markup(ViewId::Prospection, e))
        })
        .await
        .unwrap_or_else(|| locked_markup(ViewId::Prospection));
    respond(headers, ViewId::Prospection, content).await
}

pub async fn missions(State(state): State<AppState>, headers: HeaderMap) -> Html<String> {
    let content = state
        .with_store(|store| {
            views::missions::render(store, freeflow_core::missions::MissionFilter::ACTIVE)
                .unwrap_or_else(|e| error_markup(ViewId::Missions, e))
        })
        .await
        .unwrap_or_else(|| locked_markup(ViewId::Missions));
    respond(headers, ViewId::Missions, content).await
}

pub async fn facturation(State(state): State<AppState>, headers: HeaderMap) -> Html<String> {
    let content = state
        .with_store(|store| {
            views::facturation::render(store, state.today())
                .unwrap_or_else(|e| error_markup(ViewId::Facturation, e))
        })
        .await
        .unwrap_or_else(|| locked_markup(ViewId::Facturation));
    respond(headers, ViewId::Facturation, content).await
}

pub async fn clients(State(state): State<AppState>, headers: HeaderMap) -> Html<String> {
    let content = state
        .with_store(|store| {
            views::clients::render(store, freeflow_core::clients::ClientFilter::ActiveOnly)
                .unwrap_or_else(|e| error_markup(ViewId::Clients, e))
        })
        .await
        .unwrap_or_else(|| locked_markup(ViewId::Clients));
    respond(headers, ViewId::Clients, content).await
}

pub async fn devis(State(state): State<AppState>, headers: HeaderMap) -> Html<String> {
    let content = state
        .with_store(|store| {
            views::devis::render(store).unwrap_or_else(|e| error_markup(ViewId::Devis, e))
        })
        .await
        .unwrap_or_else(|| locked_markup(ViewId::Devis));
    respond(headers, ViewId::Devis, content).await
}

pub async fn depenses(State(state): State<AppState>, headers: HeaderMap) -> Html<String> {
    let content = state
        .with_store(|store| {
            views::depenses::render(store).unwrap_or_else(|e| error_markup(ViewId::Depenses, e))
        })
        .await
        .unwrap_or_else(|| locked_markup(ViewId::Depenses));
    respond(headers, ViewId::Depenses, content).await
}

pub async fn cloture(State(state): State<AppState>, headers: HeaderMap) -> Html<String> {
    let today = state.today();
    let content = state
        .with_store(|store| {
            views::cloture::render(store, today)
                .unwrap_or_else(|e| error_markup(ViewId::Cloture, e))
        })
        .await
        .unwrap_or_else(|| locked_markup(ViewId::Cloture));
    respond(headers, ViewId::Cloture, content).await
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
    dashboard(State(state), headers).await
}
