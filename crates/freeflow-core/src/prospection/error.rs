//! Erreurs métier de la prospection.

use thiserror::Error;

use crate::app::AppError;
use crate::domain::{OpportunityId, OpportunityStage};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProspectionError {
    #[error("opportunité introuvable : {0}")]
    NotFound(OpportunityId),

    #[error("l'opportunité {0} est déjà dans une étape close, aucune transition n'est possible")]
    AlreadyClosed(OpportunityId),

    #[error("{0:?} n'est pas une étape cible valide pour cette commande")]
    InvalidTarget(OpportunityStage),
}

impl From<ProspectionError> for AppError {
    fn from(e: ProspectionError) -> Self {
        Self::Domain(e.to_string())
    }
}
