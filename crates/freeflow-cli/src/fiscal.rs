//! `freeflow fiscal ...`

use clap::Subcommand;
use freeflow_core::fiscal::{FiscalDeadline, fiscal_calendar, upcoming_deadlines};
use freeflow_core::store::Store;
use serde_json::json;
use time::Date;

use crate::error::CliError;
use crate::output::format_value;
use crate::parsers::parse_date;

#[derive(Debug, Subcommand)]
pub enum FiscalCommand {
    /// Prochaines échéances calendaires de base (CA3, acompte d'IS, CFE), sans montants — dates
    /// indicatives (année civile), voir `freeflow_core::fiscal`.
    Deadlines {
        #[arg(long, value_parser = parse_date)]
        today: Date,
    },
    /// Calendrier fiscal et social complet **chiffré** sur 12 mois, dérivé de la date de clôture
    /// d'exercice du profil : TVA à reverser (CA3, ou acomptes 3514 + CA12 au réel simplifié),
    /// acomptes et solde d'IS, liasse, AG, dépôt, DSN.
    Calendar {
        #[arg(long, value_parser = parse_date)]
        today: Date,
    },
}

fn to_json(d: &FiscalDeadline) -> serde_json::Value {
    json!({
        "kind": d.kind.as_str(),
        "due_on": d.due_on.to_string(),
        "amount_cents": d.amount.map(freeflow_core::domain::Money::cents),
        "note": d.note,
    })
}

pub fn run(cmd: FiscalCommand, store: &mut Store, json_out: bool) -> Result<String, CliError> {
    let payload: Vec<_> = match cmd {
        FiscalCommand::Deadlines { today } => {
            upcoming_deadlines(today).iter().map(to_json).collect()
        }
        FiscalCommand::Calendar { today } => fiscal_calendar(store.connection(), today)?
            .iter()
            .map(to_json)
            .collect(),
    };
    Ok(format_value(&payload, json_out))
}
