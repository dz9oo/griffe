//! Écran `cloture` — les exercices clos (lot 20), sur le patron des écrans du lot 15/16 :
//! liste auto-rafraîchie + panneau latéral pour clore, réviser l'affectation, approuver,
//! supprimer un projet, et télécharger les documents de clôture (`freeflow-docs`). Depuis le
//! lot 31, un panneau « bilan » montre la balance des comptes et le bilan 2033-A dérivés du
//! grand livre, pour un exercice clos ou non. Depuis le lot 34, le panneau « parcours de
//! clôture » (`checklist_panel`) déroule les étapes calculées par `freeflow_core::closing`, avec
//! pour chacune le bouton de cette façade qui y répond.

use freeflow_core::app::AppError;
use freeflow_core::closing::{
    ClosingChecklist, ClosingPhase, ClosingStep, ClosingStepKey, GLOSSARY, StepStatus,
};
use freeflow_core::domain::Money;
use freeflow_core::domain::Side;
use freeflow_core::domain::{FiscalYearEnd, format_date};
use freeflow_core::fiscal_year::{FiscalYearRecord, list_fiscal_years};
use freeflow_core::ledger::{BalanceSheet, TrialBalance};
use freeflow_core::opening_balance::OpeningBalanceRecord;
use freeflow_core::opening_balance::opening_balance;
use freeflow_core::store::Store;
use maud::{Markup, html};

use crate::layout::{ViewId, view_head};
use crate::views::{form, panel};

/// Valeurs du formulaire de clôture — voir `ClientFormValues` (lot 15) pour la doctrine.
#[derive(Default, Clone)]
pub struct CloseFormValues {
    pub starts_on: String,
    pub ends_on: String,
    pub legal_reserve: String,
    pub dividends: String,
    /// Option de report en arrière du déficit (lot 32).
    pub carry_back: bool,
}

#[derive(Default)]
pub struct CloseFormErrors {
    pub starts_on: Option<String>,
    pub ends_on: Option<String>,
    pub legal_reserve: Option<String>,
    pub dividends: Option<String>,
    pub banner: Option<String>,
    pub conflict: Option<(String, String)>,
}

/// La clôture récurrente du profil, année civile à défaut.
fn fiscal_year_end_of(store: &Store) -> FiscalYearEnd {
    freeflow_core::company::company_profile(store.connection())
        .ok()
        .flatten()
        .and_then(|p| p.fiscal_year_end)
        .unwrap_or(FiscalYearEnd::CALENDAR)
}

/// Pré-remplit la période à clore (lot 36) : celle qui **suit le dernier exercice clos** dans
/// l'application (lendemain de sa fin, un an moins un jour), sinon celle qui **s'ouvre sur le
/// bilan d'ouverture** (à sa date, jusqu'à la clôture récurrente suivante), sinon le dernier
/// exercice écoulé d'après le profil — le geste le plus probable est de clore l'exercice qui
/// vient de se terminer, pas celui en cours, et jamais un exercice qui laisserait un trou.
pub fn default_close_values(store: &Store, today: time::Date) -> CloseFormValues {
    let fiscal_year_end = fiscal_year_end_of(store);
    let after_last_closed = list_fiscal_years(store.connection())
        .ok()
        .and_then(|years| years.iter().map(|y| y.ends_on).max())
        .and_then(|end| end.next_day())
        .map(|start| {
            let end = fiscal_year_end.containing(start).end();
            let end = if end <= start {
                fiscal_year_end
                    .containing(start.saturating_add(time::Duration::days(1)))
                    .end()
            } else {
                end
            };
            (start, end)
        });
    let from_opening = || {
        opening_balance(store.connection()).ok().flatten().map(|o| {
            let start = o.balance.opens_on;
            (start, fiscal_year_end.containing(start).end())
        })
    };
    let (starts_on, ends_on) = after_last_closed.or_else(from_opening).unwrap_or_else(|| {
        let previous = fiscal_year_end.previous(fiscal_year_end.current(today));
        (previous.start(), previous.end())
    });
    CloseFormValues {
        starts_on: format_date(starts_on),
        ends_on: format_date(ends_on),
        legal_reserve: "0".to_string(),
        dividends: "0".to_string(),
        carry_back: false,
    }
}

