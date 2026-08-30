//! `freeflow fiscal ...`

use clap::Subcommand;
use freeflow_core::fiscal::{FiscalDeadlineKind, upcoming_deadlines};
use serde_json::json;
use time::Date;

use crate::error::CliError;
use crate::output::format_value;
use crate::parsers::parse_date;

fn kind_label(kind: FiscalDeadlineKind) -> &'static str {
    match kind {
        FiscalDeadlineKind::Ca3 => "ca3",
        FiscalDeadlineKind::IsAcompte => "is_acompte",
        FiscalDeadlineKind::Cfe => "cfe",
    }
}

#[derive(Debug, Subcommand)]
pub enum FiscalCommand {
    /// Prochaines échéances CA3, acompte d'IS et CFE — dates indicatives, voir
    /// `freeflow_core::fiscal`.
    Deadlines {
        #[arg(long, value_parser = parse_date)]
        today: Date,
    },
}

pub fn run(cmd: FiscalCommand, json: bool) -> Result<String, CliError> {
    let FiscalCommand::Deadlines { today } = cmd;
    let payload: Vec<_> = upcoming_deadlines(today)
        .iter()
        .map(|d| json!({"kind": kind_label(d.kind), "due_on": d.due_on.to_string()}))
        .collect();
    Ok(format_value(&payload, json))
}
