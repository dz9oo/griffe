//! Routes `GET /view/*` : chaque écran rend son contenu ; si la requête vient d'une navigation
//! htmx boostée (`HX-Request`), seul ce contenu est renvoyé — sinon la page complète, coque
//! comprise, pour un chargement direct ou un rechargement.

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

async fn respond(headers: HeaderMap, active: ViewId, content: Markup) -> Html<String> {
    if is_htmx_request(&headers) {
        Html(content.into_string())
    } else {
        Html(layout::page(active, "déverrouillé", content).into_string())
    }
}

pub async fn dashboard(State(state): State<AppState>, headers: HeaderMap) -> Html<String> {
    let store = state.store.lock().await;
    let content =
        views::dashboard::render(&store).unwrap_or_else(|e| error_markup(ViewId::Dashboard, e));
    drop(store);
    respond(headers, ViewId::Dashboard, content).await
}

pub async fn prospection(State(state): State<AppState>, headers: HeaderMap) -> Html<String> {
    let store = state.store.lock().await;
    let content =
        views::prospection::render(&store).unwrap_or_else(|e| error_markup(ViewId::Prospection, e));
    drop(store);
    respond(headers, ViewId::Prospection, content).await
}

pub async fn missions(State(state): State<AppState>, headers: HeaderMap) -> Html<String> {
    let store = state.store.lock().await;
    let content =
        views::missions::render(&store).unwrap_or_else(|e| error_markup(ViewId::Missions, e));
    drop(store);
    respond(headers, ViewId::Missions, content).await
}

pub async fn facturation(State(state): State<AppState>, headers: HeaderMap) -> Html<String> {
    let store = state.store.lock().await;
    let content =
        views::facturation::render(&store).unwrap_or_else(|e| error_markup(ViewId::Facturation, e));
    drop(store);
    respond(headers, ViewId::Facturation, content).await
}

pub async fn console(headers: HeaderMap) -> Html<String> {
    respond(headers, ViewId::Console, views::console::render()).await
}

pub async fn index(State(state): State<AppState>, headers: HeaderMap) -> Html<String> {
    dashboard(State(state), headers).await
}
