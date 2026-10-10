//! Routes de la pièce Les affaires : liste, dossier, nouvelle conversation, lettre, rencontre.

use axum::Form;
use std::collections::HashMap;

use axum::extract::{Multipart, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue};
use axum::response::{Html, IntoResponse, Response};
use griffe_core::app::{AppError, Executor, Outcome};
use griffe_core::billing::{ImportIssuedInvoice, RetractWriteOff, WriteOffReceivable};
use griffe_core::clients::{CreateContact, UpdateClient, UpdateContact, client_by_id};
use griffe_core::company::company_profile;
use griffe_core::domain::{
    Address, ExpenseId, FollowUpSubject, InteractionKind, InvoiceId, InvoiceLine, MissionId, Money,
    Probability, SnoozeDaysError, TemplateContext, VatRate, WriteOffId, chronicle,
    entered_cycle_key, format_date, given_name, parse_date, phrase_from_editor, phrase_to_editor,
    render_template, snooze_in_days,
};
use griffe_core::dossier_work::{DossierWork, SaveDossierWork, dossier_work};
use griffe_core::expenses::{AttachReceipt, expense_by_id};
use griffe_core::follow_up::{
    ArrangeProspectPhrases, CreateProspectGenre, DropProspectGenre, FollowUpCard, KeepGenreWords,
    MarkFollowUpSent, MomentDraft, PrepareFollowUp, ProspectPhrase, SaveGenreLetter,
    SetDossierGenre, SnoozeFollowUp, events_for, follow_up_sender, phrases_for_genre,
    prospect_genres, prospect_phrases,
};
use griffe_core::mail::{
    ArmOutbound, CancelOutbound, LetterChrome, ResolveUncertain, RetryOutbound, UNDO_SECS,
};
use griffe_core::people::{PersonKey, person, resolve_person};
use griffe_core::prospection::{
    CreateOpportunity, CreateProspect, EstimationLineInput, LogInteraction, LoseOpportunity,
    ReopenOpportunity, SetEstimation, WinOpportunity, estimation_lines, opportunity_by_id,
};
use griffe_core::reference::RefMatch;
use griffe_core::work_kinds::{
    CreateWorkKind, DeleteWorkKind, RenameWorkKind, SetDossierWorkKinds, work_kinds,
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

fn hx_target_id(headers: &HeaderMap) -> &str {
    headers
        .get("hx-target")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
}

fn page(headers: &HeaderMap, content: Markup) -> Html<String> {
    let content = if hx_target_id(headers) == "affaire" {
        gens::panel_chrome(content)
    } else {
        content
    };
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

fn saved_page(headers: &HeaderMap, content: Markup) -> Response {
    let mut response = page(headers, content).into_response();
    response
        .headers_mut()
        .insert("HX-Trigger", HeaderValue::from_static("griffe:saved"));
    response
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
    let htmx = is_htmx(&headers);
    let content = state
        .with_store(|store| {
            let dossier = match resolve_person(store.connection(), &reference) {
                Ok(RefMatch::NotFound) => gens::not_found(&reference, today),
                Ok(RefMatch::Ambiguous(choices)) => gens::several(&reference, &choices),
                Ok(RefMatch::Unique(_)) => gens::dossier_page(store, &reference, today)
                    .unwrap_or_else(|e| {
                        if e.to_string().contains("introuvable") {
                            gens::not_found(&reference, today)
                        } else {
                            html! { div class="empty-state" { (e.to_string()) } }
                        }
                    }),
                Err(e) => html! { div class="empty-state" { (e.to_string()) } },
            };
            if htmx {
                dossier
            } else {
                gens::render_with_panel(store, today, dossier).unwrap_or_else(|err| {
                    html! { div class="empty-state" { (err.to_string()) } }
                })
            }
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

#[derive(Debug, Deserialize, Default)]
pub struct RelireForm {
    #[serde(default)]
    subject_line: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    genre_name: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct PhrasesQuery {
    #[serde(default)]
    depuis: String,
    #[serde(default)]
    pour: String,
    #[serde(default)]
    genre: String,
}

#[derive(Debug, Deserialize, Default)]
pub struct NewGenreQuery {
    #[serde(default)]
    depuis: String,
    #[serde(default)]
    pour: String,
    #[serde(default)]
    source: String,
}

#[derive(Debug, Deserialize, Default)]
pub struct NewGenreForm {
    #[serde(default)]
    name: String,
}

#[derive(Debug, Deserialize, Default)]
pub struct KeepForm {
    #[serde(default)]
    subject_line: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    confirmer: String,
    #[serde(default)]
    genre_name: String,
}

pub async fn phrases_get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<PhrasesQuery>,
) -> Html<String> {
    let today = state.today();
    let content = state
        .with_store(|store| phrases_markup(store, &query, today, None, None, None, None))
        .await
        .unwrap_or_else(|| html! { div class="empty-state" { "coffre verrouillé" } });
    page(&headers, content)
}

pub async fn phrases_post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<PhrasesQuery>,
    Form(fields): Form<HashMap<String, String>>,
) -> Response {
    let genre_id = query.genre.trim().to_string();
    let dropping = fields.contains_key("retirer_genre") && !genre_id.is_empty();
    let result = state
        .with_store_mut(|store| -> Result<bool, AppError> {
            if dropping {
                Executor::new(store).execute(
                    &DropProspectGenre {
                        id: genre_id.clone(),
                    },
                    &AppState::human_ctx(),
                )?;
                return Ok(true);
            }
            let phrases = if genre_id.is_empty() {
                prospect_phrases(store.connection())?
            } else {
                phrases_for_genre(store.connection(), &genre_id)?
            };
            let moments = drafts_from_form(&phrases, &fields).map_err(AppError::Domain)?;
            if genre_id.is_empty() {
                Executor::new(store)
                    .execute(&ArrangeProspectPhrases { moments }, &AppState::human_ctx())?;
            } else {
                Executor::new(store).execute(
                    &SaveGenreLetter {
                        genre_id: genre_id.clone(),
                        moments,
                    },
                    &AppState::human_ctx(),
                )?;
            }
            Ok(false)
        })
        .await;
    let today = state.today();
    match result {
        None => locked(&headers).into_response(),
        Some(Err(error)) => {
            let content = state
                .with_store(|store| {
                    phrases_markup(
                        store,
                        &query,
                        today,
                        Some(error.to_string()),
                        None,
                        Some(&fields),
                        Some(&fields),
                    )
                })
                .await
                .unwrap_or_else(|| html! { div class="empty-state" { "coffre verrouillé" } });
            page(&headers, content).into_response()
        }
        Some(Ok(true)) => {
            let mut shown = query.clone();
            shown.genre.clear();
            let content = state
                .with_store(|store| phrases_markup(store, &shown, today, None, None, None, None))
                .await
                .unwrap_or_else(|| html! { div class="empty-state" { "coffre verrouillé" } });
            with_push(
                &headers,
                &gens::phrases_href(&shown.depuis, &shown.pour, ""),
                content,
            )
        }
        Some(Ok(false)) => {
            let content = state
                .with_store(|store| {
                    phrases_markup(
                        store,
                        &query,
                        today,
                        None,
                        Some("Les phrases sont enregistrées.".to_string()),
                        None,
                        Some(&fields),
                    )
                })
                .await
                .unwrap_or_else(|| html! { div class="empty-state" { "coffre verrouillé" } });
            page(&headers, content).into_response()
        }
    }
}

/// Aperçu de la feuille. N'écrit rien, ne pousse pas l'URL, ne signale pas une sauvegarde.
pub async fn phrases_preview(
    State(state): State<AppState>,
    Query(query): Query<PhrasesQuery>,
    Form(fields): Form<HashMap<String, String>>,
) -> Html<String> {
    let today = state.today();
    let content = state
        .with_store(|store| phrases_sheet(store, &query, today, &fields))
        .await
        .unwrap_or_else(|| html! { p class="phrase-caption" { "coffre verrouillé" } });
    Html(content.into_string())
}

pub async fn genre_new_get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<NewGenreQuery>,
) -> Html<String> {
    let content = state
        .with_store(|store| {
            let (back_href, back_label) = genre_back(store, &query);
            gens::new_genre_page(
                &back_href,
                &back_label,
                &gens::new_genre_href(&query.depuis, &query.pour, &query.source),
                "",
                None,
            )
        })
        .await
        .unwrap_or_else(|| html! { div class="empty-state" { "coffre verrouillé" } });
    page(&headers, content)
}

pub async fn genre_new_post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<NewGenreQuery>,
    Form(form): Form<NewGenreForm>,
) -> Response {
    let source = query.source.trim().to_string();
    let result = state
        .with_store_mut(|store| {
            Executor::new(store).execute(
                &CreateProspectGenre {
                    name: form.name.clone(),
                    copy_from: if source.is_empty() {
                        None
                    } else {
                        Some(source.clone())
                    },
                },
                &AppState::human_ctx(),
            )
        })
        .await;
    let today = state.today();
    match result {
        None => locked(&headers).into_response(),
        Some(Err(error)) => {
            let content = state
                .with_store(|store| {
                    let (back_href, back_label) = genre_back(store, &query);
                    gens::new_genre_page(
                        &back_href,
                        &back_label,
                        &gens::new_genre_href(&query.depuis, &query.pour, &query.source),
                        &form.name,
                        Some(&error.to_string()),
                    )
                })
                .await
                .unwrap_or_else(|| html! { div class="empty-state" { "coffre verrouillé" } });
            page(&headers, content).into_response()
        }
        Some(Ok(outcome)) => {
            let Outcome::Applied(genre) = outcome else {
                return page(
                    &headers,
                    html! { div class="empty-state" { "le genre n'a pas été créé" } },
                )
                .into_response();
            };
            let shown = PhrasesQuery {
                depuis: query.depuis.clone(),
                pour: query.pour.clone(),
                genre: genre.id.clone(),
            };
            let content = state
                .with_store(|store| phrases_markup(store, &shown, today, None, None, None, None))
                .await
                .unwrap_or_else(|| html! { div class="empty-state" { "coffre verrouillé" } });
            with_push(
                &headers,
                &gens::phrases_href(&shown.depuis, &shown.pour, &shown.genre),
                content,
            )
        }
    }
}

fn genre_back(store: &griffe_core::store::Store, query: &NewGenreQuery) -> (String, String) {
    let source = query.source.trim();
    let href = gens::phrases_href(query.depuis.trim(), query.pour.trim(), source);
    let label = if source.is_empty() {
        "Les phrases".to_string()
    } else {
        prospect_genres(store.connection())
            .ok()
            .and_then(|genres| {
                genres
                    .into_iter()
                    .find(|genre| genre.id == source)
                    .map(|genre| genre.name)
            })
            .unwrap_or_else(|| "Les phrases".to_string())
    };
    (href, label)
}

const NEW_MOMENT_BODY: &str = griffe_core::domain::NEW_MOMENT_BODY;

/// `order` absent : l'ordre du coffre. `ecart_` absent ou vide : l'écart déjà noté.
/// Un enregistrement sans ces champs ne change donc pas la structure.
fn drafts_from_form(
    phrases: &[ProspectPhrase],
    fields: &HashMap<String, String>,
) -> Result<Vec<MomentDraft>, String> {
    let by_key: HashMap<&str, &ProspectPhrase> = phrases
        .iter()
        .map(|phrase| (phrase.key.as_str(), phrase))
        .collect();
    let mut keys = ordered_keys(phrases, fields);
    let structural =
        fields.contains_key("add") || fields.contains_key("drop") || fields.contains_key("move");
    if let Some(key) = fields.get("drop") {
        if !by_key.contains_key(key.as_str()) {
            return Err("Ce moment n'existe pas.".to_string());
        }
        keys.retain(|candidate| candidate != key);
    } else if let Some(spec) = fields.get("move") {
        apply_move(&mut keys, spec)?;
    }
    let mut drafts = Vec::with_capacity(keys.len() + usize::from(fields.contains_key("add")));
    for (index, key) in keys.iter().enumerate() {
        let Some(phrase) = by_key.get(key.as_str()) else {
            return Err("Ce moment n'existe pas.".to_string());
        };
        let mut draft = draft_from_posted(phrase, fields)?;
        if structural && index == 0 {
            draft.offset_days = 0;
        }
        drafts.push(draft);
    }
    if fields.contains_key("add") {
        drafts.push(new_moment(drafts.is_empty()));
    }
    Ok(drafts)
}

fn ordered_keys(phrases: &[ProspectPhrase], fields: &HashMap<String, String>) -> Vec<String> {
    match fields
        .get("order")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        Some(order) => order
            .split(',')
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .map(ToString::to_string)
            .collect(),
        None => phrases.iter().map(|phrase| phrase.key.clone()).collect(),
    }
}

fn apply_move(keys: &mut [String], spec: &str) -> Result<(), String> {
    let Some((key, direction)) = spec.split_once(':') else {
        return Err("Ce moment n'existe pas.".to_string());
    };
    let Some(index) = keys.iter().position(|candidate| candidate == key) else {
        return Err("Ce moment n'existe pas.".to_string());
    };
    match direction {
        "earlier" if index > 0 => keys.swap(index, index - 1),
        "later" if index + 1 < keys.len() => keys.swap(index, index + 1),
        "earlier" | "later" => {}
        _ => return Err("Monte ou descends le moment.".to_string()),
    }
    Ok(())
}

fn draft_from_posted(
    phrase: &ProspectPhrase,
    fields: &HashMap<String, String>,
) -> Result<MomentDraft, String> {
    let label = field_or(fields, &format!("label_{}", phrase.key), &phrase.label);
    let subject = field_or(
        fields,
        &format!("subject_{}", phrase.key),
        &phrase_to_editor(&phrase.subject),
    );
    let body = field_or(
        fields,
        &format!("body_{}", phrase.key),
        &phrase_to_editor(&phrase.body),
    );
    let revision = fields
        .get(&format!("revision_{}", phrase.key))
        .and_then(|value| value.parse().ok())
        .or(Some(phrase.revision));
    Ok(MomentDraft {
        key: Some(phrase.key.clone()),
        label,
        offset_days: posted_gap(phrase, fields)?,
        subject,
        body,
        revision,
    })
}

fn field_or(fields: &HashMap<String, String>, name: &str, fallback: &str) -> String {
    fields
        .get(name)
        .cloned()
        .unwrap_or_else(|| fallback.to_string())
}

fn posted_gap(phrase: &ProspectPhrase, fields: &HashMap<String, String>) -> Result<i64, String> {
    match fields.get(&format!("ecart_{}", phrase.key)) {
        None => Ok(phrase.offset_days),
        Some(raw) if raw.trim().is_empty() => Ok(phrase.offset_days),
        Some(raw) => raw
            .trim()
            .parse()
            .map_err(|_| "L'écart se compte en jours.".to_string()),
    }
}

fn new_moment(first: bool) -> MomentDraft {
    MomentDraft {
        key: None,
        label: "Nouveau moment".to_string(),
        offset_days: if first { 0 } else { 7 },
        subject: "{{sujet}}".to_string(),
        body: NEW_MOMENT_BODY.to_string(),
        revision: None,
    }
}

const PHRASES_LEDE: &str =
    "Changer ici ne touche pas les lettres déjà classées. Griffe n'envoie pas.";
const GENRE_LEDE: &str = "Les phrases que tu répètes à ce genre de gens. Changer ici ne touche pas les lettres déjà classées. Griffe n'envoie pas.";
const ENSEIGNE_CAPTION: &str = "Pas de prénom pour cette enseigne. La lettre dira Bonjour,. Le prénom se prend dans Qui répond.";

fn phrases_markup(
    store: &griffe_core::store::Store,
    query: &PhrasesQuery,
    today: time::Date,
    banner: Option<String>,
    status: Option<String>,
    fields: Option<&HashMap<String, String>>,
    navigation: Option<&HashMap<String, String>>,
) -> Markup {
    gens::phrases_page(&assemble_phrases(
        store, query, today, banner, status, fields, navigation,
    ))
}

fn phrases_sheet(
    store: &griffe_core::store::Store,
    query: &PhrasesQuery,
    today: time::Date,
    fields: &HashMap<String, String>,
) -> Markup {
    gens::phrase_sheet_inner(&assemble_phrases(
        store,
        query,
        today,
        None,
        None,
        Some(fields),
        Some(fields),
    ))
}

fn assemble_phrases(
    store: &griffe_core::store::Store,
    query: &PhrasesQuery,
    today: time::Date,
    mut banner: Option<String>,
    status: Option<String>,
    fields: Option<&HashMap<String, String>>,
    navigation: Option<&HashMap<String, String>>,
) -> gens::PhrasesView {
    let genre_id = query.genre.trim();
    let genres = prospect_genres(store.connection()).unwrap_or_default();
    let selected = genres.iter().find(|genre| genre.id == genre_id);
    let (phrases, current) = if let Some(genre) = selected {
        match phrases_for_genre(store.connection(), &genre.id) {
            Ok(phrases) => (phrases, Some(genre)),
            Err(error) => {
                if banner.is_none() {
                    banner = Some(error.to_string());
                }
                (
                    prospect_phrases(store.connection()).unwrap_or_default(),
                    None,
                )
            }
        }
    } else {
        if !genre_id.is_empty() && banner.is_none() {
            banner = Some("Ce genre n'existe pas.".to_string());
        }
        (
            prospect_phrases(store.connection()).unwrap_or_default(),
            None,
        )
    };
    let shown = current.map(|genre| genre.id.as_str()).unwrap_or("");
    let reading = phrase_reading(store, query, today, shown);
    let (title, lede) = if let Some(genre) = current {
        (format!("{}.", genre.name), GENRE_LEDE.to_string())
    } else {
        ("Les phrases.".to_string(), PHRASES_LEDE.to_string())
    };
    let links = std::iter::once(gens::PhraseLink {
        label: "Les phrases".to_string(),
        href: gens::phrases_href(query.depuis.trim(), query.pour.trim(), ""),
        current: current.is_none(),
    })
    .chain(genres.iter().map(|genre| gens::PhraseLink {
        label: genre.name.clone(),
        href: gens::phrases_href(query.depuis.trim(), query.pour.trim(), &genre.id),
        current: current.is_some_and(|selected| selected.id == genre.id),
    }))
    .collect();
    let alone = phrases.len() == 1;
    let open_key = resolve_open(&phrases, navigation);
    let moments: Vec<gens::PhraseMoment> = phrases
        .iter()
        .enumerate()
        .map(|(index, phrase)| {
            phrase_moment(
                phrase,
                &reading.ctx,
                fields,
                index + 1 == phrases.len(),
                alone,
                phrase.key == open_key,
            )
        })
        .collect();
    let chronicle = chronicle_of(&moments);
    gens::PhrasesView {
        back_href: reading.back_href,
        back_label: reading.back_label,
        action: reading.action,
        title,
        lede,
        links,
        other_href: gens::new_genre_href(query.depuis.trim(), query.pour.trim(), shown),
        drop: current.is_some(),
        moments,
        chronicle,
        read_caption: reading.caption,
        from_name: reading.from_name,
        chrome: reading.chrome,
        banner,
        status,
    }
}

fn chronicle_of(moments: &[gens::PhraseMoment]) -> String {
    let owned: Vec<(String, i64)> = moments
        .iter()
        .map(|moment| (moment.label.clone(), moment.gap.parse().unwrap_or(0)))
        .collect();
    let steps: Vec<(&str, i64)> = owned
        .iter()
        .map(|(label, days)| (label.as_str(), *days))
        .collect();
    chronicle(&steps)
}

fn resolve_open(
    phrases: &[ProspectPhrase],
    navigation: Option<&HashMap<String, String>>,
) -> String {
    let first = phrases
        .first()
        .map(|phrase| phrase.key.clone())
        .unwrap_or_default();
    let Some(navigation) = navigation else {
        return first;
    };
    if navigation.contains_key("add") {
        return phrases
            .last()
            .map(|phrase| phrase.key.clone())
            .unwrap_or(first);
    }
    if let Some(dropped) = navigation.get("drop")
        && let Some(neighbor) = neighbor_after_drop(navigation, dropped, phrases)
    {
        return neighbor;
    }
    if let Some((key, _)) = navigation.get("move").and_then(|spec| spec.split_once(':'))
        && phrases.iter().any(|phrase| phrase.key == key)
    {
        return key.to_string();
    }
    if let Some(key) = navigation
        .get("ouvert")
        .map(String::as_str)
        .map(str::trim)
        .filter(|key| !key.is_empty())
        && phrases.iter().any(|phrase| phrase.key == key)
    {
        return key.to_string();
    }
    first
}

fn neighbor_after_drop(
    navigation: &HashMap<String, String>,
    dropped: &str,
    phrases: &[ProspectPhrase],
) -> Option<String> {
    let keys: Vec<&str> = navigation
        .get("order")?
        .split(',')
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .collect();
    let index = keys.iter().position(|key| *key == dropped)?;
    let candidate = if index + 1 < keys.len() {
        keys[index + 1]
    } else if index > 0 {
        keys[index - 1]
    } else {
        return None;
    };
    phrases
        .iter()
        .any(|phrase| phrase.key == candidate)
        .then(|| candidate.to_string())
}

struct PhraseReading {
    caption: String,
    ctx: TemplateContext,
    from_name: String,
    chrome: LetterChrome,
    back_href: String,
    back_label: String,
    action: String,
}

fn phrase_reading(
    store: &griffe_core::store::Store,
    query: &PhrasesQuery,
    today: time::Date,
    genre: &str,
) -> PhraseReading {
    let (moi, societe) = speaker(store);
    let account = griffe_core::mail::profile(store.connection()).unwrap_or_default();
    let raw_signature = account.signature.clone();
    let from_name = account.from_name.clone();
    let chrome = LetterChrome::from_profile(&account);
    let pour = query.pour.trim();
    let depuis = query.depuis.trim();
    let action = gens::phrases_href(depuis, pour, genre);
    let example = PhraseReading {
        caption: "Exemple, pour voir.".to_string(),
        from_name: from_name.clone(),
        chrome: chrome.clone(),
        ctx: TemplateContext {
            prenom: "Camille".into(),
            sujet: "la refonte".into(),
            montant: "4 500 €".into(),
            moi: moi.clone(),
            societe: societe.clone(),
            signature: raw_signature.clone(),
            ..TemplateContext::default()
        },
        back_href: "/societe/identite".to_string(),
        back_label: "L'identité".to_string(),
        action: action.clone(),
    };
    if pour.is_empty() {
        return example;
    }
    let Ok(dossier) = person(store.connection(), pour, today) else {
        return example;
    };
    let prenom = given_name(
        dossier.contact_name.as_deref().unwrap_or(""),
        dossier.party.as_str(),
    );
    let href = gens::person_href(&dossier.name);
    let (back_href, back_label) = if depuis == "lettre" {
        (format!("{href}/ecrire"), dossier.name.clone())
    } else if depuis == "fiche" {
        (format!("{href}/fiche"), dossier.name.clone())
    } else {
        (href, dossier.name.clone())
    };
    let caption = if prenom.is_empty() {
        ENSEIGNE_CAPTION.to_string()
    } else {
        format!("{prenom} lira")
    };
    PhraseReading {
        caption,
        from_name,
        chrome,
        ctx: TemplateContext {
            prenom,
            sujet: dossier
                .current
                .opportunity_name
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| "le sujet".to_string()),
            montant: dossier
                .current
                .amount
                .map(|amount| amount.to_string())
                .unwrap_or_else(|| "—".to_string()),
            moi,
            societe,
            signature: raw_signature,
            ..TemplateContext::default()
        },
        back_href,
        back_label,
        action,
    }
}

