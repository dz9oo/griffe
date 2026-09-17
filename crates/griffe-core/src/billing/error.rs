//! Erreurs métier de la facturation.

use thiserror::Error;

use crate::app::AppError;
use crate::domain::{BankTransactionId, InvoiceId, PaymentId, WriteOffId};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum BillingError {
    #[error("facture introuvable : {0}")]
    NotFound(InvoiceId),

    #[error("une facture doit avoir au moins une ligne")]
    EmptyInvoice,

    #[error("un numéro de facture est obligatoire — c'est celui de la PA, pas un FA- inventé ici")]
    EmptyInvoiceNumber,

    #[error("une facture porte déjà le numéro {0}")]
    DuplicateInvoiceNumber(String),

    #[error("cette mission n'appartient pas à ce client")]
    MissionDoesNotBelongToClient,

    #[error(
        "la facture {0} est née ailleurs : importez l'avoir (même bouche que la facture), n'émettez pas un FA-"
    )]
    CannotCreditImportedInvoice(InvoiceId),

    #[error("impossible d'émettre un avoir sur un avoir ({0}) : annulez la facture d'origine")]
    CannotCreditACreditNote(InvoiceId),

    #[error("la facture {0} a déjà un avoir associé")]
    AlreadyCredited(InvoiceId),

    #[error("impossible de passer en perte un avoir ({0}) : ce n'est pas une créance")]
    CannotWriteOffCreditNote(InvoiceId),

    #[error("la facture {0} a déjà un avoir : on ne mélange pas avoir et perte")]
    CannotWriteOffAlreadyCredited(InvoiceId),

    #[error("la facture {0} est déjà passée en perte")]
    AlreadyWrittenOff(InvoiceId),

    #[error("rien à passer en perte : le solde de la facture {0} est déjà à zéro")]
    NothingOutstanding(InvoiceId),

    #[error("on ne peut pas constater la perte avant la date d'émission de la facture {0}")]
    WriteOffBeforeIssue(InvoiceId),

    #[error("on ne peut plus constater ni rétablir une perte : cette période est déjà déposée")]
    WriteOffPeriodAlreadyFiled,

    #[error("perte introuvable : {0}")]
    WriteOffNotFound(WriteOffId),

    #[error("cette perte est déjà rétractée")]
    WriteOffAlreadyRetracted,

    #[error("cet exercice est déjà clos")]
    ExerciseClosed,

    #[error("la facture {0} est déjà passée en perte : un avoir n'est plus le bon geste")]
    CannotCreditWrittenOff(InvoiceId),

    #[error("un avoir doit porter sur une facture du même client")]
    ImportedCreditNoteWrongClient,

    #[error(
        "un avoir importé doit totaliser un TTC strictement négatif — inversez la quantité, \
         n'ajoutez pas une seconde créance"
    )]
    ImportedCreditNoteMustBeNegative,

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
         si elle visait la mauvaise facture ou la mauvaise dépense"
    )]
    AlreadyReconciled(BankTransactionId),

    #[error("la transaction {0} n'est pas rapprochée : rien à défaire")]
    TransactionNotReconciled(BankTransactionId),

    #[error(
        "la transaction {0} n'est pas un règlement de compte de bilan : rien à défaire (pour un \
         rapprochement de facture ou de dépense, défaites-le d'abord)"
    )]
    TransactionNotSettled(BankTransactionId),

    #[error(
        "le compte {0} n'est pas dans le plan de comptes de FreeFlow : donnez-lui un libellé \
         (--label), comme au bilan d'ouverture"
    )]
    SettlementLabelRequired(String),

    #[error(
        "la transaction {0} est rapprochée (facture, dépense ou règlement) : défaites d'abord le \
         rapprochement avant de la supprimer"
    )]
    TransactionStillMatched(BankTransactionId),
}

impl From<BillingError> for AppError {
    fn from(e: BillingError) -> Self {
        Self::Domain(e.to_string())
    }
}
