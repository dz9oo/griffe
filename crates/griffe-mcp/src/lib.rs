//! Adaptateur MCP : expose la même couche applicative que la CLI (lot 7) sous forme d'outils
//! `rmcp`, en transport stdio. Aucune logique métier ici — uniquement le mapping arguments JSON
//! ⇄ `Command`/`Query` du domaine, et la mise en forme de la réponse.

mod resources;
mod server;
mod support;
mod tools;

pub use server::FreeflowServer;

use std::path::PathBuf;

use griffe_core::store::Store;
use rmcp::ServiceExt;
use rmcp::transport::stdio;

fn resolve_db_path() -> Result<PathBuf, String> {
    if let Ok(from_env) = std::env::var("GRIFFE_DB").or_else(|_| std::env::var("FREEFLOW_DB")) {
        return Ok(PathBuf::from(from_env));
    }
    Store::resolve_default_vault_path().map_err(|err| err.to_string())
}

fn open_store(db_path: &std::path::Path) -> Result<Store, String> {
    Store::open_cached(db_path).map_err(|err| {
        format!(
            "{err} — lancez `griffe unlock --remember --ttl <durée>` avant de démarrer ce serveur"
        )
    })
}

/// Serveur MCP stdio. Le coffre vient de `GRIFFE_DB`, `FREEFLOW_DB`, ou du chemin XDG,
/// et seulement d'une session déjà en cache. Ce chemin ne demande jamais la passphrase
/// et n'ouvre pas la fenêtre.
///
/// # Errors
///
/// Le coffre est verrouillé, absent, ou le transport stdio s'arrête.
pub fn run_stdio() -> Result<(), String> {
    let db_path = resolve_db_path()?;
    let store = open_store(&db_path)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|err| err.to_string())?;
    runtime.block_on(async {
        let service = FreeflowServer::new(store)
            .serve(stdio())
            .await
            .map_err(|err| err.to_string())?;
        service.waiting().await.map_err(|err| err.to_string())?;
        Ok(())
    })
}