fn close_form(action: &str, values: &CloseFormValues, errors: &CloseFormErrors) -> Markup {
    html! {
        form hx-post=(action) hx-target="#panel" hx-swap="innerHTML" {
            @if let Some((message, reload)) = &errors.conflict {
                (form::conflict_banner(message, reload))
            } @else {
                @if let Some(msg) = &errors.banner {
                    (form::error_banner(msg))
                }
                (form::date("starts_on", "Début de l'exercice", &values.starts_on, errors.starts_on.as_deref()))
                (form::date("ends_on", "Fin de l'exercice", &values.ends_on, errors.ends_on.as_deref()))
                (form::number("legal_reserve", "Dotation à la réserve légale (€)", &values.legal_reserve, "0.01", errors.legal_reserve.as_deref()))
                (form::number("dividends", "Dividendes distribués (€)", &values.dividends, "0.01", errors.dividends.as_deref()))
                (form::checkbox_checked("carry_back", "Reporter le déficit en arrière (art. 220 quinquies CGI)", values.carry_back))
                (form::field_help(
                    "Le résultat (CA, charges, IS) est recalculé et figé à la clôture ; le \
                     report à nouveau enchaîne sur l'exercice précédent, et les déficits \
                     fiscaux antérieurs s'imputent d'abord sur le bénéfice. Le report en \
                     arrière impute le déficit de l'exercice sur le bénéfice de l'exercice \
                     précédent clos ici, contre une créance d'IS ; refusé sans déficit, sans \
                     exercice précédent ou sans bénéfice d'imputation. L'exercice reste un \
                     projet éditable jusqu'à son approbation."
                ))
                (form::actions("Clore l'exercice"))
            }
        }
    }
}

pub fn new_panel(values: &CloseFormValues, errors: &CloseFormErrors) -> Markup {
    panel::sheet("Clore un exercice", close_form("/cloture", values, errors))
}

/// Formulaire de révision d'affectation d'un projet — seuls réserve et dividendes sont
/// éditables, le snapshot est figé (voir `fiscal_year::UpdateFiscalYearAppropriation`).
pub struct AmendFormValues {
    pub legal_reserve: String,
    pub dividends: String,
}

/// Valeur d'un `<input type="number">` en euros (`1234.56`) depuis des centimes — sans passer
/// par un flottant. Les montants d'affectation sont non négatifs par validation du cœur.
fn euros_input_value(m: freeflow_core::domain::Money) -> String {
    format!("{}.{:02}", m.cents() / 100, m.cents() % 100)
}

impl From<&FiscalYearRecord> for AmendFormValues {
    fn from(r: &FiscalYearRecord) -> Self {
        Self {
            legal_reserve: euros_input_value(r.legal_reserve),
            dividends: euros_input_value(r.dividends),
        }
    }
}

pub fn edit_panel(
    record: &FiscalYearRecord,
    values: &AmendFormValues,
    errors: &CloseFormErrors,
) -> Markup {
    let body = html! {
        form hx-post=(format!("/cloture/{}", record.id)) hx-target="#panel" hx-swap="innerHTML" {
            @if let Some((message, reload)) = &errors.conflict {
                (form::conflict_banner(message, reload))
            } @else {
                @if let Some(msg) = &errors.banner {
                    (form::error_banner(msg))
                }
                (form::hidden("revision", &record.revision.to_string()))
                (form::number("legal_reserve", "Dotation à la réserve légale (€)", &values.legal_reserve, "0.01", errors.legal_reserve.as_deref()))
                (form::number("dividends", "Dividendes distribués (€)", &values.dividends, "0.01", errors.dividends.as_deref()))
                (form::field_help("Le snapshot du résultat reste figé — seule l'affectation change."))
                (form::actions("Enregistrer"))
            }
        }
    };
    panel::sheet(
        &format!("Affectation — exercice {}", record.ends_on.year()),
        body,
    )
}

pub fn approve_panel(record: &FiscalYearRecord, today: time::Date) -> Markup {
    let today = format_date(today);
    let body = html! {
        form hx-post=(format!("/cloture/{}/approve", record.id)) hx-target="#panel" hx-swap="innerHTML" {
            (form::date_max("approved_on", "Date de la décision d'approbation", &today, &today, None))
            (form::field_help(
                "Une fois approuvé, l'exercice devient immuable : la décision de l'associé \
                 unique fait foi, et elle se date du jour où elle est prise (jamais dans le \
                 futur). Une sauvegarde du coffre est écrite juste avant — c'est le seul retour \
                 en arrière possible. Générez et signez le PV avant d'approuver ici."
            ))
            (form::actions("Approuver l'exercice"))
        }
    };
    panel::sheet(
        &format!("Approuver — exercice {}", record.ends_on.year()),
        body,
    )
}

