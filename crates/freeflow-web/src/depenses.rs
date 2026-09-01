//! Routes de mutation `depenses` (lot 21) — même patron et même convention de réponse que
//! `clients` (voir le commentaire de tête de `crate::clients`) : succès → `200` vide +
//! `HX-Trigger: freeflow:saved`, échec → `200` avec le panneau re-rendu. Toute action ici est un
//! acte humain direct (`AppState::human_ctx()`).

use axum::Form;
use axum::extract::{Path, State};
use axum::http::HeaderValue;
use axum::response::{Html, IntoResponse, Response};
use freeflow_core::app::{AppError, Executor, Outcome};
use freeflow_core::domain::{Expense, ExpenseCategory, ExpenseId, Money, VatRate};
use freeflow_core::expenses;
use maud::html;
use serde::Deserialize;

use crate::state::AppState;
use crate::views;
use crate::views::depenses::{ExpenseFormErrors, ExpenseFormValues};

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

fn parse_id<T: std::str::FromStr>(raw: &str) -> Option<T> {
    raw.parse().ok()
}

#[derive(Debug, Deserialize)]
pub struct ExpenseForm {
    #[serde(default)]
    revision: Option<String>,
    label: String,
    category: String,
    amount: String,
    vat_rate: String,
    vat_deductible: String,
    incurred_on: String,
}

impl From<&ExpenseForm> for ExpenseFormValues {
    fn from(f: &ExpenseForm) -> Self {
        Self {
            label: f.label.clone(),
            category: f.category.clone(),
            amount: f.amount.clone(),
            vat_rate: f.vat_rate.clone(),
            vat_deductible: f.vat_deductible.clone(),
            incurred_on: f.incurred_on.clone(),
        }
    }
}

struct ParsedExpenseForm {
    label: String,
    category: ExpenseCategory,
    amount: Money,
    vat_rate: VatRate,
    vat_deductible: Money,
    incurred_on: time::Date,
}

/// Validations attribuables à un champ précis (montants, date, libellé) — au-delà (TVA
/// déductible > montant, exercice clôturé, conflit de révision), les erreurs du cœur restent un
/// bandeau, même doctrine que `clients`.
fn parse_expense_form(form: &ExpenseForm) -> Result<ParsedExpenseForm, Box<ExpenseFormErrors>> {
    let mut errors = ExpenseFormErrors::default();

    if form.label.trim().is_empty() {
        errors.label = Some("le libellé est obligatoire".to_string());
    }
    let amount = match Money::parse_decimal(form.amount.trim()) {
        Ok(m) => Some(m),
        Err(e) => {
            errors.amount = Some(e.to_string());
            None
        }
    };
    let vat_deductible = match Money::parse_decimal(form.vat_deductible.trim()) {
        Ok(m) => Some(m),
        Err(e) => {
            errors.vat_deductible = Some(e.to_string());
            None
        }
    };
    let incurred_on = match freeflow_core::domain::parse_date(form.incurred_on.trim()) {
        Ok(d) => Some(d),
        Err(e) => {
            errors.incurred_on = Some(e.to_string());
            None
        }
    };
    // Les `<select>` ne proposent que des valeurs valides : un échec ici est une soumission
    // forgée, un bandeau suffit.
    let category = form.category.parse::<ExpenseCategory>();
    let vat_rate = form.vat_rate.parse::<VatRate>();
    if category.is_err() || vat_rate.is_err() {
        errors.banner = Some("catégorie ou taux de TVA invalide".to_string());
    }

    match (
        amount,
        vat_deductible,
        incurred_on,
        category,
        vat_rate,
        errors.label.is_none(),
    ) {
        (
            Some(amount),
            Some(vat_deductible),
            Some(incurred_on),
            Ok(category),
            Ok(vat_rate),
            true,
        ) if errors.banner.is_none() => Ok(ParsedExpenseForm {
            label: form.label.trim().to_string(),
            category,
            amount,
            vat_rate,
            vat_deductible,
            incurred_on,
        }),
        _ => Err(Box::new(errors)),
    }
}

fn expense_error_banner(e: AppError, reload_hx_get: &str) -> ExpenseFormErrors {
    match e {
        AppError::Conflict { .. } => ExpenseFormErrors {
            conflict: Some((e.to_string(), reload_hx_get.to_string())),
            ..Default::default()
        },
        other => ExpenseFormErrors {
            banner: Some(other.to_string()),
            ..Default::default()
        },
    }
}

async fn current_expense(
    state: &AppState,
    id: ExpenseId,
) -> Option<Result<Option<Expense>, AppError>> {
    state
        .with_store(|store| views::depenses::load(store, id))
        .await
}

// -- Liste ---------------------------------------------------------------------------------

pub async fn table(State(state): State<AppState>) -> Html<String> {
    match state.with_store(views::depenses::list_fragment).await {
        None => locked_fragment(),
        Some(Ok(markup)) => Html(markup.into_string()),
        Some(Err(e)) => message_fragment(&e.to_string()),
    }
}

// -- Créer / afficher / modifier / supprimer -----------------------------------------------

pub async fn new_panel() -> Html<String> {
    let today = time::OffsetDateTime::now_utc().date();
    Html(
        views::depenses::new_panel(
            &views::depenses::default_form_values(today),
            &ExpenseFormErrors::default(),
        )
        .into_string(),
    )
}

