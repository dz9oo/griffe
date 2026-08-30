//! Routeur Axum + templates Maud/htmx : la même surface est servie à la coque Tauri (lot 9,
//! via protocole URI custom in-process) et aux tests HTTP directs sur le `Router`.

mod assets;
mod audit;
mod console;
mod handlers;
mod layout;
mod state;
mod views;

pub use state::AppState;

use axum::Router;
use axum::routing::{get, post};

/// Construit le routeur complet — la seule fonction publique de ce crate, réutilisée telle
/// quelle par un binaire de dev (`freeflow-web-dev`), les tests `tower::ServiceExt::oneshot`,
/// et la coque Tauri (lot 9, via un protocole URI custom in-process, pas de port réseau).
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(handlers::index))
        .route("/view/dashboard", get(handlers::dashboard))
        .route("/view/prospection", get(handlers::prospection))
        .route("/view/missions", get(handlers::missions))
        .route("/view/facturation", get(handlers::facturation))
        .route("/view/console", get(handlers::console))
        .route("/console/run", post(console::run))
        .route("/audit/recent", get(audit::recent))
        .route("/assets/app.css", get(assets::app_css))
        .route("/assets/app.js", get(assets::app_js))
        .route("/assets/htmx.min.js", get(assets::htmx_js))
        .with_state(state)
}
