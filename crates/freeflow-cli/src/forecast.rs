//! `freeflow forecast ...`

use clap::Subcommand;
use freeflow_core::domain::Money;
use freeflow_core::forecast::{build_forecast_inputs, first_shortfall_month, forecast_12_months};
use freeflow_core::store::Store;
use serde_json::json;
use time::Date;

use crate::error::CliError;
use crate::output::format_value;
use crate::parsers::{parse_date, parse_money};

#[derive(Debug, Subcommand)]
pub enum ForecastCommand {
    /// Prévisionnel de trésorerie sur 12 mois — factures impayées, missions signées non
    /// facturées, pipeline pondéré, moins une estimation des charges récurrentes.
    Show {
        #[arg(long, value_parser = parse_money)]
        starting_cash: Money,
        #[arg(long, value_parser = parse_date)]
        today: Date,
    },
}

pub fn run(cmd: ForecastCommand, store: &Store, json: bool) -> Result<String, CliError> {
    let ForecastCommand::Show {
        starting_cash,
        today,
    } = cmd;
    let inputs = build_forecast_inputs(store.connection(), today, starting_cash)?;
    let forecast = forecast_12_months(today, &inputs);
    let shortfall = first_shortfall_month(&forecast);

    let payload = json!({
        "months": forecast.iter().map(|f| json!({
            "month": f.month.to_string(),
            "inflow_cents": f.inflow.cents(),
            "outflow_cents": f.outflow.cents(),
            "projected_cash_cents": f.projected_cash.cents(),
        })).collect::<Vec<_>>(),
        "first_shortfall_month": shortfall.map(|m| m.to_string()),
    });
    Ok(format_value(&payload, json))
}
