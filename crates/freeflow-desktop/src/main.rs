//! Coque desktop : Tauri v2, dont la fenêtre unique charge `freeflow://localhost/` — un
//! protocole URI custom asynchrone dont le handler convertit chaque requête webview en
//! `http::Request`, l'exécute directement contre le `freeflow_web::router` via
//! `tower::Service::oneshot` (in-process), puis reconvertit la réponse. Aucun port TCP
//! n'écoute jamais : pas de surface CSRF/DNS-rebinding depuis un autre process ou un onglet de
//! navigateur, conformément au plan.
//!
//! La fenêtre s'ouvre **toujours** : contrairement au comportement précédent (`exit(1)` avant
//! même de construire `tauri::Builder` si le coffre était verrouillé ou `FREEFLOW_DB` absent),
//! l'état du coffre est maintenant porté par [`freeflow_web::AppState`] et rendu comme un écran
//! ordinaire du routeur (`/unlock`, `/setup`) — lancée depuis un lanceur graphique, sans
//! terminal pour voir un `eprintln!`, l'application reste utilisable.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;

use freeflow_core::store::Store;
use freeflow_web::AppState;
use http_body_util::BodyExt;
use tauri::http;
use tower::ServiceExt;

fn resolve_db_path() -> PathBuf {
    if let Ok(from_env) = std::env::var("FREEFLOW_DB") {
        return PathBuf::from(from_env);
    }
    Store::default_vault_path().unwrap_or_else(|e| {
        // Chemin extrêmement rare (pas de répertoire de données utilisateur du tout) : un
        // chemin invalide dans le répertoire courant fera échouer l'écran de création avec un
        // message clair plutôt que de faire disparaître la fenêtre avant même de s'afficher.
        eprintln!("⚠ {e} — utilisation d'un chemin relatif au répertoire courant");
        PathBuf::from("freeflow-vault.db")
    })
}

fn error_response(status: http::StatusCode, message: String) -> http::Response<Vec<u8>> {
    http::Response::builder()
        .status(status)
        .header(http::header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(message.into_bytes())
        .expect("une réponse d'erreur minimale se construit toujours")
}

fn main() {
    let db_path = resolve_db_path();
    let state = AppState::new(db_path);

    tauri::async_runtime::block_on(state.try_open_cached());

    tauri::Builder::default()
        .register_asynchronous_uri_scheme_protocol("freeflow", move |_ctx, request, responder| {
            let router = freeflow_web::router(state.clone());
            tauri::async_runtime::spawn(async move {
                let (parts, body) = request.into_parts();
                let axum_request = http::Request::from_parts(parts, axum::body::Body::from(body));

                let response = match router.oneshot(axum_request).await {
                    Ok(response) => response,
                    Err(infallible) => match infallible {},
                };
                let (parts, body) = response.into_parts();
                let body = match body.collect().await {
                    Ok(collected) => collected.to_bytes().to_vec(),
                    Err(e) => {
                        responder.respond(error_response(
                            http::StatusCode::INTERNAL_SERVER_ERROR,
                            format!("échec de lecture de la réponse : {e}"),
                        ));
                        return;
                    }
                };
                responder.respond(http::Response::from_parts(parts, body));
            });
        })
        .run(tauri::generate_context!())
        .expect("erreur au démarrage de l'application FreeFlow");
}
