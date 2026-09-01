//! Écran `depenses` (lot 21) — même patron que `clients` (lot 15) : liste auto-rafraîchie +
//! panneau latéral pour créer/afficher/modifier/supprimer une dépense.
//!
//! Pas de justificatif ici : l'archivage d'un fichier local (lecture, hash, copie en
//! `receipts/`) passe par la CLI (`freeflow expense record --receipt …`) — un champ d'upload
//! dans la webview exigerait du multipart et une politique de fichiers propre à la coque, hors
//! périmètre de ce lot. Le panneau d'édition conserve tel quel le justificatif existant (la
//! commande du cœur voyage toujours en état complet), et la fiche l'affiche s'il y en a un.

use freeflow_core::app::AppError;
use freeflow_core::domain::{Expense, ExpenseCategory, ExpenseId, Money, VatRate};
use freeflow_core::expenses::{expense_by_id, list_expenses};
use freeflow_core::store::Store;
use maud::{Markup, html};

use crate::layout::{ViewId, view_head};
use crate::views::{form, panel};

pub const CATEGORY_OPTIONS: [(&str, &str); 7] = [
    ("software", "logiciels & abonnements"),
    ("equipment", "matériel"),
    ("travel", "déplacements"),
    ("meals", "repas"),
    ("office", "bureau"),
    ("professional", "professionnel (formation, assurance…)"),
    ("other", "autre"),
];

pub const VAT_RATE_OPTIONS: [(&str, &str); 5] = [
    ("standard", "20 % (normal)"),
    ("intermediate", "10 % (intermédiaire)"),
    ("reduced", "5,5 % (réduit)"),
    ("super_reduced", "2,1 % (super réduit)"),
    ("zero", "0 % (exonéré)"),
];

fn category_label(category: ExpenseCategory) -> &'static str {
    CATEGORY_OPTIONS
        .iter()
        .find(|(value, _)| *value == category.as_str())
        .map_or("autre", |(_, label)| label)
}

/// Montant en euros décimaux pour un `<input>` — le format que `Money::parse_decimal` relit.
fn money_input_value(m: Money) -> String {
    format!("{}.{:02}", m.cents() / 100, m.cents().rem_euclid(100))
}

#[derive(Default, Clone)]
pub struct ExpenseFormValues {
    pub label: String,
    pub category: String,
    pub amount: String,
    pub vat_rate: String,
    pub vat_deductible: String,
    pub incurred_on: String,
}

impl From<&Expense> for ExpenseFormValues {
    fn from(e: &Expense) -> Self {
        Self {
            label: e.label.clone(),
            category: e.category.as_str().to_string(),
            amount: money_input_value(e.amount),
            vat_rate: e.vat_rate.as_str().to_string(),
            vat_deductible: money_input_value(e.vat_deductible),
            incurred_on: freeflow_core::domain::format_date(e.incurred_on),
        }
    }
}

#[derive(Default)]
pub struct ExpenseFormErrors {
    pub label: Option<String>,
    pub amount: Option<String>,
    pub vat_deductible: Option<String>,
    pub incurred_on: Option<String>,
    pub banner: Option<String>,
    pub conflict: Option<(String, String)>,
}

fn expense_form(
    action: &str,
    revision: Option<i64>,
    values: &ExpenseFormValues,
    errors: &ExpenseFormErrors,
) -> Markup {
    html! {
        form hx-post=(action) hx-target="#panel" hx-swap="innerHTML" {
            @if let Some((message, reload)) = &errors.conflict {
                (form::conflict_banner(message, reload))
            } @else {
                @if let Some(msg) = &errors.banner {
                    (form::error_banner(msg))
                }
                @if let Some(revision) = revision {
                    (form::hidden("revision", &revision.to_string()))
                }
                (form::text("label", "Libellé", &values.label, errors.label.as_deref()))
                (form::select("category", "Catégorie", &CATEGORY_OPTIONS, &values.category, None))
                (form::number("amount", "Montant TTC (€)", &values.amount, "0.01", errors.amount.as_deref()))
                (form::select("vat_rate", "Taux de TVA", &VAT_RATE_OPTIONS, &values.vat_rate, None))
                (form::number("vat_deductible", "TVA déductible (€)", &values.vat_deductible, "0.01", errors.vat_deductible.as_deref()))
                (form::field_help("La TVA effectivement déductible peut être inférieure à montant × taux (véhicules, restauration…)."))
                (form::date("incurred_on", "Date d'engagement", &values.incurred_on, errors.incurred_on.as_deref()))
                (form::actions(if revision.is_some() { "Enregistrer" } else { "Enregistrer la dépense" }))
            }
        }
    }
}

pub fn new_panel(values: &ExpenseFormValues, errors: &ExpenseFormErrors) -> Markup {
    panel::sheet(
        "Nouvelle dépense",
        expense_form("/depenses", None, values, errors),
    )
}

pub fn edit_panel(
    id: ExpenseId,
    revision: i64,
    values: &ExpenseFormValues,
    errors: &ExpenseFormErrors,
) -> Markup {
    panel::sheet(
        "Modifier la dépense",
        expense_form(&format!("/depenses/{id}"), Some(revision), values, errors),
    )
}

