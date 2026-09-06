//! Écran `depenses` (lot 21) — même patron que `clients` (lot 15) : liste auto-rafraîchie +
//! panneau latéral pour créer/afficher/modifier/supprimer une dépense.
//!
//! Justificatif (lot 29) : les formulaires de création et de modification sont en
//! `multipart/form-data` (`hx-encoding`, htmx envoie alors un `FormData`, sans script inline ni
//! `eval` — compatible avec la CSP `script-src 'self'` de la fenêtre) et portent un
//! `<input type="file">` ; la webview ouvre le sélecteur de fichiers natif de la plateforme.
//! L'archivage lui-même (hash, copie en `receipts/`) est fait par `crate::depenses` via le
//! helper partagé avec la CLI. Le panneau d'édition affiche le justificatif actuel, permet de le
//! remplacer ou de le détacher (case à cocher, l'équivalent de `--clear-receipt`) — sinon il
//! voyage tel quel dans l'état complet de la commande.
//!
//! Rapprochement bancaire (lot 33) : les débits du relevé importé (depuis le lot 38, la fenêtre
//! importe elle-même) restant à rapprocher sont listés au-dessus des dépenses ; chacun
//! ouvre le formulaire de création pré-rempli (montant, date, libellé, champ caché
//! `bank_transaction_id`) — la dépense est créée rapprochée. Une dépense existante se rapproche
//! depuis sa fiche (panneau listant les débits du même montant) et le rapprochement se défait
//! depuis la fiche aussi (la dépense reste, le débit redevient « à rapprocher »).

use freeflow_core::app::AppError;
use freeflow_core::billing::{list_bank_transactions, unmatched_debits};
use freeflow_core::domain::{
    BankTransaction, DEFAULT_DURATION_MONTHS, Expense, ExpenseCategory, ExpenseId, FixedAsset,
    Money, SMALL_EQUIPMENT_THRESHOLD, VatRate, format_date,
};
use freeflow_core::expenses::{ExpenseDetail, expense_detail, list_expenses, reconciled_debits};
use freeflow_core::fixed_assets::{expense_net, list_fixed_assets, should_be_immobilized};
use freeflow_core::store::Store;
use maud::{Markup, html};

use crate::layout::{ViewId, view_head};
use crate::views::{form, panel};

pub const CATEGORY_OPTIONS: [(&str, &str); 10] = [
    ("software", "logiciels & abonnements"),
    ("equipment", "matériel"),
    ("travel", "déplacements"),
    ("meals", "repas"),
    ("office", "bureau"),
    ("professional", "professionnel (formation, assurance…)"),
    ("fees", "honoraires (expert-comptable, avocat…)"),
    ("bank_charges", "frais bancaires"),
    ("taxes", "impôts et taxes (CFE, CVAE… pas l'IS ni la TVA)"),
    ("other", "autre"),
];

/// Les comptes de bilan usuels qu'un mouvement du relevé règle (lot 37), en français : le
/// numéro est la valeur, le libellé dit à quoi il sert. « autre compte » ouvre la saisie libre.
pub const SETTLEMENT_OPTIONS: [(&str, &str); 9] = [
    (
        "401000",
        "honoraires ou facture reprise au bilan (401 fournisseurs)",
    ),
    ("444000", "solde ou acompte d'IS (444)"),
    ("445510", "TVA à décaisser reprise au bilan (4455)"),
    ("445670", "crédit de TVA remboursé (445670)"),
    (
        "455000",
        "compte courant d'associé — apport ou remboursement (455)",
    ),
    ("457000", "dividendes payés (457)"),
    ("580000", "virement entre mes comptes (580)"),
    ("164000", "emprunt — remboursement ou déblocage (164)"),
    ("", "autre compte de bilan…"),
];

pub const ASSET_ACCOUNT_OPTIONS: [(&str, &str); 5] = [
    ("218300", "matériel de bureau et informatique (2183)"),
    ("205000", "logiciel (205)"),
    ("218400", "mobilier (2184)"),
    ("218200", "matériel de transport (2182)"),
    ("215400", "matériel industriel (2154)"),
];

