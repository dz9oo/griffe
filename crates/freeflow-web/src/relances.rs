//! Routes de l'écran Relances : file d'une carte, brouillon `.eml`, attestation.

use axum::Form;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::Html;
use freeflow_core::app::{AppError, Executor};
use freeflow_core::domain::{FollowUpSubject, InvoiceId, OpportunityId, parse_date};
use freeflow_core::follow_up::{
    MarkFollowUpSent, PrepareFollowUp, RetractLastFollowUp, SetFollowUpDate, SetFollowUpSender,
    SkipFollowUpStep, SnoozeFollowUp, card_for,
};
use serde::Deserialize;

use crate::layout::ViewId;
use crate::state::AppState;
use crate::views::relances;

fn page(headers: &HeaderMap, content: maud::Markup) -> Html<String> {
    if headers.contains_key("hx-request") {
        Html(content.into_string())
    } else {
        Html(crate::layout::page(ViewId::Relances, "déverrouillé", content).into_string())
    }
}

async fn render_focus(
    state: &AppState,
    headers: HeaderMap,
    focus: Option<FollowUpSubject>,
    flash: Option<String>,
) -> Html<String> {
    let today = state.today();
    let content = state
        .with_store(|store| {
            relances::render(store, today, focus, flash.as_deref()).unwrap_or_else(|e| {
                maud::html! { div class="empty-state" { (e.to_string()) } }
            })
        })
        .await
        .unwrap_or_else(|| maud::html! { div class="empty-state" { "coffre verrouillé" } });
    page(&headers, content)
}

pub async fn view(State(state): State<AppState>, headers: HeaderMap) -> Html<String> {
    render_focus(&state, headers, None, None).await
}

