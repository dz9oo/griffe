//! Commandes de facturation : émission, avoir, encaissement, import de relevé bancaire.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use time::Date;

use crate::app::{AppError, Command};
use crate::domain::{
    BankTransactionId, ClientId, Invoice, InvoiceId, InvoiceLine, InvoiceStatus, MissionId, Money,
    Payment, PaymentId, PaymentMethod,
};

use super::error::BillingError;
use super::import::ParsedTransaction;
use super::row;
use super::totals::{CanonicalInvoice, compute_invoice_hash};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmittedInvoice {
    pub id: InvoiceId,
    pub number: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmitInvoice {
    pub client_id: ClientId,
    pub mission_id: Option<MissionId>,
    pub lines: Vec<InvoiceLine>,
    pub issued_on: Date,
    pub payment_terms_days: u32,
}

impl Command for EmitInvoice {
    type Output = EmittedInvoice;
    const NAME: &'static str = "billing.emit_invoice";

    /// Émettre une facture est un acte légal irréversible (seul un avoir peut l'annuler) : un
    /// agent ne peut jamais le déclencher sans validation humaine explicite.
    fn requires_confirmation(&self) -> bool {
        true
    }

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        if self.lines.is_empty() {
            return Err(BillingError::EmptyInvoice.into());
        }
        let fiscal_year = self.issued_on.year();
        let number = row::allocate_invoice_number(conn, fiscal_year)?;
        let due_on = self
            .issued_on
            .saturating_add(time::Duration::days(i64::from(self.payment_terms_days)));
        let previous_hash = row::last_invoice_hash(conn)?;
        let id = InvoiceId::new();

        let canonical = CanonicalInvoice {
            number: &number,
            client_id: self.client_id.to_string(),
            mission_id: self.mission_id.map(|m| m.to_string()),
            lines: &self.lines,
            issued_on: crate::domain::format_date(self.issued_on),
            due_on: crate::domain::format_date(due_on),
            credited_invoice_id: None,
        };
        let hash = compute_invoice_hash(previous_hash.as_deref(), &canonical);

        let invoice = Invoice {
            id,
            number: number.clone(),
            client_id: self.client_id,
            mission_id: self.mission_id,
            lines: self.lines.clone(),
            status: InvoiceStatus::Issued,
            issued_on: self.issued_on,
            due_on,
            previous_hash,
            hash,
            credited_invoice_id: None,
        };
        row::insert_invoice(conn, &invoice)?;
        Ok(EmittedInvoice { id, number })
    }
}

/// Émet un avoir annulant intégralement `invoice_id` : une facture à part entière, aux lignes
/// négatées, jamais une modification de l'originale.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueCreditNote {
    pub invoice_id: InvoiceId,
    pub issued_on: Date,
}

impl Command for IssueCreditNote {
    type Output = EmittedInvoice;
    const NAME: &'static str = "billing.issue_credit_note";

    /// Même exigence que [`EmitInvoice`] : un avoir est lui-même une facture à part entière.
    fn requires_confirmation(&self) -> bool {
        true
    }

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let original = row::invoice_by_id(conn, self.invoice_id)?
            .ok_or(BillingError::NotFound(self.invoice_id))?;
        if original.credited_invoice_id.is_some() {
            return Err(BillingError::CannotCreditACreditNote(self.invoice_id).into());
        }
        if row::has_credit_note(conn, self.invoice_id)? {
            return Err(BillingError::AlreadyCredited(self.invoice_id).into());
        }

        let negated_lines: Vec<InvoiceLine> = original
            .lines
            .iter()
            .map(|l| InvoiceLine {
                quantity: -l.quantity,
                ..l.clone()
            })
            .collect();

        let fiscal_year = self.issued_on.year();
        let number = row::allocate_invoice_number(conn, fiscal_year)?;
        let previous_hash = row::last_invoice_hash(conn)?;
        let id = InvoiceId::new();

        let canonical = CanonicalInvoice {
            number: &number,
            client_id: original.client_id.to_string(),
            mission_id: original.mission_id.map(|m| m.to_string()),
            lines: &negated_lines,
            issued_on: crate::domain::format_date(self.issued_on),
            due_on: crate::domain::format_date(self.issued_on),
            credited_invoice_id: Some(self.invoice_id.to_string()),
        };
        let hash = compute_invoice_hash(previous_hash.as_deref(), &canonical);

        let credit_note = Invoice {
            id,
            number: number.clone(),
            client_id: original.client_id,
            mission_id: original.mission_id,
            lines: negated_lines,
            status: InvoiceStatus::Issued,
            issued_on: self.issued_on,
            due_on: self.issued_on,
            previous_hash,
            hash,
            credited_invoice_id: Some(self.invoice_id),
        };
        row::insert_invoice(conn, &credit_note)?;
        Ok(EmittedInvoice { id, number })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordPayment {
    pub invoice_id: InvoiceId,
    pub amount: Money,
    pub received_on: Date,
    pub method: PaymentMethod,
}

impl Command for RecordPayment {
    type Output = PaymentId;
    const NAME: &'static str = "billing.record_payment";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        row::invoice_by_id(conn, self.invoice_id)?
            .ok_or(BillingError::NotFound(self.invoice_id))?;
        let payment = Payment {
            id: PaymentId::new(),
            invoice_id: self.invoice_id,
            amount: self.amount,
            received_on: self.received_on,
            method: self.method,
        };
        row::insert_payment(conn, &payment)?;
        Ok(payment.id)
    }
}

/// Importe des transactions déjà analysées (voir [`super::import`] pour les parseurs CSV/OFX,
/// des fonctions pures, séparées de cette commande). Les doublons (même date, montant,
/// libellé) sont silencieusement ignorés — un relevé réimporté par erreur ne duplique rien.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportBankTransactions {
    pub transactions: Vec<ParsedTransaction>,
}

impl Command for ImportBankTransactions {
    type Output = u32;
    const NAME: &'static str = "billing.import_bank_transactions";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let mut inserted = 0u32;
        for tx in &self.transactions {
            if row::insert_bank_transaction(conn, tx)? {
                inserted += 1;
            }
        }
        Ok(inserted)
    }
}

/// Rapproche une transaction bancaire importée avec une facture : enregistre un encaissement
/// et marque la transaction comme rapprochée.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReconcileTransaction {
    pub transaction_id: BankTransactionId,
    pub invoice_id: InvoiceId,
}

impl Command for ReconcileTransaction {
    type Output = PaymentId;
    const NAME: &'static str = "billing.reconcile_transaction";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        row::invoice_by_id(conn, self.invoice_id)?
            .ok_or(BillingError::NotFound(self.invoice_id))?;
        let tx = row::bank_transaction_by_id(conn, self.transaction_id)?
            .ok_or(BillingError::TransactionNotFound)?;

        let payment = Payment {
            id: PaymentId::new(),
            invoice_id: self.invoice_id,
            amount: Money::from_cents(tx.amount_cents),
            received_on: tx.occurred_on,
            method: PaymentMethod::BankTransfer,
        };
        row::insert_payment(conn, &payment)?;
        row::mark_transaction_matched(conn, self.transaction_id, self.invoice_id)?;
        Ok(payment.id)
    }
}
