//! Outils `people.*` — liste et dossier. Lectures pures, `today` d'adaptateur.

use griffe_core::clock::today_local;
use griffe_core::people::{people_list, person};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::server::FreeflowServer;
use crate::support::{err_text, ok_json};

fn today_or(today: Option<String>) -> Result<time::Date, String> {
    match today {
        Some(s) => griffe_core::domain::parse_date(&s).map_err(|e| e.to_string()),
        None => Ok(today_local()),
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct TodayArgs {
    /// Date `AAAA-MM-JJ`. Défaut : aujourd'hui (heure locale).
    today: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ShowArgs {
    /// Nom, préfixe d'UUID, UUID (fiche, opportunité, mission, devis, facture) ou nom chez qui ça sort.
    reference: String,
    /// Date `AAAA-MM-JJ`. Défaut : aujourd'hui (heure locale).
    today: Option<String>,
}

#[tool_router(router = people_router, vis = "pub(crate)")]
impl FreeflowServer {
    /// Les affaires : trois chapitres (en conversation, en mission, chez qui ça sort). Faits typés,
    /// sans phrase française.
    #[tool(
        name = "people.list",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn people_list_tool(&self, Parameters(args): Parameters<TodayArgs>) -> CallToolResult {
        let today = match today_or(args.today) {
            Ok(d) => d,
            Err(e) => return err_text(e),
        };
        let store = self.store.lock().await;
        match people_list(store.connection(), today) {
            Ok(list) => ok_json(list),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Dossier d'une personne : en cours, projet, papiers, histoire. `reference` comme
    /// `freeflow people show`.
    #[tool(
        name = "people.show",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn people_show_tool(&self, Parameters(args): Parameters<ShowArgs>) -> CallToolResult {
        let today = match today_or(args.today) {
            Ok(d) => d,
            Err(e) => return err_text(e),
        };
        let store = self.store.lock().await;
        match person(store.connection(), &args.reference, today) {
            Ok(dossier) => ok_json(dossier),
            Err(e) => err_text(e.to_string()),
        }
    }
}
