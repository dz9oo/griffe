//! Erreurs de la CLI et leur code de sortie associé — un code distinct par famille, pour
//! qu'un agent (ou un script) puisse réagir sans avoir à analyser le texte du message.

use std::path::PathBuf;

use freeflow_core::app::AppError;
use freeflow_core::store::StoreError;

#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error(
        "coffre verrouillé : lancez `freeflow unlock`, ou fournissez --passphrase-file, \
         --passphrase-command ou --passphrase-stdin"
    )]
    Locked,

    #[error("aucun coffre à {0} — lancez `freeflow init` pour en créer un")]
    NoVault(PathBuf),

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

    #[error("erreur inattendue : {0}")]
    Unexpected(String),
}

impl From<AppError> for CliError {
    fn from(e: AppError) -> Self {
        match e {
            AppError::Store(StoreError::Locked) => Self::Locked,
            AppError::Store(StoreError::VaultNotFound(path)) => Self::NoVault(path),
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
            Self::Unexpected(_) => 1,
        }
    }
}
