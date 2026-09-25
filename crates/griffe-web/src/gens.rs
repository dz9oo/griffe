//! Routes de la pièce Les affaires : liste, dossier, nouvelle conversation, lettre, rencontre.

use axum::Form;
use axum::extract::{Multipart, Path, State};
use axum::http::{HeaderMap, HeaderValue};
use axum::response::{Html, IntoResponse, Response};
use griffe_core::app::{AppError, Executor, Outcome};
use griffe_core::billing::{ImportIssuedInvoice, RetractWriteOff, WriteOffReceivable};
use griffe_core::clients::{
    CreateContact, UpdateClient, UpdateContact, client_by_id, list_contacts,
};
use griffe_core::domain::{
    Address, ExpenseId, InteractionKind, InvoiceId, InvoiceLine, MissionId, Money, Probability,
    VatRate, WriteOffId, format_date, parse_date,
};
use griffe_core::expenses::{AttachReceipt, expense_by_id};
use griffe_core::follow_up::{MarkFollowUpSent, PrepareFollowUp, SnoozeFollowUp};
use griffe_core::people::{PersonKey, person};
use griffe_core::prospection::{
    CreateOpportunity, CreateProspect, EstimationLineInput, LogInteraction, LoseOpportunity,
    ReopenOpportunity, SetEstimation, WinOpportunity, estimation_lines, opportunity_by_id,
};
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
) -> Option<Result<griffe_core::people::PersonDossier, AppError>> {
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
                let _ = griffe_cli::write_and_open_draft(
                    &db_path,
                    &prepared.draft.filename,
                    &prepared.draft.rfc5322,
                    griffe_cli::should_open_externally(),
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
                        Some("brouillon ouvert dans ton client mail — Griffe n'envoie pas"),
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
        Some(Ok(dossier)) => page(&headers, gens::meeting_form(&dossier, today, "", "", None)),
        _ => page(&headers, gens::not_found(&reference, today)),
    }
}

#[derive(Debug, Deserialize)]
pub struct MeetingForm {
    #[serde(default)]
    kind: String,
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
                &form.kind,
                &form.note,
                Some("pas de conversation ouverte"),
            ),
        )
        .into_response();
    };
    let kind = match form.kind.parse::<InteractionKind>() {
        Ok(kind) => kind,
        Err(_) => {
            return page(
                &headers,
                gens::meeting_form(
                    &dossier,
                    today,
                    &form.kind,
                    &form.note,
                    Some("la nature de la rencontre"),
                ),
            )
            .into_response();
        }
    };
    let occurred_on = parse_date(&form.when).unwrap_or(today);
    let occurred_at = occurred_on
        .with_hms(12, 0, 0)
        .ok()
        .map(|t| t.assume_offset(OffsetDateTime::now_utc().offset()));
    let cmd = LogInteraction {
        opportunity_id,
        kind,
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
            gens::meeting_form(
                &dossier,
                today,
                &form.kind,
                &form.note,
                Some(&e.to_string()),
            ),
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

pub async fn stop_get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(reference): Path<String>,
) -> Html<String> {
    let today = state.today();
    match load_dossier(&state, &reference).await {
        Some(Ok(dossier)) => page(&headers, gens::stop_page(&dossier, "", "", None)),
        _ => page(&headers, gens::not_found(&reference, today)),
    }
}

#[derive(Debug, Deserialize)]
pub struct StopForm {
    #[serde(default)]
    reason: String,
    #[serde(default)]
    detail: String,
}

pub async fn stop_post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(reference): Path<String>,
    Form(form): Form<StopForm>,
) -> Response {
    let today = state.today();
    let Some(Ok(dossier)) = load_dossier(&state, &reference).await else {
        return page(&headers, gens::not_found(&reference, today)).into_response();
    };
    let Some(opportunity_id) = dossier.current.opportunity_id else {
        return page(
            &headers,
            gens::stop_page(
                &dossier,
                &form.reason,
                &form.detail,
                Some("pas de conversation"),
            ),
        )
        .into_response();
    };
    let reason = match crate::views::prospection::parse_loss_reason(&form.reason, &form.detail) {
        Ok(reason) => reason,
        Err(msg) => {
            return page(
                &headers,
                gens::stop_page(&dossier, &form.reason, &form.detail, Some(&msg)),
            )
            .into_response();
        }
    };
    if matches!(reason, griffe_core::domain::LossReason::Other(ref text) if text.is_empty()) {
        return page(
            &headers,
            gens::stop_page(
                &dossier,
                &form.reason,
                &form.detail,
                Some("précise le motif"),
            ),
        )
        .into_response();
    }
    let cmd = LoseOpportunity {
        opportunity_id,
        reason,
    };
    let result = state
        .with_store_mut(|store| Executor::new(store).execute(&cmd, &AppState::human_ctx()))
        .await;
    match result {
        None => locked(&headers).into_response(),
        Some(Err(e)) => page(
            &headers,
            gens::stop_page(&dossier, &form.reason, &form.detail, Some(&e.to_string())),
        )
        .into_response(),
        Some(Ok(_)) => dossier_after(&state, &headers, &reference, &dossier.name).await,
    }
}

