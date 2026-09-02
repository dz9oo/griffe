//! `freeflow fiscal ...`

use clap::Subcommand;
use freeflow_core::clock::today_local;
use freeflow_core::domain::format_date;
use freeflow_core::fiscal::{FiscalDeadline, fiscal_calendar, upcoming_deadlines};
use freeflow_core::store::Store;
use serde_json::json;
use time::Date;

use crate::error::CliError;
use crate::output::format_json;
use crate::parsers::parse_date;
use crate::table;

#[derive(Debug, Subcommand)]
pub enum FiscalCommand {
    /// Prochaines échéances calendaires de base (CA3, acompte d'IS, CFE), sans montants — dates
    /// indicatives (année civile), voir `freeflow_core::fiscal`.
    Deadlines {
        /// Date du jour (défaut : aujourd'hui, heure locale).
        #[arg(long, value_parser = parse_date)]
        today: Option<Date>,
    },
    /// Calendrier fiscal et social complet **chiffré** sur 12 mois, dérivé de la date de clôture
    /// d'exercice du profil : TVA à reverser (CA3, ou acomptes 3514 + CA12 au réel simplifié),
    /// acomptes et solde d'IS, liasse, AG, dépôt, DSN.
    Calendar {
        /// Date du jour (défaut : aujourd'hui, heure locale).
        #[arg(long, value_parser = parse_date)]
        today: Option<Date>,
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

/// Les échéances en texte : une ligne par échéance (date, nature, montant), la note en
/// retrait — plus jamais la structure Rust brute.
fn deadlines_human(deadlines: &[FiscalDeadline]) -> String {
    if deadlines.is_empty() {
        return "aucune échéance dans l'horizon".to_string();
    }
    let rows: Vec<Vec<String>> = deadlines
        .iter()
        .map(|d| {
            vec![
                format_date(d.due_on),
                d.kind.as_str().to_string(),
                d.amount.map_or_else(|| "—".to_string(), |m| m.to_string()),
            ]
        })
        .collect();
    let mut out = table::render(&["Échéance", "Nature", "Montant"], &rows);
    let notes: Vec<String> = deadlines
        .iter()
        .filter_map(|d| {
            d.note
                .as_ref()
                .map(|n| format!("{} {} : {n}", format_date(d.due_on), d.kind.as_str()))
        })
        .collect();
    if !notes.is_empty() {
        out.push_str("\n\n");
        out.push_str(&notes.join("\n"));
    }
    out
}

pub fn run(cmd: FiscalCommand, store: &mut Store, json_out: bool) -> Result<String, CliError> {
    let deadlines = match cmd {
        FiscalCommand::Deadlines { today } => upcoming_deadlines(today.unwrap_or_else(today_local)),
        FiscalCommand::Calendar { today } => {
            fiscal_calendar(store.connection(), today.unwrap_or_else(today_local))?
        }
    };
    Ok(if json_out {
        format_json(&deadlines.iter().map(to_json).collect::<Vec<_>>())
    } else {
        deadlines_human(&deadlines)
    })
}
