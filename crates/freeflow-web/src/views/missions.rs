//! Écran `missions` : missions actives, avec TJM effectif quand du temps facturable a été
//! saisi — le même calcul que `freeflow mission rate`.

use freeflow_core::app::AppError;
use freeflow_core::clients::list_clients;
use freeflow_core::domain::MissionKind;
use freeflow_core::missions::{effective_daily_rate, list_active_missions};
use freeflow_core::store::Store;
use maud::{Markup, html};

use crate::layout::{ViewId, view_head};

pub fn render(store: &Store) -> Result<Markup, AppError> {
    let conn = store.connection();
    let missions = list_active_missions(conn)?;
    let clients = list_clients(conn)?;

    Ok(html! {
        (view_head(ViewId::Missions, &format!("{} missions actives", missions.len())))

        @if missions.is_empty() {
            div class="empty-state" { "aucune mission active — freeflow mission create pour en créer une" }
        } @else {
            @for mission in &missions {
                @let client_name = clients.iter().find(|c| c.id == mission.client_id).map_or_else(|| "?".to_string(), |c| c.name.clone());
                @let effective = effective_daily_rate(conn, mission.id)?;
                div class="mission-row" {
                    div class="mission-top" {
                        div {
                            div class="mission-id" { (mission.id.to_string()) }
                            div class="mission-client" { (client_name) }
                            div class="mission-name" { (mission.name) }
                        }
                        @match mission.kind {
                            MissionKind::Regie { .. } => span class="badge info" { "régie" },
                            MissionKind::Forfait { .. } => span class="badge warn" { "forfait" },
                            MissionKind::Recurrent { .. } => span class="badge ok" { "récurrent" },
                        }
                    }
                    div class="mission-metrics" {
                        @match mission.kind {
                            MissionKind::Regie { daily_rate } => {
                                div class="mm" { div class="v" { (daily_rate) } div class="l" { "tjm" } }
                            }
                            MissionKind::Forfait { budget } => {
                                div class="mm" { div class="v" { (budget) } div class="l" { "budget" } }
                            }
                            MissionKind::Recurrent { monthly_amount } => {
                                div class="mm" { div class="v" { (monthly_amount) } div class="l" { "mensuel" } }
                            }
                        }
                        @if let Some(rate) = effective {
                            div class="mm" { div class="v" { (rate) } div class="l" { "tjm_eff" } }
                        } @else {
                            div class="mm" { div class="v" { "—" } div class="l" { "tjm_eff" } }
                        }
                    }
                }
            }
        }
    })
}
