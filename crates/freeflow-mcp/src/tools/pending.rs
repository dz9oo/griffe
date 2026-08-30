//! Outils `pending.*`, `audit.*` — miroir de `freeflow pending/audit/confirm` (CLI, lot 7).
//!
//! `pending.confirm` ne connaît que les commandes qui déclarent
//! `requires_confirmation() == true` — à étendre au fil des lots suivants s'il y en a d'autres.

use freeflow_core::app::{self, Command, Executor, PendingActionId};
use freeflow_core::billing::{EmitInvoice, IssueCreditNote};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::server::FreeflowServer;
use crate::support::{err_text, ok_json, ok_or_return, outcome_json};

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct PendingActionIdArgs {
    /// Identifiant de l'action en attente.
    id: String,
}

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

    /// Confirme une action en attente : retrouve son type de commande d'origine par son nom
    /// stocké, puis l'applique. C'est le seul moyen d'appliquer une commande à confirmation
    /// (`invoice.emit`, `invoice.credit_note`) déposée par un agent.
    #[tool(
        name = "pending.confirm",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false
        )
    )]
    async fn pending_confirm(
        &self,
        Parameters(args): Parameters<PendingActionIdArgs>,
    ) -> CallToolResult {
        let id: PendingActionId = ok_or_return!("id", args.id.parse());
        let mut store = self.store.lock().await;
        let action = match app::pending_action_by_id(store.connection(), id) {
            Ok(Some(action)) => action,
            Ok(None) => return err_text(format!("action en attente introuvable : {id}")),
            Err(e) => return err_text(e.to_string()),
        };

        if action.command_name == EmitInvoice::NAME {
            match Executor::new(&mut store).confirm::<EmitInvoice>(id) {
                Ok(outcome) => ok_json(outcome_json(&outcome)),
                Err(e) => err_text(e.to_string()),
            }
        } else if action.command_name == IssueCreditNote::NAME {
            match Executor::new(&mut store).confirm::<IssueCreditNote>(id) {
                Ok(outcome) => ok_json(outcome_json(&outcome)),
                Err(e) => err_text(e.to_string()),
            }
        } else {
            err_text(format!(
                "commande de confirmation inconnue : {} (aucun type de commande enregistré sous ce nom)",
                action.command_name
            ))
        }
    }
}
