//! Routes `facturation` (lot 22) — détail d'une facture et annulation d'un encaissement
//! (contre-écriture). Même convention de réponse que `clients` : succès → panneau rechargé (la
//! fiche de la facture, que l'utilisateur regarde encore) + `HX-Trigger: freeflow:saved` pour
//! la liste ; échec → `200` avec le panneau re-rendu et un bandeau.

use axum::Form;
use axum::extract::{Path, State};
use axum::http::HeaderValue;
use axum::response::{Html, IntoResponse, Response};
use freeflow_core::app::{AppError, Executor, Outcome};
use freeflow_core::billing;
use freeflow_core::domain::{InvoiceId, PaymentId};
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

fn today() -> time::Date {
    time::OffsetDateTime::now_utc().date()
}

async fn execute<C: freeflow_core::app::Command>(
    state: &AppState,
    cmd: C,
) -> Option<Result<Outcome<C::Output>, AppError>> {
    state
        .with_store_mut(|store| Executor::new(store).execute(&cmd, &AppState::human_ctx()))
        .await
}

pub async fn table(State(state): State<AppState>) -> Html<String> {
    match state
        .with_store(|store| views::facturation::list_fragment(store, today()))
        .await
    {
        None => locked_fragment(),
        Some(Ok(markup)) => Html(markup.into_string()),
        Some(Err(e)) => message_fragment(&e.to_string()),
    }
}

/// Fiche d'une facture (avec un bandeau d'erreur éventuel) + l'en-tête `freeflow:saved` si la
/// liste doit se rafraîchir — après une annulation réussie, l'utilisateur garde la fiche sous
/// les yeux, à jour, pendant que la liste recharge ses badges de statut.
async fn detail_response(
    state: &AppState,
    id: InvoiceId,
    error: Option<String>,
    saved: bool,
) -> Response {
    let markup = state
        .with_store(|store| {
            views::facturation::load_detail(store, id).map(|loaded| {
                loaded.map(|(invoice, payments)| {
                    views::facturation::detail_panel(
                        store,
                        &invoice,
                        &payments,
                        today(),
                        error.as_deref(),
                    )
                })
            })
        })
        .await;
    let mut response = match markup {
        None => locked_fragment().into_response(),
        Some(Err(e)) => message_fragment(&e.to_string()).into_response(),
        Some(Ok(None)) => message_fragment("facture introuvable").into_response(),
        Some(Ok(Some(markup))) => Html(markup.into_string()).into_response(),
    };
    if saved {
        response
            .headers_mut()
            .insert("HX-Trigger", HeaderValue::from_static("freeflow:saved"));
    }
    response
}

pub async fn show_panel(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let Ok(id) = id.parse::<InvoiceId>() else {
        return message_fragment("identifiant de facture invalide").into_response();
    };
    detail_response(&state, id, None, false).await
}

async fn payment_or_not_found(
    state: &AppState,
    id: PaymentId,
) -> Result<freeflow_core::domain::Payment, Box<Response>> {
    match state
        .with_store(|store| billing::payment_by_id(store.connection(), id))
        .await
    {
        None => Err(Box::new(locked_fragment().into_response())),
        Some(Err(e)) => Err(Box::new(message_fragment(&e.to_string()).into_response())),
        Some(Ok(None)) => Err(Box::new(
            message_fragment("encaissement introuvable").into_response(),
        )),
        Some(Ok(Some(payment))) => Ok(payment),
    }
}

pub async fn void_confirm_panel(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let Ok(id) = id.parse::<PaymentId>() else {
        return message_fragment("identifiant d'encaissement invalide").into_response();
    };
    let payment = match payment_or_not_found(&state, id).await {
        Ok(p) => p,
        Err(response) => return *response,
    };
    let invoice_number = state
        .with_store(|store| billing::invoice_by_id(store.connection(), payment.invoice_id))
        .await
        .and_then(Result::ok)
        .flatten()
        .map_or_else(|| "?".to_string(), |i| i.number);
    Html(views::facturation::void_confirm_panel(&payment, &invoice_number).into_string())
        .into_response()
}

#[derive(Debug, Deserialize)]
pub struct VoidForm {
    #[serde(default)]
    reason: String,
}

pub async fn void(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(form): Form<VoidForm>,
) -> Response {
    let Ok(id) = id.parse::<PaymentId>() else {
        return message_fragment("identifiant d'encaissement invalide").into_response();
    };
    let payment = match payment_or_not_found(&state, id).await {
        Ok(p) => p,
        Err(response) => return *response,
    };
    let invoice_id = payment.invoice_id;
    let reason = {
        let trimmed = form.reason.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    };
    match execute(
        &state,
        billing::VoidPayment {
            payment_id: id,
            reason,
        },
    )
    .await
    {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => detail_response(&state, invoice_id, None, true).await,
        Some(Err(e)) => detail_response(&state, invoice_id, Some(e.to_string()), false).await,
    }
}
