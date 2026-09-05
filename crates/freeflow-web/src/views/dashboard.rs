//! Écran `dashboard` : ce qu'il y a à faire aujourd'hui, puis les indicateurs dérivés de la
//! base (pipeline, relances, impayés, occupation, calendrier fiscal, TJM effectif). Lot 46 :
//! libellés en français, bloc « Aujourd'hui » en tête.

use freeflow_core::app::AppError;
use freeflow_core::billing::{AgingBucket, aged_balance};
use freeflow_core::clients::list_clients;
use freeflow_core::domain::{Money, format_date};
use freeflow_core::fiscal::{FiscalDeadlineKind, fiscal_calendar};
use freeflow_core::missions::{effective_daily_rate, list_active_missions, monthly_capacity};
use freeflow_core::prospection::{late_actions, pipeline_by_stage, weighted_pipeline};
use freeflow_core::store::Store;
use maud::{Markup, html};

use crate::layout::{ViewId, view_head};

/// Libellé humain d'une échéance fiscale (le calendrier du cœur, lui, reste en clés stables).
fn deadline_label(kind: FiscalDeadlineKind) -> &'static str {
    match kind {
        FiscalDeadlineKind::Ca3 => "TVA (CA3)",
        FiscalDeadlineKind::VatInstalment => "Acompte de TVA (3514)",
        FiscalDeadlineKind::Ca12 => "TVA (CA12)",
        FiscalDeadlineKind::IsAcompte => "Acompte d'IS",
        FiscalDeadlineKind::IsSolde => "Solde d'IS",
        FiscalDeadlineKind::Cfe => "CFE",
        FiscalDeadlineKind::Liasse => "Liasse fiscale",
        FiscalDeadlineKind::ApprovalMeeting => "AG d'approbation",
        FiscalDeadlineKind::AccountsFiling => "Dépôt des comptes",
        FiscalDeadlineKind::Dsn => "DSN (dirigeant)",
        FiscalDeadlineKind::Das2 => "Honoraires (DAS2)",
        FiscalDeadlineKind::Dividends2777 => "Dividendes (2777)",
    }
}

fn today() -> time::Date {
    freeflow_core::clock::today_local()
}

fn current_month() -> freeflow_core::domain::Month {
    let d = today();
    freeflow_core::domain::Month::new(d.year(), u8::from(d.month()))
        .expect("le mois courant est toujours valide")
}

fn forecast_svg(by_stage: &[freeflow_core::prospection::StageSummary]) -> Markup {
    let max = by_stage
        .iter()
        .map(|s| s.weighted_amount.cents())
        .max()
        .unwrap_or(1)
        .max(1);
    let width = 560.0;
    let height = 120.0;
    let bar_width = width / (by_stage.len() as f64) * 0.6;
    let gap = width / (by_stage.len() as f64);
    html! {
        svg viewBox=(format!("0 0 {width} {height}")) width="100%" height=(height) role="img" aria-label="pipeline pondéré par étape" {
            @for (i, s) in by_stage.iter().enumerate() {
                @let bar_height = (s.weighted_amount.cents() as f64 / max as f64 * (height - 18.0)).max(2.0);
                @let x = i as f64 * gap + (gap - bar_width) / 2.0;
                @let y = height - bar_height - 16.0;
                rect x=(x) y=(y) width=(bar_width) height=(bar_height) fill="var(--accent)" opacity="0.85" rx="2" {}
                text x=(x + bar_width / 2.0) y=(height - 2.0) text-anchor="middle" font-size="9" fill="var(--text-3)" font-family="var(--font-mono)" {
                    (s.stage.as_str())
                }
            }
        }
    }
}

