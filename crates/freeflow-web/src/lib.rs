//! Routeur Axum + templates Maud/htmx : la même surface est servie à la coque Tauri (via
//! protocole URI custom in-process) et aux tests HTTP directs sur le `Router`.

mod assets;
mod audit;
mod console;
mod handlers;
mod layout;
mod state;
mod unlock;
mod views;

pub use state::AppState;

use axum::Router;
use axum::routing::{get, post};

/// Construit le routeur complet — la seule fonction publique de ce crate, réutilisée telle
/// quelle par un binaire de dev (`freeflow-web-dev`), les tests `tower::ServiceExt::oneshot`,
/// et la coque Tauri (via un protocole URI custom in-process, pas de port réseau).
///
/// Toute route à l'exception de `/unlock`, `/setup` et `/assets/*` est protégée par
/// [`unlock::require_unlocked`] : tant que le coffre n'est pas déverrouillé, elle redirige vers
/// l'écran approprié plutôt que d'atteindre un handler qui supposerait un `Store` ouvert.
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
        .route("/session/touch", post(unlock::touch))
        .route("/lock", post(unlock::lock))
        .route("/unlock", get(unlock::show).post(unlock::submit))
        .route("/setup", get(unlock::show_setup).post(unlock::submit_setup))
        .route("/assets/app.css", get(assets::app_css))
        .route("/assets/app.js", get(assets::app_js))
        .route("/assets/htmx.min.js", get(assets::htmx_js))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            unlock::require_unlocked,
        ))
        .with_state(state)
}
