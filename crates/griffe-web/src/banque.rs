//! Routes d'import de relevé bancaire (lot 38) : `GET /banque/import` (panneau), `POST
//! /banque/import/preview` (multipart : le fichier → aperçu), `POST /banque/import` (le fichier
//! renvoyé en base64 → `ImportBankTransactions`). Même convention de réponse que les autres
//! écrans : succès → `200` vide + `HX-Trigger: griffe:saved` (les listes de dépenses et de
//! factures se rafraîchissent), échec → panneau re-rendu avec un bandeau en langage courant et
//! le format attendu.

use axum::Form;
use axum::extract::{DefaultBodyLimit, Multipart, State};
use axum::http::HeaderValue;
use axum::response::{Html, IntoResponse, Response};
use base64::Engine as _;
use griffe_core::app::Executor;
use griffe_core::billing;
use maud::html;
use serde::Deserialize;

use crate::state::AppState;
use crate::views;

/// Un relevé annuel tient largement sous 32 Mio — la même borne que les justificatifs.
const STATEMENT_BODY_LIMIT: usize = 32 * 1024 * 1024;

pub fn body_limit() -> DefaultBodyLimit {
    DefaultBodyLimit::max(STATEMENT_BODY_LIMIT)
}

fn locked_fragment() -> Html<String> {
    Html(
        html! { div class="empty-state" { "coffre verrouillé — rechargez la page" } }.into_string(),
    )
}

fn saved() -> Response {
    let mut response = Html(String::new()).into_response();
    response
        .headers_mut()
        .insert("HX-Trigger", HeaderValue::from_static("griffe:saved"));
    response
}

pub async fn import_panel() -> Html<String> {
    Html(views::banque::import_panel(None).into_string())
}

/// Le fichier reçu (nom réduit à son composant final, contenu tel quel).
async fn read_statement(mut multipart: Multipart) -> Result<(String, Vec<u8>), String> {
    let mut file: Option<(String, Vec<u8>)> = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| format!("formulaire illisible : {e}"))?
    {
        if field.name() == Some("statement") {
            let filename = std::path::Path::new(field.file_name().unwrap_or_default())
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_default();
            let content = field
                .bytes()
                .await
                .map_err(|e| format!("fichier illisible : {e}"))?;
            if !content.is_empty() {
                file = Some((filename, content.to_vec()));
            }
        }
    }
    file.ok_or_else(|| "choisissez un fichier de relevé".to_string())
}

pub async fn preview(State(state): State<AppState>, multipart: Multipart) -> Response {
    let (filename, bytes) = match read_statement(multipart).await {
        Ok(f) => f,
        Err(message) => {
            return Html(views::banque::import_panel(Some(&message)).into_string()).into_response();
        }
    };
    let parsed = match billing::parse_bank_statement(&bytes, None) {
        Ok(p) => p,
        Err(e) => {
            return Html(views::banque::import_panel(Some(&e.to_string())).into_string())
                .into_response();
        }
    };
    let fresh = state
        .with_store(|store| {
            billing::new_transactions_among(store.connection(), &parsed.transactions)
        })
        .await;
    match fresh {
        None => locked_fragment().into_response(),
        Some(Err(e)) => {
            Html(views::banque::import_panel(Some(&e.to_string())).into_string()).into_response()
        }
        Some(Ok(fresh)) => {
            let payload = base64::engine::general_purpose::STANDARD.encode(&bytes);
            Html(views::banque::preview_panel(&parsed, &fresh, &payload, &filename).into_string())
                .into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ImportForm {
    #[serde(default)]
    payload: String,
    #[serde(default)]
    filename: String,
}

pub async fn import(State(state): State<AppState>, Form(form): Form<ImportForm>) -> Response {
    let bytes = match base64::engine::general_purpose::STANDARD.decode(form.payload.trim()) {
        Ok(b) => b,
        Err(_) => {
            return Html(
                views::banque::import_panel(Some("fichier illisible : recommencez l'analyse"))
                    .into_string(),
            )
            .into_response();
        }
    };
    let parsed = match billing::parse_bank_statement(&bytes, None) {
        Ok(p) => p,
        Err(e) => {
            return Html(views::banque::import_panel(Some(&e.to_string())).into_string())
                .into_response();
        }
    };
    let original_name = {
        let name = form.filename.trim();
        if name.is_empty() {
            "releve.csv".to_string()
        } else {
            std::path::Path::new(name)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| "releve.csv".to_string())
        }
    };
    let first_on = parsed.transactions.first().map(|t| t.occurred_on);
    let cmd = billing::ImportBankTransactions {
        transactions: parsed.transactions,
    };
    let outcome = state
        .with_store_mut(|store| {
            let result = Executor::new(store).execute(&cmd, &AppState::human_ctx());
            if matches!(&result, Ok(griffe_core::app::Outcome::Applied(_))) {
                let period = first_on.map(|on| {
                    let fye = griffe_core::company::company_profile(store.connection())
                        .ok()
                        .flatten()
                        .and_then(|p| p.fiscal_year_end)
                        .unwrap_or(griffe_core::domain::FiscalYearEnd::CALENDAR);
                    fye.containing(on).end().year()
                });
                let _ = griffe_cli::capture_bank_statement(
                    store,
                    &AppState::human_ctx(),
                    &original_name,
                    &bytes,
                    period,
                );
            }
            result
        })
        .await;
    match outcome {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => saved(),
        Some(Err(e)) => {
            Html(views::banque::import_panel(Some(&e.to_string())).into_string()).into_response()
        }
    }
}
