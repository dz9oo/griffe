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

    #[error("aucune session valide dans le trousseau du système : le coffre est verrouillé")]
    Locked,

    #[error("passphrase incorrecte, ou fichier de coffre corrompu")]
    WrongPassphrase,

    #[error("aucun coffre à {0} — lancez `freeflow init` pour en créer un")]
    VaultNotFound(PathBuf),

    #[error("un coffre existe déjà à {0}")]
    VaultAlreadyExists(PathBuf),

    #[error(
        "un fichier existe déjà à la destination {0} : refus de l'écraser (choisissez une \
         destination neuve)"
    )]
    BackupDestinationExists(PathBuf),

    #[error("trousseau du système indisponible : la session ne sera pas conservée")]
    KeychainUnavailable,

    #[error("impossible de déterminer un emplacement de coffre par défaut sur ce système")]
    NoDefaultVaultPath,

    #[error("erreur SQLite : {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("échec de migration : {0}")]
    Migration(rusqlite_migration::Error),

    #[error("cette version est trop ancienne pour ce coffre — téléchargez la dernière")]
    SchemaTooNew,

    #[error(
        "le coffre est ouvert par un autre process : fermez la fenêtre FreeFlow et les autres \
         commandes en cours, puis réessayez"
    )]
    VaultBusy,

    #[error(
        "un changement de passphrase a été interrompu sur {0} : réessayez avec la NOUVELLE \
         passphrase, ou restaurez la sauvegarde écrite juste avant le changement"
    )]
    PassphraseChangeInterrupted(PathBuf),
}

impl From<rusqlite_migration::Error> for StoreError {
    fn from(err: rusqlite_migration::Error) -> Self {
        match err {
            rusqlite_migration::Error::MigrationDefinition(
                rusqlite_migration::MigrationDefinitionError::DatabaseTooFarAhead,
            ) => Self::SchemaTooNew,
            other => Self::Migration(other),
        }
    }
}
