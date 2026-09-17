//! Routes de la pièce Les affaires : liste, dossier, nouvelle conversation, lettre, rencontre.

use axum::Form;
use axum::extract::{Multipart, Path, State};
use axum::http::{HeaderMap, HeaderValue};
use axum::response::{Html, IntoResponse, Response};
use griffe_core::app::{AppError, Executor, Outcome};
use griffe_core::billing::ImportIssuedInvoice;
use griffe_core::domain::{
    ExpenseId, InteractionKind, InvoiceId, InvoiceLine, MissionId, Money, Probability, VatRate,
    format_date, parse_date,
};
use griffe_core::expenses::{AttachReceipt, expense_by_id};
use griffe_core::follow_up::{MarkFollowUpSent, PrepareFollowUp, SnoozeFollowUp};
use griffe_core::people::{PersonKey, person};
use griffe_core::prospection::{CreateProspect, LogInteraction};
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
