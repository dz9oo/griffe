//! Routeur Axum + templates Maud/htmx : la même surface est servie à la coque Tauri (via
//! protocole URI custom in-process) et aux tests HTTP directs sur le `Router`.

mod assets;
mod audit;
mod clients;
mod cloture;
mod console;
mod depenses;
mod devis;
mod facturation;
mod handlers;
mod layout;
mod missions;
mod prospection;
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
        .route("/view/devis", get(handlers::devis))
        .route("/view/facturation", get(handlers::facturation))
        .route("/view/depenses", get(handlers::depenses))
        .route("/view/clients", get(handlers::clients))
        .route("/view/cloture", get(handlers::cloture))
        .route("/view/console", get(handlers::console))
        .route("/console/run", post(console::run))
        .route("/audit/recent", get(audit::recent))
        .route("/clients/table", get(clients::table))
        .route("/clients/new", get(clients::new_panel))
        .route("/clients", post(clients::create))
        .route(
            "/clients/{id}",
            get(clients::show_panel).post(clients::update),
        )
        .route("/clients/{id}/edit", get(clients::edit_panel))
        .route("/clients/{id}/archive", post(clients::archive))
        .route("/clients/{id}/unarchive", post(clients::unarchive))
        .route(
            "/clients/{id}/delete",
            get(clients::delete_confirm_panel).post(clients::delete),
        )
        .route(
            "/clients/{client_id}/contacts/new",
            get(clients::new_contact_panel),
        )
        .route(
            "/clients/{client_id}/contacts",
            post(clients::create_contact),
        )
        .route("/contacts/{id}/edit", get(clients::edit_contact_panel))
        .route("/contacts/{id}", post(clients::update_contact))
        .route("/contacts/{id}/delete", post(clients::delete_contact))
        .route("/prospection/table", get(prospection::table))
        .route("/prospection/new", get(prospection::new_panel))
        .route("/prospection", post(prospection::create))
        .route(
            "/prospection/{id}",
            get(prospection::show_panel).post(prospection::update),
        )
        .route("/prospection/{id}/edit", get(prospection::edit_panel))
        .route(
            "/prospection/{id}/advance",
            get(prospection::advance_panel).post(prospection::advance),
        )
        .route(
            "/prospection/{id}/win",
            get(prospection::win_panel).post(prospection::win),
        )
        .route(
            "/prospection/{id}/lose",
            get(prospection::lose_panel).post(prospection::lose),
        )
        .route("/prospection/{id}/archive", post(prospection::archive))
        .route("/prospection/{id}/unarchive", post(prospection::unarchive))
        .route(
            "/prospection/{id}/delete",
            get(prospection::delete_confirm_panel).post(prospection::delete),
        )
        .route(
            "/prospection/{id}/interactions/new",
            get(prospection::new_interaction_panel),
        )
        .route(
            "/prospection/{id}/interactions",
            post(prospection::create_interaction),
        )
        .route(
            "/interactions/{id}/edit",
            get(prospection::edit_interaction_panel),
        )
        .route("/interactions/{id}", post(prospection::update_interaction))
        .route(
            "/interactions/{id}/delete",
            post(prospection::delete_interaction),
        )
        .route("/missions/table", get(missions::table))
        .route("/missions/new", get(missions::new_panel))
        .route("/missions", post(missions::create))
        .route(
            "/missions/{id}",
            get(missions::show_panel).post(missions::update),
        )
        .route("/missions/{id}/edit", get(missions::edit_panel))
        .route(
            "/missions/{id}/close",
            get(missions::close_panel).post(missions::close),
        )
        .route("/missions/{id}/reopen", post(missions::reopen))
        .route("/missions/{id}/archive", post(missions::archive))
        .route("/missions/{id}/unarchive", post(missions::unarchive))
        .route(
            "/missions/{id}/delete",
            get(missions::delete_confirm_panel).post(missions::delete),
        )
        .route(
            "/missions/{id}/time/new",
            get(missions::new_time_entry_panel),
        )
        .route("/missions/{id}/time", post(missions::create_time_entry))
        .route(
            "/time-entries/{id}/edit",
            get(missions::edit_time_entry_panel),
        )
        .route("/time-entries/{id}", post(missions::update_time_entry))
        .route(
            "/time-entries/{id}/delete",
            post(missions::delete_time_entry),
        )
        .route("/facturation/table", get(facturation::table))
        .route("/facturation/{id}", get(facturation::show_panel))
        .route(
            "/payments/{id}/void",
            get(facturation::void_confirm_panel).post(facturation::void),
        )
        .route("/depenses/table", get(depenses::table))
        .route("/depenses/new", get(depenses::new_panel))
        // Les deux routes de saisie reçoivent un `multipart/form-data` (justificatif) : limite
        // de corps relevée pour elles seules, voir `depenses::body_limit`.
        .route(
            "/depenses",
            post(depenses::create).layer(depenses::body_limit()),
        )
        .route(
            "/depenses/{id}",
            get(depenses::show_panel)
                .post(depenses::update)
                .layer(depenses::body_limit()),
        )
        .route("/depenses/{id}/edit", get(depenses::edit_panel))
        .route(
            "/depenses/{id}/reconcile",
            get(depenses::reconcile_panel).post(depenses::reconcile),
        )
        .route("/depenses/{id}/unreconcile", post(depenses::unreconcile))
        .route(
            "/depenses/{id}/delete",
            get(depenses::delete_confirm_panel).post(depenses::delete),
        )
        .route("/devis/table", get(devis::table))
        .route("/devis/new", get(devis::new_panel))
        .route("/devis", post(devis::create))
        .route("/devis/{id}", get(devis::show_panel))
        .route(
            "/devis/{id}/revise",
            get(devis::revise_panel).post(devis::revise),
        )
        .route("/devis/{id}/send", post(devis::send))
        .route("/devis/{id}/decline", post(devis::decline))
        .route(
            "/devis/{id}/accept",
            get(devis::accept_panel).post(devis::accept),
        )
        .route("/cloture/table", get(cloture::table))
        .route("/cloture/new", get(cloture::new_panel))
        .route(
            "/cloture/opening",
            get(cloture::opening_panel).post(cloture::opening_save),
        )
        .route("/cloture/opening/edit", get(cloture::opening_edit_panel))
        .route(
            "/cloture/opening/delete",
            get(cloture::opening_delete_panel).post(cloture::opening_delete),
        )
        .route("/cloture", post(cloture::create))
        .route(
            "/cloture/{id}",
            get(cloture::show_panel).post(cloture::update),
        )
        .route("/cloture/{id}/edit", get(cloture::edit_panel))
        .route(
            "/cloture/{id}/approve",
            get(cloture::approve_panel).post(cloture::approve),
        )
        .route(
            "/cloture/{id}/delete",
            get(cloture::delete_confirm_panel).post(cloture::delete),
        )
        .route("/cloture/{id}/doc/{kind}", get(cloture::document))
        .route("/cloture/fec", get(cloture::fec))
        .route("/cloture/balance", get(cloture::balance_panel))
        .route("/cloture/balance.pdf", get(cloture::balance_pdf))
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
        // Filet le plus externe : une panique dans un handler (p. ex. un débordement
        // arithmétique résiduel au rendu d'un montant) devient une réponse 500 au lieu de tuer la
        // tâche du protocole `freeflow://` sans jamais répondre — ce qui figeait la webview
        // indéfiniment, sans message. Couvre aussi `freeflow-web-dev`.
        .layer(tower_http::catch_panic::CatchPanicLayer::new())
        .with_state(state)
}
