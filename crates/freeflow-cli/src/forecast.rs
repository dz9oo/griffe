//! `freeflow forecast ...`

use clap::Subcommand;
use freeflow_core::clock::today_local;
use freeflow_core::domain::Money;
use freeflow_core::forecast::{build_forecast_inputs, first_shortfall_month, forecast_12_months};
use freeflow_core::store::Store;
use serde_json::json;
use time::Date;

use crate::error::CliError;
use crate::output::format_json;
use crate::parsers::{parse_date, parse_money};
use crate::table;

#[derive(Debug, Subcommand)]
pub enum ForecastCommand {
    /// Prévisionnel de trésorerie sur 12 mois — factures impayées, missions signées non
    /// facturées, pipeline pondéré, moins une estimation des charges récurrentes.
    Show {
        #[arg(long, value_parser = parse_money)]
        starting_cash: Money,
        /// Date du jour (défaut : aujourd'hui, heure locale).
        #[arg(long, value_parser = parse_date)]
        today: Option<Date>,
    },
}

pub fn run(cmd: ForecastCommand, store: &Store, json: bool) -> Result<String, CliError> {
    let ForecastCommand::Show {
        starting_cash,
        today,
    } = cmd;
    let today = today.unwrap_or_else(today_local);
    let inputs = build_forecast_inputs(store.connection(), today, starting_cash)?;
    let forecast = forecast_12_months(today, &inputs);
    let shortfall = first_shortfall_month(&forecast);

    if json {
        let payload = json!({
            "months": forecast.iter().map(|f| json!({
                "month": f.month.to_string(),
                "inflow_cents": f.inflow.cents(),
                "outflow_cents": f.outflow.cents(),
                "projected_cash_cents": f.projected_cash.cents(),
            })).collect::<Vec<_>>(),
            "first_shortfall_month": shortfall.map(|m| m.to_string()),
        });
        return Ok(format_json(&payload));
    }
    let rows: Vec<Vec<String>> = forecast
        .iter()
        .map(|f| {
            vec![
                f.month.to_string(),
                f.inflow.to_string(),
                f.outflow.to_string(),
                f.projected_cash.to_string(),
            ]
        })
        .collect();
    let mut out = format!(
        "Prévisionnel de trésorerie sur 12 mois depuis le {} (trésorerie de départ {starting_cash})\n{}",
        freeflow_core::domain::format_date(today),
        table::render(&["Mois", "Entrées", "Sorties", "Trésorerie"], &rows)
    );
    match shortfall {
        Some(month) => out.push_str(&format!("\n⚠ première tension de trésorerie : {month}")),
        None => out.push_str("\nAucune tension de trésorerie sur l'horizon."),
    }
    Ok(out)
}
