//! Erreurs de la couche applicative.

use thiserror::Error;

use crate::store::StoreError;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("erreur de persistance : {0}")]
    Store(#[from] StoreError),

    #[error("erreur SQLite : {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("erreur de (dé)sérialisation : {0}")]
    Serde(#[from] serde_json::Error),

    #[error("horodatage invalide : {0}")]
    Time(#[from] time::error::Format),

    #[error("aucune action en attente {0} : introuvable")]
    PendingActionNotFound(String),

    #[error("l'action en attente {id} attend une commande {expected}, pas {actual}")]
    PendingActionKindMismatch {
        id: String,
        expected: &'static str,
        actual: String,
    },

    #[error("l'action en attente {0} a déjà été résolue")]
    PendingActionAlreadyResolved(String),

    #[error("échec applicatif : {0}")]
    Command(String),

    #[error("règle métier violée : {0}")]
    Domain(String),
}