pub async fn create(State(state): State<AppState>, Form(form): Form<ExpenseForm>) -> Response {
    let parsed = match parse_expense_form(&form) {
        Ok(p) => p,
        Err(errors) => {
            return Html(views::depenses::new_panel(&(&form).into(), &errors).into_string())
                .into_response();
        }
    };
    let cmd = expenses::RecordExpense {
        label: parsed.label,
        category: parsed.category,
        amount: parsed.amount,
        vat_rate: parsed.vat_rate,
        vat_deductible: parsed.vat_deductible,
        incurred_on: parsed.incurred_on,
        receipt_hash: None,
        receipt_filename: None,
    };
    match execute(&state, cmd).await {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => saved(),
        Some(Err(e)) => {
            let errors = expense_error_banner(e, "/depenses/new");
            Html(views::depenses::new_panel(&(&form).into(), &errors).into_string()).into_response()
        }
    }
}

pub async fn show_panel(State(state): State<AppState>, Path(id): Path<String>) -> Html<String> {
    let Some(id) = parse_id::<ExpenseId>(&id) else {
        return message_fragment("identifiant de dépense invalide");
    };
    match current_expense(&state, id).await {
        None => locked_fragment(),
        Some(Err(e)) => message_fragment(&e.to_string()),
        Some(Ok(None)) => {
            message_fragment("dépense introuvable — elle a peut-être été supprimée entre-temps")
        }
        Some(Ok(Some(expense))) => {
            Html(views::depenses::detail_panel(&expense, None).into_string())
        }
    }
}

pub async fn edit_panel(State(state): State<AppState>, Path(id): Path<String>) -> Html<String> {
    let Some(id) = parse_id::<ExpenseId>(&id) else {
        return message_fragment("identifiant de dépense invalide");
    };
    match current_expense(&state, id).await {
        None => locked_fragment(),
        Some(Err(e)) => message_fragment(&e.to_string()),
        Some(Ok(None)) => message_fragment("dépense introuvable"),
        Some(Ok(Some(expense))) => Html(
            views::depenses::edit_panel(
                id,
                expense.revision,
                &(&expense).into(),
                &ExpenseFormErrors::default(),
            )
            .into_string(),
        ),
    }
}

pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(form): Form<ExpenseForm>,
) -> Response {
    let Some(id) = parse_id::<ExpenseId>(&id) else {
        return message_fragment("identifiant de dépense invalide").into_response();
    };
    let revision: i64 = form
        .revision
        .as_deref()
        .and_then(|s| s.parse().ok())
        .unwrap_or_default();
    let parsed = match parse_expense_form(&form) {
        Ok(p) => p,
        Err(errors) => {
            return Html(
                views::depenses::edit_panel(id, revision, &(&form).into(), &errors).into_string(),
            )
            .into_response();
        }
    };
    // Le justificatif existant voyage tel quel dans l'état complet : le panneau ne l'édite pas
    // (voir le commentaire de tête de `views::depenses`).
    let (receipt_hash, receipt_filename) = match current_expense(&state, id).await {
        None => return locked_fragment().into_response(),
        Some(Err(e)) => return message_fragment(&e.to_string()).into_response(),
        Some(Ok(None)) => return message_fragment("dépense introuvable").into_response(),
        Some(Ok(Some(e))) => (e.receipt_hash, e.receipt_filename),
    };
    let cmd = expenses::UpdateExpense {
        id,
        revision,
        label: parsed.label,
        category: parsed.category,
        amount: parsed.amount,
        vat_rate: parsed.vat_rate,
        vat_deductible: parsed.vat_deductible,
        incurred_on: parsed.incurred_on,
        receipt_hash,
        receipt_filename,
    };
    match execute(&state, cmd).await {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => saved(),
        Some(Err(e)) => {
            let errors = expense_error_banner(e, &format!("/depenses/{id}/edit"));
            Html(views::depenses::edit_panel(id, revision, &(&form).into(), &errors).into_string())
                .into_response()
        }
    }
}

pub async fn delete_confirm_panel(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Html<String> {
    let Some(id) = parse_id::<ExpenseId>(&id) else {
        return message_fragment("identifiant de dépense invalide");
    };
    match current_expense(&state, id).await {
        None => locked_fragment(),
        Some(Err(e)) => message_fragment(&e.to_string()),
        Some(Ok(None)) => message_fragment("dépense introuvable"),
        Some(Ok(Some(expense))) => {
            Html(views::depenses::delete_confirm_panel(&expense).into_string())
        }
    }
}

pub async fn delete(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let Some(id) = parse_id::<ExpenseId>(&id) else {
        return message_fragment("identifiant de dépense invalide").into_response();
    };
    let revision = match current_expense(&state, id).await {
        None => return locked_fragment().into_response(),
        Some(Err(e)) => return message_fragment(&e.to_string()).into_response(),
        Some(Ok(None)) => {
            return message_fragment("dépense introuvable — déjà supprimée").into_response();
        }
        Some(Ok(Some(expense))) => expense.revision,
    };
    match execute(&state, expenses::DeleteExpense { id, revision }).await {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => saved(),
        Some(Err(e)) => match current_expense(&state, id).await {
            Some(Ok(Some(expense))) => {
                Html(views::depenses::detail_panel(&expense, Some(&e.to_string())).into_string())
                    .into_response()
            }
            _ => message_fragment(&e.to_string()).into_response(),
        },
    }
}