fn speaker(store: &griffe_core::store::Store) -> (String, String) {
    let profile = company_profile(store.connection()).ok().flatten();
    let sender = follow_up_sender(store.connection()).ok();
    let moi = sender
        .and_then(|settings| settings.sender_name)
        .or_else(|| profile.as_ref().and_then(|p| p.president_name.clone()))
        .or_else(|| profile.as_ref().map(|p| p.name.clone()))
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "moi".to_string());
    let societe = profile
        .map(|p| p.name)
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "la société".to_string());
    (moi, societe)
}

fn phrase_moment(
    phrase: &ProspectPhrase,
    ctx: &TemplateContext,
    fields: Option<&HashMap<String, String>>,
    last: bool,
    alone: bool,
    open: bool,
) -> gens::PhraseMoment {
    let (label, subject, body, gap) = if let Some(fields) = fields {
        (
            field_or(fields, &format!("label_{}", phrase.key), &phrase.label),
            field_or(
                fields,
                &format!("subject_{}", phrase.key),
                &phrase_to_editor(&phrase.subject),
            ),
            field_or(
                fields,
                &format!("body_{}", phrase.key),
                &phrase_to_editor(&phrase.body),
            ),
            match fields.get(&format!("ecart_{}", phrase.key)) {
                Some(raw) if !raw.trim().is_empty() => raw.clone(),
                _ => phrase.offset_days.to_string(),
            },
        )
    } else {
        (
            phrase.label.clone(),
            phrase_to_editor(&phrase.subject),
            phrase_to_editor(&phrase.body),
            phrase.offset_days.to_string(),
        )
    };
    gens::PhraseMoment {
        key: phrase.key.clone(),
        reads_subject: render_template(&phrase_from_editor(&subject), ctx),
        reads: render_template(&phrase_from_editor(&body), ctx),
        label,
        subject,
        body,
        revision: phrase.revision,
        gap,
        first: phrase.position == 0,
        last,
        alone,
        open,
    }
}

