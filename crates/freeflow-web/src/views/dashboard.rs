//! Écran `dashboard` : uniquement des chiffres réellement calculables aujourd'hui (lots 0-8).
//! Trésorerie, runway et prévisionnel 12 mois affichés dans la maquette dépendent des dépenses
//! et du moteur de prévisionnel (lot 11, pas encore construit) — plutôt que d'inventer des
//! nombres, cet écran les omet et affiche ce qui est réellement dérivé de la base : pipeline
//! pondéré, retards, capacité du mois, balance âgée, et le TJM effectif par client.

use freeflow_core::app::AppError;
use freeflow_core::billing::aged_balance;
use freeflow_core::clients::list_clients;
use freeflow_core::domain::{Money, format_date};
use freeflow_core::missions::{effective_daily_rate, list_active_missions, monthly_capacity};
use freeflow_core::prospection::{late_actions, pipeline_by_stage, weighted_pipeline};
use freeflow_core::store::Store;
use maud::{Markup, html};
use time::OffsetDateTime;

use crate::layout::{ViewId, view_head};

fn today() -> time::Date {
    OffsetDateTime::now_utc().date()
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
        (view_head(ViewId::Dashboard, &format!("dernière consultation {}", format_date(today))))

        div class="kpi-grid" {
            div class="kpi" {
                div class="kpi-label" { "pipeline_pondere" }
                div class="kpi-value" { (weighted) }
                div class="kpi-note" { (by_stage.iter().map(|s| s.count).sum::<u32>()) " opportunités ouvertes" }
            }
            div class="kpi" {
                div class="kpi-label" { "en_retard" }
                div class="kpi-value" { (late.len()) }
                @if late.is_empty() {
                    div class="kpi-note ok" { "aucune relance en attente" }
                } @else {
                    div class="kpi-note warn" { "action de relance en retard" }
                }
            }
            div class="kpi" {
                div class="kpi-label" { "impayes" }
                div class="kpi-value" { (outstanding) }
                div class="kpi-note" { (aged.len()) " facture(s) en attente" }
            }
            div class="kpi" {
                div class="kpi-label" { "capacite_" (current_month()) }
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
                div class="panel-title" { "pipeline_pondere_par_etape" }
                @if by_stage.iter().all(|s| s.count == 0) {
                    div class="empty-state" { "aucune opportunité ouverte" }
                } @else {
                    (forecast_svg(&by_stage))
                }
            }
            div class="panel" {
                div class="panel-title" { "alertes" }
                @let overdue: Vec<_> = aged.iter().filter(|a| a.bucket != freeflow_core::billing::AgingBucket::Current).collect();
                @if late.is_empty() && overdue.is_empty() {
                    div class="empty-state" { "aucune alerte" }
                } @else {
                    table {
                        @for o in late.iter().take(3) {
                            tr {
                                td { span class="badge danger" { "retard" } }
                                td { (o.name) }
                            }
                        }
                        @for a in overdue.iter().take(3) {
                            tr {
                                td { span class="badge danger" { "impaye" } }
                                td { (a.invoice_id.to_string()) " · " (a.outstanding) " · J+" (a.days_overdue) }
                            }
                        }
                    }
                }
            }
        }

        div class="panel bordered" {
            div class="panel-title" { "rentabilite_client" }
            @if profitability.is_empty() {
                div class="empty-state" { "aucune mission en régie avec du temps saisi" }
            } @else {
                table {
                    tr { th { "client" } th { "mission" } th { "tjm_contractuel" } th { "tjm_effectif" } th { "ecart" } }
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
