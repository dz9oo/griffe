//! Écran `prospection` : le pipeline ouvert, tel que le retourne
//! `freeflow_core::prospection::list_open_opportunities` — la même donnée que `freeflow
//! prospect pipeline` en CLI.

use freeflow_core::app::AppError;
use freeflow_core::clients::list_clients;
use freeflow_core::domain::format_date;
use freeflow_core::prospection::list_open_opportunities;
use freeflow_core::store::Store;
use maud::{Markup, html};
use time::OffsetDateTime;

use crate::layout::{ViewId, view_head};

pub fn render(store: &Store) -> Result<Markup, AppError> {
    let conn = store.connection();
    let today = OffsetDateTime::now_utc().date();
    let opportunities = list_open_opportunities(conn)?;
    let clients = list_clients(conn)?;

    Ok(html! {
        (view_head(ViewId::Prospection, &format!("{} opportunités ouvertes", opportunities.len())))

        @if opportunities.is_empty() {
            div class="panel bordered" { div class="empty-state" { "aucune opportunité ouverte — freeflow prospect create pour en créer une" } }
        } @else {
            div class="panel bordered" style="padding:0" {
                table {
                    tr {
                        th style="padding-left:18px" { "client" }
                        th { "étape" }
                        th { "montant" }
                        th { "proba" }
                        th { "pondéré" }
                        th { "prochaine action" }
                        th style="padding-right:18px" { "statut" }
                    }
                    @for o in &opportunities {
                        @let client_name = clients.iter().find(|c| c.id == o.client_id).map_or_else(|| "?".to_string(), |c| c.name.clone());
                        @let weighted = o.probability.weighted(o.amount);
                        @let is_late = o.next_action_at.is_some_and(|d| d < today);
                        tr {
                            td style="padding-left:18px" { (client_name) " — " (o.name) }
                            td class="mono" { (o.stage.as_str()) }
                            td class="num" { (o.amount) }
                            td {
                                span class="prob-bar" { i style=(format!("width:{}%", o.probability.percent())) {} }
                                (o.probability.percent()) "%"
                            }
                            td class="num" { (weighted) }
                            @match o.next_action_at {
                                Some(d) => td class="mono" style=(if is_late { "color:var(--red)" } else { "" }) { (format_date(d)) },
                                None => td class="mono" { "—" },
                            }
                            td style="padding-right:18px" {
                                @if is_late {
                                    span class="badge danger" { "retard" }
                                } @else {
                                    span class="badge ok" { "ok" }
                                }
                            }
                        }
                    }
                }
            }
        }
    })
}