pub async fn win_get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(reference): Path<String>,
) -> Html<String> {
    let today = state.today();
    match load_dossier(&state, &reference).await {
        Some(Ok(dossier)) => page(
            &headers,
            gens::win_page(&dossier, &format_date(today), None),
        ),
        _ => page(&headers, gens::not_found(&reference, today)),
    }
}

#[derive(Debug, Deserialize)]
pub struct WinForm {
    #[serde(default)]
    started_on: String,
}

pub async fn win_post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(reference): Path<String>,
    Form(form): Form<WinForm>,
) -> Response {
    let today = state.today();
    let Some(Ok(dossier)) = load_dossier(&state, &reference).await else {
        return page(&headers, gens::not_found(&reference, today)).into_response();
    };
    let Some(opportunity_id) = dossier.current.opportunity_id else {
        return page(
            &headers,
            gens::win_page(&dossier, &form.started_on, Some("pas de conversation")),
        )
        .into_response();
    };
    if dossier
        .current
        .amount
        .is_none_or(|amount| amount.cents() == 0)
    {
        return page(
            &headers,
            gens::win_page(
                &dossier,
                &form.started_on,
                Some("note d'abord une estimation"),
            ),
        )
        .into_response();
    }
    let started_on = parse_date(&form.started_on).unwrap_or(today);
    let cmd = WinOpportunity {
        opportunity_id,
        started_on,
    };
    let result = state
        .with_store_mut(|store| Executor::new(store).execute(&cmd, &AppState::human_ctx()))
        .await;
    match result {
        None => locked(&headers).into_response(),
        Some(Err(e)) => page(
            &headers,
            gens::win_page(&dossier, &form.started_on, Some(&e.to_string())),
        )
        .into_response(),
        Some(Ok(_)) => dossier_after(&state, &headers, &reference, &dossier.name).await,
    }
}

pub async fn reopen_get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(reference): Path<String>,
) -> Html<String> {
    let today = state.today();
    match load_dossier(&state, &reference).await {
        Some(Ok(dossier)) => page(
            &headers,
            gens::reopen_page(&dossier, &format_date(today), None),
        ),
        _ => page(&headers, gens::not_found(&reference, today)),
    }
}

#[derive(Debug, Deserialize)]
pub struct ReopenForm {
    #[serde(default)]
    when: String,
}

pub async fn reopen_post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(reference): Path<String>,
    Form(form): Form<ReopenForm>,
) -> Response {
    let today = state.today();
    let Some(Ok(dossier)) = load_dossier(&state, &reference).await else {
        return page(&headers, gens::not_found(&reference, today)).into_response();
    };
    let Some(opportunity_id) = dossier.current.opportunity_id else {
        return page(
            &headers,
            gens::reopen_page(&dossier, &form.when, Some("pas de conversation arrêtée")),
        )
        .into_response();
    };
    let when = parse_date(&form.when).unwrap_or(today);
    let cmd = ReopenOpportunity {
        opportunity_id,
        next_action_at: when,
    };
    let result = state
        .with_store_mut(|store| Executor::new(store).execute(&cmd, &AppState::human_ctx()))
        .await;
    match result {
        None => locked(&headers).into_response(),
        Some(Err(e)) => page(
            &headers,
            gens::reopen_page(&dossier, &form.when, Some(&e.to_string())),
        )
        .into_response(),
        Some(Ok(_)) => dossier_after(&state, &headers, &reference, &dossier.name).await,
    }
}

async fn dossier_after(
    state: &AppState,
    headers: &HeaderMap,
    reference: &str,
    name: &str,
) -> Response {
    let today = state.today();
    let href = person_href(name);
    let content = state
        .with_store(|store| {
            gens::dossier_page(store, reference, today)
                .unwrap_or_else(|err| html! { div class="empty-state" { (err.to_string()) } })
        })
        .await
        .unwrap_or_else(|| html! { div class="empty-state" { "coffre verrouillé" } });
    with_push(headers, &href, content)
}