/// Ce que la fenêtre montre après une approbation réussie (lot 36) : où est la sauvegarde
/// préalable, et le retard éventuel sur l'échéance légale — des informations qu'un panneau
/// vide (la convention « succès → `200` vide ») ne pourrait pas porter.
pub fn approved_panel(
    record: &FiscalYearRecord,
    backup: &std::path::Path,
    late_by_days: Option<u32>,
) -> Markup {
    let body = html! {
        @if let Some(days) = late_by_days {
            (form::error_banner(&format!(
                "Approbation {days} jour(s) après l'échéance légale de six mois : le dépôt des \
                 comptes au greffe est lui aussi en retard, faites-le sans attendre."
            )))
        }
        div class="detail-note" {
            "Exercice du " (format_date(record.starts_on)) " au " (format_date(record.ends_on))
            " approuvé : il est désormais immuable."
        }
        div class="detail-note" {
            "Sauvegarde préalable : " code { (backup.display()) }
        }
        div class="form-actions" {
            button class="btn" hx-get=(format!("/cloture/{}", record.id)) hx-target="#panel" hx-swap="innerHTML" { "voir l'exercice et ses documents" }
        }
    };
    panel::sheet(
        &format!("Exercice {} approuvé", record.ends_on.year()),
        body,
    )
}

pub fn delete_confirm_panel(record: &FiscalYearRecord) -> Markup {
    let body = html! {
        div class="detail-note" {
            "Supprimer le projet de clôture de l'exercice du " (format_date(record.starts_on))
            " au " (format_date(record.ends_on)) " ? Le snapshot et l'affectation saisie seront "
            "perdus — les factures et dépenses de la période, elles, ne bougent pas."
        }
        div class="form-actions" {
            // Révision relue côté serveur au moment du clic — même raison que la suppression
            // d'un client (lot 15).
            button class="btn danger" hx-post=(format!("/cloture/{}/delete", record.id)) hx-target="#panel" hx-swap="innerHTML" {
                "confirmer la suppression"
            }
        }
    };
    panel::sheet("Confirmer la suppression", body)
}

fn status_badge(record: &FiscalYearRecord) -> Markup {
    html! {
        @if let Some(approved_on) = record.approved_on {
            span class="badge ok" { "approuvé le " (format_date(approved_on)) }
        } @else {
            span class="badge warn" { "projet" }
        }
    }
}

pub fn detail_panel(record: &FiscalYearRecord, editable: bool, error: Option<&str>) -> Markup {
    let id = record.id;
    let body = html! {
        @if let Some(msg) = error {
            (form::error_banner(msg))
        }
        div class="detail-head" {
            div class="detail-title" {
                "Exercice du " (format_date(record.starts_on)) " au " (format_date(record.ends_on))
            }
            (status_badge(record))
        }
        dl class="detail-fields" {
            dt { "CA HT" } dd class="mono" { (record.revenue_ht) }
            dt { "Charges externes" } dd class="mono" { (record.expenses) }
            dt { "Rémunération dirigeant" } dd class="mono" { (record.director_remuneration) }
            dt { "Résultat avant IS" } dd class="mono" { (record.result_before_tax) }
            dt { "Déficits antérieurs imputés" } dd class="mono" { (record.losses_imputed) }
            dt { "Résultat fiscal" } dd class="mono" { (record.taxable_result()) }
            dt { "IS" } dd class="mono" { (record.corporate_tax) }
            @if !record.carried_back.is_zero() {
                dt { "Déficit reporté en arrière" } dd class="mono" { (record.carried_back) }
                dt { "Créance de report en arrière" } dd class="mono" { (record.carry_back_credit) }
            }
            dt { "Résultat net" } dd class="mono" { (record.net_result) }
            dt { "Réserve légale" } dd class="mono" { (record.legal_reserve) }
            dt { "Dividendes" } dd class="mono" { (record.dividends) }
            dt { "Report à nouveau" } dd class="mono" { (record.retained_earnings) }
            dt { "Déficits reportables en avant" } dd class="mono" { (record.losses_carried_forward) }
        }

        @if editable {
            div class="detail-actions" {
                button class="btn" hx-get=(format!("/cloture/{id}/edit")) hx-target="#panel" hx-swap="innerHTML" { "réviser l'affectation" }
                button class="btn primary" hx-get=(format!("/cloture/{id}/approve")) hx-target="#panel" hx-swap="innerHTML" { "approuver" }
                button class="btn danger" hx-get=(format!("/cloture/{id}/delete")) hx-target="#panel" hx-swap="innerHTML" { "supprimer" }
            }
        } @else if record.approved_on.is_none() {
            div class="detail-note" {
                "Un exercice plus récent a hérité du report à nouveau de celui-ci : il n'est \
                 plus ni éditable ni supprimable."
            }
        }

        div class="detail-section" {
            div class="detail-section-head" { span { "Documents de clôture" } }
            div class="detail-note" {
                "Modèles générés depuis les données saisies — à faire relire avant signature ou \
                 transmission."
            }
            div class="detail-actions" {
                a class="btn small" href=(format!("/cloture/{id}/doc/minutes")) target="_blank" { "PV d'approbation (PDF)" }
                a class="btn small" href=(format!("/cloture/{id}/doc/appropriation")) target="_blank" { "affectation (PDF)" }
                a class="btn small" href=(format!("/cloture/{id}/doc/synthesis")) target="_blank" { "compte de résultat (PDF)" }
                a class="btn small" href=(format!("/cloture/{id}/doc/liasse")) target="_blank" { "liasse (JSON)" }
                a class="btn small" href=(format!("/cloture/balance.pdf?period={}", record.ends_on.year())) target="_blank" { "bilan et balance (PDF)" }
                a class="btn small" href=(format!("/cloture/fec?period={}", record.ends_on.year())) target="_blank" { "FEC (txt)" }
            }
            div class="detail-actions" {
                button class="btn small" hx-get=(format!("/cloture/balance?period={}", record.ends_on.year())) hx-target="#panel" hx-swap="innerHTML" { "voir le bilan et la balance" }
                button class="btn small" hx-get=(format!("/cloture/checklist?period={}", record.ends_on.year())) hx-target="#panel" hx-swap="innerHTML" { "parcours de clôture" }
            }
        }
    };
    panel::sheet(&format!("Exercice {}", record.ends_on.year()), body)
}

