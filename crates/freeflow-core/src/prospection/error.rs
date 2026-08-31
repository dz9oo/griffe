//! Erreurs métier de la prospection.

use thiserror::Error;

use crate::app::AppError;
use crate::domain::{InteractionId, OpportunityId, OpportunityStage};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProspectionError {
    #[error("opportunité introuvable : {0}")]
    NotFound(OpportunityId),

    #[error("interaction introuvable : {0}")]
    InteractionNotFound(InteractionId),

    #[error("l'opportunité {0} est déjà dans une étape close, aucune transition n'est possible")]
    AlreadyClosed(OpportunityId),

    #[error("{0:?} n'est pas une étape cible valide pour cette commande")]
    InvalidTarget(OpportunityStage),

    #[error("suppression impossible : {0}")]
    HasReferences(String),
}

impl From<ProspectionError> for AppError {
    fn from(e: ProspectionError) -> Self {
        Self::Domain(e.to_string())
    }
}