fn blank(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn parse_address(street: &str, postal_code: &str, city: &str) -> Result<Option<Address>, String> {
    let street = street.trim();
    let postal_code = postal_code.trim();
    let city = city.trim();
    if street.is_empty() && postal_code.is_empty() && city.is_empty() {
        return Ok(None);
    }
    if street.is_empty() || postal_code.is_empty() || city.is_empty() {
        return Err("rue, code et ville — ou rien".into());
    }
    Ok(Some(Address {
        street: street.to_string(),
        postal_code: postal_code.to_string(),
        city: city.to_string(),
        country: "FR".into(),
    }))
}

fn fiche_values_from(
    client: &griffe_core::domain::Client,
    contact: Option<&griffe_core::domain::Contact>,
) -> gens::FicheValues {
    let (street, postal_code, city) = client.address.as_ref().map_or_else(
        || (String::new(), String::new(), String::new()),
        |a| (a.street.clone(), a.postal_code.clone(), a.city.clone()),
    );
    let representative = contact
        .filter(|c| c.name != client.name)
        .map(|c| c.name.clone())
        .unwrap_or_default();
    gens::FicheValues {
        who: client.name.clone(),
        street,
        postal_code,
        city,
        representative,
        email: contact.and_then(|c| c.email.clone()).unwrap_or_default(),
        phone: contact.and_then(|c| c.phone.clone()).unwrap_or_default(),
        client_revision: client.revision,
        contact_id: contact.map(|c| c.id.to_string()).unwrap_or_default(),
        contact_revision: contact.map(|c| c.revision.to_string()).unwrap_or_default(),
    }
}

pub async fn fiche_get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(reference): Path<String>,
) -> Html<String> {
    let today = state.today();
    let Some(Ok(dossier)) = load_dossier(&state, &reference).await else {
        return page(&headers, gens::not_found(&reference, today));
    };
    let PersonKey::Client { id } = dossier.key else {
        return page(
            &headers,
            gens::dossier_markup(&dossier, today, Some("cette chemise n'a pas de fiche")),
        );
    };
    let loaded = state
        .with_store(|store| -> Result<_, AppError> {
            let client = client_by_id(store.connection(), id)?
                .ok_or_else(|| AppError::from(griffe_core::clients::ClientError::NotFound(id)))?;
            let contact = list_contacts(store.connection(), id)?.into_iter().next();
            Ok((client, contact))
        })
        .await;
    match loaded {
        None => locked(&headers),
        Some(Err(e)) => page(
            &headers,
            gens::dossier_markup(&dossier, today, Some(&e.to_string())),
        ),
        Some(Ok((client, contact))) => {
            let values = fiche_values_from(&client, contact.as_ref());
            page(
                &headers,
                gens::fiche_page(
                    &dossier,
                    &values,
                    &gens::FicheErrors {
                        who: None,
                        address: None,
                        banner: None,
                    },
                ),
            )
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct FicheForm {
    #[serde(default)]
    who: String,
    #[serde(default)]
    street: String,
    #[serde(default)]
    postal_code: String,
    #[serde(default)]
    city: String,
    #[serde(default)]
    representative: String,
    #[serde(default)]
    email: String,
    #[serde(default)]
    phone: String,
    #[serde(default)]
    client_revision: String,
    #[serde(default)]
    contact_id: String,
    #[serde(default)]
    contact_revision: String,
}

pub async fn fiche_post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(reference): Path<String>,
    Form(form): Form<FicheForm>,
) -> Response {
    let today = state.today();
    let Some(Ok(dossier)) = load_dossier(&state, &reference).await else {
        return page(&headers, gens::not_found(&reference, today)).into_response();
    };
    let PersonKey::Client { id } = dossier.key else {
        return page(
            &headers,
            gens::dossier_markup(&dossier, today, Some("cette chemise n'a pas de fiche")),
        )
        .into_response();
    };
    let who = form.who.trim().to_string();
    let address = match parse_address(&form.street, &form.postal_code, &form.city) {
        Ok(address) => address,
        Err(msg) => {
            let values = gens::FicheValues {
                who,
                street: form.street,
                postal_code: form.postal_code,
                city: form.city,
                representative: form.representative,
                email: form.email,
                phone: form.phone,
                client_revision: form.client_revision.parse().unwrap_or(0),
                contact_id: form.contact_id,
                contact_revision: form.contact_revision,
            };
            return page(
                &headers,
                gens::fiche_page(
                    &dossier,
                    &values,
                    &gens::FicheErrors {
                        who: None,
                        address: Some(msg),
                        banner: None,
                    },
                ),
            )
            .into_response();
        }
    };
    if who.is_empty() {
        let values = gens::FicheValues {
            who,
            street: form.street,
            postal_code: form.postal_code,
            city: form.city,
            representative: form.representative,
            email: form.email,
            phone: form.phone,
            client_revision: form.client_revision.parse().unwrap_or(0),
            contact_id: form.contact_id,
            contact_revision: form.contact_revision,
        };
        return page(
            &headers,
            gens::fiche_page(
                &dossier,
                &values,
                &gens::FicheErrors {
                    who: Some("un nom, pour commencer".into()),
                    address: None,
                    banner: None,
                },
            ),
        )
        .into_response();
    }
    let Some(client_revision) = form.client_revision.parse::<i64>().ok() else {
        return page(
            &headers,
            gens::dossier_markup(&dossier, today, Some("la fiche a changé, rechargez")),
        )
        .into_response();
    };
    let representative = blank(&form.representative);
    let email = blank(&form.email);
    let phone = blank(&form.phone);
    let contact_name = representative.clone().unwrap_or_else(|| who.clone());
    let posted_contact_revision = form.contact_revision.parse::<i64>().ok();
    let result = state
        .with_store_mut(|store| -> Result<(), AppError> {
            let client = client_by_id(store.connection(), id)?
                .ok_or_else(|| AppError::from(griffe_core::clients::ClientError::NotFound(id)))?;
            let contact = list_contacts(store.connection(), id)?.into_iter().next();
            Executor::new(store).execute(
                &UpdateClient {
                    id,
                    revision: client_revision,
                    name: who.clone(),
                    siren: client.siren,
                    vat_number: client.vat_number,
                    address: address.clone(),
                },
                &AppState::human_ctx(),
            )?;
            match contact {
                Some(current) => {
                    let revision = posted_contact_revision.unwrap_or(current.revision);
                    Executor::new(store).execute(
                        &UpdateContact {
                            id: current.id,
                            revision,
                            name: contact_name.clone(),
                            email: email.clone(),
                            phone: phone.clone(),
                            role: current.role,
                        },
                        &AppState::human_ctx(),
                    )?;
                }
                None if representative.is_some() || email.is_some() || phone.is_some() => {
                    Executor::new(store).execute(
                        &CreateContact {
                            client_id: id,
                            name: contact_name.clone(),
                            email: email.clone(),
                            phone: phone.clone(),
                            role: None,
                        },
                        &AppState::human_ctx(),
                    )?;
                }
                None => {}
            }
            Ok(())
        })
        .await;
    match result {
        None => locked(&headers).into_response(),
        Some(Err(e)) => {
            let values = gens::FicheValues {
                who,
                street: form.street,
                postal_code: form.postal_code,
                city: form.city,
                representative: form.representative,
                email: form.email,
                phone: form.phone,
                client_revision,
                contact_id: form.contact_id,
                contact_revision: form.contact_revision,
            };
            page(
                &headers,
                gens::fiche_page(
                    &dossier,
                    &values,
                    &gens::FicheErrors {
                        who: None,
                        address: None,
                        banner: Some(e.to_string()),
                    },
                ),
            )
            .into_response()
        }
        Some(Ok(())) => {
            let rendered = state
                .with_store(|store| -> Result<_, AppError> {
                    let dossier = person(store.connection(), &id.to_string(), today)?;
                    let href = person_href(&dossier.name);
                    let markup = gens::dossier_markup(&dossier, today, Some("Fiche à jour."));
                    Ok((href, markup))
                })
                .await;
            match rendered {
                None => locked(&headers).into_response(),
                Some(Err(e)) => page(
                    &headers,
                    gens::dossier_markup(&dossier, today, Some(&e.to_string())),
                )
                .into_response(),
                Some(Ok((href, markup))) => with_push(&headers, &href, markup),
            }
        }
    }
}

pub async fn estimate_get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(reference): Path<String>,
) -> Html<String> {
    let today = state.today();
    let Some(Ok(dossier)) = load_dossier(&state, &reference).await else {
        return page(&headers, gens::not_found(&reference, today));
    };
    let values = state
        .with_store(|store| -> Result<gens::EstimateValues, AppError> {
            let opp = dossier
                .current
                .opportunity_id
                .and_then(|id| opportunity_by_id(store.connection(), id).ok().flatten());
            Ok(match opp {
                Some(opp) => {
                    let stored = estimation_lines(store.connection(), opp.id)?;
                    let mut lines: Vec<(String, String)> = stored
                        .into_iter()
                        .map(|(label, amount)| (label, amount.to_decimal_string()))
                        .collect();
                    if lines.is_empty() && opp.amount.cents() > 0 {
                        lines.push((opp.name.clone(), opp.amount.to_decimal_string()));
                    }
                    if lines.is_empty() {
                        lines.push((String::new(), String::new()));
                    }
                    gens::EstimateValues {
                        phrase: opp.name,
                        lines,
                        revision: opp.revision.to_string(),
                    }
                }
                None => gens::EstimateValues {
                    phrase: dossier.current.opportunity_name.clone().unwrap_or_default(),
                    lines: vec![(String::new(), String::new())],
                    revision: String::new(),
                },
            })
        })
        .await;
    match values {
        None => locked(&headers),
        Some(Err(e)) => page(
            &headers,
            gens::dossier_markup(&dossier, today, Some(&e.to_string())),
        ),
        Some(Ok(values)) => page(
            &headers,
            gens::estimate_page(
                &dossier,
                &values,
                &gens::EstimateErrors {
                    phrase: None,
                    lines: None,
                    banner: None,
                },
            ),
        ),
    }
}

#[derive(Debug, Deserialize)]
pub struct EstimateForm {
    #[serde(default)]
    phrase: String,
    #[serde(default)]
    label: Vec<String>,
    /// Ancien formulaire : un seul montant.
    #[serde(default)]
    amount: String,
    #[serde(default)]
    euros: Vec<String>,
    #[serde(default)]
    revision: String,
    #[serde(default)]
    add: String,
}

pub async fn estimate_post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(reference): Path<String>,
    Form(form): Form<EstimateForm>,
) -> Response {
    let today = state.today();
    let Some(Ok(dossier)) = load_dossier(&state, &reference).await else {
        return page(&headers, gens::not_found(&reference, today)).into_response();
    };
    let PersonKey::Client { id: client_id } = dossier.key else {
        return page(
            &headers,
            gens::dossier_markup(&dossier, today, Some("cette chemise n'est pas une cliente")),
        )
        .into_response();
    };
    let phrase = form.phrase.trim().to_string();
    let mut paired: Vec<(String, String)> = if form.label.is_empty() && !form.amount.trim().is_empty()
    {
        vec![(phrase.clone(), form.amount.clone())]
    } else {
        form.label
            .iter()
            .zip(form.euros.iter())
            .map(|(label, amount)| (label.clone(), amount.clone()))
            .filter(|(label, amount)| !label.trim().is_empty() || !amount.trim().is_empty())
            .collect()
    };
    if !form.add.is_empty() {
        paired.push((String::new(), String::new()));
        let values = gens::EstimateValues {
            phrase,
            lines: if paired.is_empty() {
                vec![(String::new(), String::new())]
            } else {
                paired
            },
            revision: form.revision,
        };
        return page(
            &headers,
            gens::estimate_page(
                &dossier,
                &values,
                &gens::EstimateErrors {
                    phrase: None,
                    lines: None,
                    banner: None,
                },
            ),
        )
        .into_response();
    }
    let mut errors = gens::EstimateErrors {
        phrase: None,
        lines: None,
        banner: None,
    };
    if phrase.is_empty() {
        errors.phrase = Some("ce dont il s'agit".into());
    }
    let mut parsed = Vec::new();
    for (label, amount) in &paired {
        match Money::parse_decimal(amount.trim()) {
            Ok(amount) if amount.cents() > 0 && !label.trim().is_empty() => {
                parsed.push(EstimationLineInput {
                    label: label.trim().to_string(),
                    amount,
                });
            }
            _ => errors.lines = Some("chaque ligne a un libellé et un montant".into()),
        }
    }
    if parsed.is_empty() {
        errors.lines = Some("au moins une ligne de travaux".into());
    }
    if errors.phrase.is_some() || errors.lines.is_some() {
        let values = gens::EstimateValues {
            phrase,
            lines: if paired.is_empty() {
                vec![(String::new(), String::new())]
            } else {
                paired
            },
            revision: form.revision,
        };
        return page(&headers, gens::estimate_page(&dossier, &values, &errors)).into_response();
    }
    let opportunity_id = dossier.current.opportunity_id;
    let revision = form.revision.parse::<i64>().ok();
    let result = state
        .with_store_mut(|store| -> Result<(), AppError> {
            let id = if let Some(id) = opportunity_id {
                id
            } else {
                let outcome = Executor::new(store).execute(
                    &CreateOpportunity {
                        client_id,
                        name: phrase.clone(),
                        amount: Money::from_cents(1),
                        probability: Probability::new(50).expect("50 ≤ 100"),
                        next_action_at: today,
                        source: None,
                    },
                    &AppState::human_ctx(),
                )?;
                match outcome {
                    Outcome::Applied(id) | Outcome::AlreadyApplied(id) => id,
                    Outcome::DryRun | Outcome::PendingConfirmation(_) => {
                        return Err(AppError::from(
                            griffe_core::prospection::ProspectionError::EstimationRequired,
                        ));
                    }
                }
            };
            let opp = opportunity_by_id(store.connection(), id)?.ok_or_else(|| {
                AppError::from(griffe_core::prospection::ProspectionError::NotFound(id))
            })?;
            Executor::new(store).execute(
                &SetEstimation {
                    id,
                    revision: revision.unwrap_or(opp.revision),
                    name: phrase.clone(),
                    lines: parsed.clone(),
                },
                &AppState::human_ctx(),
            )?;
            Ok(())
        })
        .await;
    match result {
        None => locked(&headers).into_response(),
        Some(Err(e)) => {
            let values = gens::EstimateValues {
                phrase,
                lines: paired,
                revision: form.revision,
            };
            errors.banner = Some(e.to_string());
            page(&headers, gens::estimate_page(&dossier, &values, &errors)).into_response()
        }
        Some(Ok(())) => {
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

/// Joint un justificatif à une note de la chemise, sans quitter le dossier.
pub async fn attach_note(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((reference, expense_id)): Path<(String, String)>,
    mut multipart: Multipart,
) -> Response {
    let today = state.today();
    let Some(Ok(dossier)) = load_dossier(&state, &reference).await else {
        return page(&headers, gens::not_found(&reference, today)).into_response();
    };
    let Some(id) = expense_id.parse::<ExpenseId>().ok() else {
        return page(
            &headers,
            gens::dossier_markup(&dossier, today, Some("cette note est introuvable")),
        )
        .into_response();
    };
    let mut revision: Option<i64> = None;
    let mut filename = String::new();
    let mut content: Vec<u8> = Vec::new();
    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or_default().to_string();
        if name == "receipt" {
            filename = field.file_name().unwrap_or_default().to_string();
            if let Ok(bytes) = field.bytes().await {
                content = bytes.to_vec();
            }
            continue;
        }
        if name == "revision"
            && let Ok(value) = field.text().await
        {
            revision = value.parse().ok();
        }
    }
    if filename.is_empty() || content.is_empty() {
        return page(
            &headers,
            gens::dossier_markup(&dossier, today, Some("choisissez un papier à joindre")),
        )
        .into_response();
    }
    let Some(rev) = revision else {
        return page(
            &headers,
            gens::dossier_markup(&dossier, today, Some("la note a changé, rechargez")),
        )
        .into_response();
    };
    let original = filename
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("justificatif")
        .to_string();
    let archived = state
        .with_store(
            |store| -> Result<griffe_core::receipts::ArchivedReceipt, String> {
                let Some(expense) = expense_by_id(store.connection(), id).ok().flatten() else {
                    return Err("cette note est introuvable".into());
                };
                if expense.supplier.as_deref() != Some(dossier.name.as_str()) {
                    return Err("cette note n'appartient pas à ce dossier".into());
                }
                griffe_core::receipts::archive(store, &original, &content)
                    .map_err(|e| e.to_string())
            },
        )
        .await;
    let archived = match archived {
        None => return locked(&headers).into_response(),
        Some(Err(msg)) => {
            return page(&headers, gens::dossier_markup(&dossier, today, Some(&msg)))
                .into_response();
        }
        Some(Ok(a)) => a,
    };
    let cmd = AttachReceipt {
        id,
        revision: rev,
        receipt_hash: archived.hash,
        receipt_filename: archived.filename,
    };
    let result = state
        .with_store_mut(|store| Executor::new(store).execute(&cmd, &AppState::human_ctx()))
        .await;
    match result {
        None => locked(&headers).into_response(),
        Some(Err(e)) => page(
            &headers,
            gens::dossier_markup(&dossier, today, Some(&e.to_string())),
        )
        .into_response(),
        Some(Ok(_)) => {
            let content = state
                .with_store(|store| {
                    gens::dossier_page(store, &reference, today).unwrap_or_else(
                        |err| html! { div class="empty-state" { (err.to_string()) } },
                    )
                })
                .await
                .unwrap_or_else(|| html! { div class="empty-state" { "coffre verrouillé" } });
            page(&headers, content).into_response()
        }
    }
}

#[derive(Default)]
struct ImportInvoiceFields {
    number: String,
    issued_on: String,
    payment_terms_days: String,
    description: String,
    quantity: String,
    unit_price: String,
    vat_rate: String,
    mission: String,
    credits: String,
    filename: String,
    content: Vec<u8>,
}

/// Colle au dossier une facture née ailleurs : le numéro est celui du PDF, jamais un `FA-`.
pub async fn import_invoice(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(reference): Path<String>,
    mut multipart: Multipart,
) -> Response {
    let today = state.today();
    let dossier = match load_dossier(&state, &reference).await {
        None => return locked(&headers).into_response(),
        Some(Err(_)) => return page(&headers, gens::not_found(&reference, today)).into_response(),
        Some(Ok(dossier)) => dossier,
    };
    if dossier.not_yet_client {
        return page(
            &headers,
            gens::dossier_markup(
                &dossier,
                today,
                Some("cette fiche n'est pas encore cliente"),
            ),
        )
        .into_response();
    }
    let PersonKey::Client { id: client_id } = dossier.key else {
        return page(
            &headers,
            gens::dossier_markup(&dossier, today, Some("cette chemise n'est pas une cliente")),
        )
        .into_response();
    };

    let mut fields = ImportInvoiceFields::default();
    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or_default().to_string();
        if name == "file" {
            fields.filename = field.file_name().unwrap_or_default().to_string();
            if let Ok(bytes) = field.bytes().await {
                fields.content = bytes.to_vec();
            }
            continue;
        }
        let Ok(value) = field.text().await else {
            continue;
        };
        match name.as_str() {
            "number" => fields.number = value,
            "issued_on" => fields.issued_on = value,
            "payment_terms_days" => fields.payment_terms_days = value,
            "description" => fields.description = value,
            "quantity" => fields.quantity = value,
            "unit_price" => fields.unit_price = value,
            "vat_rate" => fields.vat_rate = value,
            "mission" => fields.mission = value,
            "credits" => fields.credits = value,
            _ => {}
        }
    }

    if fields.filename.is_empty() || fields.content.is_empty() {
        return page(
            &headers,
            gens::dossier_markup(&dossier, today, Some("choisissez le PDF")),
        )
        .into_response();
    }

    let parsed = match parse_import_fields(&fields) {
        Ok(parsed) => parsed,
        Err(msg) => {
            return page(&headers, gens::dossier_markup(&dossier, today, Some(&msg)))
                .into_response();
        }
    };

    let original = fields
        .filename
        .rsplit(['/', '\\'])
        .next()
        .filter(|n| !n.is_empty())
        .unwrap_or("facture.pdf")
        .to_string();
    let bytes = fields.content;
    let cmd = ImportIssuedInvoice {
        number: parsed.number,
        client_id,
        mission_id: parsed.mission_id,
        lines: vec![parsed.line],
        issued_on: parsed.issued_on,
        payment_terms_days: parsed.payment_terms_days,
        credited_invoice_id: parsed.credited_invoice_id,
    };
    let result = state
        .with_store_mut(|store| -> Result<String, String> {
            match Executor::new(store).execute(&cmd, &AppState::human_ctx()) {
                Ok(Outcome::Applied(emitted) | Outcome::AlreadyApplied(emitted)) => {
                    let number = emitted.number;
                    let note =
                        griffe_cli::invoice_capture_note(griffe_cli::capture_imported_invoice(
                            store,
                            &AppState::human_ctx(),
                            emitted.id,
                            &original,
                            &bytes,
                        ));
                    Ok(griffe_cli::append_capture_note(
                        format!("facture {number} collée au dossier"),
                        false,
                        note,
                    ))
                }
                Ok(_) => Err("la facture n'a pas été collée".into()),
                Err(e) => Err(e.to_string()),
            }
        })
        .await;
    match result {
        None => locked(&headers).into_response(),
        Some(Err(msg)) => {
            page(&headers, gens::dossier_markup(&dossier, today, Some(&msg))).into_response()
        }
        Some(Ok(flash)) => {
            let content = state
                .with_store(
                    |store| match person(store.connection(), &reference, today) {
                        Ok(fresh) => gens::dossier_markup(&fresh, today, Some(&flash)),
                        Err(err) => html! { div class="empty-state" { (err.to_string()) } },
                    },
                )
                .await
                .unwrap_or_else(|| html! { div class="empty-state" { "coffre verrouillé" } });
            page(&headers, content).into_response()
        }
    }
}

struct ParsedImport {
    number: String,
    issued_on: time::Date,
    payment_terms_days: u32,
    line: InvoiceLine,
    mission_id: Option<MissionId>,
    credited_invoice_id: Option<InvoiceId>,
}

fn parse_import_fields(fields: &ImportInvoiceFields) -> Result<ParsedImport, String> {
    let issued_on = parse_date(fields.issued_on.trim())
        .map_err(|_| "indiquez la date (AAAA-MM-JJ)".to_string())?;
    let payment_terms_days = if fields.payment_terms_days.trim().is_empty() {
        30
    } else {
        fields
            .payment_terms_days
            .trim()
            .parse::<u32>()
            .map_err(|_| "indiquez le délai en jours".to_string())?
    };
    let description = fields.description.trim();
    if description.is_empty() {
        return Err("indiquez la description".into());
    }
    let quantity = if fields.quantity.trim().is_empty() {
        1.0
    } else {
        fields
            .quantity
            .trim()
            .parse::<f64>()
            .map_err(|_| "indiquez la quantité".to_string())?
    };
    let unit_price = Money::parse_decimal(fields.unit_price.trim())
        .map_err(|_| "indiquez le prix HT".to_string())?;
    let vat_raw = fields.vat_rate.trim();
    let vat_rate = if vat_raw.is_empty() {
        VatRate::Standard
    } else {
        vat_raw
            .parse::<VatRate>()
            .map_err(|_| "taux de TVA inconnu".to_string())?
    };
    let mission_id = optional_id::<MissionId>(&fields.mission, "cette mission est introuvable")?;
    let credited_invoice_id =
        optional_id::<InvoiceId>(&fields.credits, "cette facture est introuvable")?;
    Ok(ParsedImport {
        number: fields.number.clone(),
        issued_on,
        payment_terms_days,
        line: InvoiceLine {
            description: description.to_string(),
            quantity,
            unit_price,
            vat_rate,
        },
        mission_id,
        credited_invoice_id,
    })
}

fn optional_id<T: std::str::FromStr>(raw: &str, err: &str) -> Result<Option<T>, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    trimmed.parse::<T>().map(Some).map_err(|_| err.to_string())
}