fn optional_letter_field(value: String) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value)
    }
}

pub async fn relire(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(reference): Path<String>,
    Form(form): Form<RelireForm>,
) -> Html<String> {
    let today = state.today();
    let Some(Ok(dossier)) = load_dossier(&state, &reference).await else {
        return page(&headers, gens::not_found(&reference, today));
    };
    let Some(_) = dossier.follow_up_subject else {
        return page(
            &headers,
            gens::dossier_markup(&dossier, today, Some("rien à écrire pour l'instant")),
        );
    };
    let keep = gens::KeepUi {
        reading: true,
        subject: Some(form.subject_line),
        body: Some(form.body),
        proposed_name: form.genre_name,
        ..gens::KeepUi::default()
    };
    page(
        &headers,
        render_letter(&state, &dossier, today, None, &keep).await,
    )
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
            gens::letter_page(
                store,
                &dossier,
                card.as_ref(),
                today,
                None,
                &gens::KeepUi::default(),
            )
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
                        &gens::KeepUi::default(),
                    )
                    .unwrap_or_else(|err| html! { div class="empty-state" { (err.to_string()) } })
                })
                .await
                .unwrap_or_else(|| html! { div class="empty-state" { "coffre verrouillé" } });
            page(&headers, content)
        }
    }
}