/// L'année proposée par défaut partout dans la barre (parcours, bilan, FEC — lot 36) : celle de
/// la clôture du dernier exercice **écoulé** d'après le profil — le geste le plus probable,
/// et jamais un exercice en cours dont le bilan serait incomplet.
fn default_period(store: &Store, today: time::Date) -> i32 {
    let fiscal_year_end = fiscal_year_end_of(store);
    fiscal_year_end
        .previous(fiscal_year_end.current(today))
        .end()
        .year()
}

/// La liste, avec son rafraîchissement automatique sur `freeflow:saved` — même convention que
/// les autres écrans.
pub fn list_fragment(store: &Store, today: time::Date) -> Result<Markup, AppError> {
    let years = list_fiscal_years(store.connection())?;
    let period = default_period(store, today);
    Ok(html! {
        div id="cloture-list"
            hx-get="/cloture/table"
            hx-trigger="freeflow:saved from:body"
            hx-target="this"
            hx-swap="outerHTML" {
            div class="pipe-toolbar" {
                button class="btn primary" hx-get="/cloture/new" hx-target="#panel" hx-swap="innerHTML" { "+ clore un exercice" }
                button class="btn" hx-get="/cloture/opening" hx-target="#panel" hx-swap="innerHTML" title="Reprise du dernier bilan tenu avant FreeFlow (expert-comptable)" { "bilan d'ouverture" }
                form hx-get="/cloture/checklist" hx-target="#panel" hx-swap="innerHTML" style="display:inline-flex;gap:6px;align-items:center;margin-left:auto" title="Parcours de clôture guidé : ce qui bloque, ce qui mérite attention, ce qui reste à faire" {
                    label { "parcours de l'exercice clos en " }
                    input type="number" name="period" value=(period) min="2000" max="2100" style="width:6em" {}
                    button class="btn small primary" type="submit" { "parcours" }
                }
                form hx-get="/cloture/balance" hx-target="#panel" hx-swap="innerHTML" style="display:inline-flex;gap:6px;align-items:center" title="Balance des comptes et bilan 2033-A dérivés du grand livre de l'exercice clos dans cette année civile (clos ou non)" {
                    label { "exercice clos en " }
                    input type="number" name="period" value=(period) min="2000" max="2100" style="width:6em" {}
                    button class="btn small" type="submit" { "bilan" }
                }
                form method="get" action="/cloture/fec" target="_blank" style="display:inline-flex;gap:6px;align-items:center" title="Fichier des Écritures Comptables de l'exercice clos dans cette année civile (clos ou non)" {
                    label { "FEC de l'exercice clos en " }
                    input type="number" name="period" value=(period) min="2000" max="2100" style="width:6em" {}
                    button class="btn small" type="submit" { "exporter" }
                }
            }
            @if years.is_empty() {
                div class="empty-state" { "aucun exercice clos — cliquez sur « clore un exercice »" }
            } @else {
                div class="panel bordered" style="padding:0" {
                    table {
                        tr {
                            th style="padding-left:18px" { "exercice" }
                            th { "résultat net" }
                            th { "dividendes" }
                            th { "report" }
                            th style="padding-right:18px" { "statut" }
                        }
                        @for year in years.iter().rev() {
                            tr class="row-clickable" hx-get=(format!("/cloture/{}", year.id)) hx-target="#panel" hx-swap="innerHTML" {
                                td style="padding-left:18px" { (format_date(year.starts_on)) " → " (format_date(year.ends_on)) }
                                td class="mono" { (year.net_result) }
                                td class="mono" { (year.dividends) }
                                td class="mono" { (year.retained_earnings) }
                                td style="padding-right:18px" { (status_badge(year)) }
                            }
                        }
                    }
                }
            }
        }
    })
}

