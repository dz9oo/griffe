//! Chapitre Les papiers : `GET /societe/papiers`, `POST /societe/papiers` (dépôt
//! multipart), `GET /societe/papiers/{id}` (pièce déchiffrée, inline),
//! `POST /societe/papiers/export` (pack contrôle en clair, dossier temporaire).

use axum::extract::{DefaultBodyLimit, Form, Multipart, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue};
use axum::response::{Html, IntoResponse, Response};
use griffe_core::domain::{FiscalYearEnd, PaperId, PaperKind, PaperOrigin};
use griffe_core::papers::{NewPaper, archive_paper, mime_from_name, paper_by_id};
use maud::html;
use serde::Deserialize;

use crate::layout::{self, ViewId};
use crate::state::AppState;
use crate::views;

const BODY_LIMIT: usize = 32 * 1024 * 1024;

pub fn body_limit() -> DefaultBodyLimit {
    DefaultBodyLimit::max(BODY_LIMIT)
}

#[derive(Debug, Default, Deserialize)]
pub struct PapiersQuery {
    pub period: Option<i32>,
}

fn is_htmx_request(headers: &HeaderMap) -> bool {
    headers.contains_key("hx-request")
}

fn default_period(store: &griffe_core::store::Store, today: time::Date) -> i32 {
    let fiscal_year_end = griffe_core::company::company_profile(store.connection())
        .ok()
        .flatten()
        .and_then(|p| p.fiscal_year_end)
        .unwrap_or(FiscalYearEnd::CALENDAR);
    fiscal_year_end
        .previous(fiscal_year_end.current(today))
        .end()
        .year()
}

fn locked() -> Html<String> {
    Html(
        html! { div class="empty-state" { "coffre verrouillé — rechargez la page" } }.into_string(),
    )
}

fn message_fragment(message: &str) -> Html<String> {
    Html(html! { div class="empty-state" { (message) } }.into_string())
}

fn parse_id(raw: &str) -> Option<PaperId> {
    raw.parse().ok()
}

/// `GET /societe/papiers/{id}` : la pièce déchiffrée à la volée — même geste que
/// le justificatif de dépense, IO d'adaptateur (`receipts::read`), pas de Command.
pub async fn show(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let Some(id) = parse_id(&id) else {
        return message_fragment("identifiant de pièce invalide").into_response();
    };
    let loaded = state
        .with_store(
            |store| -> Result<Option<(String, String, Vec<u8>)>, String> {
                let Some(paper) = paper_by_id(store.connection(), id).map_err(|e| e.to_string())?
                else {
                    return Ok(None);
                };
                let bytes = griffe_core::receipts::read(store, &paper.filename)
                    .map_err(|e| e.to_string())?;
                Ok(Some((paper.original_name, paper.mime, bytes)))
            },
        )
        .await;
    match loaded {
        None => locked().into_response(),
        Some(Err(e)) => message_fragment(&e).into_response(),
        Some(Ok(None)) => message_fragment("cette pièce n'est pas au coffre").into_response(),
        Some(Ok(Some((original, mime, bytes)))) => file_response(bytes, &mime, &original),
    }
}

fn file_response(bytes: Vec<u8>, mime: &str, filename: &str) -> Response {
    let content_type = if mime.is_empty() {
        mime_from_name(filename)
    } else {
        mime.to_string()
    };
    let mut response = bytes.into_response();
    if let Ok(value) = HeaderValue::from_str(&content_type) {
        response
            .headers_mut()
            .insert(axum::http::header::CONTENT_TYPE, value);
    }
    if let Ok(disposition) = HeaderValue::from_str(&format!(
        "inline; filename=\"{}\"",
        filename.replace('"', "")
    )) {
        response
            .headers_mut()
            .insert(axum::http::header::CONTENT_DISPOSITION, disposition);
    }
    response
}

fn saved() -> Response {
    let mut response = Html(String::new()).into_response();
    response
        .headers_mut()
        .insert("HX-Trigger", HeaderValue::from_static("griffe:saved"));
    response
}

fn original_name(raw: &str) -> String {
    std::path::Path::new(raw)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "document".to_string())
}

pub async fn chapter(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<PapiersQuery>,
) -> Html<String> {
    let today = state.today();
    let content = state
        .with_store(|store| {
            let period = query.period.unwrap_or_else(|| default_period(store, today));
            views::papiers::chapter(store, today, period, None).unwrap_or_else(|e| {
                html! { div class="empty-state" { "erreur de lecture : " (e.to_string()) } }
            })
        })
        .await
        .unwrap_or_else(
            || html! { div class="empty-state" { "coffre verrouillé — rechargez la page" } },
        );
    if is_htmx_request(&headers) {
        Html(content.into_string())
    } else {
        Html(layout::page(ViewId::Societe, "déverrouillé", content).into_string())
    }
}

