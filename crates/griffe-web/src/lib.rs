//! Routeur Axum + templates Maud/htmx : la même surface est servie à la coque Tauri (via
//! protocole URI custom in-process) et aux tests HTTP directs sur le `Router`.

mod assets;
mod audit;
mod banque;
mod brand;
mod clients;
mod cloture;
mod console;
mod depenses;
mod devis;
mod facturation;
mod gens;
mod handlers;
mod layout;
mod missions;
mod papiers;
mod premiers_pas;
mod prospection;
mod relances;
mod societe;
mod state;
mod unlock;
mod views;

pub use state::AppState;

use axum::Router;
use axum::routing::{get, post};

/// Construit le routeur complet — la seule fonction publique de ce crate, réutilisée telle
/// quelle par un binaire de dev (`griffe-web-dev`), les tests `tower::ServiceExt::oneshot`,
/// et la coque Tauri (via un protocole URI custom in-process, pas de port réseau).
///
/// Toute route à l'exception de `/unlock`, `/setup` et `/assets/*` est protégée par
/// [`unlock::require_unlocked`] : tant que le coffre n'est pas déverrouillé, elle redirige vers
/// l'écran approprié plutôt que d'atteindre un handler qui supposerait un `Store` ouvert.
/// Les rejets d'extraction d'axum (formulaire illisible, corps trop grand, type de contenu
/// inattendu) sortent en anglais, en texte brut — la fenêtre les traduit (lot 39) : un `4xx`
/// sans HTML devient une phrase en français, dans un fragment que le panneau peut afficher.
async fn french_rejections(response: axum::response::Response) -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse as _;
    let status = response.status();
    let is_html = response
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/html"));
    if !status.is_client_error() || is_html {
        return response;
    }
    let message = match status {
        StatusCode::NOT_FOUND => "page introuvable — rechargez la fenêtre".to_string(),
        StatusCode::PAYLOAD_TOO_LARGE => "fichier trop volumineux (32 Mio au plus)".to_string(),
        StatusCode::UNSUPPORTED_MEDIA_TYPE
        | StatusCode::BAD_REQUEST
        | StatusCode::UNPROCESSABLE_ENTITY => {
            "formulaire illisible — rechargez la page et réessayez".to_string()
        }
        other => format!("requête refusée ({})", other.as_u16()),
    };
    let body = maud::html! { div class="empty-state" role="alert" { (message) } }.into_string();
    let mut fixed = axum::response::Html(body).into_response();
    *fixed.status_mut() = status;
    fixed
}