pub fn render(store: &Store, today: time::Date) -> Result<Markup, AppError> {
    let count = list_fiscal_years(store.connection())?.len();
    Ok(html! {
        (view_head(ViewId::Cloture, &format!("{count} exercice(s) clos")))
        (list_fragment(store, today)?)
    })
}

// -- Parcours de clôture guidé (lot 34) -----------------------------------------------------

const fn status_badge_class(status: StepStatus) -> &'static str {
    match status {
        StepStatus::Done => "badge ok",
        StepStatus::Todo => "badge",
        StepStatus::Warning => "badge warn",
        StepStatus::Blocked => "badge danger",
        StepStatus::Info => "badge info",
        StepStatus::Later => "badge muted",
    }
}

const fn status_label(status: StepStatus) -> &'static str {
    match status {
        StepStatus::Done => "fait",
        StepStatus::Todo => "à faire",
        StepStatus::Warning => "attention",
        StepStatus::Blocked => "bloquant",
        StepStatus::Info => "info",
        StepStatus::Later => "plus tard",
    }
}

const fn stage_badge_class(stage: freeflow_core::closing::ClosingStage) -> &'static str {
    use freeflow_core::closing::ClosingStage;
    match stage {
        ClosingStage::Approved | ClosingStage::Ready => "badge ok",
        ClosingStage::Draft | ClosingStage::NotEnded => "badge warn",
        ClosingStage::Blocked => "badge danger",
    }
}

/// Le bouton de cette façade qui répond à une étape, quand il y en a un — l'équivalent de la
/// commande suggérée par la CLI (`freeflow-cli::year::cli_hint`), branché sur la clé stable.
fn step_action(checklist: &ClosingChecklist, step: &ClosingStep) -> Markup {
    let period = checklist.period;
    let id = checklist.fiscal_year.as_ref().map(|r| r.id);
    let actionable = matches!(
        step.status,
        StepStatus::Todo | StepStatus::Warning | StepStatus::Blocked
    );
    if !actionable {
        return html! {};
    }
    let panel_button = |href: String, label: &str| {
        html! { button class="btn small" hx-get=(href) hx-target="#panel" hx-swap="innerHTML" { (label) } }
    };
    let nav_link = |href: &str, label: &str| {
        html! { a class="btn small" href=(href) { (label) } }
    };
    match step.key {
        ClosingStepKey::Profile => nav_link(
            "/view/console",
            "renseigner le profil (console : company set-profile)",
        ),
        ClosingStepKey::OpeningBalance => {
            panel_button("/cloture/opening".to_string(), "bilan d'ouverture")
        }
        ClosingStepKey::PreviousYear => nav_link("/view/cloture", "voir les exercices"),
        ClosingStepKey::Invoices => nav_link("/view/facturation", "facturation"),
        ClosingStepKey::Expenses | ClosingStepKey::Bank => {
            nav_link("/view/depenses", "dépenses et relevé")
        }
        ClosingStepKey::Result | ClosingStepKey::BalanceSheet => panel_button(
            format!("/cloture/balance?period={period}"),
            "bilan et balance",
        ),
        ClosingStepKey::Close if step.status == StepStatus::Todo => {
            let reserve = checklist
                .minimum_legal_reserve
                .map_or_else(|| "0".to_string(), euros_input_value);
            panel_button(
                format!(
                    "/cloture/new?starts_on={}&ends_on={}&legal_reserve={reserve}",
                    format_date(checklist.exercise.start()),
                    format_date(checklist.exercise.end())
                ),
                "clore l'exercice",
            )
        }
        ClosingStepKey::Appropriation => id.map_or_else(
            || html! {},
            |id| panel_button(format!("/cloture/{id}/edit"), "réviser l'affectation"),
        ),
        ClosingStepKey::Approve => id.map_or_else(
            || html! {},
            |id| panel_button(format!("/cloture/{id}/approve"), "approuver"),
        ),
        ClosingStepKey::Documents | ClosingStepKey::Liasse => id.map_or_else(
            || html! {},
            |id| panel_button(format!("/cloture/{id}"), "documents de l'exercice"),
        ),
        ClosingStepKey::Close
        | ClosingStepKey::PeriodEnded
        | ClosingStepKey::CorporateTax
        | ClosingStepKey::Filing => html! {},
    }
}

