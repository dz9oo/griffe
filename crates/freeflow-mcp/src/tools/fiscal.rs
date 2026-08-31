//! Outil `fiscal.calendar` — miroir de `freeflow fiscal calendar` (CLI, lot 19). Lecture seule :
//! le calendrier fiscal et social chiffré, dérivé de la date de clôture d'exercice du profil.
//! Combleble la dette de parité MCP sur `fiscal` (jusqu'ici absent du serveur).

use freeflow_core::domain::Money;
use freeflow_core::fiscal::fiscal_calendar;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::server::FreeflowServer;
use crate::support::{err_text, ok_json, ok_or_return};

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct FiscalCalendarArgs {
    /// Date du jour, au format `AAAA-MM-JJ` — le calendrier ne renvoie que les échéances à venir.
    today: String,
}

#[tool_router(router = fiscal_router, vis = "pub(crate)")]
impl FreeflowServer {
    /// Calendrier fiscal et social chiffré sur 12 mois (TVA, acomptes et solde d'IS, liasse, AG,
    /// dépôt au greffe, DSN) — dates et montants indicatifs, voir `freeflow_core::fiscal`.
    #[tool(
        name = "fiscal.calendar",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn fiscal_calendar(
        &self,
        Parameters(args): Parameters<FiscalCalendarArgs>,
    ) -> CallToolResult {
        let today = ok_or_return!("today", freeflow_core::domain::parse_date(&args.today));
        let store = self.store.lock().await;
        match fiscal_calendar(store.connection(), today) {
            Ok(calendar) => ok_json(
                calendar
                    .iter()
                    .map(|d| {
                        json!({
                            "kind": d.kind.as_str(),
                            "due_on": d.due_on.to_string(),
                            "amount_cents": d.amount.map(Money::cents),
                            "note": d.note,
                        })
                    })
                    .collect::<Vec<_>>(),
            ),
            Err(e) => err_text(e.to_string()),
        }
    }
}
