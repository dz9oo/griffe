//! Outils `society.*` — paysage, se payer, relevé, chapitres. Lectures pures, `today` d'adaptateur.

use freeflow_core::clock::today_local;
use freeflow_core::society::{
    closing_story, pay_yourself, society_duties, society_home, society_identity, statement_moves,
};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::server::FreeflowServer;
use crate::support::{err_text, ok_json};

fn today_or(today: Option<String>) -> Result<time::Date, String> {
    match today {
        Some(s) => freeflow_core::domain::parse_date(&s).map_err(|e| e.to_string()),
        None => Ok(today_local()),
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct TodayArgs {
    /// Date `AAAA-MM-JJ`. Défaut : aujourd'hui (heure locale).
    today: Option<String>,
}

#[tool_router(router = society_router, vis = "pub(crate)")]
impl FreeflowServer {
    /// La société : identité courte, paysage, conversations de l'année, chapitres.
    #[tool(
        name = "society.show",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn society_show_tool(&self, Parameters(args): Parameters<TodayArgs>) -> CallToolResult {
        let today = match today_or(args.today) {
            Ok(d) => d,
            Err(e) => return err_text(e),
        };
        let store = self.store.lock().await;
        match society_home(store.connection(), today) {
            Ok(home) => ok_json(home),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Se payer : montant possible ce mois-ci sans casser la piste, portes salaire / dividende.
    #[tool(
        name = "society.pay",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn society_pay_tool(&self, Parameters(args): Parameters<TodayArgs>) -> CallToolResult {
        let today = match today_or(args.today) {
            Ok(d) => d,
            Err(e) => return err_text(e),
        };
        let store = self.store.lock().await;
        match pay_yourself(store.connection(), today) {
            Ok(pay) => ok_json(pay),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Ce que tu dois à l'État : dates, montants, où déposer. Faits typés, sans sigle.
    #[tool(
        name = "society.duties",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn society_duties_tool(&self, Parameters(args): Parameters<TodayArgs>) -> CallToolResult {
        let today = match today_or(args.today) {
            Ok(d) => d,
            Err(e) => return err_text(e),
        };
        let store = self.store.lock().await;
        match society_duties(store.connection(), today) {
            Ok(duties) => ok_json(duties),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Clore l'exercice, en quelques phrases (pas seize étapes techniques).
    #[tool(
        name = "society.closing",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn society_closing_tool(
        &self,
        Parameters(args): Parameters<TodayArgs>,
    ) -> CallToolResult {
        let today = match today_or(args.today) {
            Ok(d) => d,
            Err(e) => return err_text(e),
        };
        let store = self.store.lock().await;
        match closing_story(store.connection(), today) {
            Ok(story) => ok_json(story),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Le relevé : chaque mouvement sans lecture, avec une lecture suggérée (dépense, dette, te payer).
    #[tool(
        name = "society.statement",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn society_statement_tool(
        &self,
        Parameters(args): Parameters<TodayArgs>,
    ) -> CallToolResult {
        let today = match today_or(args.today) {
            Ok(d) => d,
            Err(e) => return err_text(e),
        };
        let store = self.store.lock().await;
        match statement_moves(store.connection(), today) {
            Ok(moves) => ok_json(moves),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// La carte d'identité de la société.
    #[tool(
        name = "society.identity",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn society_identity_tool(&self) -> CallToolResult {
        let store = self.store.lock().await;
        match society_identity(store.connection()) {
            Ok(card) => ok_json(card),
            Err(e) => err_text(e.to_string()),
        }
    }
}
