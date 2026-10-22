//! Page Boîte. La socket, si la fenêtre en a installé une, reste hors du cœur.

use axum::extract::{Form, Query, State};
use axum::http::{HeaderMap, HeaderValue};
use axum::response::{Html, IntoResponse, Response};
use griffe_core::app::{Executor, Outcome};
use griffe_core::mail::{
    AddressFault, BoxQuery, BoxVue, LectureAsk, LectureShow, MarkAddress, SeenMark, SeenTarget,
    file_human, french_submit_error, lecture_face, prepare_seen, read_box, releve_plan, seal_count,
};
use maud::{Markup, html};
use serde::Deserialize;

use crate::layout::{self, ViewId};
use crate::releve::kick_releve;
use crate::state::AppState;
use crate::views::boite::{Keep, LectureView};

#[derive(Debug, Deserialize, Default)]
pub struct BoiteForm {
    #[serde(default)]
    vue: String,
    #[serde(default)]
    q: String,
    #[serde(default)]
    lettre: String,
    #[serde(default)]
    dossier: String,
    #[serde(default)]
    message_id: String,
    #[serde(default)]
    client_id: String,
    #[serde(default)]
    opportunity_id: String,
    #[serde(default)]
    address: String,
}

#[derive(Debug, Deserialize, Default)]
pub(crate) struct LectureForm {
    #[serde(default)]
    vue: String,
    #[serde(default)]
    q: String,
    #[serde(default)]
    lettre: String,
    #[serde(default)]
    dossier: String,
    #[serde(default, rename = "gen")]
    generation: u64,
    #[serde(default)]
    mode: String,
    #[serde(default)]
    col: String,
    #[serde(default)]
    stall: String,
}

pub async fn page(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(form): Query<BoiteForm>,
) -> Response {
    kick_releve(&state).await;
    show(&state, &headers, &form, None, false).await
}