#[derive(Debug, Deserialize)]
pub struct WriteOffForm {
    #[serde(default)]
    on: String,
    #[serde(default)]
    write_off_id: String,
}

fn parse_on(raw: &str, today: time::Date) -> Result<time::Date, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        Ok(today)
    } else {
        parse_date(trimmed).map_err(|_| "indiquez la date (AAAA-MM-JJ)".to_string())
    }
}

/// Constate que le reste dû n'est plus attendu — confirmation = soumission du formulaire daté.
pub async fn write_off_receivable(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((reference, invoice_id)): Path<(String, String)>,
    Form(form): Form<WriteOffForm>,
) -> Response {
    let today = state.today();
    let dossier = match load_dossier(&state, &reference).await {
        None => return locked(&headers).into_response(),
        Some(Err(_)) => return page(&headers, gens::not_found(&reference, today)).into_response(),
        Some(Ok(dossier)) => dossier,
    };
    let Ok(invoice_id) = invoice_id.parse::<InvoiceId>() else {
        return page(
            &headers,
            gens::dossier_markup(&dossier, today, Some("cette facture est introuvable")),
        )
        .into_response();
    };
    if !dossier
        .papers
        .iter()
        .any(|p| p.invoice_id == Some(invoice_id))
    {
        return page(
            &headers,
            gens::dossier_markup(
                &dossier,
                today,
                Some("cette facture n'est pas dans ce dossier"),
            ),
        )
        .into_response();
    }
    let written_off_on = match parse_on(&form.on, today) {
        Ok(on) => on,
        Err(msg) => {
            return page(&headers, gens::dossier_markup(&dossier, today, Some(&msg)))
                .into_response();
        }
    };
    let cmd = WriteOffReceivable {
        invoice_id,
        written_off_on,
    };
    let result = state
        .with_store_mut(|store| -> Result<String, String> {
            match Executor::new(store).execute(&cmd, &AppState::human_ctx()) {
                Ok(Outcome::Applied(write_off)) => {
                    if write_off.recovers_vat
                        && let Err(e) = griffe_cli::write_uncollectible_notice(
                            store,
                            &AppState::human_ctx(),
                            &write_off,
                        )
                    {
                        return Ok(format!(
                            "on ne l'attend plus {} — duplicata non figé : {e}",
                            write_off.ttc
                        ));
                    }
                    Ok(format!("on ne l'attend plus {}", write_off.ttc))
                }
                Ok(Outcome::AlreadyApplied(write_off)) => {
                    Ok(format!("on ne l'attend plus {}", write_off.ttc))
                }
                Ok(_) => Err("la perte n'a pas été enregistrée".into()),
                Err(e) => Err(e.to_string()),
            }
        })
        .await;
    dossier_flash(&state, &headers, &reference, today, &dossier, result).await
}

