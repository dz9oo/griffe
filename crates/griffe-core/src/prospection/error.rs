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

    #[error("seule une conversation arrêtée peut être reprise ({0})")]
    NotLost(OpportunityId),

    #[error("l'estimation a besoin d'au moins une ligne de travaux")]
    EstimationRequired,

    #[error("chaque ligne de travaux a besoin d'un libellé et d'un montant")]
    EstimationLineInvalid,

    #[error("{0:?} n'est pas une étape cible valide pour cette commande")]
    InvalidTarget(OpportunityStage),

    #[error("suppression impossible : {0}")]
    HasReferences(String),

    #[error("le nom du prospect est obligatoire")]
    ProspectNameRequired,

    #[error(
        "plusieurs fiches portent déjà le nom « {0} » — précisez laquelle, ou choisissez un autre nom"
    )]
    AmbiguousProspect(String),

    #[error(
        "ce prospect est déjà un client (un devis ou une facture le référence) — modifiez-le depuis l'écran Clients"
    )]
    AlreadyAClient,
}

impl From<ProspectionError> for AppError {
    fn from(e: ProspectionError) -> Self {
        Self::Domain(e.to_string())
    }
}
