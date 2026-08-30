//! Écran `facturation` : les factures émises, les plus récentes d'abord — la même donnée que
//! `freeflow invoice` en CLI (il n'existe pas de commande `list` dédiée, la GUI en avait besoin
//! la première : `freeflow_core::billing::list_invoices`, ajouté pour ce lot).

use freeflow_core::app::AppError;
use freeflow_core::billing::{compute_totals, list_invoices};
use freeflow_core::clients::list_clients;
use freeflow_core::domain::{InvoiceStatus, Money, format_date};
use freeflow_core::store::Store;
use maud::{Markup, html};

use crate::layout::{ViewId, view_head};

pub fn render(store: &Store) -> Result<Markup, AppError> {
    let conn = store.connection();
    let invoices = list_invoices(conn)?;
    let clients = list_clients(conn)?;

    let ca_total: Money = invoices
        .iter()
        .map(|i| compute_totals(&i.lines).total_ttc)
        .sum();

    Ok(html! {
        (view_head(ViewId::Facturation, &format!("{} factures émises", invoices.len())))

        div class="kpi-grid" {
            div class="kpi" {
                div class="kpi-label" { "ca_ttc_total" }
                div class="kpi-value" { (ca_total) }
            }
            div class="kpi" {
                div class="kpi-label" { "factures_emises" }
                div class="kpi-value" { (invoices.len()) }
            }
        }

        @if invoices.is_empty() {
            div class="empty-state" { "aucune facture émise" }
        } @else {
            div class="panel bordered" style="padding:0" {
                table {
                    tr {
                        th style="padding-left:18px" { "n°" }
                        th { "client" }
                        th { "émise" }
                        th { "échéance" }
                        th { "statut" }
                        th style="padding-right:18px" { "ttc" }
                    }
                    @for invoice in &invoices {
                        @let client_name = clients.iter().find(|c| c.id == invoice.client_id).map_or_else(|| "?".to_string(), |c| c.name.clone());
                        @let totals = compute_totals(&invoice.lines);
                        tr {
                            td style="padding-left:18px" class="mono" { (invoice.number) }
                            td { (client_name) }
                            td class="mono" { (format_date(invoice.issued_on)) }
                            td class="mono" { (format_date(invoice.due_on)) }
                            td {
                                @match invoice.status {
                                    InvoiceStatus::Paid => span class="badge ok" { "payée" },
                                    InvoiceStatus::PartiallyPaid => span class="badge warn" { "partielle" },
                                    InvoiceStatus::Overdue => span class="badge danger" { "retard" },
                                    InvoiceStatus::Cancelled => span class="badge info" { "annulée" },
                                    InvoiceStatus::Issued => span class="badge info" { "émise" },
                                }
                            }
                            td class="num" style="padding-right:18px" { (totals.total_ttc) }
                        }
                    }
                }
            }
        }
    })
}
