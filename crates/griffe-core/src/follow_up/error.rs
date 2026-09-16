//! Erreurs métier des relances.

use thiserror::Error;

use crate::app::AppError;
use crate::domain::{FollowUpEventId, FollowUpSubject, InvalidEmail};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FollowUpError {
    #[error("aucune relance ne correspond à ce dossier")]
    NotFound(FollowUpSubject),

    #[error("cette opportunité n'est plus à relancer (gagnée, perdue ou archivée)")]
    OpportunityInactive,

    #[error("cette facture n'est plus à relancer (soldée ou annulée par avoir)")]
    InvoiceInactive,

    #[error("la cadence est terminée — posez une prochaine date, ou laissez le dossier")]
    Exhausted,

    #[error("indiquez l'email avec lequel vous écrivez (follow-up from)")]
    SenderMissing,

    #[error("aucun contact avec un email : ajoutez-en un sur la fiche")]
    RecipientMissing,

    #[error("{0}")]
    InvalidEmail(InvalidEmail),

    #[error("une date de relance doit être après aujourd'hui")]
    DateNotInFuture,

    #[error("rien à annuler : aucun geste de relance sur ce dossier")]
    NothingToRetract,

    #[error("événement de relance introuvable : {0}")]
    EventNotFound(FollowUpEventId),
}

impl From<FollowUpError> for AppError {
    fn from(e: FollowUpError) -> Self {
        Self::Domain(e.to_string())
    }
}

impl From<InvalidEmail> for FollowUpError {
    fn from(e: InvalidEmail) -> Self {
        Self::InvalidEmail(e)
    }
}
