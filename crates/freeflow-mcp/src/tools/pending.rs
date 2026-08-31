//! Outils `pending.*`, `audit.*` — miroir de `freeflow pending/audit` (CLI, lot 7).
//!
//! **Pas d'outil `pending.confirm` ici, volontairement** (lot 15) : rien côté MCP ne distingue
//! un appel d'outil émis par l'agent lui-même d'une confirmation humaine réelle — un agent
//! pouvait donc proposer une action sensible (`invoice.emit`…) *et* la confirmer dans le même
//! tour, ce qui annulait entièrement le rail de confirmation que `requires_confirmation()` est
//! censé garantir (voir `CLAUDE.md`). `pending.list` reste : l'agent peut voir ce qu'il attend,
//! mais la confirmation elle-même se fait au terminal (`freeflow confirm <id>`) ou dans la
//! fenêtre — un vrai geste humain, hors du canal que l'agent contrôle.

use freeflow_core::app;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use serde_json::json;

use crate::server::FreeflowServer;
use crate::support::{err_text, ok_json};

#[tool_router(router = pending_router, vis = "pub(crate)")]
impl FreeflowServer {
    /// Liste les actions en attente de confirmation humaine, proposées par un agent.
    #[tool(
        name = "pending.list",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn pending_list(&self) -> CallToolResult {
        let store = self.store.lock().await;
        match app::list_pending_actions(store.connection()) {
            Ok(actions) => ok_json(actions),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Revérifie la chaîne de hash du journal d'audit.
    #[tool(
        name = "audit.verify_chain",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn audit_verify_chain(&self) -> CallToolResult {
        let store = self.store.lock().await;
        match app::verify_chain(store.connection()) {
            Ok(app::ChainStatus::Intact) => ok_json(json!({"status": "intact"})),
            Ok(app::ChainStatus::BrokenAt(sequence)) => {
                ok_json(json!({"status": "broken", "broken_at_sequence": sequence}))
            }
            Err(e) => err_text(e.to_string()),
        }
    }
}
