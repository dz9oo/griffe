//! `FreeflowServer` : l'adaptateur MCP, strictement équivalent à la CLI (lot 7) sur la même
//! couche applicative. Tout appel d'outil s'exécute avec `Actor::Agent { session }` — la
//! politique de confirmation (lot 2) décide seule, par commande, si l'effet s'applique
//! directement ou si une `PendingAction` est déposée pour confirmation humaine.

use std::sync::Arc;

use freeflow_core::app::{Actor, ExecutionContext};
use freeflow_core::store::Store;
use rmcp::ServerHandler;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::model::{Implementation, ServerCapabilities, ServerInfo};
use rmcp::tool_handler;
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct FreeflowServer {
    pub(crate) store: Arc<Mutex<Store>>,
    session: String,
}

impl FreeflowServer {
    #[must_use]
    pub fn new(store: Store) -> Self {
        Self {
            store: Arc::new(Mutex::new(store)),
            session: format!("mcp-{}", std::process::id()),
        }
    }

    pub(crate) fn ctx(&self) -> ExecutionContext {
        ExecutionContext::new(
            Actor::Agent {
                session: self.session.clone(),
            },
            false,
        )
    }

    pub(crate) fn tool_router() -> ToolRouter<Self> {
        Self::clients_router()
            + Self::prospection_router()
            + Self::missions_router()
            + Self::quotes_router()
            + Self::billing_router()
            + Self::pending_router()
    }
}

#[tool_handler]
impl ServerHandler for FreeflowServer {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.server_info = Implementation::from_build_env();
        info.instructions = Some(
            "FreeFlow — gestion pour indépendant. Les outils exposent exactement les mêmes \
             commandes et requêtes que la CLI `freeflow`. Les actions à effet légal ou \
             financier significatif (émission de facture, avoir) ne s'appliquent pas \
             directement : elles renvoient une action en attente (`pending_action_id`) que \
             seul un humain peut confirmer via `pending.confirm`."
                .to_string(),
        );
        info
    }
}
