//! État partagé du routeur : une seule connexion `SQLCipher`, protégée par un mutex — la GUI
//! est mono-process comme la CLI et le serveur MCP (lot 8), et suit le même modèle de
//! concurrence multi-process décrit dans le plan (WAL + `busy_timeout`, pas de démon).

use std::sync::Arc;

use freeflow_core::app::{Actor, ExecutionContext};
use freeflow_core::store::Store;
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Mutex<Store>>,
}

impl AppState {
    #[must_use]
    pub fn new(store: Store) -> Self {
        Self {
            store: Arc::new(Mutex::new(store)),
        }
    }

    /// Toute action déclenchée depuis la GUI est un acte humain direct — contrairement au
    /// serveur MCP (lot 8), il n'y a pas d'acteur agent ici : la console exécute la même CLI,
    /// mais tapée par la personne qui a la souris.
    #[must_use]
    pub fn human_ctx() -> ExecutionContext {
        ExecutionContext::new(Actor::Human, false)
    }
}