fn people_routes() -> Router<AppState> {
    Router::new()
        .route("/", get(handlers::gens))
        .route("/nouvelle", get(gens::new_get).post(gens::new_post))
        .route("/{reference}", get(gens::show))
        .route(
            "/{reference}/ecrire",
            get(gens::write_get).post(gens::write),
        )
        .route("/{reference}/envoye", post(gens::sent))
        .route(
            "/{reference}/rencontre",
            get(gens::meeting_get).post(gens::meeting_post),
        )
        .route("/{reference}/reporter", post(gens::snooze))
        .route("/{reference}/devis", get(gens::quote_panel))
        .route(
            "/{reference}/notes/{expense_id}/receipt",
            post(gens::attach_note).layer(depenses::body_limit()),
        )
        .route(
            "/{reference}/facture",
            post(gens::import_invoice).layer(depenses::body_limit()),
        )
        .route(
            "/{reference}/facture/{invoice_id}/ne-plus-attendre",
            post(gens::write_off_receivable),
        )
        .route(
            "/{reference}/facture/{invoice_id}/j-attends-encore",
            post(gens::retract_write_off),
        )
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(handlers::index))
        .route("/jour", get(handlers::jour))
        .nest("/affaires", people_routes())
        .nest("/dossiers", people_routes())
        .nest("/gens", people_routes())
        .route("/view/dashboard", get(handlers::dashboard))
        .route("/view/relances", get(relances::view))
        .route("/relances/sender", post(relances::set_sender))
        .route(
            "/relances/opportunity/{id}",
            get(relances::show_opportunity),
        )
        .route("/relances/invoice/{id}", get(relances::show_invoice))
        .route(
            "/relances/opportunity/{id}/draft",
            post(relances::draft_opportunity),
        )
        .route(
            "/relances/invoice/{id}/draft",
            post(relances::draft_invoice),
        )
        .route(
            "/relances/opportunity/{id}/sent",
            post(relances::sent_opportunity),
        )
        .route("/relances/invoice/{id}/sent", post(relances::sent_invoice))
        .route(
            "/relances/opportunity/{id}/skip",
            post(relances::skip_opportunity),
        )
        .route("/relances/invoice/{id}/skip", post(relances::skip_invoice))
        .route(
            "/relances/opportunity/{id}/retract",
            post(relances::retract_opportunity),
        )
        .route(
            "/relances/invoice/{id}/retract",
            post(relances::retract_invoice),
        )
        .route(
            "/relances/opportunity/{id}/snooze",
            post(relances::snooze_opportunity),
        )
        .route(
            "/relances/invoice/{id}/snooze",
            post(relances::snooze_invoice),
        )
        .route(
            "/relances/opportunity/{id}/schedule",
            post(relances::schedule_opportunity),
        )
        .route(
            "/relances/invoice/{id}/schedule",
            post(relances::schedule_invoice),
        )
        .route("/view/prospection", get(handlers::prospection))
        .route("/view/missions", get(handlers::missions))
        .route("/view/devis", get(handlers::devis))
        .route("/view/facturation", get(handlers::facturation))
        .route("/view/depenses", get(handlers::depenses))
        .route("/view/clients", get(handlers::clients))
        .route("/view/cloture", get(handlers::cloture))
        .route("/view/console", get(handlers::console))
        .route("/view/societe", get(handlers::societe))
        .route("/societe", get(handlers::societe_piece).post(societe::save))
        .route("/societe/payer", get(handlers::societe_pay))
        .route("/societe/impots", get(handlers::societe_duties))
        .route(
            "/societe/impots/catch-up",
            post(handlers::societe_duties_catch_up),
        )
        .route("/societe/impots/{kind}", get(handlers::societe_duty))
        .route(
            "/societe/impots/{kind}/open",
            post(handlers::societe_duty_open),
        )
        .route(
            "/societe/impots/{kind}/filed",
            post(handlers::societe_duty_filed),
        )
        .route(
            "/societe/impots/{kind}/unfiled",
            post(handlers::societe_duty_unfiled),
        )
        .route(
            "/societe/impots/{kind}/{period}",
            get(handlers::societe_duty_at),
        )
        .route(
            "/societe/impots/{kind}/{period}/open",
            post(handlers::societe_duty_open_at),
        )
        .route(
            "/societe/impots/{kind}/{period}/filed",
            post(handlers::societe_duty_filed_at),
        )
        .route(
            "/societe/impots/{kind}/{period}/unfiled",
            post(handlers::societe_duty_unfiled_at),
        )
        .route(
            "/societe/impots/{kind}/vat-credit",
            post(handlers::societe_vat_credit),
        )
        .route(
            "/societe/impots/{kind}/{period}/vat-credit",
            post(handlers::societe_vat_credit_at),
        )
        .route(
            "/societe/impots/{kind}/vat-refund",
            post(handlers::societe_vat_refund),
        )
        .route(
            "/societe/impots/{kind}/{period}/vat-refund",
            post(handlers::societe_vat_refund_at),
        )
        .route(
            "/societe/impots/{kind}/vat-refund/retract",
            post(handlers::societe_vat_refund_retract),
        )
        .route(
            "/societe/impots/{kind}/{period}/vat-refund/retract",
            post(handlers::societe_vat_refund_retract_at),
        )
        .route(
            "/societe/impots/{kind}/vat-reversal",
            post(handlers::societe_vat_reversal),
        )
        .route(
            "/societe/impots/{kind}/{period}/vat-reversal",
            post(handlers::societe_vat_reversal_at),
        )
        .route(
            "/societe/impots/{kind}/vat-reversal/retract",
            post(handlers::societe_vat_reversal_retract),
        )
        .route(
            "/societe/impots/{kind}/{period}/vat-reversal/retract",
            post(handlers::societe_vat_reversal_retract_at),
        )
        .route("/societe/cloture", get(handlers::societe_closing))
        .route("/societe/releve", get(handlers::societe_statement))
        .route("/societe/identite", get(handlers::societe_identity))
        .route(
            "/societe/papiers",
            get(papiers::chapter)
                .post(papiers::deposit)
                .layer(papiers::body_limit()),
        )
        .route("/societe/papiers/export", post(papiers::export))
        .route("/societe/papiers/{id}", get(papiers::show))
        .route("/premiers-pas", get(premiers_pas::show))
        .route(
            "/premiers-pas/nouvelle",
            post(premiers_pas::declare_new_company),
        )
        .route("/aide", get(handlers::aide))
        .route("/aide/{slug}", get(handlers::aide_recipe))
        .route("/lexique", get(handlers::lexique))
        .route("/depenses/{id}/receipt", get(depenses::receipt))
        .route(
            "/cloture/opening/import",
            post(cloture::opening_import).layer(banque::body_limit()),
        )
        .route(
            "/cloture/opening/from-2033a",
            post(cloture::opening_from_2033a),
        )
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
        .route(
            "/banque/import",
            get(banque::import_panel).post(banque::import),
        )
        .route(
            "/banque/import/preview",
            post(banque::preview).layer(banque::body_limit()),
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
            "/depenses/transaction/{id}/settle",
            get(depenses::settle_panel).post(depenses::settle),
        )
        .route(
            "/depenses/transaction/{id}/unsettle",
            post(depenses::unsettle),
        )
        .route(
            "/depenses/{id}/delete",
            get(depenses::delete_confirm_panel).post(depenses::delete),
        )
        .route(
            "/depenses/{id}/immobilize",
            get(depenses::immobilize_panel).post(depenses::immobilize),
        )
        .route("/depenses/assets/{id}", get(depenses::asset_panel))
        .route("/depenses/assets/{id}/delete", post(depenses::delete_asset))
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
        .route("/cloture/fec/check", get(cloture::fec_check_panel))
        .route("/cloture/checklist", get(cloture::checklist_panel))
        .route("/cloture/balance", get(cloture::balance_panel))
        .route("/cloture/balance.pdf", get(cloture::balance_pdf))
        .route("/session/touch", post(unlock::touch))
        .route("/lock", post(unlock::lock))
        .route("/unlock", get(unlock::show).post(unlock::submit))
        .route("/setup", get(unlock::show_setup).post(unlock::submit_setup))
        .route("/assets/app.css", get(assets::app_css))
        .route("/assets/app.js", get(assets::app_js))
        .route("/assets/htmx.min.js", get(assets::htmx_js))
        .route("/assets/fonts/{name}", get(assets::font))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            unlock::require_unlocked,
        ))
        .layer(axum::middleware::map_response(french_rejections))
        // Filet le plus externe : une panique dans un handler (p. ex. un débordement
        // arithmétique résiduel au rendu d'un montant) devient une réponse 500 au lieu de tuer la
        // tâche du protocole `griffe://` sans jamais répondre — ce qui figeait la webview
        // indéfiniment, sans message. Couvre aussi `griffe-web-dev`.
        .layer(tower_http::catch_panic::CatchPanicLayer::new())
        .with_state(state)
}