/// Le parcours de clôture : un en-tête (exercice, stade), puis une section par phase et une
/// ligne par étape — statut, titre, échéance, détail, et le bouton qui y répond.
pub fn checklist_panel(checklist: &ClosingChecklist) -> Markup {
    let exercise = checklist.exercise;
    let body = html! {
        div class="detail-head" {
            div class="detail-title" {
                "Exercice du " (format_date(exercise.start())) " au " (format_date(exercise.end()))
                " — vu le " (format_date(checklist.today))
            }
            span class=(stage_badge_class(checklist.stage)) { (checklist.stage.label()) }
        }
        div class="detail-note" {
            "Les étapes sont calculées depuis le coffre ; rien n'est écrit. « Bloquant » : la \
             clôture serait refusée ou fausse ; « attention » : à regarder, sans empêcher ; \
             « plus tard » : pas encore atteignable."
        }
        @for phase in ClosingPhase::ALL {
            div class="detail-section" {
                div class="detail-section-head" { span { (phase.label()) } }
                @for step in checklist.steps_in(phase) {
                    div class="checklist-step" {
                        div class="checklist-step-head" {
                            span class=(status_badge_class(step.status)) { (status_label(step.status)) }
                            strong { (step.title) }
                            @if let Some(due) = step.due_on {
                                span class="checklist-due" { "échéance " (format_date(due)) }
                            }
                        }
                        div class="checklist-detail" { (step.detail) }
                        div class="detail-actions" { (step_action(checklist, step)) }
                    }
                }
            }
        }
        @if let Some(id) = checklist.fiscal_year.as_ref().map(|r| r.id) {
            div class="detail-actions" {
                button class="btn small" hx-get=(format!("/cloture/{id}")) hx-target="#panel" hx-swap="innerHTML" { "fiche de l'exercice" }
            }
        }
        details class="glossary" {
            summary { "Lexique — les mots de la clôture, sans jargon" }
            dl class="glossary-list" {
                @for entry in GLOSSARY {
                    dt { (entry.term) }
                    dd { (entry.meaning) }
                }
            }
        }
    };
    panel::sheet(&format!("Parcours de clôture {}", checklist.period), body)
}

// -- Bilan et balance dérivés (lot 31) -------------------------------------------------------

fn money_cell(amount: Money) -> Markup {
    html! { td class="num" { @if !amount.is_zero() { (amount) } } }
}