pub(crate) async fn lecture(
    State(state): State<AppState>,
    Query(form): Query<LectureForm>,
) -> Response {
    let keep = keep_of(&form.vue, &form.q, &form.lettre, &form.dossier);
    let Some(page) = read_page(&state, &keep).await else {
        return Html(r#"<p class="plateau-sentence">Le coffre est verrouillé.</p>"#.to_string())
            .into_response();
    };
    let Ok(page) = page else {
        return Html(r#"<p class="plateau-sentence">La boîte ne s'ouvre pas.</p>"#.to_string())
            .into_response();
    };
    if !state.lecture_engaged() {
        return Html(crate::views::boite::render(&page, &keep, None, None).into_string())
            .into_response();
    }
    let readable = account_readable(&state).await;
    let snap = state.take_lecture(Some(form.generation));
    let face = lecture_face(&LectureAsk {
        phase: snap.phase,
        address: snap.address,
        mirror_empty: page.counts.tout == 0,
        readable,
    });
    let col = if matches!(face.show, LectureShow::Glyph { .. }) {
        "glyph"
    } else {
        "letter"
    };
    let stall = match &face.show {
        LectureShow::Glyph { stall: true, .. } => "1",
        _ => "0",
    };
    let requested_col = if form.col.is_empty() && form.mode == "glyph" {
        "glyph"
    } else {
        form.col.as_str()
    };
    let refresh_list = form.generation != snap.generation;
    let refresh_column =
        keep.lettre.is_empty() && (requested_col != col || (col == "glyph" && form.stall != stall));
    let view = LectureView {
        face,
        generation: snap.generation,
        fresh: snap.fresh_ids,
    };
    let mut response = Html(
        crate::views::boite::poll_fragment(&page, &keep, &view, refresh_list, refresh_column)
            .into_string(),
    )
    .into_response();
    if refresh_list {
        response
            .headers_mut()
            .insert("HX-Trigger", HeaderValue::from_static("griffe:box"));
    }
    response
}

pub async fn seal(State(state): State<AppState>) -> Html<String> {
    let count = state
        .with_store(|store| seal_count(store).unwrap_or(0))
        .await
        .unwrap_or(0);
    Html(layout::boite_link(count, false).into_string())
}

pub async fn classer(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<BoiteForm>,
) -> Response {
    let filed = state
        .with_store_mut(|store| {
            file_human(
                store,
                &AppState::human_ctx(),
                &form.message_id,
                &form.client_id,
                Some(form.opportunity_id.clone()).filter(|id| !id.is_empty()),
            )
        })
        .await;
    finish(&state, &headers, &form, filed).await
}

pub async fn adresse(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<BoiteForm>,
) -> Response {
    let marked = state
        .with_store_mut(|store| {
            Executor::new(store).execute(
                &MarkAddress {
                    client_id: form.client_id.clone(),
                    address: form.address.clone(),
                    status: AddressFault::Confirmed,
                    message_id: None,
                },
                &AppState::human_ctx(),
            )
        })
        .await;
    finish(&state, &headers, &form, marked).await
}

async fn finish(
    state: &AppState,
    headers: &HeaderMap,
    form: &BoiteForm,
    outcome: Option<Result<Outcome<()>, griffe_core::app::AppError>>,
) -> Response {
    match outcome {
        None => locked(state, headers),
        Some(Err(error)) => show(state, headers, form, Some(error.to_string()), false).await,
        Some(Ok(done)) => {
            let saved = matches!(done, Outcome::Applied(()) | Outcome::AlreadyApplied(()));
            show(state, headers, form, None, saved).await
        }
    }
}

async fn show(
    state: &AppState,
    headers: &HeaderMap,
    form: &BoiteForm,
    note: Option<String>,
    saved: bool,
) -> Response {
    let keep = Keep {
        vue: BoxVue::parse(&form.vue),
        q: form.q.clone(),
        lettre: form.lettre.clone(),
        dossier: form.dossier.clone(),
    };
    let query = BoxQuery {
        vue: keep.vue,
        q: &keep.q,
        letter: nonempty(&keep.lettre),
        folder: nonempty(&keep.dossier),
    };
    let Some(first) = state.with_store(|store| read_box(store, query)).await else {
        return locked(state, headers);
    };
    let first = match first {
        Ok(page) => page,
        Err(error) => {
            return page_response(state, headers, error_box(&error.to_string()), false, false);
        }
    };
    let open_id = first.open.as_ref().map(|letter| letter.id.clone());
    let marked = if let Some(id) = &open_id {
        mark_letter(state, id).await
    } else {
        false
    };
    let page = if marked {
        let Some(id) = open_id.as_deref() else {
            return page_response(
                state,
                headers,
                crate::views::boite::render(&first, &keep, note.as_deref(), None),
                saved,
                false,
            );
        };
        let again = BoxQuery {
            vue: keep.vue,
            q: &keep.q,
            letter: Some(id),
            folder: nonempty(&keep.dossier),
        };
        match state.with_store(|store| read_box(store, again)).await {
            Some(Ok(page)) => page,
            Some(Err(error)) => {
                return page_response(state, headers, error_box(&error.to_string()), false, false);
            }
            None => return locked(state, headers),
        }
    } else {
        first
    };
    let lecture = paint_lecture(state, &page).await;
    page_response(
        state,
        headers,
        crate::views::boite::render(&page, &keep, note.as_deref(), lecture.as_ref()),
        saved,
        marked,
    )
}

fn keep_of(vue: &str, q: &str, lettre: &str, dossier: &str) -> Keep {
    Keep {
        vue: BoxVue::parse(vue),
        q: q.to_string(),
        lettre: lettre.to_string(),
        dossier: dossier.to_string(),
    }
}

async fn read_page(
    state: &AppState,
    keep: &Keep,
) -> Option<Result<griffe_core::mail::BoxPage, griffe_core::app::AppError>> {
    let query = BoxQuery {
        vue: keep.vue,
        q: &keep.q,
        letter: nonempty(&keep.lettre),
        folder: nonempty(&keep.dossier),
    };
    state.with_store(|store| read_box(store, query)).await
}

async fn account_readable(state: &AppState) -> bool {
    state
        .with_store(|store| releve_plan(store).ok().flatten().is_some())
        .await
        .unwrap_or(false)
}

async fn paint_lecture(state: &AppState, page: &griffe_core::mail::BoxPage) -> Option<LectureView> {
    if !state.lecture_engaged() {
        return None;
    }
    let readable = account_readable(state).await;
    let snap = state.take_lecture(None);
    let face = lecture_face(&LectureAsk {
        phase: snap.phase,
        address: snap.address,
        mirror_empty: page.counts.tout == 0,
        readable,
    });
    Some(LectureView {
        face,
        generation: snap.generation,
        fresh: snap.fresh_ids,
    })
}

async fn mark_letter(state: &AppState, id: &str) -> bool {
    let Some(prepared) = state.with_store(|store| prepare_seen(store, id)).await else {
        return false;
    };
    let Ok(SeenMark::Due { remote }) = prepared else {
        return false;
    };
    if let Some(target) = remote {
        tell_server(state, target).await;
    }
    state
        .with_store(|store| {
            if !griffe_core::mailbox::mailbox_file(store.db_path()).exists() {
                return false;
            }
            match store.open_mailbox() {
                Ok(mailbox) => mailbox.mark_seen(id).unwrap_or(false),
                Err(_) => false,
            }
        })
        .await
        .unwrap_or(false)
}

async fn tell_server(state: &AppState, target: SeenTarget) {
    let Some(hook) = state.mark_seen() else {
        return;
    };
    let Some(plan) = state
        .with_store(|store| releve_plan(store).ok().flatten())
        .await
        .flatten()
    else {
        return;
    };
    let icloud = plan.endpoint.icloud;
    let SeenTarget { mailbox, uid } = target;
    let joined =
        tokio::task::spawn_blocking(move || hook(&plan.endpoint, &plan.secret, &mailbox, uid))
            .await;
    if let Ok(Err(error)) = joined {
        eprintln!("⚠ {}", french_submit_error(&error, icloud));
    }
}

fn nonempty(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn error_box(message: &str) -> Markup {
    html! { div class="empty-state" role="alert" { (message) } }
}

fn locked(state: &AppState, headers: &HeaderMap) -> Response {
    page_response(
        state,
        headers,
        html! { div class="empty-state" { "coffre verrouillé — rechargez la page" } },
        false,
        false,
    )
}

fn page_response(
    state: &AppState,
    headers: &HeaderMap,
    content: Markup,
    saved: bool,
    read: bool,
) -> Response {
    let body = if headers.contains_key("hx-request") {
        content.into_string()
    } else {
        layout::page(ViewId::Boite, "déverrouillé", state.today(), content).into_string()
    };
    let mut response = Html(body).into_response();
    if saved {
        response
            .headers_mut()
            .insert("HX-Trigger", HeaderValue::from_static("griffe:saved"));
    } else if read {
        response
            .headers_mut()
            .insert("HX-Trigger", HeaderValue::from_static("griffe:box"));
    }
    response
}
