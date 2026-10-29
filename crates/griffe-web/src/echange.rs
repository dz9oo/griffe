//! `GET` et `POST /societe/fichier`. Le fichier arrive dans le routeur déjà en mémoire.
//!
//! Lire appelle le contrôle à blanc et n'écrit pas. Appliquer exécute `ApplyExchange`
//! comme un acte humain. Le corps est le compte-rendu de la commande. Une écriture
//! réussie répond `200` avec `HX-Trigger: griffe:saved`. Le corps reste le compte-rendu :
//! un corps vide l'effacerait.

use axum::extract::{DefaultBodyLimit, Multipart, State};
use axum::http::{HeaderMap, HeaderValue};
use axum::response::{Html, IntoResponse, Response};
use griffe_core::app::{Executor, Outcome};
use griffe_core::exchange::{ApplyExchange, ExchangeDocument, account, document_from_str, preview};
use maud::{Markup, html};

use crate::layout::{self, ViewId};
use crate::state::AppState;
use crate::views::societe::{FichierNotice, bureau, fichier_body, fichier_letter};

const BODY_LIMIT: usize = 8 * 1024 * 1024;

pub fn body_limit() -> DefaultBodyLimit {
    DefaultBodyLimit::max(BODY_LIMIT)
}

enum Geste {
    Lire,
    Appliquer,
}

struct Posted {
    geste: Geste,
    bytes: Vec<u8>,
}

fn is_htmx(headers: &HeaderMap) -> bool {
    headers.contains_key("hx-request")
}

fn locked() -> Markup {
    html! { div class="empty-state" { "coffre verrouillé — rechargez la page" } }
}

fn faute(text: impl Into<String>) -> FichierNotice {
    FichierNotice::Faute(text.into())
}

/// `GET /societe/fichier`.
pub async fn show(State(state): State<AppState>, headers: HeaderMap) -> Response {
    emit(&state, &headers, None, false).await
}

/// `POST /societe/fichier`. `geste=lire` n'écrit pas. `geste=appliquer` écrit.
pub async fn soumettre(
    State(state): State<AppState>,
    headers: HeaderMap,
    multipart: Multipart,
) -> Response {
    let today = state.today();
    let posted = match read_post(multipart).await {
        Ok(posted) => posted,
        Err(text) => return emit(&state, &headers, Some(faute(text)), false).await,
    };
    let raw = match String::from_utf8(posted.bytes) {
        Ok(raw) => raw,
        Err(_) => {
            return emit(
                &state,
                &headers,
                Some(faute("Le fichier n'est pas du texte.")),
                false,
            )
            .await;
        }
    };
    let document = match document_from_str(&raw) {
        Ok(document) => document,
        Err(error) => {
            return emit(
                &state,
                &headers,
                Some(faute(crate::views::errors::message(&error))),
                false,
            )
            .await;
        }
    };
    match posted.geste {
        Geste::Lire => lire(&state, &headers, today, &document).await,
        Geste::Appliquer => appliquer(&state, &headers, today, document).await,
    }
}

async fn lire(
    state: &AppState,
    headers: &HeaderMap,
    today: time::Date,
    document: &ExchangeDocument,
) -> Response {
    let read = state
        .with_store(|store| preview(store.connection(), document, today))
        .await;
    let Some(read) = read else {
        return emit(
            state,
            headers,
            Some(faute("coffre verrouillé — rechargez la page")),
            false,
        )
        .await;
    };
    match read {
        Ok(report) => {
            emit(
                state,
                headers,
                Some(FichierNotice::Compte(account(document, &report))),
                false,
            )
            .await
        }
        Err(error) => {
            emit(
                state,
                headers,
                Some(faute(crate::views::errors::message(&error))),
                false,
            )
            .await
        }
    }
}

async fn appliquer(
    state: &AppState,
    headers: &HeaderMap,
    today: time::Date,
    document: ExchangeDocument,
) -> Response {
    let command = ApplyExchange {
        document: document.clone(),
        today,
    };
    let outcome = state
        .with_store_mut(|store| Executor::new(store).execute(&command, &AppState::human_ctx()))
        .await;
    let Some(outcome) = outcome else {
        return emit(
            state,
            headers,
            Some(faute("coffre verrouillé — rechargez la page")),
            false,
        )
        .await;
    };
    match outcome {
        Ok(Outcome::Applied(report) | Outcome::AlreadyApplied(report)) => {
            emit(
                state,
                headers,
                Some(FichierNotice::Compte(account(&document, &report))),
                true,
            )
            .await
        }
        Ok(Outcome::DryRun) => emit(state, headers, Some(faute("Rien n'est écrit.")), false).await,
        Ok(Outcome::PendingConfirmation(_)) => {
            emit(
                state,
                headers,
                Some(faute("Ce geste attend une personne.")),
                false,
            )
            .await
        }
        Err(error) => {
            emit(
                state,
                headers,
                Some(faute(crate::views::errors::message(&error))),
                false,
            )
            .await
        }
    }
}

async fn emit(
    state: &AppState,
    headers: &HeaderMap,
    notice: Option<FichierNotice>,
    wrote: bool,
) -> Response {
    let today = state.today();
    let markup = if is_htmx(headers) {
        if let Some(notice) = notice.as_ref() {
            fichier_body(notice)
        } else {
            fold(state, today, None).await
        }
    } else {
        layout::page(
            ViewId::Societe,
            "déverrouillé",
            today,
            fold(state, today, notice.as_ref()).await,
        )
    };
    let mut response = Html(markup.into_string()).into_response();
    if wrote {
        response
            .headers_mut()
            .insert("HX-Trigger", HeaderValue::from_static("griffe:saved"));
    }
    response
}

async fn fold(state: &AppState, today: time::Date, notice: Option<&FichierNotice>) -> Markup {
    state
        .with_store(|store| {
            bureau(store, today, "/societe/fichier", fichier_letter(notice)).unwrap_or_else(
                |error| {
                    html! { div class="empty-state" { "erreur de lecture : " (error.to_string()) } }
                },
            )
        })
        .await
        .unwrap_or_else(locked)
}

async fn read_post(mut multipart: Multipart) -> Result<Posted, String> {
    let mut geste = String::new();
    let mut bytes = Vec::new();
    let mut saw_file = false;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|error| format!("formulaire illisible : {error}"))?
    {
        let name = field.name().unwrap_or_default().to_string();
        if name == "fichier" {
            saw_file = true;
            bytes = field
                .bytes()
                .await
                .map_err(|error| format!("fichier illisible : {error}"))?
                .to_vec();
            continue;
        }
        if name == "geste" {
            geste = field
                .text()
                .await
                .map_err(|error| format!("champ illisible : {error}"))?;
        }
    }
    if !saw_file || bytes.is_empty() {
        return Err("Choisissez le fichier.".into());
    }
    let geste = match geste.as_str() {
        "lire" => Geste::Lire,
        "appliquer" => Geste::Appliquer,
        _ => return Err("Choisissez Lire ou Appliquer.".into()),
    };
    Ok(Posted { geste, bytes })
}
