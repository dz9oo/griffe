//! Coque desktop : Tauri v2, dont la fenêtre unique charge `griffe://localhost/` — un
//! protocole URI custom asynchrone dont le handler convertit chaque requête webview en
//! `http::Request`, l'exécute directement contre le `griffe_web::router` via
//! `tower::Service::oneshot` (in-process), puis reconvertit la réponse. Aucun port TCP
//! n'écoute jamais : pas de surface CSRF/DNS-rebinding depuis un autre process ou un onglet de
//! navigateur, conformément au plan.
//!
//! La fenêtre s'ouvre **toujours** : contrairement au comportement précédent (`exit(1)` avant
//! même de construire `tauri::Builder` si le coffre était verrouillé ou `FREEFLOW_DB` absent),
//! l'état du coffre est maintenant porté par [`griffe_web::AppState`] et rendu comme un écran
//! ordinaire du routeur (`/unlock`, `/setup`) — lancée depuis un lanceur graphique, sans
//! terminal pour voir un `eprintln!`, l'application reste utilisable.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use griffe_core::app::{Actor, AppError, ExecutionContext};
use griffe_core::mail::{DeliveryOutcome, SentCopyStatus, SubmissionBatch};
use griffe_core::store::Store;
use griffe_web::AppState;
use http_body_util::BodyExt;
use tauri::http;
use time::OffsetDateTime;
use tower::ServiceExt;

fn resolve_db_path() -> PathBuf {
    if let Ok(from_env) = std::env::var("GRIFFE_DB").or_else(|_| std::env::var("FREEFLOW_DB")) {
        return PathBuf::from(from_env);
    }
    Store::resolve_default_vault_path().unwrap_or_else(|e| {
        // Chemin extrêmement rare (pas de répertoire de données utilisateur du tout) : un
        // chemin invalide dans le répertoire courant fera échouer l'écran de création avec un
        // message clair plutôt que de faire disparaître la fenêtre avant même de s'afficher.
        eprintln!("⚠ {e} — utilisation d'un chemin relatif au répertoire courant");
        PathBuf::from("griffe-vault.db")
    })
}

fn spawn_mail_clock(state: AppState) {
    tauri::async_runtime::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            if let Err(error) = post_due(&state).await {
                eprintln!("⚠ courrier : {error}");
            }
        }
    });
}

async fn post_due(state: &AppState) -> Result<(), String> {
    if !state.vault_is_open().await {
        return Ok(());
    }
    let token = state.mail_session().to_string();
    let today = state.today();
    let now = OffsetDateTime::now_utc();
    let ctx = ExecutionContext::new(Actor::System, false);
    let prepared = state
        .with_store_mut(|store| -> Result<_, AppError> {
            let tick = griffe_core::mail::mail_tick(store.connection(), &token, now, today, true)?;
            if !tick.pending() {
                return Ok(None);
            }
            // La copie ne passe pas par take_due : ces commandes écrivent l'audit
            // même quand elles ne changent rien, et le plafond horaire ne la concerne pas.
            if tick.send {
                griffe_core::mail::abandon_held(store, &ctx)?;
            }
            let sending = if tick.send {
                griffe_core::mail::take_due(store, &ctx, &token, now, today, true)?
            } else {
                None
            };
            let copies = if tick.copy {
                griffe_core::mail::take_copies(store, &ctx, now)?
            } else {
                None
            };
            Ok(Some((sending, copies)))
        })
        .await;
    let Some(prepared) = prepared else {
        return Ok(());
    };
    let Some((sending, copies)) = prepared.map_err(|error| error.to_string())? else {
        return Ok(());
    };
    if let Some(batch) = sending.filter(|batch| !batch.letters.is_empty()) {
        post_letters(state, &ctx, today, batch).await?;
    }
    if let Some(batch) = copies.filter(|batch| !batch.letters.is_empty()) {
        let outcomes = tokio::task::spawn_blocking(move || griffe_mail::file_copies(batch))
            .await
            .map_err(|error| error.to_string())?;
        let recorded = state
            .with_store_mut(|store| griffe_core::mail::record_copies(store, &ctx, &outcomes))
            .await;
        if let Some(result) = recorded {
            result.map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

async fn post_letters(
    state: &AppState,
    ctx: &ExecutionContext,
    today: time::Date,
    batch: SubmissionBatch,
) -> Result<(), String> {
    let SubmissionBatch {
        endpoint,
        secret,
        imap,
        letters,
    } = batch;
    let mail = match griffe_mail::LettreMail::new(endpoint, secret) {
        Ok(mail) => Arc::new(mail),
        Err(error) => {
            let outcomes = letters
                .into_iter()
                .map(|letter| DeliveryOutcome {
                    id: letter.id,
                    result: Err(error.clone()),
                    copy: SentCopyStatus::Skipped,
                })
                .collect::<Vec<_>>();
            return record_outcomes(state, ctx, today, &outcomes).await;
        }
    };
    for letter in letters {
        let id = letter.id.clone();
        let engaged = state
            .with_store_mut(|store| griffe_core::mail::commit_outbound(store, ctx, &id))
            .await;
        let Some(engaged) = engaged else {
            return Ok(());
        };
        if !engaged.map_err(|error| error.to_string())? {
            continue;
        }
        let mail = Arc::clone(&mail);
        let copy = imap.clone();
        let outcome = tokio::task::spawn_blocking(move || {
            griffe_mail::submit_letter(&mail, copy.as_ref(), letter)
        })
        .await
        .map_err(|error| error.to_string())?;
        record_outcomes(state, ctx, today, &[outcome]).await?;
    }
    Ok(())
}

async fn record_outcomes(
    state: &AppState,
    ctx: &ExecutionContext,
    today: time::Date,
    outcomes: &[DeliveryOutcome],
) -> Result<(), String> {
    if outcomes.is_empty() {
        return Ok(());
    }
    let recorded = state
        .with_store_mut(|store| griffe_core::mail::record_deliveries(store, ctx, today, outcomes))
        .await;
    if let Some(result) = recorded {
        result.map_err(|error| error.to_string())?;
    }
    Ok(())
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
    state.set_mail_probe(|endpoint, copy, secret| {
        griffe_mail::probe_account(&endpoint, copy.as_ref(), &secret)
    });

    tauri::async_runtime::block_on(state.try_open_cached());
    spawn_mail_clock(state.clone());

    tauri::Builder::default()
        .register_asynchronous_uri_scheme_protocol("griffe", move |_ctx, request, responder| {
            let router = griffe_web::router(state.clone());
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
        .expect("erreur au démarrage de l'application Griffe");
}
