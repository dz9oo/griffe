//! Erreurs métier de la facturation.

use thiserror::Error;

use crate::app::AppError;
use crate::domain::{BankTransactionId, InvoiceId, PaymentId};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum BillingError {
    #[error("facture introuvable : {0}")]
    NotFound(InvoiceId),

    #[error("une facture doit avoir au moins une ligne")]
    EmptyInvoice,

    #[error("impossible d'émettre un avoir sur un avoir ({0}) : annulez la facture d'origine")]
    CannotCreditACreditNote(InvoiceId),

    #[error("la facture {0} a déjà un avoir associé")]
    AlreadyCredited(InvoiceId),

    #[error("transaction bancaire introuvable")]
    TransactionNotFound,

    #[error("montant d'encaissement invalide : doit être strictement positif")]
    InvalidPaymentAmount,

    #[error("encaissement introuvable : {0}")]
    PaymentNotFound(PaymentId),

    #[error("l'encaissement {0} est déjà annulé")]
    PaymentAlreadyVoided(PaymentId),

    #[error(
        "la transaction {0} est déjà rapprochée — défaites d'abord ce rapprochement \
         (bank unreconcile) si elle visait la mauvaise facture"
    )]
    AlreadyReconciled(BankTransactionId),

    #[error("la transaction {0} n'est pas rapprochée : rien à défaire")]
    TransactionNotReconciled(BankTransactionId),
}

impl From<BillingError> for AppError {
    fn from(e: BillingError) -> Self {
        Self::Domain(e.to_string())
    }
}