pub async fn keep_words(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(reference): Path<String>,
    Form(form): Form<KeepForm>,
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
    let FollowUpSubject::Opportunity(_) = subject else {
        return page(
            &headers,
            gens::dossier_markup(
                &dossier,
                today,
                Some("Le genre est celui d'une conversation."),
            ),
        );
    };
    let confirming = form.confirmer.trim() == "1";
    let proposed = form.genre_name.trim().to_string();
    let has_genre = state
        .with_store(|store| {
            let FollowUpSubject::Opportunity(id) = subject else {
                return false;
            };
            griffe_core::follow_up::prospect_genre_for(store.connection(), id)
                .ok()
                .flatten()
                .is_some()
        })
        .await
        .unwrap_or(false);
    let written = gens::KeepUi {
        subject: Some(form.subject_line.clone()),
        body: Some(form.body.clone()),
        ..gens::KeepUi::default()
    };
    if !confirming {
        let keep = if has_genre {
            gens::KeepUi {
                confirm: true,
                ..written
            }
        } else if proposed.is_empty() {
            gens::KeepUi {
                ask: true,
                ..written
            }
        } else {
            gens::KeepUi {
                confirm: true,
                proposed_name: proposed,
                ..written
            }
        };
        return page(
            &headers,
            render_letter(&state, &dossier, today, None, &keep).await,
        );
    }
    if !has_genre && proposed.is_empty() {
        return page(
            &headers,
            render_letter(
                &state,
                &dossier,
                today,
                None,
                &gens::KeepUi {
                    ask: true,
                    ..written
                },
            )
            .await,
        );
    }
    let result = state
        .with_store_mut(|store| {
            Executor::new(store).execute(
                &KeepGenreWords {
                    subject,
                    today,
                    subject_line: Some(form.subject_line.clone()),
                    body: Some(form.body.clone()),
                    genre_name: if has_genre {
                        None
                    } else {
                        Some(proposed.clone())
                    },
                },
                &AppState::human_ctx(),
            )
        })
        .await;
    match result {
        None => locked(&headers),
        Some(Err(error)) => page(
            &headers,
            render_letter(
                &state,
                &dossier,
                today,
                Some(&error.to_string()),
                &gens::KeepUi::default(),
            )
            .await,
        ),
        Some(Ok(_)) => {
            let fresh = load_dossier(&state, &reference)
                .await
                .and_then(Result::ok)
                .unwrap_or(dossier);
            page(
                &headers,
                render_letter(
                    &state,
                    &fresh,
                    today,
                    Some("Ces mots sont gardés. Rien n'est classé."),
                    &gens::KeepUi::default(),
                )
                .await,
            )
        }
    }
}

