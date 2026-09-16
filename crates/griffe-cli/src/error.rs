//! Erreurs de la CLI et leur code de sortie associé — un code distinct par famille, pour
//! qu'un agent (ou un script) puisse réagir sans avoir à analyser le texte du message.

use std::path::PathBuf;

use griffe_core::app::AppError;
use griffe_core::store::StoreError;

#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error(
        "coffre verrouillé : lancez `freeflow unlock`, ou fournissez --passphrase-file, \
         --passphrase-command ou --passphrase-stdin"
    )]
    Locked,

    #[error("aucun coffre à {0} — lancez `freeflow init` pour en créer un")]
    NoVault(PathBuf),

    #[error(
        "le coffre est occupé par un autre process : fermez la fenêtre FreeFlow et les autres \
         commandes en cours, puis réessayez"
    )]
    VaultBusy,

    #[error("erreur de coffre : {0}")]
    Store(#[from] StoreError),

    #[error("{0}")]
    Domain(String),

    #[error("action en attente introuvable ou déjà résolue : {0}")]
    PendingAction(String),

    #[error(
        "commande de confirmation inconnue : {0} (aucun type de commande enregistré sous ce nom)"
    )]
    UnknownConfirmableCommand(String),

    #[error("JSON invalide pour --lines : {0}")]
    InvalidLinesJson(String),

    #[error("{0}")]
    Conflict(String),

    #[error("erreur inattendue : {0}")]
    Unexpected(String),
}

impl From<AppError> for CliError {
    fn from(e: AppError) -> Self {
        match e {
            AppError::Store(StoreError::Locked) => Self::Locked,
            AppError::Store(StoreError::VaultNotFound(path)) => Self::NoVault(path),
            AppError::Store(StoreError::VaultBusy) => Self::VaultBusy,
            AppError::Store(store_err) => Self::Store(store_err),
            AppError::Domain(msg) => Self::Domain(msg),
            AppError::PendingActionNotFound(id) | AppError::PendingActionAlreadyResolved(id) => {
                Self::PendingAction(id)
            }
            AppError::PendingActionKindMismatch {
                id,
                expected,
                actual,
            } => Self::PendingAction(format!("{id} attend {expected}, pas {actual}")),
            AppError::Command(msg) => Self::Domain(msg),
            conflict @ AppError::Conflict { .. } => Self::Conflict(conflict.to_string()),
            other => Self::Unexpected(other.to_string()),
        }
    }
}

impl CliError {
    /// Code de sortie normalisé, stable par famille d'erreur — c'est le contrat que consomme
    /// un agent ou un script pilotant la CLI, indépendamment du message d'erreur.
    #[must_use]
    pub const fn exit_code(&self) -> i32 {
        match self {
            Self::Locked => 2,
            Self::Store(_) => 3,
            Self::Domain(_) => 4,
            Self::PendingAction(_) => 5,
            Self::UnknownConfirmableCommand(_) | Self::InvalidLinesJson(_) => 6,
            Self::NoVault(_) => 7,
            Self::VaultBusy => 8,
            Self::Conflict(_) => 9,
            Self::Unexpected(_) => 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// La CLI n'expose jamais de moyen direct de provoquer un conflit d'écriture (voir
    /// `cli_integration.rs::editing_twice_in_a_row_reads_the_fresh_revision_each_time` : `edit`
    /// relit toujours la révision fraîche dans la même invocation) — ce test vérifie donc
    /// seulement la traduction `AppError::Conflict -> CliError::Conflict -> code 9`, dont le
    /// déclenchement réel (deux écritures concurrentes sur la même révision) est couvert côté
    /// cœur par `griffe_core::clients::tests::updating_with_a_stale_revision_is_a_conflict…`.
    #[test]
    fn a_conflict_maps_to_its_own_exit_code() {
        let app_err = AppError::Conflict {
            entity: "client",
            id: "some-id".to_string(),
        };
        let cli_err: CliError = app_err.into();
        assert!(matches!(cli_err, CliError::Conflict(_)));
        assert_eq!(cli_err.exit_code(), 9);
    }
}