pub fn render(store: &Store) -> Result<Markup, AppError> {
    let conn = store.connection();
    let today = today();

    let weighted = weighted_pipeline(conn)?;
    let by_stage = pipeline_by_stage(conn)?;
    let late = late_actions(conn, today)?;
    let aged = aged_balance(conn, today)?;
    let outstanding: Money = aged.iter().map(|a| a.outstanding).sum();
    let capacity = monthly_capacity(conn, current_month())?;
    let clients = list_clients(conn)?;
    let missions = list_active_missions(conn)?;
    let calendar = fiscal_calendar(conn, today)?;
    let setup = freeflow_core::setup::setup_status(conn)?;

    let mut profitability = Vec::new();
    for mission in missions
        .iter()
        .filter(|m| matches!(m.kind, freeflow_core::domain::MissionKind::Regie { .. }))
    {
        let freeflow_core::domain::MissionKind::Regie { daily_rate } = mission.kind else {
            unreachable!()
        };
        if let Some(effective) = effective_daily_rate(conn, mission.id)? {
            let client_name = clients
                .iter()
                .find(|c| c.id == mission.client_id)
                .map_or_else(|| "?".to_string(), |c| c.name.clone());
            profitability.push((client_name, mission.name.clone(), daily_rate, effective));
        }
    }

    Ok(html! {
        (view_head(ViewId::Dashboard, &format!("au {}", format_date(today))))

        @if !setup.is_done() {
            div class="panel bordered" id="next-step" style="margin-bottom:14px" {
                div class="panel-title" { "Prochaine étape" }
                div class="detail-note" { (setup.next_step.text()) }
                div class="form-actions" {
                    a class="btn primary" href="/premiers-pas" hx-get="/premiers-pas" hx-target="#content" hx-push-url="true" { "Configurer ma société" }
                }
            }
        }

        @let overdue: Vec<_> = aged.iter().filter(|a| a.bucket != AgingBucket::Current).collect();
        @let upcoming: Vec<_> = calendar.iter().take(3).collect();
        div class="panel bordered" id="today" style="margin-bottom:14px" {
            div class="panel-title" { "Aujourd'hui" }
            @if late.is_empty() && overdue.is_empty() && upcoming.is_empty() {
                div class="empty-state" { "Rien d'urgent — le calendrier et le pipeline sont plus bas." }
            } @else {
                div class="today-list" {
                    @for o in late.iter().take(5) {
                        div class="today-row" {
                            span class="badge danger" { "Relance" }
                            span class="today-text" {
                                (o.name)
                                @if let Some(at) = o.next_action_at {
                                    " — prévue le " (format_date(at))
                                }
                            }
                        }
                    }
                    @for a in overdue.iter().take(5) {
                        div class="today-row" {
                            span class="badge danger" { "Impayé" }
                            span class="today-text" {
                                (a.outstanding) " · J+" (a.days_overdue)
                            }
                        }
                    }
                    @for d in &upcoming {
                        div class="today-row" {
                            span class="badge warn" { "Échéance" }
                            span class="today-text" {
                                (deadline_label(d.kind)) " le " (format_date(d.due_on))
                            }
                        }
                    }
                }
            }
        }

        div class="kpi-grid" {
            div class="kpi" {
                div class="kpi-label" { "Pipeline pondéré" }
                div class="kpi-value" { (weighted) }
                div class="kpi-note" { (by_stage.iter().map(|s| s.count).sum::<u32>()) " opportunités ouvertes" }
            }
            div class="kpi" {
                div class="kpi-label" { "Relances en retard" }
                div class="kpi-value" { (late.len()) }
                @if late.is_empty() {
                    div class="kpi-note ok" { "aucune relance en attente" }
                } @else {
                    div class="kpi-note warn" { "à traiter dans Prospection" }
                }
            }
            div class="kpi" {
                div class="kpi-label" { "Impayés" }
                div class="kpi-value" { (outstanding) }
                div class="kpi-note" { (aged.len()) " facture(s) en attente" }
            }
            div class="kpi" {
                div class="kpi-label" { "Occupation du mois" }
                div class="kpi-value" { (format!("{:.0}%", capacity.utilization_percent())) }
                @if capacity.utilization_percent() < 50.0 {
                    div class="kpi-note warn" { "sous-remplissage" }
                } @else {
                    div class="kpi-note ok" { (format!("{:.1}", capacity.billable_days)) " j facturables" }
                }
            }
        }

        div class="grid-2" {
            div class="panel" {
                div class="panel-title" { "Pipeline par étape" }
                @if by_stage.iter().all(|s| s.count == 0) {
                    div class="empty-state" { "aucune opportunité ouverte" }
                } @else {
                    (forecast_svg(&by_stage))
                }
            }
            div class="panel" {
                div class="panel-title" { "Alertes" }
                @if late.is_empty() && overdue.is_empty() {
                    div class="empty-state" { "Aucune alerte" }
                } @else {
                    table {
                        @for o in late.iter().take(3) {
                            tr {
                                td { span class="badge danger" { "Relance" } }
                                td { (o.name) }
                            }
                        }
                        @for a in overdue.iter().take(3) {
                            tr {
                                td { span class="badge danger" { "Impayé" } }
                                td { (a.outstanding) " · J+" (a.days_overdue) }
                            }
                        }
                    }
                }
            }
        }

        div class="panel bordered" {
            div class="panel-title" { "Échéances fiscales" }
            @if calendar.is_empty() {
                div class="empty-state" { "aucune échéance dans les 12 prochains mois" }
            } @else {
                table {
                    tr { th { "échéance" } th { "date" } th { "montant estimé" } th { "note" } }
                    @for d in calendar.iter().take(6) {
                        tr {
                            td { (deadline_label(d.kind)) }
                            td { (format_date(d.due_on)) }
                            td class="num" {
                                @match d.amount {
                                    Some(amount) => (amount),
                                    None => "—",
                                }
                            }
                            td class="kpi-note" { (d.note.as_deref().unwrap_or("")) }
                        }
                    }
                }
                div class="kpi-note" { "montants et dates indicatifs — à vérifier sur impots.gouv.fr" }
            }
        }

        div class="panel bordered" {
            div class="panel-title" { "Rentabilité (régie)" }
            @if profitability.is_empty() {
                div class="empty-state" { "Aucune mission en régie avec du temps saisi" }
            } @else {
                table {
                    tr { th { "Client" } th { "Mission" } th { "TJM contractuel" } th { "TJM effectif" } th { "Écart" } }
                    @for (client_name, mission_name, contractual, effective) in &profitability {
                        @let delta_percent = (effective.cents() as f64 - contractual.cents() as f64) / contractual.cents() as f64 * 100.0;
                        tr {
                            td { (client_name) }
                            td { (mission_name) }
                            td class="num" { (contractual) }
                            td class="num" { (effective) }
                            td class="num" style=(if delta_percent < 0.0 { "color:var(--amber)" } else { "color:var(--accent)" }) {
                                (format!("{delta_percent:+.1}%"))
                            }
                        }
                    }
                }
            }
        }
    })
}