async fn render_letter(
    state: &AppState,
    dossier: &griffe_core::people::PersonDossier,
    today: time::Date,
    flash: Option<&str>,
    keep: &gens::KeepUi,
) -> Markup {
    let Some(subject) = dossier.follow_up_subject else {
        return gens::dossier_markup(dossier, today, flash);
    };
    state
        .with_store(|store| {
            let card = gens::load_card(store, subject, today).ok().flatten();
            gens::letter_page(store, dossier, card.as_ref(), today, flash, keep)
                .unwrap_or_else(|err| html! { div class="empty-state" { (err.to_string()) } })
        })
        .await
        .unwrap_or_else(|| html! { div class="empty-state" { "coffre verrouillé" } })
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

#[derive(Debug, Deserialize)]
pub struct LetterPost {
    #[serde(default)]
    id: String,
    #[serde(default)]
    sent: String,
}

pub async fn send_letter(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(reference): Path<String>,
    Form(form): Form<LetterForm>,
) -> Response {
    let today = state.today();
    let Some(Ok(dossier)) = load_dossier(&state, &reference).await else {
        return page(&headers, gens::not_found(&reference, today)).into_response();
    };
    let name = dossier.name.clone();
    let subject_line = form.subject_line.trim().to_string();
    let body = form.body.trim().to_string();
    if subject_line.is_empty() || body.is_empty() {
        let markup = depart_for(
            &state,
            &name,
            Some("La lettre n'a pas de sujet, ou pas de texte."),
        )
        .await;
        return mail_fragment(&headers, markup, false);
    }
    let token = state.mail_session().to_string();
    let subject = dossier.follow_up_subject;
    let result = state
        .with_store_mut(|store| {
            let client_id = match dossier.key {
                griffe_core::people::PersonKey::Client { id } => Some(id.to_string()),
                griffe_core::people::PersonKey::Supplier { .. } => None,
            };
            arm_letter(
                store,
                subject,
                &name,
                &subject_line,
                &body,
                &token,
                today,
                client_id,
            )
        })
        .await;
    match result {
        None => locked(&headers).into_response(),
        Some(Err(error)) => {
            let note = mail_notice(&error);
            let markup = depart_for(&state, &name, Some(&note)).await;
            mail_fragment(&headers, markup, false)
        }
        Some(Ok(())) => {
            let markup = depart_for(&state, &name, None).await;
            mail_fragment(&headers, markup, true)
        }
    }
}

pub async fn send_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(reference): Path<String>,
) -> Response {
    let today = state.today();
    let Some(Ok(dossier)) = load_dossier(&state, &reference).await else {
        return page(&headers, gens::not_found(&reference, today)).into_response();
    };
    let (markup, polling) = depart_poll_for(&state, &dossier.name).await;
    mail_status(&headers, markup, polling)
}

pub async fn send_cancel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(reference): Path<String>,
    Form(form): Form<LetterPost>,
) -> Response {
    gesture(&state, &headers, &reference, &form.id, |store, id| {
        Executor::new(store)
            .execute(
                &CancelOutbound { id: id.to_string() },
                &AppState::human_ctx(),
            )
            .map(|_| ())
    })
    .await
}

pub async fn send_retry(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(reference): Path<String>,
    Form(form): Form<LetterPost>,
) -> Response {
    let token = state.mail_session().to_string();
    gesture(&state, &headers, &reference, &form.id, move |store, id| {
        Executor::new(store)
            .execute(
                &RetryOutbound {
                    id: id.to_string(),
                    session_token: Some(token),
                },
                &AppState::human_ctx(),
            )
            .map(|_| ())
    })
    .await
}

pub async fn send_decision(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(reference): Path<String>,
    Form(form): Form<LetterPost>,
) -> Response {
    let sent = form.sent == "1";
    let today = state.today();
    gesture(&state, &headers, &reference, &form.id, move |store, id| {
        Executor::new(store)
            .execute(
                &ResolveUncertain {
                    id: id.to_string(),
                    sent,
                    today,
                },
                &AppState::human_ctx(),
            )
            .map(|_| ())
    })
    .await
}

async fn gesture(
    state: &AppState,
    headers: &HeaderMap,
    reference: &str,
    id: &str,
    apply: impl FnOnce(&mut griffe_core::store::Store, &str) -> Result<(), AppError>,
) -> Response {
    let today = state.today();
    let Some(Ok(dossier)) = load_dossier(state, reference).await else {
        return page(headers, gens::not_found(reference, today)).into_response();
    };
    if id.trim().is_empty() {
        let markup = depart_for(state, &dossier.name, Some("Cette lettre est introuvable.")).await;
        return mail_fragment(headers, markup, false);
    }
    let name = dossier.name.clone();
    let result = state.with_store_mut(|store| apply(store, id)).await;
    match result {
        None => locked(headers).into_response(),
        Some(Err(error)) => {
            let note = mail_notice(&error);
            let markup = depart_for(state, &name, Some(&note)).await;
            mail_fragment(headers, markup, false)
        }
        Some(Ok(())) => {
            let markup = depart_for(state, &name, None).await;
            mail_fragment(headers, markup, true)
        }
    }
}

#[allow(clippy::too_many_arguments)] // lettre, jeton, jour, et la fiche qui la reçoit
fn arm_letter(
    store: &mut griffe_core::store::Store,
    subject: Option<FollowUpSubject>,
    name: &str,
    subject_line: &str,
    body: &str,
    token: &str,
    today: time::Date,
    client_id: Option<String>,
) -> Result<(), AppError> {
    let profile = griffe_core::mail::profile(store.connection())?;
    if !profile.ready {
        return Err(AppError::Domain(
            "Le courrier n'est pas encore branché.".into(),
        ));
    }
    let card = subject.and_then(|item| gens::load_card(store, item, today).ok().flatten());
    let Some(to) = card
        .as_ref()
        .and_then(|item| item.contact_email.clone())
        .and_then(|value| griffe_core::domain::parse_email(&value).ok())
    else {
        return Err(AppError::Domain(griffe_core::mail::MISSING_ADDRESS.into()));
    };
    let kind = match subject {
        Some(FollowUpSubject::Invoice(_)) => "invoice",
        Some(FollowUpSubject::Opportunity(_)) => "opportunity",
        None => "letter",
    };
    let slot = follow_slot(store, subject, card.as_ref())?;
    Executor::new(store)
        .execute(
            &ArmOutbound {
                kind: kind.to_string(),
                anchor: Some(name.to_string()),
                to_address: to,
                subject: subject_line.to_string(),
                body: body.to_string(),
                delay_secs: UNDO_SECS,
                session_token: Some(token.to_string()),
                follow_subject: slot.subject,
                follow_subject_id: slot.subject_id,
                follow_cycle: slot.cycle,
                follow_step: slot.step,
                client_id,
            },
            &AppState::human_ctx(),
        )
        .map(|_| ())
}

struct FollowSlot {
    subject: Option<String>,
    subject_id: Option<String>,
    cycle: Option<String>,
    step: Option<String>,
}

fn empty_slot() -> FollowSlot {
    FollowSlot {
        subject: None,
        subject_id: None,
        cycle: None,
        step: None,
    }
}

fn follow_slot(
    store: &griffe_core::store::Store,
    subject: Option<FollowUpSubject>,
    card: Option<&FollowUpCard>,
) -> Result<FollowSlot, AppError> {
    let (Some(subject), Some(step)) = (subject, card.and_then(|item| item.step_key.clone())) else {
        return Ok(empty_slot());
    };
    let kind = match subject {
        FollowUpSubject::Opportunity(_) => "opportunity",
        FollowUpSubject::Invoice(_) => "invoice",
    };
    let id = match subject {
        FollowUpSubject::Opportunity(id) => id.to_string(),
        FollowUpSubject::Invoice(id) => id.to_string(),
    };
    let events = events_for(store.connection(), subject)?;
    Ok(FollowSlot {
        subject: Some(kind.to_string()),
        subject_id: Some(id),
        cycle: Some(entered_cycle_key(&events)),
        step: Some(step),
    })
}

fn fiche_courriel(
    store: &griffe_core::store::Store,
    name: &str,
    today: time::Date,
) -> Option<String> {
    let dossier = person(store.connection(), name, today).ok()?;
    let griffe_core::people::PersonKey::Client { id } = dossier.key else {
        return None;
    };
    let contact = griffe_core::clients::correspondent(store.connection(), id).ok()??;
    let email = contact.email.as_deref()?;
    griffe_core::domain::parse_email(email).ok()
}

