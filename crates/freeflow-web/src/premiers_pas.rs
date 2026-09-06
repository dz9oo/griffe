//! Routes de l'assistant de premier lancement (lot 39) : `GET /premiers-pas` (page ou
//! fragment), `POST /premiers-pas/nouvelle` (« société nouvelle », `DeclareNewCompany`).

use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::Html;
use freeflow_core::app::Executor;
use freeflow_core::setup::DeclareNewCompany;
use maud::{Markup, html};
use serde::Deserialize;

use crate::layout::{self, ViewId};
use crate::state::AppState;
use crate::views;

async fn content(state: &AppState) -> Markup {
    state
        .with_store(|store| {
            views::premiers_pas::render(store).unwrap_or_else(|e| {
                html! { div class="empty-state" { "erreur de lecture : " (e.to_string()) } }
            })
        })
        .await
        .unwrap_or_else(|| {
            html! { div class="empty-state" { "coffre verrouillé — rechargez la page" } }
        })
}

/// La page complète de l'assistant (coque comprise) — ce sur quoi la fenêtre atterrit après la
/// création d'un coffre.
pub async fn page(state: &AppState) -> String {
    layout::page(ViewId::Jour, "déverrouillé", content(state).await).into_string()
}

pub async fn show(State(state): State<AppState>, headers: HeaderMap) -> Html<String> {
    let markup = content(&state).await;
    if headers.contains_key("hx-request") {
        Html(markup.into_string())
    } else {
        Html(layout::page(ViewId::Jour, "déverrouillé", markup).into_string())
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct NewCompanyQuery {
    #[serde(default)]
    undo: Option<String>,
}

pub async fn declare_new_company(
    State(state): State<AppState>,
    Query(query): Query<NewCompanyQuery>,
) -> Html<String> {
    let declared = query.undo.is_none();
    let _ = state
        .with_store_mut(|store| {
            Executor::new(store).execute(&DeclareNewCompany { declared }, &AppState::human_ctx())
        })
        .await;
    Html(content(&state).await.into_string())
}
