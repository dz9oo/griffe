//! Routes de la pièce Les affaires : liste, dossier, nouvelle conversation, lettre, rencontre.

use axum::Form;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue};
use axum::response::{Html, IntoResponse, Response};
use freeflow_core::app::{AppError, Executor, Outcome};
use freeflow_core::domain::{InteractionKind, Money, Probability, format_date, parse_date};
use freeflow_core::follow_up::{MarkFollowUpSent, PrepareFollowUp, SnoozeFollowUp};
use freeflow_core::people::person;
use freeflow_core::prospection::{CreateProspect, LogInteraction};
use maud::{Markup, html};
use serde::Deserialize;
use time::{Duration, OffsetDateTime};

use crate::layout::ViewId;
use crate::state::AppState;
use crate::views;
use crate::views::devis::{QuoteFormErrors, QuoteFormValues};
use crate::views::gens::{self, person_href};

fn is_htmx(headers: &HeaderMap) -> bool {
    headers.contains_key("hx-request")
}

fn page(headers: &HeaderMap, content: Markup) -> Html<String> {
    if is_htmx(headers) {
        Html(content.into_string())
    } else {
        Html(crate::layout::page(ViewId::Gens, "déverrouillé", content).into_string())
    }
}

fn locked(headers: &HeaderMap) -> Html<String> {
    page(
        headers,
        html! { div class="empty-state" { "coffre verrouillé — rechargez la page" } },
    )
}

fn with_push(headers: &HeaderMap, url: &str, content: Markup) -> Response {
    let mut response = page(headers, content).into_response();
    if let Ok(value) = HeaderValue::from_str(url) {
        response.headers_mut().insert("HX-Push-Url", value);
    }
    response
}

pub async fn show(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(reference): Path<String>,
) -> Html<String> {
    let today = state.today();
    let content = state
        .with_store(|store| {
            gens::dossier_page(store, &reference, today).unwrap_or_else(|e| {
                if e.to_string().contains("introuvable") {
                    gens::not_found(&reference, today)
                } else {
                    html! { div class="empty-state" { (e.to_string()) } }
                }
            })
        })
        .await
        .unwrap_or_else(|| html! { div class="empty-state" { "coffre verrouillé" } });
    page(&headers, content)
}

pub async fn new_get(State(state): State<AppState>, headers: HeaderMap) -> Html<String> {
    page(
        &headers,
        gens::new_conversation(state.today(), "", "", None, None, None),
    )
}

#[derive(Debug, Deserialize)]
pub struct NewForm {
    #[serde(default)]
    who: String,
    #[serde(default)]
    phrase: String,
}

pub async fn new_post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<NewForm>,
) -> Response {
    let today = state.today();
    let who = form.who.trim();
    let phrase = form.phrase.trim();
    if who.is_empty() {
        return page(
            &headers,
            gens::new_conversation(
                today,
                "",
                phrase,
                Some("un nom, pour commencer"),
                None,
                None,
            ),
        )
        .into_response();
    }
    if phrase.is_empty() {
        return page(
            &headers,
            gens::new_conversation(
                today,
                who,
                "",
                None,
                Some("une phrase, comme tu le dirais à voix haute"),
                None,
            ),
        )
        .into_response();
    }
    let cmd = CreateProspect {
        prospect_name: who.to_string(),
        address: None,
        representative: None,
        email: None,
        phone: None,
        name: phrase.to_string(),
        amount: Money::ZERO,
        probability: Probability::new(50).expect("50 ≤ 100"),
        next_action_at: today,
        source: None,
    };
    let result = state
        .with_store_mut(|store| Executor::new(store).execute(&cmd, &AppState::human_ctx()))
        .await;
    match result {
        None => locked(&headers).into_response(),
        Some(Err(e)) => page(
            &headers,
            gens::new_conversation(today, who, phrase, None, None, Some(&e.to_string())),
        )
        .into_response(),
        Some(Ok(_)) => {
            let href = person_href(who);
            let content = state
                .with_store(|store| {
                    gens::dossier_page(store, who, today).unwrap_or_else(
                        |err| html! { div class="empty-state" { (err.to_string()) } },
                    )
                })
                .await
                .unwrap_or_else(|| html! { div class="empty-state" { "coffre verrouillé" } });
            with_push(&headers, &href, content)
        }
    }
}

