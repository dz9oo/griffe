//! Boîte : liste et relève. Le mot de passe ne sort pas de l'outil.

use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::{tool, tool_router};

use crate::server::FreeflowServer;
use crate::support::err_text;

#[tool_router(router = boite_router, vis = "pub(crate)")]
impl FreeflowServer {
    #[tool(
        name = "boite.list",
        description = "Lettres déjà relevées. Le corps n'est pas renvoyé.",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn boite_list(&self) -> CallToolResult {
        let mut store = self.store.lock().await;
        match griffe_cli::boite::run(
            griffe_cli::boite::BoiteCommand::Liste,
            &mut store,
            &self.ctx(false),
            true,
        ) {
            Ok(text) => CallToolResult::success(vec![ContentBlock::text(text)]),
            Err(error) => err_text(error.to_string()),
        }
    }

    #[tool(
        name = "boite.relever",
        description = "Relève la boîte de réception et Envoyés du compte déjà réglé. Ne renvoie pas le mot de passe ni les corps."
    )]
    async fn boite_relever(&self) -> CallToolResult {
        let mut store = self.store.lock().await;
        match griffe_cli::boite::run(
            griffe_cli::boite::BoiteCommand::Relever,
            &mut store,
            &self.ctx(false),
            true,
        ) {
            Ok(text) => CallToolResult::success(vec![ContentBlock::text(text)]),
            Err(error) => err_text(error.to_string()),
        }
    }
}
