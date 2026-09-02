//! Routes `devis` — lecture et transitions du cycle de vie (lot 21), création et révision
//! (lot 23). Même convention de réponse que `clients` : succès → `200` vide + `HX-Trigger:
//! freeflow:saved`, échec → `200` avec le panneau re-rendu (erreurs de champ ou bandeau). Voir
//! le commentaire de tête de `views::devis` pour la forme de l'éditeur de lignes.

use axum::Form;
use axum::extract::{Path, State};
use axum::http::HeaderValue;
use axum::response::{Html, IntoResponse, Response};
use freeflow_core::app::{AppError, Executor, Outcome};
use freeflow_core::domain::{Discount, Money, QuoteId, QuoteLine};
use freeflow_core::quotes;
use maud::html;
use serde::Deserialize;

use crate::state::AppState;
use crate::views;
use crate::views::devis::{QuoteFormErrors, QuoteFormValues};

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

// -- Créer / réviser -----------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct QuoteForm {
    #[serde(default)]
    client: String,
    #[serde(default)]
    opportunity: String,
    #[serde(default)]
    lines: String,
    #[serde(default)]
    discount_percent: String,
    #[serde(default)]
    discount_amount: String,
    #[serde(default)]
    terms: String,
    #[serde(default)]
    valid_until: String,
}

impl From<&QuoteForm> for QuoteFormValues {
    fn from(f: &QuoteForm) -> Self {
        Self {
            client: f.client.clone(),
            opportunity: f.opportunity.clone(),
            lines: f.lines.clone(),
            discount_percent: f.discount_percent.clone(),
            discount_amount: f.discount_amount.clone(),
            terms: f.terms.clone(),
            valid_until: f.valid_until.clone(),
        }
    }
}

/// Le contenu commun à la création et à la révision — tout ce qui n'est pas le client ni
/// l'opportunité (hérités de la racine dans le cas d'une révision).
struct ParsedQuoteContent {
    lines: Vec<QuoteLine>,
    discount: Option<Discount>,
    terms: Option<String>,
    valid_until: time::Date,
}