/// Rétablit une créance : contre-écriture par date, pas une suppression.
pub async fn retract_write_off(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((reference, invoice_id)): Path<(String, String)>,
    Form(form): Form<WriteOffForm>,
) -> Response {
    let today = state.today();
    let dossier = match load_dossier(&state, &reference).await {
        None => return locked(&headers).into_response(),
        Some(Err(_)) => return page(&headers, gens::not_found(&reference, today)).into_response(),
        Some(Ok(dossier)) => dossier,
    };
    let Ok(invoice_id) = invoice_id.parse::<InvoiceId>() else {
        return page(
            &headers,
            gens::dossier_markup(&dossier, today, Some("cette facture est introuvable")),
        )
        .into_response();
    };
    let paper = dossier
        .papers
        .iter()
        .find(|p| p.invoice_id == Some(invoice_id));
    let Some(paper) = paper else {
        return page(
            &headers,
            gens::dossier_markup(
                &dossier,
                today,
                Some("cette facture n'est pas dans ce dossier"),
            ),
        )
        .into_response();
    };
    let write_off_id = match form.write_off_id.trim().parse::<WriteOffId>() {
        Ok(id) => id,
        Err(_) => {
            return page(
                &headers,
                gens::dossier_markup(&dossier, today, Some("cette perte est introuvable")),
            )
            .into_response();
        }
    };
    if paper.write_off_id != Some(write_off_id) {
        return page(
            &headers,
            gens::dossier_markup(
                &dossier,
                today,
                Some("cette perte n'est pas celle du dossier"),
            ),
        )
        .into_response();
    }
    let retracted_on = match parse_on(&form.on, today) {
        Ok(on) => on,
        Err(msg) => {
            return page(&headers, gens::dossier_markup(&dossier, today, Some(&msg)))
                .into_response();
        }
    };
    let cmd = RetractWriteOff {
        write_off_id,
        retracted_on,
    };
    let result = state
        .with_store_mut(|store| -> Result<String, String> {
            match Executor::new(store).execute(&cmd, &AppState::human_ctx()) {
                Ok(Outcome::Applied(()) | Outcome::AlreadyApplied(())) => {
                    Ok("on l'attend encore".into())
                }
                Ok(_) => Err("la créance n'a pas été rétablie".into()),
                Err(e) => Err(e.to_string()),
            }
        })
        .await;
    dossier_flash(&state, &headers, &reference, today, &dossier, result).await
}

async fn dossier_flash(
    state: &AppState,
    headers: &HeaderMap,
    reference: &str,
    today: time::Date,
    dossier: &griffe_core::people::PersonDossier,
    result: Option<Result<String, String>>,
) -> Response {
    match result {
        None => locked(headers).into_response(),
        Some(Err(msg)) => {
            page(headers, gens::dossier_markup(dossier, today, Some(&msg))).into_response()
        }
        Some(Ok(flash)) => {
            let content = state
                .with_store(|store| match person(store.connection(), reference, today) {
                    Ok(fresh) => gens::dossier_markup(&fresh, today, Some(&flash)),
                    Err(err) => html! { div class="empty-state" { (err.to_string()) } },
                })
                .await
                .unwrap_or_else(|| html! { div class="empty-state" { "coffre verrouillé" } });
            page(headers, content).into_response()
        }
    }
}
