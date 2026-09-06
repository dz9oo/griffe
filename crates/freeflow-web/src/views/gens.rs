//! Les gens : une liste, trois chapitres. Lot 48 assemble les listes déjà là (opportunités,
//! missions, bénéficiaires de dépenses). Le dossier unique arrive au lot 50.

use std::collections::BTreeMap;

use freeflow_core::app::AppError;
use freeflow_core::clients::list_clients;
use freeflow_core::domain::{ClientId, MissionKind};
use freeflow_core::expenses::list_expenses;
use freeflow_core::missions::list_active_missions;
use freeflow_core::prospection::list_open_opportunities;
use freeflow_core::store::Store;
use maud::{Markup, html};
use time::Date;

use crate::layout::ViewId;
use crate::views::copy::letter_date;

pub fn render(store: &Store, today: Date) -> Result<Markup, AppError> {
    let conn = store.connection();
    let clients = list_clients(conn)?;
    let name_of = |id: ClientId| {
        clients
            .iter()
            .find(|c| c.id == id)
            .map_or_else(|| "?".to_string(), |c| c.name.clone())
    };

    let conversations = list_open_opportunities(conn)?;
    let missions = list_active_missions(conn)?;
    let mut suppliers: BTreeMap<String, u32> = BTreeMap::new();
    for expense in list_expenses(conn)? {
        if let Some(supplier) = expense.supplier.filter(|s| !s.is_empty()) {
            *suppliers.entry(supplier).or_insert(0) += 1;
        }
    }

    let empty = conversations.is_empty() && missions.is_empty() && suppliers.is_empty();

    Ok(html! {
        div class="letter" data-view=(ViewId::Gens.slug()) {
            div class="date" { "Les gens · " (letter_date(today)) }
            h1 { "Les gens." }
            @if empty {
                p class="lede" { "Personne pour l'instant. Une conversation commence par un nom et une phrase." }
            } @else {
                p class="lede" { "Avec qui j'en suis. Un nom, pas un type de document." }
            }
            div class="letter-actions" {
                button class="seal" type="button"
                  hx-get="/prospection/new" hx-target="#panel" hx-swap="innerHTML" {
                    "Nouvelle conversation"
                }
            }

            p class="section-label" { "En conversation" }
            @if conversations.is_empty() {
                p class="empty-state" { "Aucune conversation ouverte." }
            } @else {
                ul class="people" {
                    @for o in &conversations {
                        li {
                            a href=(format!("/prospection/{}", o.id))
                              hx-get=(format!("/prospection/{}", o.id))
                              hx-target="#panel" hx-swap="innerHTML" {
                                div {
                                    div class="nm" { (name_of(o.client_id)) }
                                    div class="st" { (o.name) }
                                }
                                span class="amt" { (o.amount) }
                            }
                        }
                    }
                }
            }

            p class="section-label" { "En mission" }
            @if missions.is_empty() {
                p class="empty-state" { "Aucune mission en cours." }
            } @else {
                ul class="people" {
                    @for m in &missions {
                        li {
                            a href=(format!("/missions/{}", m.id))
                              hx-get=(format!("/missions/{}", m.id))
                              hx-target="#panel" hx-swap="innerHTML" {
                                div {
                                    div class="nm" { (name_of(m.client_id)) }
                                    div class="st" { (m.name) " · " (kind_fr(&m.kind)) }
                                }
                            }
                        }
                    }
                }
            }

            p class="section-label" { "Fournisseurs" }
            @if suppliers.is_empty() {
                p class="empty-state" { "Les bénéficiaires d'une dépense (honoraires, notamment) apparaîtront ici." }
            } @else {
                ul class="people" {
                    @for (name, n) in &suppliers {
                        li {
                            a href=(ViewId::Depenses.path())
                              hx-get=(ViewId::Depenses.path())
                              hx-target="#content" hx-push-url="true" {
                                div {
                                    div class="nm" { (name) }
                                    div class="st" {
                                        @if *n == 1 { "une dépense" } @else { (n) " dépenses" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    })
}

fn kind_fr(kind: &MissionKind) -> &'static str {
    match kind {
        MissionKind::Regie { .. } => "régie",
        MissionKind::Forfait { .. } => "forfait",
        MissionKind::Recurrent { .. } => "récurrent",
    }
}
