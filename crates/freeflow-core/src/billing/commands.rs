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

    // Effet comptable sensible : un encaissement marque une facture (partiellement) payée, fausse
    // la balance âgée, le prévisionnel et la CA3. Déclenché par un agent, il attend une
    // confirmation humaine — au même titre qu'une suppression de client.
    fn requires_confirmation(&self) -> bool {
        true
    }

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        // Montant strictement positif : un montant nul, négatif ou absurde n'est pas un
        // encaissement — il empoisonnerait les sommes (jusqu'au débordement de `aged_balance`).
        if self.amount.cents() <= 0 {
            return Err(BillingError::InvalidPaymentAmount.into());
        }
        row::invoice_by_id(conn, self.invoice_id)?
            .ok_or(BillingError::NotFound(self.invoice_id))?;
        let payment = Payment {
            id: PaymentId::new(),
            invoice_id: self.invoice_id,
            amount: self.amount,
            received_on: self.received_on,
            method: self.method,
            bank_transaction_id: None,
            voided_at: None,
        };
        row::insert_payment(conn, &payment)?;
        Ok(payment.id)
    }
}

/// Annule un encaissement saisi à tort — une contre-écriture, jamais une suppression : le
/// paiement reste dans l'historique (et dans le journal d'audit chaîné) mais sort de tous les
/// calculs (statut payé, balance âgée, prévisionnel). S'il était issu d'un rapprochement
/// bancaire, la transaction est libérée dans le même geste : la laisser « rapprochée » vers une
/// facture sans encaissement mentirait au prochain rapprochement.
///
/// `reason` n'a pas de colonne : il voyage dans la commande elle-même, donc dans le
/// `command_json` du journal d'audit — c'est là que vit la trace d'une correction comptable,
/// pas dans la ligne corrigée.
///
/// Aucune garde d'exercice clos, contrairement aux dépenses : la TVA comme l'IS sont calculés
/// sur les débits (factures émises), jamais sur les encaissements — annuler un paiement n'a
/// aucun effet fiscal (voir `accounting::vat_due_for_period`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoidPayment {
    pub payment_id: PaymentId,
    #[serde(default)]
    pub reason: Option<String>,
}

impl Command for VoidPayment {
    type Output = ();
    const NAME: &'static str = "billing.void_payment";

    // Le miroir de `RecordPayment` : mêmes conséquences comptables, même barrière.
    fn requires_confirmation(&self) -> bool {
        true
    }

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let payment = row::payment_by_id(conn, self.payment_id)?
            .ok_or(BillingError::PaymentNotFound(self.payment_id))?;
        // Verrou sémantique plutôt qu'optimiste : l'annulation est la seule mutation possible
        // d'un paiement et n'arrive qu'une fois — voir le commentaire de la migration 0013.
        if payment.is_voided() {
            return Err(BillingError::PaymentAlreadyVoided(self.payment_id).into());
        }
        let voided_at = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)?;
        row::mark_payment_voided(conn, self.payment_id, &voided_at)?;
        if let Some(transaction_id) = payment.bank_transaction_id {
            row::clear_transaction_match(conn, transaction_id)?;
        }
        Ok(())
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

    // Même effet qu'un encaissement manuel (il en crée un) : confirmation humaine requise pour un
    // agent.
    fn requires_confirmation(&self) -> bool {
        true
    }

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        row::invoice_by_id(conn, self.invoice_id)?
            .ok_or(BillingError::NotFound(self.invoice_id))?;
        let tx = row::bank_transaction_by_id(conn, self.transaction_id)?
            .ok_or(BillingError::TransactionNotFound)?;

        // Bug latent corrigé au lot 22 : rapprocher une transaction déjà rapprochée créait un
        // second encaissement pour la même ligne de relevé — un doublon silencieux dans le
        // solde de la facture. Défaire d'abord (`UnreconcileTransaction`) si le rapprochement
        // visait la mauvaise facture.
        if tx.matched_invoice_id.is_some() {
            return Err(BillingError::AlreadyReconciled(self.transaction_id).into());
        }

        // Un débit (montant négatif) rapproché comme un encaissement fausserait le solde de la
        // facture : seul un crédit (montant positif) est un règlement.
        if tx.amount_cents <= 0 {
            return Err(BillingError::InvalidPaymentAmount.into());
        }

        let payment = Payment {
            id: PaymentId::new(),
            invoice_id: self.invoice_id,
            amount: Money::from_cents(tx.amount_cents),
            received_on: tx.occurred_on,
            method: PaymentMethod::BankTransfer,
            bank_transaction_id: Some(self.transaction_id),
            voided_at: None,
        };
        row::insert_payment(conn, &payment)?;
        row::mark_transaction_matched(conn, self.transaction_id, self.invoice_id)?;
        Ok(payment.id)
    }
}

/// Défait un rapprochement : libère la transaction bancaire et annule l'encaissement qui en
/// était issu (contre-écriture, comme [`VoidPayment`]) — l'exact inverse de
/// [`ReconcileTransaction`], en un seul geste atomique.
///
/// Pour un rapprochement antérieur à la migration `0013`, la lignée transaction → paiement
/// n'existe pas en base : seule la transaction est libérée (`Output = None`), et le paiement
/// orphelin s'annule séparément via `VoidPayment` — plutôt qu'une heuristique par date et
/// montant qui pourrait annuler le mauvais paiement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnreconcileTransaction {
    pub transaction_id: BankTransactionId,
}

impl Command for UnreconcileTransaction {
    /// L'encaissement annulé dans le même geste, s'il a pu être retrouvé par sa lignée.
    type Output = Option<PaymentId>;
    const NAME: &'static str = "billing.unreconcile_transaction";

    // Le miroir de `ReconcileTransaction` : mêmes conséquences comptables, même barrière.
    fn requires_confirmation(&self) -> bool {
        true
    }

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let tx = row::bank_transaction_by_id(conn, self.transaction_id)?
            .ok_or(BillingError::TransactionNotFound)?;
        if tx.matched_invoice_id.is_none() {
            return Err(BillingError::TransactionNotReconciled(self.transaction_id).into());
        }

        let linked_payment = row::all_payments(conn)?
            .into_iter()
            .find(|p| p.bank_transaction_id == Some(self.transaction_id) && !p.is_voided());
        let voided = match linked_payment {
            Some(payment) => {
                let voided_at = time::OffsetDateTime::now_utc()
                    .format(&time::format_description::well_known::Rfc3339)?;
                row::mark_payment_voided(conn, payment.id, &voided_at)?;
                Some(payment.id)
            }
            None => None,
        };
        row::clear_transaction_match(conn, self.transaction_id)?;
        Ok(voided)
    }
}