pub async fn show_opportunity(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Html<String> {
    let Ok(id) = id.parse::<OpportunityId>() else {
        return render_focus(&state, headers, None, Some("référence illisible".into())).await;
    };
    render_focus(
        &state,
        headers,
        Some(FollowUpSubject::Opportunity(id)),
        None,
    )
    .await
}

pub async fn show_invoice(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Html<String> {
    let Ok(id) = id.parse::<InvoiceId>() else {
        return render_focus(&state, headers, None, Some("référence illisible".into())).await;
    };
    render_focus(&state, headers, Some(FollowUpSubject::Invoice(id)), None).await
}

#[derive(Deserialize)]
pub struct SenderForm {
    email: String,
    name: String,
}

pub async fn set_sender(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<SenderForm>,
) -> Html<String> {
    let name = form.name.trim();
    let cmd = SetFollowUpSender {
        email: form.email,
        name: if name.is_empty() {
            None
        } else {
            Some(name.to_string())
        },
    };
    let result = state
        .with_store_mut(|store| Executor::new(store).execute(&cmd, &AppState::human_ctx()))
        .await;
    match result {
        Some(Ok(_)) => {
            render_focus(&state, headers, None, Some("expéditeur enregistré".into())).await
        }
        Some(Err(e)) => {
            let content = relances::sender_form(Some(&e.to_string()));
            page(&headers, content)
        }
        None => render_focus(&state, headers, None, Some("coffre verrouillé".into())).await,
    }
}

async fn mutate(
    state: &AppState,
    headers: HeaderMap,
    subject: FollowUpSubject,
    action: impl FnOnce(
        &mut freeflow_core::store::Store,
        FollowUpSubject,
        time::Date,
    ) -> Result<String, AppError>,
) -> Html<String> {
    let today = state.today();
    let result = state
        .with_store_mut(|store| action(store, subject, today))
        .await;
    match result {
        Some(Ok(flash)) => render_focus(state, headers, Some(subject), Some(flash)).await,
        Some(Err(e)) => {
            let banner = relances::error_banner(&e);
            let today = state.today();
            let body = state
                .with_store(|store| {
                    relances::render(store, today, Some(subject), None).unwrap_or_else(|err| {
                        maud::html! { div class="empty-state" { (err.to_string()) } }
                    })
                })
                .await
                .unwrap_or_else(|| maud::html! { div class="empty-state" { "coffre verrouillé" } });
            page(
                &headers,
                maud::html! {
                    (banner)
                    (body)
                },
            )
        }
        None => {
            render_focus(
                state,
                headers,
                Some(subject),
                Some("coffre verrouillé".into()),
            )
            .await
        }
    }
}

fn parse_opportunity(id: String) -> Result<FollowUpSubject, String> {
    id.parse::<OpportunityId>()
        .map(FollowUpSubject::Opportunity)
        .map_err(|_| "référence illisible".to_string())
}

fn parse_invoice(id: String) -> Result<FollowUpSubject, String> {
    id.parse::<InvoiceId>()
        .map(FollowUpSubject::Invoice)
        .map_err(|_| "référence illisible".to_string())
}

pub async fn draft_opportunity(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Html<String> {
    let Ok(subject) = parse_opportunity(id) else {
        return render_focus(&state, headers, None, Some("référence illisible".into())).await;
    };
    draft(&state, headers, subject).await
}

pub async fn draft_invoice(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Html<String> {
    let Ok(subject) = parse_invoice(id) else {
        return render_focus(&state, headers, None, Some("référence illisible".into())).await;
    };
    draft(&state, headers, subject).await
}

async fn draft(state: &AppState, headers: HeaderMap, subject: FollowUpSubject) -> Html<String> {
    let db_path = state.db_path().to_path_buf();
    mutate(state, headers, subject, move |store, subject, today| {
        let outcome = Executor::new(store)
            .execute(&PrepareFollowUp { subject, today }, &AppState::human_ctx())?;
        if let freeflow_core::app::Outcome::Applied(prepared) = outcome {
            let _ = freeflow_cli::write_and_open_draft(
                &db_path,
                &prepared.draft.filename,
                &prepared.draft.rfc5322,
                true,
            );
            Ok("brouillon ouvert dans votre client mail — dites ensuite si c'est envoyé".into())
        } else {
            Ok("rien n'a été écrit".into())
        }
    })
    .await
}

macro_rules! simple_post {
    ($opp:ident, $inv:ident, $run:ident) => {
        pub async fn $opp(
            State(state): State<AppState>,
            headers: HeaderMap,
            Path(id): Path<String>,
        ) -> Html<String> {
            let Ok(subject) = parse_opportunity(id) else {
                return render_focus(&state, headers, None, Some("référence illisible".into()))
                    .await;
            };
            $run(&state, headers, subject).await
        }
        pub async fn $inv(
            State(state): State<AppState>,
            headers: HeaderMap,
            Path(id): Path<String>,
        ) -> Html<String> {
            let Ok(subject) = parse_invoice(id) else {
                return render_focus(&state, headers, None, Some("référence illisible".into()))
                    .await;
            };
            $run(&state, headers, subject).await
        }
    };
}

simple_post!(sent_opportunity, sent_invoice, sent);
simple_post!(skip_opportunity, skip_invoice, skip);
simple_post!(retract_opportunity, retract_invoice, retract);

async fn sent(state: &AppState, headers: HeaderMap, subject: FollowUpSubject) -> Html<String> {
    mutate(state, headers, subject, |store, subject, today| {
        Executor::new(store)
            .execute(&MarkFollowUpSent { subject, today }, &AppState::human_ctx())?;
        let card = card_for(store.connection(), subject, today)?;
        Ok(format!(
            "noté comme envoyé — {}",
            card.step_label.unwrap_or_else(|| "suite".into())
        ))
    })
    .await
}

async fn skip(state: &AppState, headers: HeaderMap, subject: FollowUpSubject) -> Html<String> {
    mutate(state, headers, subject, |store, subject, today| {
        Executor::new(store)
            .execute(&SkipFollowUpStep { subject, today }, &AppState::human_ctx())?;
        Ok("étape sautée".into())
    })
    .await
}

async fn retract(state: &AppState, headers: HeaderMap, subject: FollowUpSubject) -> Html<String> {
    mutate(state, headers, subject, |store, subject, today| {
        Executor::new(store).execute(
            &RetractLastFollowUp { subject, today },
            &AppState::human_ctx(),
        )?;
        Ok("dernier geste annulé".into())
    })
    .await
}

#[derive(Deserialize)]
pub struct UntilForm {
    until: String,
}

pub async fn snooze_opportunity(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(form): Form<UntilForm>,
) -> Html<String> {
    let Ok(subject) = parse_opportunity(id) else {
        return render_focus(&state, headers, None, Some("référence illisible".into())).await;
    };
    snooze(&state, headers, subject, form.until).await
}

pub async fn snooze_invoice(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(form): Form<UntilForm>,
) -> Html<String> {
    let Ok(subject) = parse_invoice(id) else {
        return render_focus(&state, headers, None, Some("référence illisible".into())).await;
    };
    snooze(&state, headers, subject, form.until).await
}

async fn snooze(
    state: &AppState,
    headers: HeaderMap,
    subject: FollowUpSubject,
    until: String,
) -> Html<String> {
    let Ok(until) = parse_date(&until) else {
        return render_focus(state, headers, Some(subject), Some("date illisible".into())).await;
    };
    mutate(state, headers, subject, move |store, subject, today| {
        Executor::new(store).execute(
            &SnoozeFollowUp {
                subject,
                until,
                today,
            },
            &AppState::human_ctx(),
        )?;
        Ok(format!("reporté au {until}"))
    })
    .await
}

#[derive(Deserialize)]
pub struct OnForm {
    on: String,
}

pub async fn schedule_opportunity(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(form): Form<OnForm>,
) -> Html<String> {
    let Ok(subject) = parse_opportunity(id) else {
        return render_focus(&state, headers, None, Some("référence illisible".into())).await;
    };
    schedule(&state, headers, subject, form.on).await
}

pub async fn schedule_invoice(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(form): Form<OnForm>,
) -> Html<String> {
    let Ok(subject) = parse_invoice(id) else {
        return render_focus(&state, headers, None, Some("référence illisible".into())).await;
    };
    schedule(&state, headers, subject, form.on).await
}

async fn schedule(
    state: &AppState,
    headers: HeaderMap,
    subject: FollowUpSubject,
    on: String,
) -> Html<String> {
    let Ok(on) = parse_date(&on) else {
        return render_focus(state, headers, Some(subject), Some("date illisible".into())).await;
    };
    mutate(state, headers, subject, move |store, subject, today| {
        Executor::new(store).execute(
            &SetFollowUpDate { subject, on, today },
            &AppState::human_ctx(),
        )?;
        Ok(format!("prochaine relance le {on}"))
    })
    .await
}
