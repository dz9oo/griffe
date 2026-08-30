//! Binaire `freeflow-mcp` : serveur MCP en transport stdio, à brancher dans un agent (Claude
//! Code, etc.). Le coffre est résolu exactement comme pour la CLI (lot 7) : `FREEFLOW_DB` pour
//! le chemin, clé en cache dans le trousseau OS en priorité puis repli sur
//! `FREEFLOW_PASSPHRASE` — le chemin non interactif qu'emprunte un agent.

use std::path::PathBuf;

use freeflow_core::store::{Store, StoreError};
use freeflow_mcp::FreeflowServer;
use rmcp::ServiceExt;
use rmcp::transport::stdio;

fn resolve_db_path() -> Result<PathBuf, String> {
    std::env::var("FREEFLOW_DB")
        .map(PathBuf::from)
        .map_err(|_| "aucun coffre indiqué : définissez FREEFLOW_DB".to_string())
}

fn open_store(db_path: &std::path::Path) -> Result<Store, String> {
    match Store::open_cached(db_path) {
        Ok(store) => return Ok(store),
        Err(StoreError::Locked) => {}
        Err(e) => return Err(e.to_string()),
    }
    let passphrase = std::env::var("FREEFLOW_PASSPHRASE")
        .map_err(|_| "coffre verrouillé : définissez FREEFLOW_PASSPHRASE".to_string())?;
    Store::open_with_passphrase(db_path, &passphrase).map_err(|e| e.to_string())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let db_path = resolve_db_path()?;
    let store = open_store(&db_path)?;
    let service = FreeflowServer::new(store).serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
