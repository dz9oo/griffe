//! Erreurs métier des missions.

use thiserror::Error;

use crate::app::AppError;
use crate::domain::MissionId;
use time::Date;

#[derive(Debug, Error, PartialEq)]
pub enum MissionsError {
    #[error("mission introuvable : {0}")]
    NotFound(MissionId),

    #[error("saisie de temps introuvable : {0}")]
    TimeEntryNotFound(crate::domain::TimeEntryId),

    #[error("suppression impossible : {0}")]
    HasReferences(String),

    #[error("la mission {0} est déjà clôturée")]
    AlreadyClosed(MissionId),

    #[error("la mission {0} n'est pas clôturée")]
    NotClosed(MissionId),

    #[error(
        "la mission {mission} ne peut pas se terminer ({ended_on}) avant d'avoir commencé ({started_on})"
    )]
    EndsBeforeStart {
        mission: MissionId,
        started_on: Date,
        ended_on: Date,
    },

    #[error("nombre de jours invalide : {0} (doit être fini et strictement positif)")]
    InvalidDays(f64),

    #[error("la somme des parts des jalons dépasse 100 % ({total_bps} dix-millièmes)")]
    MilestonesOverBudget { total_bps: u32 },

    #[error("un jalon ne peut pas avoir un libellé vide")]
    EmptyMilestoneLabel,

    #[error("le devis lié à la mission {0} ne peut pas être changé après création")]
    QuoteLinkImmutable(MissionId),
}

impl From<MissionsError> for AppError {
    fn from(e: MissionsError) -> Self {
        Self::Domain(e.to_string())
    }
}
