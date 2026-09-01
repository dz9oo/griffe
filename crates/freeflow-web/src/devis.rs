//! Routes `devis` (lot 21) — lecture et transitions du cycle de vie (envoyer, décliner,
//! accepter). Même convention de réponse que `clients` : succès → `200` vide + `HX-Trigger:
//! freeflow:saved`, échec → `200` avec le panneau re-rendu (fiche avec bandeau). Pas de routes
//! de création/révision — voir le commentaire de tête de `views::devis`.

use axum::Form;
use axum::extract::{Path, State};
use axum::http::HeaderValue;
use axum::response::{Html, IntoResponse, Response};
use freeflow_core::app::{AppError, Executor, Outcome};
use freeflow_core::domain::QuoteId;
use freeflow_core::quotes;
use maud::html;
use serde::Deserialize;

use crate::state::AppState;
use crate::views;

fn locked_fragment() -> Html<String> {
    Html(
        html! { div class="empty-state" { "coffre verrouillé — rechargez la page" } }.into_string(),
    )
}

fn message_fragment(message: &str) -> Html<String> {
    Html(html! { div class="empty-state" { (message) } }.into_string())
}

fn saved() -> Response {
    let mut response = Html(String::new()).into_response();
    response
        .headers_mut()
        .insert("HX-Trigger", HeaderValue::from_static("freeflow:saved"));
    response
}

async fn execute<C: freeflow_core::app::Command>(
    state: &AppState,
    cmd: C,
) -> Option<Result<Outcome<C::Output>, AppError>> {
    state
        .with_store_mut(|store| Executor::new(store).execute(&cmd, &AppState::human_ctx()))
        .await
}

fn parse_id(raw: &str) -> Option<QuoteId> {
    raw.parse().ok()
}

pub async fn table(State(state): State<AppState>) -> Html<String> {
    match state.with_store(views::devis::list_fragment).await {
        None => locked_fragment(),
        Some(Ok(markup)) => Html(markup.into_string()),
        Some(Err(e)) => message_fragment(&e.to_string()),
    }
}

/// Fiche re-rendue avec un bandeau d'erreur — l'échec d'une transition (devis déjà clos,
/// concurrence avec un autre process) laisse l'utilisateur devant l'état frais, pas devant un
/// message orphelin.
async fn detail_with_error(state: &AppState, id: QuoteId, error: &str) -> Response {
    match state
        .with_store(|store| {
            views::devis::load(store, id).map(|loaded| {
                loaded.map(|(quote, refs)| views::devis::detail_panel(store, &quote, refs, None))
            })
        })
        .await
    {
        Some(Ok(Some(markup))) => {
            let body = html! {
                div class="form-error" { (error) }
                (markup)
            };
            Html(body.into_string()).into_response()
        }
        _ => message_fragment(error).into_response(),
    }
}

pub async fn show_panel(State(state): State<AppState>, Path(id): Path<String>) -> Html<String> {
    let Some(id) = parse_id(&id) else {
        return message_fragment("identifiant de devis invalide");
    };
    match state
        .with_store(|store| {
            views::devis::load(store, id).map(|loaded| {
                loaded.map(|(quote, refs)| views::devis::detail_panel(store, &quote, refs, None))
            })
        })
        .await
    {
        None => locked_fragment(),
        Some(Err(e)) => message_fragment(&e.to_string()),
        Some(Ok(None)) => message_fragment("devis introuvable"),
        Some(Ok(Some(markup))) => Html(markup.into_string()),
    }
}

pub async fn send(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let Some(quote_id) = parse_id(&id) else {
        return message_fragment("identifiant de devis invalide").into_response();
    };
    match execute(&state, quotes::SendQuote { quote_id }).await {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => saved(),
        Some(Err(e)) => detail_with_error(&state, quote_id, &e.to_string()).await,
    }
}

pub async fn decline(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let Some(quote_id) = parse_id(&id) else {
        return message_fragment("identifiant de devis invalide").into_response();
    };
    match execute(&state, quotes::DeclineQuote { quote_id }).await {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => saved(),
        Some(Err(e)) => detail_with_error(&state, quote_id, &e.to_string()).await,
    }
}

pub async fn accept_panel(State(state): State<AppState>, Path(id): Path<String>) -> Html<String> {
    let Some(id) = parse_id(&id) else {
        return message_fragment("identifiant de devis invalide");
    };
    match state
        .with_store(|store| freeflow_core::quotes::quote_by_id(store.connection(), id))
        .await
    {
        None => locked_fragment(),
        Some(Err(e)) => message_fragment(&e.to_string()),
        Some(Ok(None)) => message_fragment("devis introuvable"),
        Some(Ok(Some(quote))) => Html(views::devis::accept_panel(&quote, None).into_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct AcceptForm {
    started_on: String,
}

pub async fn accept(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(form): Form<AcceptForm>,
) -> Response {
    let Some(quote_id) = parse_id(&id) else {
        return message_fragment("identifiant de devis invalide").into_response();
    };
    let started_on = match freeflow_core::domain::parse_date(form.started_on.trim()) {
        Ok(d) => d,
        Err(e) => {
            let quote = match state
                .with_store(|store| {
                    freeflow_core::quotes::quote_by_id(store.connection(), quote_id)
                })
                .await
            {
                Some(Ok(Some(q))) => q,
                _ => return message_fragment("devis introuvable").into_response(),
            };
            return Html(views::devis::accept_panel(&quote, Some(&e.to_string())).into_string())
                .into_response();
        }
    };
    match execute(
        &state,
        quotes::AcceptQuote {
            quote_id,
            started_on,
        },
    )
    .await
    {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => saved(),
        Some(Err(e)) => detail_with_error(&state, quote_id, &e.to_string()).await,
    }
}