async fn depart_for(state: &AppState, name: &str, note: Option<&str>) -> Markup {
    let href = person_href(name);
    let today = state.today();
    state
        .with_store(|store| {
            let view = griffe_core::mail::outbound_for_anchor(
                store.connection(),
                name,
                OffsetDateTime::now_utc(),
            )
            .ok()
            .flatten();
            let destination = fiche_courriel(store, name, today);
            gens::depart_markup(
                &href,
                view.as_ref(),
                note,
                "Envoyer",
                destination.as_deref(),
            )
        })
        .await
        .unwrap_or_else(|| html! { div id="depart" { "coffre verrouillé" } })
}

async fn depart_poll_for(state: &AppState, name: &str) -> (Markup, bool) {
    let href = person_href(name);
    let today = state.today();
    state
        .with_store(|store| {
            let view = griffe_core::mail::outbound_for_anchor(
                store.connection(),
                name,
                OffsetDateTime::now_utc(),
            )
            .ok()
            .flatten();
            let destination = fiche_courriel(store, name, today);
            gens::depart_poll_fragment(
                &href,
                view.as_ref(),
                None,
                "Envoyer",
                destination.as_deref(),
            )
        })
        .await
        .unwrap_or_else(|| (html! { div id="depart" { "coffre verrouillé" } }, false))
}

fn mail_status(headers: &HeaderMap, content: Markup, polling: bool) -> Response {
    let mut response = mail_fragment(headers, content, false);
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, no-cache, must-revalidate"),
    );
    if !polling {
        response
            .headers_mut()
            .insert("HX-Reswap", HeaderValue::from_static("outerHTML"));
    }
    response
}

fn mail_fragment(headers: &HeaderMap, content: Markup, saved: bool) -> Response {
    let mut response = if is_htmx(headers) {
        Html(content.into_string()).into_response()
    } else {
        page(headers, content).into_response()
    };
    if saved {
        response
            .headers_mut()
            .insert("HX-Trigger", HeaderValue::from_static("griffe:saved"));
    }
    response
}

