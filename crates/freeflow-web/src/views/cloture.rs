//! Écran `cloture` — les exercices clos (lot 20), sur le patron des écrans du lot 15/16 :
//! liste auto-rafraîchie + panneau latéral pour clore, réviser l'affectation, approuver,
//! supprimer un projet, et télécharger les documents de clôture (`freeflow-docs`).

use freeflow_core::app::AppError;
use freeflow_core::domain::{FiscalYearEnd, format_date};
use freeflow_core::fiscal_year::{FiscalYearRecord, list_fiscal_years};
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

/// Pré-remplit la période avec le dernier exercice **écoulé** d'après la clôture récurrente du
/// profil (année civile à défaut) : le geste le plus probable est de clore l'exercice qui vient
/// de se terminer, pas celui en cours.
pub fn default_close_values(store: &Store, today: time::Date) -> CloseFormValues {
    let fiscal_year_end = freeflow_core::company::company_profile(store.connection())
        .ok()
        .flatten()
        .and_then(|p| p.fiscal_year_end)
        .unwrap_or(FiscalYearEnd::CALENDAR);
    let current = fiscal_year_end.current(today);
    let previous = fiscal_year_end.previous(current);
    CloseFormValues {
        starts_on: format_date(previous.start()),
        ends_on: format_date(previous.end()),
        legal_reserve: "0".to_string(),
        dividends: "0".to_string(),
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
                (form::field_help(
                    "Le résultat (CA, charges, IS) est recalculé et figé à la clôture ; le \
                     report à nouveau enchaîne sur l'exercice précédent. L'exercice reste un \
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
    let body = html! {
        form hx-post=(format!("/cloture/{}/approve", record.id)) hx-target="#panel" hx-swap="innerHTML" {
            (form::date("approved_on", "Date de l'AG d'approbation", &format_date(today), None))
            (form::field_help(
                "Une fois approuvé, l'exercice devient immuable : la décision d'AG fait foi. \
                 Générez et faites signer le PV avant d'approuver ici."
            ))
            (form::actions("Approuver l'exercice"))
        }
    };
    panel::sheet(
        &format!("Approuver — exercice {}", record.ends_on.year()),
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
            dt { "IS" } dd class="mono" { (record.corporate_tax) }
            dt { "Résultat net" } dd class="mono" { (record.net_result) }
            dt { "Réserve légale" } dd class="mono" { (record.legal_reserve) }
            dt { "Dividendes" } dd class="mono" { (record.dividends) }
            dt { "Report à nouveau" } dd class="mono" { (record.retained_earnings) }
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
                a class="btn small" href=(format!("/cloture/fec?period={}", record.ends_on.year())) target="_blank" { "FEC (txt)" }
            }
        }
    };
    panel::sheet(&format!("Exercice {}", record.ends_on.year()), body)
}

/// La liste, avec son rafraîchissement automatique sur `freeflow:saved` — même convention que
/// les autres écrans.
pub fn list_fragment(store: &Store) -> Result<Markup, AppError> {
    let years = list_fiscal_years(store.connection())?;
    Ok(html! {
        div id="cloture-list"
            hx-get="/cloture/table"
            hx-trigger="freeflow:saved from:body"
            hx-target="this"
            hx-swap="outerHTML" {
            div class="pipe-toolbar" {
                button class="btn primary" hx-get="/cloture/new" hx-target="#panel" hx-swap="innerHTML" { "+ clore un exercice" }
                form method="get" action="/cloture/fec" target="_blank" style="display:inline-flex;gap:6px;align-items:center;margin-left:auto" title="Fichier des Écritures Comptables de l'exercice clos dans cette année civile (clos ou non)" {
                    label { "FEC de l'exercice clos en " }
                    input type="number" name="period" value=(time::OffsetDateTime::now_utc().year()) min="2000" max="2100" style="width:6em" {}
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

pub fn render(store: &Store) -> Result<Markup, AppError> {
    let count = list_fiscal_years(store.connection())?.len();
    Ok(html! {
        (view_head(ViewId::Cloture, &format!("{count} exercice(s) clos")))
        (list_fragment(store)?)
    })
}
