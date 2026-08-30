//! Coque desktop : Tauri v2, dont la fenêtre unique charge `freeflow://localhost/` — un
//! protocole URI custom asynchrone dont le handler convertit chaque requête webview en
//! `http::Request`, l'exécute directement contre le `freeflow_web::router` via
//! `tower::Service::oneshot` (in-process), puis reconvertit la réponse. Aucun port TCP
//! n'écoute jamais : pas de surface CSRF/DNS-rebinding depuis un autre process ou un onglet de
//! navigateur, conformément au plan.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;

use freeflow_core::store::{Store, StoreError};
use freeflow_web::AppState;
use http_body_util::BodyExt;
use tauri::http;
use tower::ServiceExt;

fn resolve_db_path() -> Result<PathBuf, String> {
    std::env::var("FREEFLOW_DB")
        .map(PathBuf::from)
        .map_err(|_| "aucun coffre indiqué : définissez FREEFLOW_DB".to_string())
}

fn open_store(db_path: &std::path::Path) -> Result<Store, String> {
    match Store::open_cached(db_path) {
        Ok(store) => return Ok(store),
        Err(StoreError::Locked) => {}
        Err(e) => return Err(e.to_string()),
    }
    let passphrase = std::env::var("FREEFLOW_PASSPHRASE")
        .map_err(|_| "coffre verrouillé : définissez FREEFLOW_PASSPHRASE".to_string())?;
    Store::open_with_passphrase(db_path, &passphrase).map_err(|e| e.to_string())
}

fn error_response(status: http::StatusCode, message: String) -> http::Response<Vec<u8>> {
    http::Response::builder()
        .status(status)
        .header(http::header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(message.into_bytes())
        .expect("une réponse d'erreur minimale se construit toujours")
}

fn main() {
    let db_path = match resolve_db_path() {
        Ok(path) => path,
        Err(e) => {
            eprintln!("✗ {e}");
            std::process::exit(1);
        }
    };
    let store = match open_store(&db_path) {
        Ok(store) => store,
        Err(e) => {
            eprintln!("✗ {e}");
            std::process::exit(1);
        }
    };
    let state = AppState::new(store);

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