/// Le bilan 2033-A (actif brut/amortissements/net, passif) puis la balance des comptes, avec le
/// lien vers le même document en PDF.
pub fn balance_panel(period: i32, balance: &TrialBalance, sheet: &BalanceSheet) -> Markup {
    let body = html! {
        div class="detail-head" {
            div class="detail-title" {
                "Bilan au " (format_date(sheet.exercise.end()))
                " — exercice du " (format_date(sheet.exercise.start()))
                " au " (format_date(sheet.exercise.end()))
            }
            @if sheet.is_balanced() {
                span class="badge ok" { "équilibré" }
            } @else {
                span class="badge danger" { "déséquilibré" }
            }
        }
        div class="detail-note" {
            "Dérivé du grand livre : à-nouveaux, factures et avoirs, encaissements, dépenses, \
             rémunération du dirigeant réputée due et IS. Présentation 2033-A, sans \
             amortissement de l'exercice, provision ni régularisation — à faire relire par \
             l'expert-comptable."
        }
        div class="detail-section" {
            div class="detail-section-head" { span { "Actif" } }
            div class="panel bordered" style="padding:0" {
                table {
                    tr { th { "Case" } th { "Rubrique" } th { "Brut" } th { "Amort." } th { "Net" } }
                    @for a in &sheet.assets {
                        @if !a.gross.is_zero() || !a.depreciation.is_zero() {
                            tr { td class="mono" { (a.case_gross) } td { (a.label) } (money_cell(a.gross)) (money_cell(a.depreciation)) (money_cell(a.net)) }
                        }
                    }
                    tr { td class="mono" { "110/112" } td { b { "Total général" } } (money_cell(sheet.total_assets_gross)) (money_cell(sheet.total_depreciation)) td class="num" { b { (sheet.total_assets_net) } } }
                }
            }
        }
        div class="detail-section" {
            div class="detail-section-head" { span { "Passif" } }
            div class="panel bordered" style="padding:0" {
                table {
                    tr { th { "Case" } th { "Rubrique" } th { "Montant" } }
                    @for l in &sheet.liabilities {
                        @if !l.amount.is_zero() {
                            tr { td class="mono" { (l.case) } td { (l.label) } (money_cell(l.amount)) }
                        }
                    }
                    tr { td class="mono" { "142" } td { "Total I — capitaux propres" } (money_cell(sheet.total_equity)) }
                    tr { td class="mono" { "180" } td { b { "Total général" } } td class="num" { b { (sheet.total_liabilities) } } }
                }
            }
        }
        div class="detail-section" {
            div class="detail-section-head" { span { "Balance des comptes" } }
            div class="panel bordered" style="padding:0" {
                table {
                    tr { th { "Compte" } th { "Libellé" } th { "Débit" } th { "Crédit" } th { "Solde" } }
                    @for r in &balance.rows {
                        tr { td class="mono" { (r.account.number) } td { (r.account.label) } (money_cell(r.debit)) (money_cell(r.credit)) td class="num" { (r.balance) } }
                    }
                    tr { td {} td { b { "Totaux" } } td class="num" { b { (balance.total_debit) } } td class="num" { b { (balance.total_credit) } } td {} }
                }
            }
        }
        div class="detail-actions" {
            a class="btn small" href=(format!("/cloture/balance.pdf?period={period}")) target="_blank" { "bilan et balance (PDF)" }
            a class="btn small" href=(format!("/cloture/fec?period={period}")) target="_blank" { "FEC (txt)" }
        }
    };
    panel::sheet(&format!("Bilan {}", sheet.exercise.end().year()), body)
}

// -- Bilan d'ouverture (lot 30) -------------------------------------------------------------

/// Valeurs du formulaire de bilan d'ouverture : les lignes voyagent en texte, une par ligne du
/// textarea, dans la syntaxe `compte:libellé:D|C:montant` du domaine — même raison que les
/// jalons (lot 16) : `axum::Form` ne garde que la dernière valeur d'une clé répétée.
#[derive(Default, Clone)]
pub struct OpeningFormValues {
    pub opens_on: String,
    pub source: String,
    pub lines: String,
    /// Déficits fiscaux antérieurs reportables, en euros (lot 32).
    pub tax_losses: String,
}

impl From<&OpeningBalanceRecord> for OpeningFormValues {
    fn from(r: &OpeningBalanceRecord) -> Self {
        Self {
            opens_on: format_date(r.balance.opens_on),
            source: r.balance.source.clone().unwrap_or_default(),
            lines: r
                .balance
                .lines
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
            tax_losses: r.balance.tax_losses.to_decimal_string(),
        }
    }
}

#[derive(Default)]
pub struct OpeningFormErrors {
    pub opens_on: Option<String>,
    pub lines: Option<String>,
    pub tax_losses: Option<String>,
    pub banner: Option<String>,
    pub conflict: Option<(String, String)>,
}

