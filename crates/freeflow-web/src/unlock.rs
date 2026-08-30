//! Écrans et middleware de déverrouillage : `GET/POST /unlock`, `GET/POST /setup`, `POST /lock`,
//! `POST /session/touch`, et le middleware qui protège toutes les autres routes.
//!
//! **Pas de redirection HTTP (`Location`) pour une navigation de premier niveau.** La coque
//! desktop pont son routeur Axum vers un protocole URI custom Tauri (`freeflow://`, voir
//! `freeflow-desktop/src/main.rs`) ; observé en pratique, WebKitGTK ne suit pas de façon fiable
//! un `303 Location: ...` renvoyé par ce protocole pour le chargement initial d'une page (page
//! blanche silencieuse — aucune erreur Rust, aucune erreur réseau, juste rien de rendu). Une
//! requête htmx (`fetch`) suit un vrai 30x sans problème, mais **htmx a de toute façon son propre
//! mécanisme dédié** (`HX-Redirect`, lu par son JavaScript, qui déclenche lui-même la navigation
//! suivante) précisément parce que dépendre du suivi de redirection réseau serait fragile pour de
//! l'AJAX — c'est le même raisonnement qu'on applique ici à la navigation de premier niveau :
//! ne jamais dépendre du moteur de rendu pour suivre un `Location`, toujours rendre le contenu
//! final directement (200) ou passer par un mécanisme piloté côté client. Concrètement :
//! - le middleware rend l'écran cible directement plutôt que de rediriger vers lui ;
//! - `POST /unlock` et `POST /setup` (formulaires HTML ordinaires, sans htmx : `bare_page` ne
//!   charge pas htmx.min.js) rendent le tableau de bord directement en cas de succès ;
//! - `POST /lock` (bouton htmx de la coque applicative) continue d'utiliser `HX-Redirect`, géré
//!   par htmx lui-même, pas par le moteur de rendu.
//!
//! La passphrase ne transite que dans un corps `POST`, jamais en query string ; ce crate n'a
//! aucune dépendance de journalisation (`tracing`/`log`), et les structs de formulaire n'ont
//! volontairement pas de `Debug` — un futur `{:?}` dessus est une erreur de compilation, pas une
//! fuite silencieuse.

use axum::Form;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{Html, IntoResponse, Response};
use freeflow_core::store::Passphrase;
use serde::Deserialize;

use crate::handlers::dashboard_page;
use crate::layout;
use crate::state::{AppState, VaultSnapshot};
use crate::views;

fn is_htmx_request(request: &Request) -> bool {
    request.headers().contains_key("hx-request")
}

fn hx_redirect(location: &'static str) -> Response {
    let mut response = StatusCode::NO_CONTENT.into_response();
    response
        .headers_mut()
        .insert("HX-Redirect", HeaderValue::from_static(location));
    response
}

fn unlock_page(error: Option<&str>) -> Html<String> {
    Html(layout::bare_page("déverrouiller", views::unlock::unlock_form(error)).into_string())
}

fn setup_page(error: Option<&str>) -> Html<String> {
    Html(layout::bare_page("créer le coffre", views::unlock::setup_form(error)).into_string())
}

/// Protège toutes les routes sauf `/unlock`, `/setup` et `/assets/*` : tant que le coffre n'est
/// pas déverrouillé, une requête htmx reçoit `HX-Redirect` (géré par htmx.js), une navigation
/// directe reçoit l'écran cible rendu directement (voir le commentaire de module — jamais un 303
/// `Location`, que le protocole URI custom de la coque desktop ne suit pas de façon fiable).
/// Marque aussi l'activité humaine pour l'auto-verrouillage — sauf pour `/audit/recent`, dont le
/// polling toutes les 2s n'en est pas une.
pub async fn require_unlocked(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();
    if path == "/unlock" || path == "/setup" || path.starts_with("/assets/") {
        return next.run(request).await;
    }

    let snapshot = state.snapshot().await;
    if snapshot != VaultSnapshot::Unlocked {
        if is_htmx_request(&request) {
            let target = if snapshot == VaultSnapshot::Absent {
                "/setup"
            } else {
                "/unlock"
            };
            return hx_redirect(target);
        }
        return if snapshot == VaultSnapshot::Absent {
            setup_page(None).into_response()
        } else {
            unlock_page(None).into_response()
        };
    }

    if path != "/audit/recent" {
        state.touch().await;
    }
    next.run(request).await
}

pub async fn show(State(state): State<AppState>) -> Response {
    match state.snapshot().await {
        VaultSnapshot::Unlocked => Html(dashboard_page(&state).await).into_response(),
        VaultSnapshot::Absent => setup_page(None).into_response(),
        VaultSnapshot::Locked => unlock_page(None).into_response(),
    }
}

#[derive(Deserialize)]
pub struct UnlockForm {
    passphrase: String,
    #[serde(default)]
    remember: Option<String>,
}

pub async fn submit(State(state): State<AppState>, Form(form): Form<UnlockForm>) -> Response {
    let passphrase = Passphrase::from(form.passphrase);
    let remember = form.remember.is_some();
    match state.unlock(&passphrase, remember).await {
        Ok(()) => Html(dashboard_page(&state).await).into_response(),
        Err(e) => unlock_page(Some(&e.to_string())).into_response(),
    }
}

pub async fn show_setup(State(state): State<AppState>) -> Response {
    match state.snapshot().await {
        VaultSnapshot::Unlocked => Html(dashboard_page(&state).await).into_response(),
        VaultSnapshot::Locked => unlock_page(None).into_response(),
        VaultSnapshot::Absent => setup_page(None).into_response(),
    }
}

#[derive(Deserialize)]
pub struct SetupForm {
    passphrase: String,
    confirm: String,
    #[serde(default)]
    remember: Option<String>,
}

pub async fn submit_setup(State(state): State<AppState>, Form(form): Form<SetupForm>) -> Response {
    if form.passphrase != form.confirm {
        return setup_page(Some("les deux saisies ne correspondent pas")).into_response();
    }
    if form.passphrase.is_empty() {
        return setup_page(Some("la passphrase ne peut pas être vide")).into_response();
    }

    let passphrase = Passphrase::from(form.passphrase);
    let remember = form.remember.is_some();
    match state.create(&passphrase, remember).await {
        Ok(()) => Html(dashboard_page(&state).await).into_response(),
        Err(e) => setup_page(Some(&e.to_string())).into_response(),
    }
}

pub async fn lock(State(state): State<AppState>) -> Response {
    state.lock().await;
    hx_redirect("/unlock")
}

pub async fn touch(State(state): State<AppState>) -> StatusCode {
    state.touch().await;
    StatusCode::NO_CONTENT
}