async fn load_dossier(
    state: &AppState,
    reference: &str,
) -> Option<Result<freeflow_core::people::PersonDossier, AppError>> {
    let today = state.today();
    state
        .with_store(|store| person(store.connection(), reference, today))
        .await
}

#[derive(Debug, Deserialize)]
pub struct LetterForm {
    #[serde(default)]
    subject_line: String,
    #[serde(default)]
    body: String,
}

fn optional_letter_field(value: String) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value)
    }
}

pub async fn write_get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(reference): Path<String>,
) -> Html<String> {
    let today = state.today();
    let Some(Ok(dossier)) = load_dossier(&state, &reference).await else {
        return page(&headers, gens::not_found(&reference, today));
    };
    let Some(subject) = dossier.follow_up_subject else {
        return page(
            &headers,
            gens::dossier_markup(&dossier, today, Some("rien à écrire pour l'instant")),
        );
    };
    let content = state
        .with_store(|store| {
            let card = gens::load_card(store, subject, today).ok().flatten();
            gens::letter_page(store, &dossier, card.as_ref(), today, None)
                .unwrap_or_else(|err| html! { div class="empty-state" { (err.to_string()) } })
        })
        .await
        .unwrap_or_else(|| html! { div class="empty-state" { "coffre verrouillé" } });
    page(&headers, content)
}

pub async fn write(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(reference): Path<String>,
    Form(form): Form<LetterForm>,
) -> Html<String> {
    let today = state.today();
    let Some(Ok(dossier)) = load_dossier(&state, &reference).await else {
        return page(&headers, gens::not_found(&reference, today));
    };
    let Some(subject) = dossier.follow_up_subject else {
        return page(
            &headers,
            gens::dossier_markup(&dossier, today, Some("rien à écrire pour l'instant")),
        );
    };
    let db_path = state.db_path().to_path_buf();
    let subject_line = optional_letter_field(form.subject_line);
    let body = optional_letter_field(form.body);
    let result = state
        .with_store_mut(|store| {
            let outcome = Executor::new(store).execute(
                &PrepareFollowUp {
                    subject,
                    today,
                    subject_line,
                    body,
                },
                &AppState::human_ctx(),
            )?;
            if let Outcome::Applied(prepared) = outcome {
                let _ = freeflow_cli::write_and_open_draft(
                    &db_path,
                    &prepared.draft.filename,
                    &prepared.draft.rfc5322,
                    true,
                );
            }
            gens::load_card(store, subject, today)
        })
        .await;
    match result {
        None => locked(&headers),
        Some(Err(e)) => page(
            &headers,
            gens::dossier_markup(&dossier, today, Some(&e.to_string())),
        ),
        Some(Ok(card)) => {
            let content = state
                .with_store(|store| {
                    gens::letter_page(
                        store,
                        &dossier,
                        card.as_ref(),
                        today,
                        Some("brouillon ouvert dans ton client mail — FreeFlow n'envoie pas"),
                    )
                    .unwrap_or_else(|err| html! { div class="empty-state" { (err.to_string()) } })
                })
                .await
                .unwrap_or_else(|| html! { div class="empty-state" { "coffre verrouillé" } });
            page(&headers, content)
        }
    }
}

pub async fn sent(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(reference): Path<String>,
    Form(form): Form<LetterForm>,
) -> Html<String> {
    let today = state.today();
    let Some(Ok(dossier)) = load_dossier(&state, &reference).await else {
        return page(&headers, gens::not_found(&reference, today));
    };
    let Some(subject) = dossier.follow_up_subject else {
        return page(&headers, gens::dossier_markup(&dossier, today, None));
    };
    let subject_line = optional_letter_field(form.subject_line);
    let body = optional_letter_field(form.body);
    let result = state
        .with_store_mut(|store| {
            Executor::new(store).execute(
                &MarkFollowUpSent {
                    subject,
                    today,
                    subject_line,
                    body,
                },
                &AppState::human_ctx(),
            )
        })
        .await;
    match result {
        None => locked(&headers),
        Some(Err(e)) => page(
            &headers,
            gens::dossier_markup(&dossier, today, Some(&e.to_string())),
        ),
        Some(Ok(_)) => {
            let content = state
                .with_store(|store| {
                    gens::dossier_page(store, &reference, today).unwrap_or_else(|err| {
                        html! { div class="empty-state" { (err.to_string()) } }
                    })
                })
                .await
                .unwrap_or_else(|| html! { div class="empty-state" { "coffre verrouillé" } });
            page(&headers, content)
        }
    }
}