fn opt(s: &str) -> Option<String> {
    let trimmed = s.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn parse_quote_content(form: &QuoteForm) -> Result<ParsedQuoteContent, Box<QuoteFormErrors>> {
    let mut errors = QuoteFormErrors::default();

    // Une ligne de devis par ligne de textarea, lignes vides ignorées — la première ligne
    // invalide porte l'erreur, avec le rappel de syntaxe du parseur du domaine.
    let mut lines = Vec::new();
    for raw in form.lines.lines().map(str::trim).filter(|l| !l.is_empty()) {
        match raw.parse::<QuoteLine>() {
            Ok(line) => lines.push(line),
            Err(e) => {
                errors.lines = Some(e.to_string());
                break;
            }
        }
    }
    if errors.lines.is_none() && lines.is_empty() {
        errors.lines = Some("au moins une ligne est requise".to_string());
    }

    let discount_percent = opt(&form.discount_percent);
    let discount_amount = opt(&form.discount_amount);
    let discount = match (discount_percent, discount_amount) {
        (Some(_), Some(_)) => {
            errors.discount =
                Some("les deux remises sont exclusives — n'en renseigner qu'une".to_string());
            None
        }
        (Some(bps), None) => match bps.parse::<u32>() {
            Ok(bps) => Some(Discount::Percentage(bps)),
            Err(_) => {
                errors.discount = Some(format!(
                    "remise en dix-millièmes invalide : {bps} (1000 = 10 %)"
                ));
                None
            }
        },
        (None, Some(amount)) => match Money::parse_decimal(&amount) {
            Ok(m) => Some(Discount::FixedAmount(m)),
            Err(e) => {
                errors.discount = Some(e.to_string());
                None
            }
        },
        (None, None) => None,
    };

    let valid_until = match freeflow_core::domain::parse_date(form.valid_until.trim()) {
        Ok(d) => Some(d),
        Err(e) => {
            errors.valid_until = Some(e.to_string());
            None
        }
    };

    match (
        valid_until,
        errors.lines.is_none(),
        errors.discount.is_none(),
    ) {
        (Some(valid_until), true, true) => Ok(ParsedQuoteContent {
            lines,
            discount,
            terms: opt(&form.terms),
            valid_until,
        }),
        _ => Err(Box::new(errors)),
    }
}

pub async fn new_panel() -> Html<String> {
    // Un mois de validité par défaut — modifiable, mais jamais un champ vide qui obligerait à
    // choisir une date avant même d'avoir saisi une ligne.
    let valid_until = freeflow_core::clock::today_local() + time::Duration::days(30);
    let values = QuoteFormValues {
        valid_until: freeflow_core::domain::format_date(valid_until),
        ..Default::default()
    };
    Html(views::devis::new_panel(&values, &QuoteFormErrors::default()).into_string())
}

pub async fn create(State(state): State<AppState>, Form(form): Form<QuoteForm>) -> Response {
    let new_panel_with = |errors: QuoteFormErrors| {
        Html(views::devis::new_panel(&(&form).into(), &errors).into_string()).into_response()
    };
    let client_id = match state
        .with_store(|store| {
            freeflow_core::reference::resolve_client(store.connection(), &form.client)
        })
        .await
    {
        None => return locked_fragment().into_response(),
        Some(Ok(freeflow_core::reference::RefMatch::Unique(id))) => id,
        Some(Ok(_)) | Some(Err(_)) => {
            return new_panel_with(QuoteFormErrors {
                client: Some("client introuvable ou ambigu".to_string()),
                ..Default::default()
            });
        }
    };
    let opportunity_id = match opt(&form.opportunity) {
        None => None,
        Some(reference) => match state
            .with_store(|store| {
                freeflow_core::reference::resolve_opportunity(store.connection(), &reference)
            })
            .await
        {
            None => return locked_fragment().into_response(),
            Some(Ok(freeflow_core::reference::RefMatch::Unique(id))) => Some(id),
            Some(Ok(_)) | Some(Err(_)) => {
                return new_panel_with(QuoteFormErrors {
                    opportunity: Some("opportunité introuvable ou ambiguë".to_string()),
                    ..Default::default()
                });
            }
        },
    };
    let content = match parse_quote_content(&form) {
        Ok(c) => c,
        Err(errors) => return new_panel_with(*errors),
    };
    let cmd = quotes::CreateQuote {
        client_id,
        opportunity_id,
        lines: content.lines,
        discount: content.discount,
        terms: content.terms,
        valid_until: content.valid_until,
    };
    match execute(&state, cmd).await {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => saved(),
        Some(Err(e)) => new_panel_with(QuoteFormErrors {
            banner: Some(e.to_string()),
            ..Default::default()
        }),
    }
}

async fn quote_with_client_name(
    state: &AppState,
    id: QuoteId,
) -> Option<Result<Option<(freeflow_core::domain::Quote, String)>, AppError>> {
    state
        .with_store(|store| {
            let Some(quote) = quotes::quote_by_id(store.connection(), id)? else {
                return Ok(None);
            };
            let client = freeflow_core::clients::list_clients(store.connection())?
                .into_iter()
                .find(|c| c.id == quote.client_id)
                .map_or_else(|| "?".to_string(), |c| c.name);
            Ok(Some((quote, client)))
        })
        .await
}

pub async fn revise_panel(State(state): State<AppState>, Path(id): Path<String>) -> Html<String> {
    let Some(id) = parse_id(&id) else {
        return message_fragment("identifiant de devis invalide");
    };
    match quote_with_client_name(&state, id).await {
        None => locked_fragment(),
        Some(Err(e)) => message_fragment(&e.to_string()),
        Some(Ok(None)) => message_fragment("devis introuvable"),
        Some(Ok(Some((quote, client)))) => Html(
            views::devis::revise_panel(
                &quote,
                &client,
                &QuoteFormValues::from_quote(&quote),
                &QuoteFormErrors::default(),
            )
            .into_string(),
        ),
    }
}

pub async fn revise(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(form): Form<QuoteForm>,
) -> Response {
    let Some(id) = parse_id(&id) else {
        return message_fragment("identifiant de devis invalide").into_response();
    };
    let (quote, client) = match quote_with_client_name(&state, id).await {
        None => return locked_fragment().into_response(),
        Some(Err(e)) => return message_fragment(&e.to_string()).into_response(),
        Some(Ok(None)) => return message_fragment("devis introuvable").into_response(),
        Some(Ok(Some(found))) => found,
    };
    let revise_panel_with = |errors: QuoteFormErrors| {
        Html(views::devis::revise_panel(&quote, &client, &(&form).into(), &errors).into_string())
            .into_response()
    };
    let content = match parse_quote_content(&form) {
        Ok(c) => c,
        Err(errors) => return revise_panel_with(*errors),
    };
    // N'importe quelle version de la lignée sert de point d'entrée : la révision repart
    // toujours de la racine partagée — même geste que la CLI (`quote revise`).
    let cmd = quotes::ReviseQuote {
        root_id: quote.root_id,
        lines: content.lines,
        discount: content.discount,
        terms: content.terms,
        valid_until: content.valid_until,
    };
    match execute(&state, cmd).await {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => saved(),
        Some(Err(e)) => revise_panel_with(QuoteFormErrors {
            banner: Some(e.to_string()),
            ..Default::default()
        }),
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