pub const VAT_RATE_OPTIONS: [(&str, &str); 5] = [
    ("standard", "20 % (normal)"),
    ("intermediate", "10 % (intermédiaire)"),
    ("reduced", "5,5 % (réduit)"),
    ("super_reduced", "2,1 % (super réduit)"),
    ("zero", "0 % (exonéré)"),
];

/// Types de fichiers proposés par le sélecteur — un filtre d'ergonomie côté navigateur, pas une
/// validation : tout contenu reçu est archivé tel quel, comme `--receipt` en CLI.
const RECEIPT_ACCEPT: &str = ".pdf,.png,.jpg,.jpeg,.heic,.webp,image/*,application/pdf";

fn category_label(category: ExpenseCategory) -> &'static str {
    CATEGORY_OPTIONS
        .iter()
        .find(|(value, _)| *value == category.as_str())
        .map_or("autre", |(_, label)| label)
}

#[derive(Default, Clone)]
pub struct ExpenseFormValues {
    pub label: String,
    pub category: String,
    pub amount: String,
    pub vat_rate: String,
    pub vat_deductible: String,
    pub incurred_on: String,
    /// Bénéficiaire (fournisseur), lot 41 — vide = aucun.
    pub supplier: String,
    /// Nom du justificatif déjà archivé (édition seulement) — affiché, jamais persisté depuis
    /// le formulaire.
    pub current_receipt: Option<String>,
    /// Débit du relevé que la dépense créée paiera (création seulement, lot 33) : id en champ
    /// caché, et son résumé pour l'aide du formulaire.
    pub bank_transaction_id: Option<String>,
    pub bank_transaction_note: Option<String>,
}

impl From<&Expense> for ExpenseFormValues {
    fn from(e: &Expense) -> Self {
        Self {
            label: e.label.clone(),
            category: e.category.as_str().to_string(),
            amount: e.amount.to_decimal_string(),
            vat_rate: e.vat_rate.as_str().to_string(),
            vat_deductible: e.vat_deductible.to_decimal_string(),
            incurred_on: format_date(e.incurred_on),
            supplier: e.supplier.clone().unwrap_or_default(),
            current_receipt: e.receipt_filename.clone(),
            bank_transaction_id: None,
            bank_transaction_note: None,
        }
    }
}

/// Résumé d'un débit pour l'aide du formulaire et la fiche.
fn debit_summary(tx: &BankTransaction) -> String {
    format!(
        "{} — {} ({})",
        format_date(tx.occurred_on),
        Money::from_cents(-tx.amount_cents),
        tx.description
    )
}

