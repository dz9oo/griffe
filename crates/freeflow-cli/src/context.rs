//! Résolution du coffre : chemin (`--db` ou `FREEFLOW_DB`) et déverrouillage (clé en cache
//! dans le trousseau OS en priorité — le chemin non interactif qu'emprunte un agent — puis
//! repli sur `FREEFLOW_PASSPHRASE`).

use std::path::PathBuf;

use freeflow_core::store::{Store, StoreError};

use crate::error::CliError;

/// # Errors
pub fn resolve_db_path(explicit: Option<PathBuf>) -> Result<PathBuf, CliError> {
    if let Some(path) = explicit {
        return Ok(path);
    }
    std::env::var("FREEFLOW_DB")
        .map(PathBuf::from)
        .map_err(|_| {
            CliError::Unexpected(
                "aucun coffre indiqué : utilisez --db ou définissez FREEFLOW_DB".to_string(),
            )
        })
}

/// # Errors
pub fn open_store(db_path: &std::path::Path) -> Result<Store, CliError> {
    match Store::open_cached(db_path) {
        Ok(store) => return Ok(store),
        Err(StoreError::Locked) => {}
        Err(e) => return Err(e.into()),
    }
    let passphrase = std::env::var("FREEFLOW_PASSPHRASE").map_err(|_| CliError::Locked)?;
    Store::open_with_passphrase(db_path, &passphrase).map_err(Into::into)
}
