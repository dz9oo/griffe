//! Boîte : liste, relève, classement, adresse. Le mot de passe ne sort pas de l'outil.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::server::FreeflowServer;
use crate::support::err_text;

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ClasserArgs {
    /// Identifiant du message, ou celui du miroir.
    message: String,
    /// Fiche : nom ou identifiant.
    fiche: String,
    /// Dossier ouvert. Absent : la fiche seule.
    dossier: Option<String>,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct AdresseArgs {
    /// Fiche : nom ou identifiant.
    fiche: String,
    /// Adresse à confirmer.
    adresse: String,
    #[serde(default)]
    dry_run: bool,
}

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

    #[tool(
        name = "boite.classer",
        description = "Range une lettre sur une fiche. Un agent la laisse en attente de confirmation.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false
        )
    )]
    async fn boite_classer(&self, Parameters(args): Parameters<ClasserArgs>) -> CallToolResult {
        let mut store = self.store.lock().await;
        match griffe_cli::boite::run(
            griffe_cli::boite::BoiteCommand::Classer {
                message: args.message,
                fiche: args.fiche,
                dossier: args.dossier,
            },
            &mut store,
            &self.ctx(args.dry_run),
            true,
        ) {
            Ok(text) => CallToolResult::success(vec![ContentBlock::text(text)]),
            Err(error) => err_text(error.to_string()),
        }
    }

    #[tool(
        name = "boite.adresse",
        description = "Dit qu'une adresse en échec est bonne. Un agent la laisse en attente de confirmation.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false
        )
    )]
    async fn boite_adresse(&self, Parameters(args): Parameters<AdresseArgs>) -> CallToolResult {
        let mut store = self.store.lock().await;
        match griffe_cli::boite::run(
            griffe_cli::boite::BoiteCommand::Adresse {
                fiche: args.fiche,
                adresse: args.adresse,
            },
            &mut store,
            &self.ctx(args.dry_run),
            true,
        ) {
            Ok(text) => CallToolResult::success(vec![ContentBlock::text(text)]),
            Err(error) => err_text(error.to_string()),
        }
    }
}
