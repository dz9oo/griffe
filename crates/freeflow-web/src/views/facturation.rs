//! Écran `facturation` : les factures émises, les plus récentes d'abord — la même donnée que
//! `freeflow invoice` en CLI.
//!
//! Lot 22 : l'écran passe de lecture pure à liste cliquable + panneau de détail (facture,
//! encaissements, annulation d'un encaissement saisi à tort — contre-écriture). Le statut
//! affiché est enfin **calculé** (payée/partielle/retard, depuis les encaissements non annulés)
//! plutôt que lu depuis `invoices.status`, une colonne figée à `issued` dont les autres
//! variantes n'étaient jamais produites — les branches de badge correspondantes étaient mortes
//! depuis le lot 9.

use freeflow_core::app::AppError;
use freeflow_core::billing::{compute_totals, list_invoices, list_payments, payments_for_invoice};
use freeflow_core::clients::list_clients;
use freeflow_core::domain::{Invoice, InvoiceId, Money, Payment, format_date};
use freeflow_core::store::Store;
use maud::{Markup, html};
use time::Date;

use crate::layout::{ViewId, view_head};
use crate::views::{form, panel};

/// Statut réel d'une facture, calculé — jamais lu depuis la colonne `status`.
fn status_badge(invoice: &Invoice, paid: Money, today: Date) -> Markup {
    let ttc = compute_totals(&invoice.lines).total_ttc;
    html! {
        @if invoice.credited_invoice_id.is_some() {
            span class="badge info" { "avoir" }
        } @else if paid.cents() >= ttc.cents() && ttc.cents() > 0 {
            span class="badge ok" { "payée" }
        } @else if paid.cents() > 0 {
            span class="badge warn" { "partielle" }
        } @else if invoice.due_on < today {
            span class="badge danger" { "retard" }
        } @else {
            span class="badge info" { "émise" }
        }
    }
}

pub fn list_fragment(store: &Store, today: Date) -> Result<Markup, AppError> {
    let conn = store.connection();
    let invoices = list_invoices(conn)?;
    let clients = list_clients(conn)?;
    let payments = list_payments(conn)?;
    let paid_for = |id: InvoiceId| -> Money {
        payments
            .iter()
            .filter(|p| p.invoice_id == id && !p.is_voided())
            .map(|p| p.amount)
            .sum()
    };

    Ok(html! {
        div id="facturation-list"
            hx-get="/facturation/table"
            hx-trigger="freeflow:saved from:body"
            hx-target="this"
            hx-swap="outerHTML" {
            @if invoices.is_empty() {
                div class="empty-state" {
                    "aucune facture émise — émettez-en une depuis la CLI : " code { "freeflow invoice emit" }
                }
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
                            tr class="row-clickable" hx-get=(format!("/facturation/{}", invoice.id)) hx-target="#panel" hx-swap="innerHTML" {
                                td style="padding-left:18px" class="mono" { (invoice.number) }
                                td { (client_name) }
                                td class="mono" { (format_date(invoice.issued_on)) }
                                td class="mono" { (format_date(invoice.due_on)) }
                                td { (status_badge(invoice, paid_for(invoice.id), today)) }
                                td class="num" style="padding-right:18px" { (totals.total_ttc) }
                            }
                        }
                    }
                }
            }
        }
    })
}

pub fn render(store: &Store, today: Date) -> Result<Markup, AppError> {
    let conn = store.connection();
    let invoices = list_invoices(conn)?;
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

        (list_fragment(store, today)?)
    })
}

fn payment_row(payment: &Payment) -> Markup {
    html! {
        tr {
            td style="padding-left:18px" class="mono" { (format_date(payment.received_on)) }
            td class="mono" { (payment.amount) }
            td { (payment.method.as_str()) }
            td {
                @if payment.is_voided() {
                    span class="badge danger" { "annulé" }
                } @else if payment.bank_transaction_id.is_some() {
                    span class="badge ok" { "rapproché" }
                } @else {
                    "—"
                }
            }
            td style="padding-right:18px" {
                @if !payment.is_voided() {
                    button class="btn small danger" hx-get=(format!("/payments/{}/void", payment.id)) hx-target="#panel" hx-swap="innerHTML" { "annuler" }
                }
            }
        }
    }
}

pub fn detail_panel(
    store: &Store,
    invoice: &Invoice,
    payments: &[Payment],
    today: Date,
    error: Option<&str>,
) -> Markup {
    let client_name = list_clients(store.connection())
        .ok()
        .and_then(|clients| clients.into_iter().find(|c| c.id == invoice.client_id))
        .map_or_else(|| "?".to_string(), |c| c.name);
    let totals = compute_totals(&invoice.lines);
    let paid: Money = payments
        .iter()
        .filter(|p| !p.is_voided())
        .map(|p| p.amount)
        .sum();
    let body = html! {
        @if let Some(msg) = error {
            (form::error_banner(msg))
        }
        div class="detail-head" {
            div class="detail-title" { (invoice.number) " — " (client_name) }
            (status_badge(invoice, paid, today))
        }
        dl class="detail-fields" {
            dt { "Total TTC" } dd class="mono" { (totals.total_ttc) }
            dt { "Encaissé" } dd class="mono" { (paid) }
            dt { "Solde restant dû" } dd class="mono" { (totals.total_ttc - paid) }
            dt { "Émise le" } dd { (format_date(invoice.issued_on)) }
            dt { "Échéance" } dd { (format_date(invoice.due_on)) }
        }
        div class="detail-section" {
            div class="detail-section-head" { span { "Encaissements" } }
            @if payments.is_empty() {
                div class="empty-state" { "aucun encaissement — voir " code { "freeflow payment record" } " ou " code { "freeflow bank reconcile" } }
            } @else {
                div class="panel bordered" style="padding:0" {
                    table {
                        tr {
                            th style="padding-left:18px" { "reçu le" }
                            th { "montant" }
                            th { "méthode" }
                            th { "origine" }
                            th style="padding-right:18px" {}
                        }
                        @for payment in payments {
                            (payment_row(payment))
                        }
                    }
                }
            }
        }
        div class="detail-note" {
            "Annuler un encaissement est une contre-écriture : il reste dans l'historique mais "
            "sort de tous les calculs, et libère sa transaction bancaire s'il venait d'un rapprochement. "
            "La facture elle-même, immuable, ne s'annule que par un avoir ("
            code { "freeflow invoice credit-note" } ")."
        }
    };
    panel::sheet(&invoice.number, body)
}

pub fn void_confirm_panel(payment: &Payment, invoice_number: &str) -> Markup {
    let body = html! {
        div class="detail-note" {
            "Annuler l'encaissement de " (payment.amount) " du "
            (format_date(payment.received_on)) " sur la facture " (invoice_number)
            " ? La facture redeviendra (partiellement) impayée"
            @if payment.bank_transaction_id.is_some() {
                ", et la transaction bancaire rapprochée sera libérée"
            }
            "."
        }
        form hx-post=(format!("/payments/{}/void", payment.id)) hx-target="#panel" hx-swap="innerHTML" {
            (form::text("reason", "Motif (journalisé dans l'audit)", "", None))
            (form::actions("Annuler l'encaissement"))
        }
    };
    panel::sheet("Annuler un encaissement", body)
}

pub fn load_detail(
    store: &Store,
    id: InvoiceId,
) -> Result<Option<(Invoice, Vec<Payment>)>, AppError> {
    let Some(invoice) = freeflow_core::billing::invoice_by_id(store.connection(), id)? else {
        return Ok(None);
    };
    let payments = payments_for_invoice(store.connection(), id)?;
    Ok(Some((invoice, payments)))
}
