//! Outil `forecast.*` — miroir de `freeflow forecast show` (CLI). Une lecture pure : le
//! prévisionnel est recalculé à chaque appel depuis les factures impayées, les missions signées
//! non facturées et le pipeline pondéré — rien n'est persisté.

use griffe_core::domain::Money;
use griffe_core::forecast::{build_forecast_inputs, first_shortfall_month, forecast_12_months};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::server::FreeflowServer;
use crate::support::{err_text, ok_json, ok_or_return};

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ForecastShowArgs {
    /// Trésorerie de départ, en centimes.
    starting_cash_cents: i64,
    /// Date du jour, au format `AAAA-MM-JJ`.
    today: String,
}

#[tool_router(router = forecast_router, vis = "pub(crate)")]
impl FreeflowServer {
    /// Prévisionnel de trésorerie sur 12 mois — factures impayées, missions signées non
    /// facturées, pipeline pondéré, moins une estimation des charges récurrentes. Renvoie aussi
    /// le premier mois projeté en découvert (`first_shortfall_month`), s'il y en a un.
    #[tool(
        name = "forecast.show",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn forecast_show(
        &self,
        Parameters(args): Parameters<ForecastShowArgs>,
    ) -> CallToolResult {
        let today = ok_or_return!("today", griffe_core::domain::parse_date(&args.today));
        let store = self.store.lock().await;
        let inputs = match build_forecast_inputs(
            store.connection(),
            today,
            Money::from_cents(args.starting_cash_cents),
        ) {
            Ok(inputs) => inputs,
            Err(e) => return err_text(e.to_string()),
        };
        let forecast = forecast_12_months(today, &inputs);
        let shortfall = first_shortfall_month(&forecast);
        ok_json(json!({
            "months": forecast.iter().map(|f| json!({
                "month": f.month.to_string(),
                "inflow_cents": f.inflow.cents(),
                "outflow_cents": f.outflow.cents(),
                "projected_cash_cents": f.projected_cash.cents(),
            })).collect::<Vec<_>>(),
            "first_shortfall_month": shortfall.map(|m| m.to_string()),
        }))
    }
}
