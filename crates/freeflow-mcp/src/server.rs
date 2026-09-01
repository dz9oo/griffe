//! `FreeflowServer` : l'adaptateur MCP, strictement équivalent à la CLI (lot 7) sur la même
//! couche applicative. Tout appel d'outil s'exécute avec `Actor::Agent { session }` — la
//! politique de confirmation (lot 2) décide seule, par commande, si l'effet s'applique
//! directement ou si une `PendingAction` est déposée pour confirmation humaine.

use std::sync::Arc;

use freeflow_core::app::{Actor, ExecutionContext};
use freeflow_core::store::Store;
use rmcp::RoleServer;
use rmcp::ServerHandler;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::model::{
    ErrorData as McpError, Implementation, ListResourceTemplatesResult, ListResourcesResult,
    PaginatedRequestParams, ReadResourceRequestParams, ReadResourceResponse, ServerCapabilities,
    ServerInfo,
};
use rmcp::service::RequestContext;
use rmcp::tool_handler;
use tokio::sync::Mutex;

use crate::resources;

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

    /// `dry_run` : les outils `clients.*` l'exposent comme argument (`dry_run: bool`, défaut
    /// `false`) — l'équivalent du `--dry-run` de la CLI, absent des autres modules d'outils
    /// pour l'instant (voir la feuille de route du lot 15 dans `CLAUDE.md`).
    pub(crate) fn ctx(&self, dry_run: bool) -> ExecutionContext {
        ExecutionContext::new(
            Actor::Agent {
                session: self.session.clone(),
            },
            dry_run,
        )
    }

    pub(crate) fn tool_router() -> ToolRouter<Self> {
        Self::clients_router()
            + Self::prospection_router()
            + Self::missions_router()
            + Self::quotes_router()
            + Self::expenses_router()
            + Self::billing_router()
            + Self::pending_router()
            + Self::fiscal_router()
    }
}

#[tool_handler]
impl ServerHandler for FreeflowServer {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_resources()
            .build();
        info.server_info = Implementation::from_build_env();
        info.instructions = Some(
            "FreeFlow — gestion pour indépendant. Les outils exposent les mêmes commandes et \
             requêtes que la CLI `freeflow`. Les actions à effet légal, financier ou \
             destructeur significatif (émission de facture, avoir, suppression d'un client, \
             d'une opportunité ou d'une mission) ne s'appliquent pas directement : elles \
             renvoient une action en attente (`pending_action_id`, consultable via \
             `pending.list`) qu'un humain doit confirmer lui-même, au terminal (`freeflow \
             confirm <id>`) ou dans la fenêtre — il n'existe volontairement aucun outil MCP \
             `pending.confirm` : un agent ne peut pas confirmer sa propre proposition. Les \
             références à un client, une opportunité, une mission, un devis ou une dépense \
             (`client`, `opportunity`, `mission`, `quote`, `expense`, \
             `clients.show`/`prospect.show`/`mission.show`/`quote.show`/`expense.show`…) \
             acceptent un UUID, un préfixe d'UUID, ou un nom/libellé (pour un devis : le nom du \
             client porteur) — voir les outils `*.list`, ou les ressources \
             `freeflow://clients`, `freeflow://opportunities`, `freeflow://missions`, \
             `freeflow://quotes`, `freeflow://expenses`."
                .to_string(),
        );
        info
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        Ok(resources::list())
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, McpError> {
        Ok(resources::list_templates())
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        let store = self.store.lock().await;
        resources::read(&store, &request.uri).map(Into::into)
    }
}