struct DepositForm {
    kind: String,
    period: Option<i32>,
    note: Option<String>,
    filename: String,
    bytes: Vec<u8>,
}

async fn read_deposit(mut multipart: Multipart) -> Result<DepositForm, String> {
    let mut kind = String::new();
    let mut period = None;
    let mut note = None;
    let mut filename = String::new();
    let mut bytes = Vec::new();
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| format!("formulaire illisible : {e}"))?
    {
        let name = field.name().unwrap_or_default().to_string();
        if name == "file" {
            filename = original_name(field.file_name().unwrap_or_default());
            bytes = field
                .bytes()
                .await
                .map_err(|e| format!("fichier illisible : {e}"))?
                .to_vec();
            continue;
        }
        let value = field
            .text()
            .await
            .map_err(|e| format!("champ « {name} » illisible : {e}"))?;
        match name.as_str() {
            "kind" => kind = value,
            "period" => {
                period = value.parse().ok();
            }
            "note" => note = Some(value).filter(|s| !s.is_empty()),
            _ => {}
        }
    }
    if filename.is_empty() || bytes.is_empty() {
        return Err("choisissez un fichier à déposer".into());
    }
    if kind.is_empty() {
        return Err("choisissez la nature de la pièce".into());
    }
    Ok(DepositForm {
        kind,
        period,
        note,
        filename,
        bytes,
    })
}

async fn chapter_with_banner(state: &AppState, period: i32, banner: &str) -> Response {
    let today = state.today();
    let content = state
        .with_store(|store| {
            views::papiers::chapter(store, today, period, Some(banner)).unwrap_or_else(|e| {
                html! { div class="empty-state" { "erreur de lecture : " (e.to_string()) } }
            })
        })
        .await
        .unwrap_or_else(
            || html! { div class="empty-state" { "coffre verrouillé — rechargez la page" } },
        );
    let mut response = Html(content.into_string()).into_response();
    response
        .headers_mut()
        .insert("HX-Retarget", HeaderValue::from_static("#papiers-letter"));
    response
        .headers_mut()
        .insert("HX-Reswap", HeaderValue::from_static("outerHTML"));
    response
}

pub async fn deposit(State(state): State<AppState>, multipart: Multipart) -> Response {
    let form = match read_deposit(multipart).await {
        Ok(form) => form,
        Err(e) => return chapter_with_banner(&state, 0, &e).await,
    };
    let kind: PaperKind = match form.kind.parse() {
        Ok(k) => k,
        Err(_) => {
            return chapter_with_banner(&state, form.period.unwrap_or(0), "nature inconnue").await;
        }
    };
    let period = form.period;
    let hash = griffe_core::expenses::hash_receipt(&form.bytes);
    let spec = NewPaper {
        kind,
        origin: PaperOrigin::Uploaded,
        mime: mime_from_name(&form.filename),
        original_name: form.filename,
        period,
        issued_on: None,
        client_id: None,
        invoice_id: None,
        expense_id: None,
        fiscal_year_id: None,
        note: form.note,
        idempotency_key: Some(format!("papers:upload:{hash}:{kind}")),
    };
    let outcome = state
        .with_store_mut(|store| archive_paper(store, spec, &form.bytes, &AppState::human_ctx()))
        .await;
    match outcome {
        None => locked().into_response(),
        Some(Ok(_)) => saved(),
        Some(Err(e)) => {
            chapter_with_banner(&state, period.unwrap_or(0), &views::errors::message(&e)).await
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ExportForm {
    pub period: i32,
}

fn control_pack_temp_dir(period: i32) -> std::path::PathBuf {
    let base = std::env::temp_dir().join(format!("freeflow-controle-{period}"));
    if !base.exists() {
        return base;
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("freeflow-controle-{period}-{nanos}"))
}

fn open_dir(path: &std::path::Path) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(opener).arg(path).spawn();
}

pub async fn export(State(state): State<AppState>, Form(form): Form<ExportForm>) -> Response {
    let today = state.today();
    let dest = control_pack_temp_dir(form.period);
    let outcome = state
        .with_store(|store| griffe_cli::write_control_pack(store, form.period, &dest, today))
        .await;
    match outcome {
        None => locked().into_response(),
        Some(Ok(report)) => {
            open_dir(&dest);
            Html(views::papiers::export_done(&report).into_string()).into_response()
        }
        Some(Err(e)) => chapter_with_banner(&state, form.period, &e.to_string()).await,
    }
}
