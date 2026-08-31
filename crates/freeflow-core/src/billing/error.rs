//! Erreurs métier de la facturation.

use thiserror::Error;

use crate::app::AppError;
use crate::domain::InvoiceId;

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
}

impl From<BillingError> for AppError {
    fn from(e: BillingError) -> Self {
        Self::Domain(e.to_string())
    }
}
