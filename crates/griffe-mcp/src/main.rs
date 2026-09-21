//! Binaire `griffe-mcp` : serveur MCP en transport stdio, à brancher dans un agent (Claude
//! Code, etc.). Le coffre est résolu comme pour la CLI (`GRIFFE_DB` / `FREEFLOW_DB`, ou
//! l'emplacement XDG par défaut), mais uniquement via une session déjà en cache dans le
//! trousseau OS : ce process n'a pas de terminal, il ne peut donc jamais prompter. Lancez
//! `griffe unlock --remember --ttl <durée>` avant de démarrer ce serveur.

use std::path::PathBuf;

use griffe_core::store::Store;
use griffe_mcp::FreeflowServer;
use rmcp::ServiceExt;
use rmcp::transport::stdio;

fn resolve_db_path() -> Result<PathBuf, String> {
    if let Ok(from_env) = std::env::var("GRIFFE_DB").or_else(|_| std::env::var("FREEFLOW_DB")) {
        return Ok(PathBuf::from(from_env));
    }
    Store::resolve_default_vault_path().map_err(|e| e.to_string())
}

fn open_store(db_path: &std::path::Path) -> Result<Store, String> {
    Store::open_cached(db_path).map_err(|e| {
        format!(
            "{e} — lancez `griffe unlock --remember --ttl <durée>` avant de démarrer ce serveur"
        )
    })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let db_path = resolve_db_path()?;
    let store = open_store(&db_path)?;
    let service = FreeflowServer::new(store).serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
