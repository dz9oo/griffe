//! Erreurs métier des missions.

use thiserror::Error;

use crate::app::AppError;
use crate::domain::MissionId;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum MissionsError {
    #[error("mission introuvable : {0}")]
    NotFound(MissionId),
}

impl From<MissionsError> for AppError {
    fn from(e: MissionsError) -> Self {
        Self::Domain(e.to_string())
    }
}