/// Formulaire de saisie/modification : `revision` présent = remplacement de l'existant.
pub fn opening_form_panel(
    values: &OpeningFormValues,
    errors: &OpeningFormErrors,
    revision: Option<i64>,
) -> Markup {
    let body = html! {
        form hx-post="/cloture/opening" hx-target="#panel" hx-swap="innerHTML" {
            @if let Some((message, reload)) = &errors.conflict {
                (form::conflict_banner(message, reload))
            } @else {
                @if let Some(msg) = &errors.banner {
                    (form::error_banner(msg))
                }
                @if let Some(revision) = revision {
                    (form::hidden("revision", &revision.to_string()))
                }
                (form::date("opens_on", "Premier jour de l'exercice qui s'ouvre sur ce bilan", &values.opens_on, errors.opens_on.as_deref()))
                (form::text("source", "Provenance (facultatif)", &values.source, None))
                (form::textarea("lines", "Lignes de la balance", &values.lines, 12, errors.lines.as_deref()))
                (form::field_help(
                    "Une ligne par compte : compte:libellé:D|C:montant — ex. \
                     101000:Capital social:C:1000.00 puis 512000:Banque:D:1000.00. Comptes de bilan \
                     (classes 1 à 5) seulement ; le total des débits doit égaler celui des crédits. \
                     Reprenez la balance de clôture de l'expert-comptable, après affectation du \
                     résultat (un résultat encore en 120/129 est réputé affecté en report à nouveau)."
                ))
                (form::number("tax_losses", "Déficits fiscaux antérieurs reportables (€)", &values.tax_losses, "0.01", errors.tax_losses.as_deref()))
                (form::field_help(
                    "Hors bilan : le total des déficits restant à reporter (case 870 du dernier \
                     tableau 2033-D déposé), imputé sur les bénéfices des exercices clos ici. \
                     Zéro si aucun."
                ))
                (form::actions(if revision.is_some() { "Remplacer le bilan d'ouverture" } else { "Enregistrer le bilan d'ouverture" }))
            }
        }
    };
    panel::sheet("Bilan d'ouverture", body)
}

/// Fiche du bilan enregistré : lignes, totaux, capitaux propres repris, actions.
pub fn opening_detail_panel(record: &OpeningBalanceRecord, frozen_by: Option<&str>) -> Markup {
    let equity = record.equity();
    let body = html! {
        div class="detail-head" {
            div class="detail-title" { "Bilan d'ouverture au " (format_date(record.balance.opens_on)) }
            @if frozen_by.is_some() {
                span class="badge ok" { "figé" }
            } @else {
                span class="badge warn" { "modifiable" }
            }
        }
        @if let Some(source) = &record.balance.source {
            div class="detail-note" { (source) }
        }
        div class="panel bordered" style="padding:0" {
            table {
                tr { th { "compte" } th { "libellé" } th { "débit" } th { "crédit" } }
                @for l in &record.balance.lines {
                    tr {
                        td class="mono" { (l.account) }
                        td { (l.label) }
                        td class="mono" { @if l.side == Side::Debit { (l.amount) } }
                        td class="mono" { @if l.side == Side::Credit { (l.amount) } }
                    }
                }
                tr {
                    td {} td { "totaux" }
                    td class="mono" { (record.balance.total_debit()) }
                    td class="mono" { (record.balance.total_credit()) }
                }
            }
        }
        dl class="detail-fields" {
            dt { "Capital repris" } dd class="mono" { (equity.share_capital) }
            dt { "Réserve légale reprise" } dd class="mono" { (equity.legal_reserve) }
            dt { "Report à nouveau repris" } dd class="mono" { (equity.retained_earnings) }
            dt { "Déficits fiscaux reportables repris" } dd class="mono" { (record.balance.tax_losses) }
        }
        @if let Some(period) = frozen_by {
            div class="detail-note" {
                "Un exercice est clos dans l'application (" (period) ") et a hérité de ce bilan : \
                 il n'est plus ni modifiable ni supprimable tant que ce projet de clôture existe."
            }
        } @else {
            div class="detail-actions" {
                button class="btn" hx-get="/cloture/opening/edit" hx-target="#panel" hx-swap="innerHTML" { "modifier" }
                button class="btn danger" hx-get="/cloture/opening/delete" hx-target="#panel" hx-swap="innerHTML" { "supprimer" }
            }
        }
    };
    panel::sheet("Bilan d'ouverture", body)
}

pub fn opening_delete_panel(record: &OpeningBalanceRecord) -> Markup {
    let body = html! {
        div class="detail-note" {
            "Supprimer le bilan d'ouverture au " (format_date(record.balance.opens_on))
            " ? Le report à nouveau et la réserve légale repris repartiront de zéro, et le FEC du \
             premier exercice n'aura plus d'à-nouveaux."
        }
        div class="form-actions" {
            button class="btn danger" hx-post="/cloture/opening/delete" hx-target="#panel" hx-swap="innerHTML" {
                "confirmer la suppression"
            }
        }
    };
    panel::sheet("Confirmer la suppression", body)
}