pub async fn meeting_get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(reference): Path<String>,
) -> Html<String> {
    let today = state.today();
    match load_dossier(&state, &reference).await {
        Some(Ok(dossier)) => page(&headers, gens::meeting_form(&dossier, today, "", None)),
        _ => page(&headers, gens::not_found(&reference, today)),
    }
}

#[derive(Debug, Deserialize)]
pub struct MeetingForm {
    #[serde(default)]
    when: String,
    #[serde(default)]
    note: String,
}

pub async fn meeting_post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(reference): Path<String>,
    Form(form): Form<MeetingForm>,
) -> Response {
    let today = state.today();
    let Some(Ok(dossier)) = load_dossier(&state, &reference).await else {
        return page(&headers, gens::not_found(&reference, today)).into_response();
    };
    let Some(opportunity_id) = dossier.current.opportunity_id else {
        return page(
            &headers,
            gens::meeting_form(
                &dossier,
                today,
                &form.note,
                Some("pas de conversation ouverte"),
            ),
        )
        .into_response();
    };
    let occurred_on = parse_date(&form.when).unwrap_or(today);
    let occurred_at = occurred_on
        .with_hms(12, 0, 0)
        .ok()
        .map(|t| t.assume_offset(OffsetDateTime::now_utc().offset()));
    let cmd = LogInteraction {
        opportunity_id,
        kind: InteractionKind::Meeting,
        note: form.note.trim().to_string(),
        occurred_at,
    };
    let result = state
        .with_store_mut(|store| Executor::new(store).execute(&cmd, &AppState::human_ctx()))
        .await;
    match result {
        None => locked(&headers).into_response(),
        Some(Err(e)) => page(
            &headers,
            gens::meeting_form(&dossier, today, &form.note, Some(&e.to_string())),
        )
        .into_response(),
        Some(Ok(_)) => {
            let href = person_href(&dossier.name);
            let content = state
                .with_store(|store| {
                    gens::dossier_page(store, &reference, today).unwrap_or_else(
                        |err| html! { div class="empty-state" { (err.to_string()) } },
                    )
                })
                .await
                .unwrap_or_else(|| html! { div class="empty-state" { "coffre verrouillé" } });
            with_push(&headers, &href, content)
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct SnoozeForm {
    #[serde(default)]
    until: String,
}

pub async fn snooze(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(reference): Path<String>,
    Form(form): Form<SnoozeForm>,
) -> Html<String> {
    let today = state.today();
    let Some(Ok(dossier)) = load_dossier(&state, &reference).await else {
        return page(&headers, gens::not_found(&reference, today));
    };
    let Some(subject) = dossier.follow_up_subject else {
        return page(&headers, gens::dossier_markup(&dossier, today, None));
    };
    let until = parse_date(&form.until).unwrap_or(today + Duration::days(3));
    let result = state
        .with_store_mut(|store| {
            Executor::new(store).execute(
                &SnoozeFollowUp {
                    subject,
                    until,
                    today,
                },
                &AppState::human_ctx(),
            )
        })
        .await;
    match result {
        None => locked(&headers),
        Some(Err(e)) => page(
            &headers,
            gens::dossier_markup(&dossier, today, Some(&e.to_string())),
        ),
        Some(Ok(_)) => {
            let content = state
                .with_store(|store| {
                    gens::dossier_page(store, &reference, today).unwrap_or_else(
                        |err| html! { div class="empty-state" { (err.to_string()) } },
                    )
                })
                .await
                .unwrap_or_else(|| html! { div class="empty-state" { "coffre verrouillé" } });
            page(&headers, content)
        }
    }
}

pub async fn quote_panel(
    State(state): State<AppState>,
    Path(reference): Path<String>,
) -> Html<String> {
    let today = state.today();
    let valid_until = format_date(today + Duration::days(30));
    match load_dossier(&state, &reference).await {
        Some(Ok(dossier)) => {
            let values = QuoteFormValues {
                client: dossier.party.clone(),
                opportunity: dossier.current.opportunity_name.clone().unwrap_or_default(),
                valid_until,
                ..QuoteFormValues::default()
            };
            Html(views::devis::new_panel(&values, &QuoteFormErrors::default()).into_string())
        }
        _ => Html(html! { div class="empty-state" { "personne introuvable" } }.into_string()),
    }
}