/// Le formulaire de création pré-rempli depuis un débit du relevé (lot 33) : montant et date
/// du débit, libellé du relevé, catégorie « autre » à préciser, TVA à renseigner par
/// l'utilisateur (le relevé ne la connaît pas — zéro déductible par défaut, jamais deviné).
#[must_use]
pub fn form_values_from_debit(tx: &BankTransaction) -> ExpenseFormValues {
    ExpenseFormValues {
        label: tx.description.clone(),
        category: "other".to_string(),
        amount: Money::from_cents(-tx.amount_cents).to_decimal_string(),
        vat_rate: VatRate::Standard.as_str().to_string(),
        vat_deductible: Money::ZERO.to_decimal_string(),
        incurred_on: format_date(tx.occurred_on),
        supplier: String::new(),
        current_receipt: None,
        bank_transaction_id: Some(tx.id.to_string()),
        bank_transaction_note: Some(debit_summary(tx)),
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
        form hx-post=(action) hx-target="#panel" hx-swap="innerHTML" hx-encoding="multipart/form-data" enctype="multipart/form-data" {
            @if let Some((message, reload)) = &errors.conflict {
                (form::conflict_banner(message, reload))
            } @else {
                @if let Some(msg) = &errors.banner {
                    (form::error_banner(msg))
                }
                @if let Some(revision) = revision {
                    (form::hidden("revision", &revision.to_string()))
                }
                @if let Some(transaction) = &values.bank_transaction_id {
                    (form::hidden("bank_transaction_id", transaction))
                    div class="detail-note" {
                        "Rapprochée au débit du relevé : "
                        (values.bank_transaction_note.clone().unwrap_or_default())
                        ". Le montant doit rester celui du débit."
                    }
                }
                (form::text("label", "Libellé", &values.label, errors.label.as_deref()))
                (form::select("category", "Catégorie", &CATEGORY_OPTIONS, &values.category, None))
                (form::number("amount", "Montant TTC (€)", &values.amount, "0.01", errors.amount.as_deref()))
                (form::select("vat_rate", "Taux de TVA", &VAT_RATE_OPTIONS, &values.vat_rate, None))
                (form::number("vat_deductible", "TVA déductible (€)", &values.vat_deductible, "0.01", errors.vat_deductible.as_deref()))
                (form::field_help("La TVA effectivement déductible peut être inférieure à montant × taux (véhicules, restauration…)."))
                (form::date("incurred_on", "Date d'engagement", &values.incurred_on, errors.incurred_on.as_deref()))
                (form::text("supplier", "Bénéficiaire (fournisseur)", &values.supplier, None))
                (form::field_help("Pour des honoraires, c'est ce nom qui cumule sur la DAS2 (seuil 2 400 € par bénéficiaire et par année civile)."))
                @if let Some(current) = &values.current_receipt {
                    (form::hidden("current_receipt", current))
                    (form::file("receipt", "Remplacer le justificatif", RECEIPT_ACCEPT))
                    (form::field_help(&format!("Justificatif actuel : {current}")))
                    (form::checkbox("clear_receipt", "Détacher le justificatif (le fichier archivé reste en place)"))
                } @else {
                    (form::file("receipt", "Justificatif", RECEIPT_ACCEPT))
                    (form::field_help("Facture fournisseur, ticket, note de frais — archivé à côté du coffre avec son hash d'intégrité SHA-256."))
                }
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

pub fn detail_panel(
    detail: &ExpenseDetail,
    asset: Option<&FixedAsset>,
    error: Option<&str>,
) -> Markup {
    let expense = &detail.expense;
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
            dt { "Engagée le" } dd { (format_date(expense.incurred_on)) }
            @if let Some(supplier) = &expense.supplier {
                dt { "Bénéficiaire" } dd { (supplier) }
            }
            @if let Some(filename) = &expense.receipt_filename {
                dt { "Justificatif" } dd class="mono" { (filename) }
            } @else if expense.receipt_hash.is_some() {
                dt { "Justificatif" } dd { "haché, non archivé" }
            }
            dt { "Relevé bancaire" }
            @if let Some(tx) = &detail.bank_transaction {
                dd { span class="badge ok" { "rapprochée" } " " (debit_summary(tx)) }
            } @else {
                dd { "non rapprochée — réputée payée à sa date dans le grand livre" }
            }
            @if let Some(asset) = asset {
                dt { "Immobilisation" }
                dd {
                    span class="badge ok" { "immobilisée" }
                    " " (asset.account) " — " (asset.duration_months) " mois, base "
                    (asset.base)
                }
            } @else if expense.category == ExpenseCategory::Equipment
                && expense_net(expense) > SMALL_EQUIPMENT_THRESHOLD
            {
                dt { "Immobilisation" }
                dd {
                    "matériel de " (expense_net(expense)) " HT — au-delà de "
                    (SMALL_EQUIPMENT_THRESHOLD)
                    ", passez-le à l'actif plutôt qu'en charge"
                }
            }
        }
        @if expense.receipt_hash.is_none() {
            div class="detail-note" {
                "Aucun justificatif attaché — ajoutez-en un via « modifier » (possible même après                  la clôture : une pièce ne change ni le montant ni la date)."
            }
        } @else {
            div class="detail-note" {
                a class="btn small" href=(format!("/depenses/{}/receipt", expense.id)) target="_blank" { "voir le justificatif" }
                " (déchiffré à la volée depuis le coffre)"
            }
        }
        div class="detail-actions" {
            button class="btn" hx-get=(format!("/depenses/{}/edit", expense.id)) hx-target="#panel" hx-swap="innerHTML" { "modifier" }
            @if let Some(asset) = asset {
                button class="btn" hx-get=(format!("/depenses/assets/{}", asset.id)) hx-target="#panel" hx-swap="innerHTML" { "voir l'immobilisation" }
            } @else if expense.category == ExpenseCategory::Equipment
                && expense_net(expense) > SMALL_EQUIPMENT_THRESHOLD
            {
                button class="btn" hx-get=(format!("/depenses/{}/immobilize", expense.id)) hx-target="#panel" hx-swap="innerHTML" { "immobiliser" }
            }
            @if detail.bank_transaction.is_some() {
                button class="btn" hx-post=(format!("/depenses/{}/unreconcile", expense.id)) hx-target="#panel" hx-swap="innerHTML" { "défaire le rapprochement" }
            } @else {
                button class="btn" hx-get=(format!("/depenses/{}/reconcile", expense.id)) hx-target="#panel" hx-swap="innerHTML" { "rapprocher d'un débit" }
            }
            button class="btn danger" hx-get=(format!("/depenses/{}/delete", expense.id)) hx-target="#panel" hx-swap="innerHTML" { "supprimer" }
        }
    };
    panel::sheet(&expense.label, body)
}

/// Panneau de rapprochement d'une dépense existante : les débits du relevé restant à
/// rapprocher **au montant exact** de la dépense — la seule garde que le cœur accepte, autant
/// ne proposer que ce qui passera.
pub fn reconcile_panel(
    expense: &Expense,
    candidates: &[BankTransaction],
    error: Option<&str>,
) -> Markup {
    let options: Vec<(String, String)> = candidates
        .iter()
        .map(|tx| (tx.id.to_string(), debit_summary(tx)))
        .collect();
    let borrowed: Vec<(&str, &str)> = options
        .iter()
        .map(|(id, text)| (id.as_str(), text.as_str()))
        .collect();
    let body = html! {
        @if let Some(msg) = error {
            (form::error_banner(msg))
        }
        div class="detail-note" {
            "Débits du relevé importé de " (expense.amount) " restant à rapprocher. "
            "Le rapprochement date le décaissement du relevé dans le grand livre (401 puis 512)."
        }
        @if candidates.is_empty() {
            div class="empty-state" {
                "aucun débit de ce montant à rapprocher — importez d'abord le relevé (bouton « importer un relevé » de l'écran dépenses)"
            }
        } @else {
            form hx-post=(format!("/depenses/{}/reconcile", expense.id)) hx-target="#panel" hx-swap="innerHTML" {
                (form::select("transaction", "Débit du relevé", &borrowed, borrowed[0].0, None))
                (form::actions("Rapprocher"))
            }
        }
    };
    panel::sheet(&format!("Rapprocher « {} »", expense.label), body)
}

/// Le panneau de règlement d'un mouvement du relevé (lot 37) : le compte de bilan réglé, choisi
/// dans une liste en français, ou saisi pour un compte hors liste (avec son libellé).
pub fn settle_panel(
    tx: &BankTransaction,
    values: &SettleFormValues,
    error: Option<&str>,
) -> Markup {
    let body = html! {
        @if let Some(msg) = error {
            (form::error_banner(msg))
        }
        div class="detail-note" {
            "Mouvement du " (format_date(tx.occurred_on)) " : " (tx.description) ", "
            (Money::from_cents(tx.amount_cents)) ". "
            "Un règlement solde un compte du bilan sans passer par une charge : c'est le bon \
             geste pour une dette reprise du cabinet (honoraires, IS, TVA), un mouvement de \
             compte courant, un virement entre vos comptes. Une charge de l'exercice, elle, se \
             saisit en dépense."
        }
        form hx-post=(format!("/depenses/transaction/{}/settle", tx.id)) hx-target="#panel" hx-swap="innerHTML" {
            (form::select("account", "Compte réglé", &SETTLEMENT_OPTIONS, &values.account, None))
            (form::text("other_account", "Autre compte (numéro, classes 1 à 5)", &values.other_account, None))
            (form::text("label", "Libellé du compte (requis hors liste)", &values.label, None))
            (form::actions("Enregistrer le règlement"))
        }
    };
    panel::sheet("Régler un compte depuis le relevé", body)
}

/// Valeurs du formulaire de règlement.
#[derive(Default, Clone)]
pub struct SettleFormValues {
    pub account: String,
    pub other_account: String,
    pub label: String,
}

pub fn immobilize_panel(expense: &Expense, error: Option<&str>) -> Markup {
    let net = expense_net(expense);
    let duration = DEFAULT_DURATION_MONTHS.to_string();
    let body = html! {
        @if let Some(msg) = error {
            (form::error_banner(msg))
        }
        div class="detail-note" {
            "« " (expense.label) " » coûte " (net) " HT (TTC − TVA déductible), au-delà de la "
            "tolérance de " (SMALL_EQUIPMENT_THRESHOLD) ". Immobiliser remplace la charge par "
            "une entrée à l'actif et une dotation linéaire chaque exercice."
        }
        form hx-post=(format!("/depenses/{}/immobilize", expense.id)) hx-target="#panel" hx-swap="innerHTML" {
            (form::select("account", "Compte", &ASSET_ACCOUNT_OPTIONS, "218300", None))
            (form::number("duration", "Durée d'usage (mois)", &duration, "1", None))
            (form::field_help("36 mois pour du matériel informatique, 60 pour du mobilier."))
            (form::actions("Immobiliser"))
        }
    };
    panel::sheet("Immobiliser cette dépense", body)
}

pub fn asset_panel(asset: &FixedAsset, period_depreciation: Money, error: Option<&str>) -> Markup {
    let body = html! {
        @if let Some(msg) = error {
            (form::error_banner(msg))
        }
        div class="detail-head" {
            div class="detail-title" { (asset.label) }
            span class="badge" { (asset.account.as_str()) }
        }
        dl class="detail-fields" {
            dt { "Compte" } dd { (asset.account.as_str()) " — "
                (freeflow_core::domain::asset_account_label(asset.account.as_str())) }
            dt { "Amortissement" } dd class="mono" { (asset.depreciation_account().to_string()) }
            dt { "Mise en service" } dd { (format_date(asset.acquired_on)) }
            dt { "Base" } dd class="mono" { (asset.base) }
            dt { "Durée" } dd { (asset.duration_months) " mois" }
            dt { "Cumul repris" } dd class="mono" { (asset.prior_depreciation) }
            dt { "Dotation de l'exercice" } dd class="mono" { (period_depreciation) }
        }
        div class="detail-actions" {
            button class="btn danger" hx-post=(format!("/depenses/assets/{}/delete", asset.id)) hx-target="#panel" hx-swap="innerHTML" { "supprimer l'immobilisation" }
        }
    };
    panel::sheet(&asset.label, body)
}

pub fn delete_confirm_panel(expense: &Expense) -> Markup {
    let body = html! {
        div class="detail-note" {
            "Supprimer définitivement « " (expense.label) " » ("
            (expense.amount) ", " (format_date(expense.incurred_on))
            ") ? Cette action est irréversible — refusée si la dépense tombe dans un exercice "
            "déjà clôturé. Le justificatif archivé, lui, reste en place, et le débit du relevé "
            "rapproché, s'il y en a un, redevient « à rapprocher »."
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
    let assets = list_fixed_assets(store.connection())?;
    let to_immobilize: Vec<&Expense> = expenses
        .iter()
        .filter(|e| should_be_immobilized(e, &assets))
        .collect();
    let reconciled = reconciled_debits(store.connection())?;
    let debits = unmatched_debits(store.connection())?;
    // Les crédits non rapprochés se règlent aussi (remboursement de crédit de TVA, apport en
    // compte courant) ; un encaissement de facture, lui, reste `bank reconcile` (CLI/MCP).
    let all = list_bank_transactions(store.connection())?;
    let credits: Vec<&BankTransaction> = all
        .iter()
        .filter(|t| !t.is_debit() && !t.is_matched())
        .collect();
    let settled: Vec<&BankTransaction> = all.iter().filter(|t| t.is_settled()).collect();
    Ok(html! {
        div id="depenses-list"
            hx-get="/depenses/table"
            hx-trigger="freeflow:saved from:body"
            hx-target="this"
            hx-swap="outerHTML" {
            div class="pipe-toolbar" {
                button class="btn primary" hx-get="/depenses/new" hx-target="#panel" hx-swap="innerHTML" { "+ nouvelle dépense" }
                (crate::views::banque::import_button())
            }
            @if !to_immobilize.is_empty() {
                div class="panel bordered" style="padding:0;margin-bottom:12px" {
                    table {
                        tr {
                            th style="padding-left:18px" { "matériel à immobiliser (> 500 € HT)" }
                            th { "net HT" }
                            th style="padding-right:18px" { }
                        }
                        @for expense in &to_immobilize {
                            tr {
                                td style="padding-left:18px" { (expense.label) " — " (format_date(expense.incurred_on)) }
                                td class="mono" { (expense_net(expense)) }
                                td style="padding-right:18px" {
                                    button class="btn" hx-get=(format!("/depenses/{}/immobilize", expense.id)) hx-target="#panel" hx-swap="innerHTML" { "immobiliser" }
                                }
                            }
                        }
                    }
                }
            }
            @if !assets.is_empty() {
                div class="panel bordered" style="padding:0;margin-bottom:12px" {
                    table {
                        tr {
                            th style="padding-left:18px" { "immobilisation" }
                            th { "compte" }
                            th { "base" }
                            th style="padding-right:18px" { "mise en service" }
                        }
                        @for asset in &assets {
                            tr class="row-clickable" hx-get=(format!("/depenses/assets/{}", asset.id)) hx-target="#panel" hx-swap="innerHTML" {
                                td style="padding-left:18px" { (asset.label) }
                                td class="mono" { (asset.account.as_str()) }
                                td class="mono" { (asset.base) }
                                td style="padding-right:18px" class="mono" { (format_date(asset.acquired_on)) }
                            }
                        }
                    }
                }
            }
            @if !debits.is_empty() {
                div class="panel bordered" style="padding:0;margin-bottom:12px" {
                    table {
                        tr {
                            th style="padding-left:18px" { "débit du relevé à rapprocher" }
                            th { "libellé" }
                            th { "montant" }
                            th style="padding-right:18px" { }
                        }
                        @for tx in &debits {
                            tr {
                                td style="padding-left:18px" class="mono" { (format_date(tx.occurred_on)) }
                                td { (tx.description) }
                                td class="mono" { (Money::from_cents(-tx.amount_cents)) }
                                td style="padding-right:18px" {
                                    button class="btn" hx-get=(format!("/depenses/new?transaction={}", tx.id)) hx-target="#panel" hx-swap="innerHTML" title="Une charge : logiciel, honoraires, frais…" { "c'est une dépense" }
                                    " "
                                    button class="btn" hx-get=(format!("/depenses/transaction/{}/settle", tx.id)) hx-target="#panel" hx-swap="innerHTML" title="Ni charge ni produit : dette reprise au bilan, IS, TVA, compte courant, virement interne" { "c'est le règlement d'une dette ou d'un compte" }
                                }
                            }
                        }
                    }
                }
            }
            @if !credits.is_empty() {
                div class="panel bordered" style="padding:0;margin-bottom:12px" {
                    table {
                        tr {
                            th style="padding-left:18px" { "crédit du relevé à rapprocher" }
                            th { "libellé" }
                            th { "montant" }
                            th style="padding-right:18px" { }
                        }
                        @for tx in &credits {
                            tr {
                                td style="padding-left:18px" class="mono" { (format_date(tx.occurred_on)) }
                                td { (tx.description) }
                                td class="mono" { (Money::from_cents(tx.amount_cents)) }
                                td style="padding-right:18px" {
                                    button class="btn" hx-get=(format!("/depenses/transaction/{}/settle", tx.id)) hx-target="#panel" hx-swap="innerHTML" title="Remboursement d'un crédit de TVA, apport en compte courant, virement interne… (un règlement de facture se rapproche depuis l'écran facturation)" { "c'est le règlement d'un compte" }
                                }
                            }
                        }
                    }
                }
            }
            @if !settled.is_empty() {
                div class="panel bordered" style="padding:0;margin-bottom:12px" {
                    table {
                        tr {
                            th style="padding-left:18px" { "règlement de compte de bilan" }
                            th { "libellé" }
                            th { "montant" }
                            th { "compte réglé" }
                            th style="padding-right:18px" { }
                        }
                        @for tx in &settled {
                            tr {
                                td style="padding-left:18px" class="mono" { (format_date(tx.occurred_on)) }
                                td { (tx.description) }
                                td class="mono" { (Money::from_cents(tx.amount_cents)) }
                                td { (tx.settlement_account.as_ref().map_or(String::new(), ToString::to_string)) " " (tx.settlement_label.as_deref().unwrap_or_default()) }
                                td style="padding-right:18px" {
                                    button class="btn" hx-post=(format!("/depenses/transaction/{}/unsettle", tx.id)) hx-target="#panel" hx-swap="innerHTML" { "défaire" }
                                }
                            }
                        }
                    }
                }
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
                            th { "justificatif" }
                            th style="padding-right:18px" { "relevé" }
                        }
                        @for expense in &expenses {
                            tr class="row-clickable" hx-get=(format!("/depenses/{}", expense.id)) hx-target="#panel" hx-swap="innerHTML" {
                                td style="padding-left:18px" class="mono" { (format_date(expense.incurred_on)) }
                                td { (expense.label) }
                                td { (category_label(expense.category)) }
                                td class="mono" { (expense.amount) }
                                td class="mono" { (expense.vat_deductible) }
                                td {
                                    @if expense.receipt_hash.is_some() { span class="badge ok" { "oui" } }
                                    @else { "—" }
                                }
                                td style="padding-right:18px" class="mono" {
                                    @if let Some(tx) = reconciled.get(&expense.id) { (format_date(tx.occurred_on)) }
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

#[allow(dead_code)]
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

pub fn load(store: &Store, id: ExpenseId) -> Result<Option<ExpenseDetail>, AppError> {
    expense_detail(store.connection(), id)
}

/// Les débits restant à rapprocher au montant exact d'une dépense (lot 33).
/// Les débits du relevé rapprochables d'une dépense : même montant (la seule garde que le cœur
/// accepte), **le plus proche de la date de la dépense en premier** (lot 36) — c'est lui que le
/// `<select>` présélectionne, puisque `reconcile_panel` prend le premier candidat.
pub fn reconcile_candidates(
    store: &Store,
    expense: &Expense,
) -> Result<Vec<BankTransaction>, AppError> {
    let mut candidates: Vec<BankTransaction> = unmatched_debits(store.connection())?
        .into_iter()
        .filter(|tx| Money::from_cents(-tx.amount_cents) == expense.amount)
        .collect();
    candidates.sort_by_key(|tx| {
        (
            (tx.occurred_on - expense.incurred_on).whole_days().abs(),
            tx.occurred_on,
        )
    });
    Ok(candidates)
}

/// Sélectionne le taux par défaut du formulaire de création — le taux normal, celui de
/// l'écrasante majorité des dépenses d'un indépendant du numérique.
pub fn default_form_values(today: time::Date) -> ExpenseFormValues {
    ExpenseFormValues {
        category: "software".to_string(),
        vat_rate: VatRate::Standard.as_str().to_string(),
        incurred_on: format_date(today),
        ..Default::default()
    }
}
