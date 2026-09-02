//! Requêtes de facturation : vérification de la chaîne de factures, balance âgée.

use rusqlite::Connection;
use time::Date;

use crate::app::AppError;
use crate::domain::{
    BankTransaction, BankTransactionId, ClientId, Invoice, InvoiceId, Money, Payment, PaymentId,
};

use super::row;
use super::totals::{CanonicalInvoice, compute_invoice_hash};

/// Toutes les factures, les plus récemment émises d'abord — alimente l'écran facturation de la
/// GUI (lot 9).
///
/// # Errors
pub fn list_invoices(conn: &Connection) -> Result<Vec<Invoice>, AppError> {
    let mut invoices = row::all_invoices(conn)?;
    invoices.reverse();
    Ok(invoices)
}

/// # Errors
pub fn invoice_by_id(conn: &Connection, id: InvoiceId) -> Result<Option<Invoice>, AppError> {
    row::invoice_by_id(conn, id)
}

/// Tous les encaissements, annulés compris, les plus récents d'abord — l'historique complet est
/// la liste par défaut ici (contrairement aux entités archivables) : un paiement annulé reste
/// une écriture qu'on doit pouvoir montrer, c'est le sens même de la contre-écriture.
///
/// # Errors
pub fn list_payments(conn: &Connection) -> Result<Vec<Payment>, AppError> {
    row::all_payments(conn)
}

/// # Errors
pub fn payment_by_id(conn: &Connection, id: PaymentId) -> Result<Option<Payment>, AppError> {
    row::payment_by_id(conn, id)
}

/// Encaissements d'une facture (annulés compris, les plus anciens d'abord) — alimente le
/// panneau de détail d'une facture dans la fenêtre.
///
/// # Errors
pub fn payments_for_invoice(
    conn: &Connection,
    invoice_id: InvoiceId,
) -> Result<Vec<Payment>, AppError> {
    row::payments_for_invoice(conn, invoice_id)
}

/// Solde encaissé (hors annulés) d'une facture — la brique du statut « payée / partielle »
/// affiché par les façades, pour ne jamais le recalculer chacune à sa façon.
///
/// # Errors
pub fn paid_amount(conn: &Connection, invoice_id: InvoiceId) -> Result<Money, AppError> {
    Ok(row::payments_for_invoice(conn, invoice_id)?
        .iter()
        .filter(|p| !p.is_voided())
        .map(|p| p.amount)
        .sum())
}

/// Toutes les transactions bancaires importées, les plus récentes d'abord — comble le trou du
/// lot 5 : après un import, il n'existait aucun moyen (CLI, MCP ou GUI) d'obtenir l'id d'une
/// transaction à rapprocher sans requête SQL manuelle.
///
/// # Errors
pub fn list_bank_transactions(conn: &Connection) -> Result<Vec<BankTransaction>, AppError> {
    row::all_bank_transactions(conn)
}

/// Une transaction importée par son id (lot 33 : les façades pré-remplissent une dépense depuis
/// un débit du relevé).
///
/// # Errors
pub fn bank_transaction_by_id(
    conn: &Connection,
    id: BankTransactionId,
) -> Result<Option<BankTransaction>, AppError> {
    row::bank_transaction_by_id(conn, id)
}

/// Les débits du relevé restant à rapprocher d'une dépense, les plus récents d'abord (lot 33).
///
/// # Errors
pub fn unmatched_debits(conn: &Connection) -> Result<Vec<BankTransaction>, AppError> {
    Ok(row::all_bank_transactions(conn)?
        .into_iter()
        .filter(|t| t.is_debit() && !t.is_matched())
        .collect())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainStatus {
    Intact,
    /// Facture à partir de laquelle la chaîne ne correspond plus à ce qui est stocké.
    BrokenAt(InvoiceId),
}

/// Revérifie l'intégralité de la chaîne de factures en recalculant chaque hash à partir du
/// contenu stocké — détecte toute altération directe de la base, en contournement de la
/// couche applicative (dont les triggers d'immuabilité, si jamais ils étaient eux-mêmes
/// contournés par un accès direct au fichier).
///
/// # Errors
pub fn verify_chain(conn: &Connection) -> Result<ChainStatus, AppError> {
    let invoices = row::all_invoices(conn)?;
    let mut expected_previous: Option<String> = None;
    for invoice in &invoices {
        if invoice.previous_hash != expected_previous {
            return Ok(ChainStatus::BrokenAt(invoice.id));
        }
        let canonical = CanonicalInvoice {
            number: &invoice.number,
            client_id: invoice.client_id.to_string(),
            mission_id: invoice.mission_id.map(|m| m.to_string()),
            lines: &invoice.lines,
            issued_on: crate::domain::format_date(invoice.issued_on),
            due_on: crate::domain::format_date(invoice.due_on),
            credited_invoice_id: invoice.credited_invoice_id.map(|c| c.to_string()),
        };
        let recomputed = compute_invoice_hash(invoice.previous_hash.as_deref(), &canonical);
        if recomputed != invoice.hash {
            return Ok(ChainStatus::BrokenAt(invoice.id));
        }
        expected_previous = Some(invoice.hash.clone());
    }
    Ok(ChainStatus::Intact)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgingBucket {
    Current,
    Due1To30,
    Due31To60,
    Due61To90,
    Due91Plus,
}

#[must_use]
fn bucket_for(days_overdue: i64) -> AgingBucket {
    match days_overdue {
        i64::MIN..=0 => AgingBucket::Current,
        1..=30 => AgingBucket::Due1To30,
        31..=60 => AgingBucket::Due31To60,
        61..=90 => AgingBucket::Due61To90,
        _ => AgingBucket::Due91Plus,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgedInvoice {
    pub invoice_id: InvoiceId,
    pub client_id: ClientId,
    pub outstanding: Money,
    pub days_overdue: i64,
    pub bucket: AgingBucket,
}

/// Balance âgée : pour chaque facture (hors avoir) dont le solde restant dû est non nul,
/// l'antériorité du retard par rapport à `today`. Une facture payée d'avance (solde négatif)
/// ou pas encore échue apparaît en `Current`.
///
/// # Errors
pub fn aged_balance(conn: &Connection, today: Date) -> Result<Vec<AgedInvoice>, AppError> {
    let invoices = row::all_invoices(conn)?;
    let payments = row::all_payments(conn)?;

    let mut aged = Vec::new();
    for invoice in &invoices {
        if invoice.credited_invoice_id.is_some() {
            continue; // un avoir n'a pas de solde propre à surveiller.
        }
        let totals = super::totals::compute_totals(&invoice.lines);
        let paid: Money = payments
            .iter()
            .filter(|p| p.invoice_id == invoice.id && !p.is_voided())
            .map(|p| p.amount)
            .sum();
        let outstanding = totals.total_ttc - paid;
        if outstanding.is_zero() {
            continue;
        }
        let days_overdue = (today - invoice.due_on).whole_days();
        aged.push(AgedInvoice {
            invoice_id: invoice.id,
            client_id: invoice.client_id,
            outstanding,
            days_overdue,
            bucket: bucket_for(days_overdue),
        });
    }
    Ok(aged)
}