fn mail_notice(error: &AppError) -> String {
    let text = error.to_string();
    if text.contains("n'est plus annulable") {
        return "Elle est déjà en route.".to_string();
    }
    if text.contains("UNIQUE") || text.contains("constraint failed") {
        return "Cette lettre est déjà engagée.".to_string();
    }
    text.strip_prefix("règle métier violée : ")
        .unwrap_or(&text)
        .to_string()
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

/// La ligne « un autre » gagne lorsqu'elle a un texte. Sinon, le mot coché.
/// `None` : « un autre » est coché et la ligne est vide — on ne retire pas le genre.
fn applied_genre_name(checked: &str, other: &str) -> Option<String> {
    let other = other.trim();
    if !other.is_empty() {
        return Some(other.to_string());
    }
    if checked.trim() == gens::GENRE_OTHER {
        return None;
    }
    Some(checked.trim().to_string())
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

async fn load_genre_field(
    state: &AppState,
    dossier: &griffe_core::people::PersonDossier,
    posted: Option<&gens::GenrePost>,
    created: bool,
) -> gens::GenreField {
    state
        .with_store(|store| gens::genre_field(store, dossier, posted, created))
        .await
        .and_then(Result::ok)
        .unwrap_or_default()
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
            let contact = griffe_core::clients::correspondent(store.connection(), id)?;
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
            let genre = load_genre_field(&state, &dossier, None, false).await;
            page(
                &headers,
                gens::fiche_page(
                    &dossier,
                    &values,
                    &gens::FicheErrors {
                        who: None,
                        address: None,
                        email: None,
                        banner: None,
                    },
                    &genre,
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
    #[serde(default)]
    genre: String,
    #[serde(default)]
    genre_other: String,
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
    let posted_genre = gens::GenrePost {
        checked: form.genre.clone(),
        other: form.genre_other.clone(),
    };
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
            let genre = load_genre_field(&state, &dossier, Some(&posted_genre), false).await;
            return page(
                &headers,
                gens::fiche_page(
                    &dossier,
                    &values,
                    &gens::FicheErrors {
                        who: None,
                        address: Some(msg),
                        email: None,
                        banner: None,
                    },
                    &genre,
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
        let genre = load_genre_field(&state, &dossier, Some(&posted_genre), false).await;
        return page(
            &headers,
            gens::fiche_page(
                &dossier,
                &values,
                &gens::FicheErrors {
                    who: Some("un nom, pour commencer".into()),
                    address: None,
                    email: None,
                    banner: None,
                },
                &genre,
            ),
        )
        .into_response();
    }
    if let Some(email) = blank(&form.email)
        && griffe_core::domain::parse_email(&email).is_err()
    {
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
        let genre = load_genre_field(&state, &dossier, Some(&posted_genre), false).await;
        return page(
            &headers,
            gens::fiche_page(
                &dossier,
                &values,
                &gens::FicheErrors {
                    who: None,
                    address: None,
                    email: Some("Cette adresse est illisible.".into()),
                    banner: None,
                },
                &genre,
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
    let Some(posted_genre_name) = applied_genre_name(&posted_genre.checked, &posted_genre.other)
    else {
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
        let genre = load_genre_field(&state, &dossier, Some(&posted_genre), false).await;
        return page(
            &headers,
            gens::fiche_page(
                &dossier,
                &values,
                &gens::FicheErrors {
                    who: None,
                    address: None,
                    email: None,
                    banner: None,
                },
                &genre,
            ),
        )
        .into_response();
    };
    let representative = blank(&form.representative);
    let email = blank(&form.email);
    let phone = blank(&form.phone);
    let contact_name = representative.clone().unwrap_or_else(|| who.clone());
    let posted_contact_revision = form.contact_revision.parse::<i64>().ok();
    let result = state
        .with_store_mut(|store| -> Result<bool, AppError> {
            let client = client_by_id(store.connection(), id)?
                .ok_or_else(|| AppError::from(griffe_core::clients::ClientError::NotFound(id)))?;
            let contact = griffe_core::clients::correspondent(store.connection(), id)?;
            let posted_id = form.contact_id.trim();
            let mismatch = match &contact {
                Some(current) => posted_id.is_empty() || posted_id != current.id.to_string(),
                None => !posted_id.is_empty(),
            };
            if mismatch {
                return Err(AppError::Domain("Cette fiche a changé, rechargez.".into()));
            }
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
            let Some(opportunity_id) = dossier.current.opportunity_id else {
                return Ok(false);
            };
            let outcome = Executor::new(store).execute(
                &SetDossierGenre {
                    subject: FollowUpSubject::Opportunity(opportunity_id),
                    name: posted_genre_name.clone(),
                },
                &AppState::human_ctx(),
            )?;
            Ok(matches!(outcome, Outcome::Applied(assignment) if assignment.created))
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
            let genre = load_genre_field(&state, &dossier, Some(&posted_genre), false).await;
            page(
                &headers,
                gens::fiche_page(
                    &dossier,
                    &values,
                    &gens::FicheErrors {
                        who: None,
                        address: None,
                        email: None,
                        banner: Some(e.to_string()),
                    },
                    &genre,
                ),
            )
            .into_response()
        }
        Some(Ok(true)) => {
            let rendered = state
                .with_store(|store| -> Result<_, AppError> {
                    let dossier = person(store.connection(), &id.to_string(), today)?;
                    let client = client_by_id(store.connection(), id)?.ok_or_else(|| {
                        AppError::from(griffe_core::clients::ClientError::NotFound(id))
                    })?;
                    let contact = griffe_core::clients::correspondent(store.connection(), id)?;
                    let values = fiche_values_from(&client, contact.as_ref());
                    let genre = gens::genre_field(store, &dossier, None, true)?;
                    let href = format!("{}/fiche", person_href(&dossier.name));
                    let markup = gens::fiche_page(
                        &dossier,
                        &values,
                        &gens::FicheErrors {
                            who: None,
                            address: None,
                            email: None,
                            banner: None,
                        },
                        &genre,
                    );
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
        Some(Ok(false)) => {
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
    let mut paired: Vec<(String, String)> =
        if form.label.is_empty() && !form.amount.trim().is_empty() {
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
    #[serde(default)]
    days: String,
}

fn posted_snooze_until(today: time::Date, days: &str, until: &str) -> Result<time::Date, String> {
    let days = days.trim();
    if !days.is_empty() {
        let n = days
            .parse::<i64>()
            .map_err(|_| SnoozeDaysError.to_string())?;
        return snooze_in_days(today, n).map_err(|err| err.to_string());
    }
    let until = until.trim();
    if until.is_empty() {
        return Err(SnoozeDaysError.to_string());
    }
    parse_date(until).map_err(|err| err.to_string())
}

pub async fn snooze(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(reference): Path<String>,
    Form(form): Form<SnoozeForm>,
) -> Response {
    let today = state.today();
    let Some(Ok(dossier)) = load_dossier(&state, &reference).await else {
        return page(&headers, gens::not_found(&reference, today)).into_response();
    };
    let Some(subject) = dossier.follow_up_subject else {
        return page(&headers, gens::dossier_markup(&dossier, today, None)).into_response();
    };
    let until = match posted_snooze_until(today, &form.days, &form.until) {
        Ok(date) => date,
        Err(msg) => {
            return page(&headers, gens::dossier_markup(&dossier, today, Some(&msg)))
                .into_response();
        }
    };
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
            saved_page(&headers, content)
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

#[derive(Debug, Deserialize)]
pub struct TravauxForm {
    #[serde(default)]
    body: String,
    #[serde(default)]
    revision: String,
    #[serde(default)]
    intent: String,
}

pub async fn travaux_get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(reference): Path<String>,
) -> Html<String> {
    travaux_show(&state, &headers, &reference, false).await
}

pub async fn travaux_edit_get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(reference): Path<String>,
) -> Html<String> {
    travaux_show(&state, &headers, &reference, true).await
}

async fn travaux_show(
    state: &AppState,
    headers: &HeaderMap,
    reference: &str,
    editing: bool,
) -> Html<String> {
    let today = state.today();
    let Some(Ok(dossier)) = load_dossier(state, reference).await else {
        return page(headers, gens::not_found(reference, today));
    };
    match load_travaux(state, &dossier).await {
        TravauxLoad::Locked => locked(headers),
        TravauxLoad::NotAFiche => page(
            headers,
            gens::dossier_markup(&dossier, today, Some("Les travaux tiennent sur une fiche.")),
        ),
        TravauxLoad::Failed(msg) => {
            page(headers, gens::dossier_markup(&dossier, today, Some(&msg)))
        }
        TravauxLoad::Ready(note) if editing => page(
            headers,
            gens::travaux_edit(&dossier, &note, &note.body, None, false),
        ),
        TravauxLoad::Ready(note) => page(headers, gens::travaux_page(&dossier, &note)),
    }
}

pub async fn travaux_post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(reference): Path<String>,
    Form(form): Form<TravauxForm>,
) -> Response {
    let today = state.today();
    let Some(Ok(dossier)) = load_dossier(&state, &reference).await else {
        return page(&headers, gens::not_found(&reference, today)).into_response();
    };
    let before = match load_travaux(&state, &dossier).await {
        TravauxLoad::Ready(note) => note,
        other => return travaux_closed(&headers, &dossier, today, other),
    };
    let PersonKey::Client { id } = dossier.key else {
        return travaux_closed(&headers, &dossier, today, TravauxLoad::NotAFiche);
    };
    // Une révision illisible ne bloque pas : on repart de celle du coffre, lue à l'instant.
    let revision = form.revision.parse::<i64>().unwrap_or(before.revision);
    let reread = form.intent == "relire";
    let cmd = SaveDossierWork {
        client: id,
        body: form.body.clone(),
        revision,
    };
    let result = state
        .with_store_mut(|store| Executor::new(store).execute(&cmd, &AppState::human_ctx()))
        .await;
    let read = format!("{}/travaux", gens::person_href(&dossier.name));
    match result {
        None => locked(&headers).into_response(),
        Some(Err(error)) => travaux_refused(
            &headers, &dossier, &before, &form.body, revision, reread, &error,
        ),
        Some(Ok(outcome)) => match outcome {
            Outcome::Applied(note) | Outcome::AlreadyApplied(note) => {
                let wrote = note.revision != before.revision;
                if reread {
                    travaux_ok(
                        &headers,
                        gens::travaux_page(&dossier, &note),
                        note.revision,
                        true,
                        Some(&read),
                    )
                } else {
                    travaux_ok(
                        &headers,
                        gens::travaux_kept(note.revision),
                        note.revision,
                        wrote,
                        None,
                    )
                }
            }
            Outcome::DryRun | Outcome::PendingConfirmation(_) => travaux_refused(
                &headers,
                &dossier,
                &before,
                &form.body,
                revision,
                reread,
                &AppError::Command("Rechargez la page.".into()),
            ),
        },
    }
}

fn travaux_closed(
    headers: &HeaderMap,
    dossier: &griffe_core::people::PersonDossier,
    today: time::Date,
    load: TravauxLoad,
) -> Response {
    match load {
        TravauxLoad::Locked | TravauxLoad::Ready(_) => locked(headers).into_response(),
        TravauxLoad::NotAFiche => page(
            headers,
            gens::dossier_markup(dossier, today, Some("Les travaux tiennent sur une fiche.")),
        )
        .into_response(),
        TravauxLoad::Failed(msg) => {
            page(headers, gens::dossier_markup(dossier, today, Some(&msg))).into_response()
        }
    }
}

fn travaux_refusal(error: &AppError) -> (String, bool, &'static str) {
    match error {
        AppError::Conflict { .. } => (
            "Ce récit a changé. Voici celui du coffre.".into(),
            true,
            "conflit",
        ),
        AppError::Domain(message) => (message.clone(), false, "refuse"),
        other => (other.to_string(), false, "refuse"),
    }
}

fn travaux_refused(
    headers: &HeaderMap,
    dossier: &griffe_core::people::PersonDossier,
    before: &DossierWork,
    body: &str,
    revision: i64,
    reread: bool,
    error: &AppError,
) -> Response {
    let (message, offer_vault, mark) = travaux_refusal(error);
    let shown = DossierWork {
        body: body.to_string(),
        revision,
    };
    let edit = format!("{}/travaux/ecrire", gens::person_href(&dossier.name));
    let markup = if reread {
        gens::travaux_edit(dossier, &shown, &before.body, Some(&message), offer_vault)
    } else {
        gens::travaux_notice(&message, offer_vault.then_some(edit.as_str()))
    };
    travaux_marked(headers, markup, mark)
}

fn travaux_ok(
    headers: &HeaderMap,
    body: Markup,
    revision: i64,
    trigger: bool,
    push: Option<&str>,
) -> Response {
    let mut response = travaux_marked(headers, body, "ok");
    if let Ok(value) = HeaderValue::from_str(&revision.to_string()) {
        response
            .headers_mut()
            .insert("X-Griffe-Travaux-Revision", value);
    }
    if trigger {
        response
            .headers_mut()
            .insert("HX-Trigger", HeaderValue::from_static("griffe:saved"));
    }
    if let Some(url) = push
        && let Ok(value) = HeaderValue::from_str(url)
    {
        response.headers_mut().insert("HX-Push-Url", value);
    }
    response
}

fn travaux_marked(headers: &HeaderMap, body: Markup, mark: &'static str) -> Response {
    let mut response = page(headers, body).into_response();
    response
        .headers_mut()
        .insert("X-Griffe-Travaux", HeaderValue::from_static(mark));
    response
}

enum TravauxLoad {
    Locked,
    NotAFiche,
    Failed(String),
    Ready(DossierWork),
}

async fn load_travaux(
    state: &AppState,
    dossier: &griffe_core::people::PersonDossier,
) -> TravauxLoad {
    let PersonKey::Client { id } = dossier.key else {
        return TravauxLoad::NotAFiche;
    };
    match state
        .with_store(|store| dossier_work(store.connection(), id))
        .await
    {
        None => TravauxLoad::Locked,
        Some(Err(error)) => TravauxLoad::Failed(error.to_string()),
        Some(Ok(note)) => TravauxLoad::Ready(note),
    }
}

#[derive(Debug, Deserialize, Default)]
struct DossierTypesForm {
    #[serde(default)]
    kind: Vec<String>,
    #[serde(default)]
    kind_other_on: String,
    #[serde(default)]
    kind_other: String,
}

#[derive(Debug, Deserialize, Default)]
pub(crate) struct TypeNameForm {
    #[serde(default)]
    name: String,
}

#[derive(Debug, Deserialize, Default)]
pub(crate) struct TypeGesteForm {
    #[serde(default)]
    name: String,
    #[serde(default)]
    geste: String,
}

fn dossier_types_form(pairs: &[(String, String)]) -> DossierTypesForm {
    let mut form = DossierTypesForm::default();
    for (key, value) in pairs {
        match key.as_str() {
            "kind" => form.kind.push(value.clone()),
            "kind_other_on" => form.kind_other_on.clone_from(value),
            "kind_other" => form.kind_other.clone_from(value),
            _ => {}
        }
    }
    form
}

pub async fn dossier_types(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(reference): Path<String>,
    Form(pairs): Form<Vec<(String, String)>>,
) -> Response {
    let form = dossier_types_form(&pairs);
    let today = state.today();
    let Some(loaded) = load_dossier(&state, &reference).await else {
        return locked(&headers).into_response();
    };
    let dossier = match loaded {
        Ok(dossier) => dossier,
        Err(error) => {
            let message = error.to_string();
            if message.contains("introuvable") {
                return page(&headers, gens::not_found(&reference, today)).into_response();
            }
            return page(&headers, html! { div class="empty-state" { (message) } }).into_response();
        }
    };
    let PersonKey::Client { id } = dossier.key else {
        return page(
            &headers,
            gens::dossier_markup(&dossier, today, Some("Les types tiennent sur une fiche.")),
        )
        .into_response();
    };
    let other = form.kind_other.trim().to_string();
    let toggle_on = form.kind_other_on == "1";
    // « un autre » coché et vide : la sélection postée reste, rien n'est écrit.
    if toggle_on && other.is_empty() {
        let posted = gens::KindFormState {
            selected: form.kind,
            other_open: true,
            other: String::new(),
            naming: true,
        };
        return page(
            &headers,
            gens::dossier_with_kinds(&dossier, today, None, &posted),
        )
        .into_response();
    }
    let mut names = form.kind;
    if !other.is_empty() {
        names.push(other.clone());
    }
    let posted = gens::KindFormState {
        selected: names.clone(),
        other_open: toggle_on,
        other: other.clone(),
        naming: false,
    };
    let cmd = SetDossierWorkKinds { client: id, names };
    let result = state
        .with_store_mut(|store| Executor::new(store).execute(&cmd, &AppState::human_ctx()))
        .await;
    match result {
        None => locked(&headers).into_response(),
        Some(Err(error)) => {
            let message = error.to_string();
            page(
                &headers,
                gens::dossier_with_kinds(&dossier, today, Some(&message), &posted),
            )
            .into_response()
        }
        Some(Ok(_)) => match load_dossier(&state, &dossier.name).await {
            None => locked(&headers).into_response(),
            Some(Err(error)) => page(
                &headers,
                html! { div class="empty-state" { (error.to_string()) } },
            )
            .into_response(),
            Some(Ok(fresh)) => saved_page(&headers, gens::dossier_markup(&fresh, today, None)),
        },
    }
}

pub async fn types_get(State(state): State<AppState>, headers: HeaderMap) -> Html<String> {
    match types_markup(&state, "", None, None).await {
        None => locked(&headers),
        Some(Err(error)) => page(
            &headers,
            html! { div class="empty-state" { (error.to_string()) } },
        ),
        Some(Ok(markup)) => page(&headers, markup),
    }
}

pub async fn types_create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<TypeNameForm>,
) -> Response {
    let cmd = CreateWorkKind {
        name: form.name.clone(),
    };
    let result = state
        .with_store_mut(|store| Executor::new(store).execute(&cmd, &AppState::human_ctx()))
        .await;
    match result {
        None => locked(&headers).into_response(),
        Some(Err(error)) => {
            let message = error.to_string();
            types_response(&state, &headers, &form.name, Some(&message), None, false).await
        }
        Some(Ok(_)) => types_response(&state, &headers, "", None, None, true).await,
    }
}

pub async fn types_geste(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(form): Form<TypeGesteForm>,
) -> Response {
    if form.geste == "retirer" {
        let known = state
            .with_store(|store| {
                work_kinds(store.connection()).map(|kinds| kinds.iter().any(|kind| kind.id == id))
            })
            .await;
        return match known {
            None => locked(&headers).into_response(),
            Some(Err(error)) => page(
                &headers,
                html! { div class="empty-state" { (error.to_string()) } },
            )
            .into_response(),
            Some(Ok(true)) => types_response(&state, &headers, "", None, Some(&id), false).await,
            Some(Ok(false)) => {
                types_response(
                    &state,
                    &headers,
                    "",
                    Some("Ce type n'existe pas."),
                    None,
                    false,
                )
                .await
            }
        };
    }
    if form.geste == "confirmer" {
        let cmd = DeleteWorkKind { id: id.clone() };
        let result = state
            .with_store_mut(|store| Executor::new(store).execute(&cmd, &AppState::human_ctx()))
            .await;
        return match result {
            None => locked(&headers).into_response(),
            Some(Err(error)) => {
                let message = error.to_string();
                types_response(&state, &headers, "", Some(&message), None, false).await
            }
            Some(Ok(_)) => types_response(&state, &headers, "", None, None, true).await,
        };
    }
    let cmd = RenameWorkKind {
        id,
        name: form.name,
    };
    let result = state
        .with_store_mut(|store| Executor::new(store).execute(&cmd, &AppState::human_ctx()))
        .await;
    match result {
        None => locked(&headers).into_response(),
        Some(Err(error)) => {
            let message = error.to_string();
            types_response(&state, &headers, "", Some(&message), None, false).await
        }
        Some(Ok(_)) => types_response(&state, &headers, "", None, None, true).await,
    }
}

async fn types_markup(
    state: &AppState,
    draft: &str,
    banner: Option<&str>,
    confirming: Option<&str>,
) -> Option<Result<Markup, AppError>> {
    state
        .with_store(|store| {
            let kinds = work_kinds(store.connection())?;
            Ok(gens::types_page(&kinds, draft, banner, confirming))
        })
        .await
}

async fn types_response(
    state: &AppState,
    headers: &HeaderMap,
    draft: &str,
    banner: Option<&str>,
    confirming: Option<&str>,
    saved: bool,
) -> Response {
    match types_markup(state, draft, banner, confirming).await {
        None => locked(headers).into_response(),
        Some(Err(error)) => page(
            headers,
            html! { div class="empty-state" { (error.to_string()) } },
        )
        .into_response(),
        Some(Ok(markup)) => {
            if saved {
                saved_page(headers, markup)
            } else {
                page(headers, markup).into_response()
            }
        }
    }
}
