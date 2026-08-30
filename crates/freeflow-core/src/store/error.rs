//! Erreurs de la couche de persistance.

use std::io;
use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("impossible d'accéder au fichier du coffre : {0}")]
    Io(#[from] io::Error),

    #[error("le fichier de paramètres de dérivation de clé est corrompu : {0}")]
    CorruptKdfParams(PathBuf),

    #[error("dérivation de la clé impossible : {0}")]
    KeyDerivation(String),

    #[error("aucune clé n'est en cache dans le trousseau du système : le coffre est verrouillé")]
    Locked,

    #[error("passphrase incorrecte, ou fichier de coffre corrompu")]
    WrongPassphrase,

    #[error("erreur SQLite : {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("échec de migration : {0}")]
    Migration(#[from] rusqlite_migration::Error),
}