pub fn detail_panel(expense: &Expense, error: Option<&str>) -> Markup {
    let body = html! {
        @if let Some(msg) = error {
            (form::error_banner(msg))
        }
        div class="detail-head" {
            div class="detail-title" { (expense.label) }
            span class="badge" { (category_label(expense.category)) }
        }
        dl class="detail-fields" {
            dt { "Montant TTC" } dd class="mono" { (expense.amount) }
            dt { "TVA déductible" } dd class="mono" { (expense.vat_deductible) }
            dt { "Taux de TVA" } dd { (expense.vat_rate.as_str()) }
            dt { "Engagée le" } dd { (freeflow_core::domain::format_date(expense.incurred_on)) }
            @if let Some(filename) = &expense.receipt_filename {
                dt { "Justificatif" } dd class="mono" { (filename) }
            } @else if expense.receipt_hash.is_some() {
                dt { "Justificatif" } dd { "haché, non archivé" }
            }
        }
        @if expense.receipt_hash.is_none() {
            div class="detail-note" {
                "Aucun justificatif attaché — l'archivage d'un fichier passe par la CLI : "
                code { "freeflow expense edit " (expense.id) " --receipt <fichier>" }
            }
        }
        div class="detail-actions" {
            button class="btn" hx-get=(format!("/depenses/{}/edit", expense.id)) hx-target="#panel" hx-swap="innerHTML" { "modifier" }
            button class="btn danger" hx-get=(format!("/depenses/{}/delete", expense.id)) hx-target="#panel" hx-swap="innerHTML" { "supprimer" }
        }
    };
    panel::sheet(&expense.label, body)
}

pub fn delete_confirm_panel(expense: &Expense) -> Markup {
    let body = html! {
        div class="detail-note" {
            "Supprimer définitivement « " (expense.label) " » ("
            (expense.amount) ", " (freeflow_core::domain::format_date(expense.incurred_on))
            ") ? Cette action est irréversible — refusée si la dépense tombe dans un exercice "
            "déjà clôturé. Le justificatif archivé, lui, reste en place."
        }
        div class="form-actions" {
            // Révision relue côté serveur au moment du clic — même raison que la suppression de
            // client (voir `views::clients::delete_confirm_panel`).
            button class="btn danger" hx-post=(format!("/depenses/{}/delete", expense.id)) hx-target="#panel" hx-swap="innerHTML" {
                "confirmer la suppression"
            }
        }
    };
    panel::sheet("Confirmer la suppression", body)
}

pub fn list_fragment(store: &Store) -> Result<Markup, AppError> {
    let expenses = list_expenses(store.connection())?;
    Ok(html! {
        div id="depenses-list"
            hx-get="/depenses/table"
            hx-trigger="freeflow:saved from:body"
            hx-target="this"
            hx-swap="outerHTML" {
            div class="pipe-toolbar" {
                button class="btn primary" hx-get="/depenses/new" hx-target="#panel" hx-swap="innerHTML" { "+ nouvelle dépense" }
            }
            @if expenses.is_empty() {
                div class="empty-state" { "aucune dépense — cliquez sur « nouvelle dépense »" }
            } @else {
                div class="panel bordered" style="padding:0" {
                    table {
                        tr {
                            th style="padding-left:18px" { "date" }
                            th { "libellé" }
                            th { "catégorie" }
                            th { "montant ttc" }
                            th { "tva déductible" }
                            th style="padding-right:18px" { "justificatif" }
                        }
                        @for expense in &expenses {
                            tr class="row-clickable" hx-get=(format!("/depenses/{}", expense.id)) hx-target="#panel" hx-swap="innerHTML" {
                                td style="padding-left:18px" class="mono" { (freeflow_core::domain::format_date(expense.incurred_on)) }
                                td { (expense.label) }
                                td { (category_label(expense.category)) }
                                td class="mono" { (expense.amount) }
                                td class="mono" { (expense.vat_deductible) }
                                td style="padding-right:18px" {
                                    @if expense.receipt_hash.is_some() { span class="badge ok" { "oui" } }
                                    @else { "—" }
                                }
                            }
                        }
                    }
                }
            }
        }
    })
}

pub fn render(store: &Store) -> Result<Markup, AppError> {
    let expenses = list_expenses(store.connection())?;
    let total: Money = expenses.iter().map(|e| e.amount).sum();
    let deductible: Money = expenses.iter().map(|e| e.vat_deductible).sum();
    Ok(html! {
        (view_head(ViewId::Depenses, &format!(
            "{} dépense(s) — {total} TTC, {deductible} de TVA déductible",
            expenses.len()
        )))
        (list_fragment(store)?)
    })
}

pub fn load(store: &Store, id: ExpenseId) -> Result<Option<Expense>, AppError> {
    expense_by_id(store.connection(), id)
}

/// Sélectionne le taux par défaut du formulaire de création — le taux normal, celui de
/// l'écrasante majorité des dépenses d'un indépendant du numérique.
pub fn default_form_values(today: time::Date) -> ExpenseFormValues {
    ExpenseFormValues {
        category: "software".to_string(),
        vat_rate: VatRate::Standard.as_str().to_string(),
        incurred_on: freeflow_core::domain::format_date(today),
        ..Default::default()
    }
}
