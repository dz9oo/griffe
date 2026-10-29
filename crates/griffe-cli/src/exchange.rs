//! `griffe echange` — vérifier un fichier, l'appliquer, écrire les séries.

use std::path::{Path, PathBuf};

use clap::Subcommand;
use griffe_core::app::{ExecutionContext, Executor};
use griffe_core::clock::today_local;
use griffe_core::exchange::{
    ApplyExchange, ExchangeDocument, ExchangeReport, document_from_str, series_catalog,
};
use griffe_core::store::Store;

use crate::error::CliError;
use crate::output::{format_json, format_outcome_as};

#[derive(Debug, Subcommand)]
pub enum EchangeCommand {
    /// Lit le fichier et montre ce qui serait écrit. N'écrit rien.
    Verifier {
        /// Fichier d'échange, version 1.
        #[arg(value_name = "FICHIER")]
        fichier: PathBuf,
    },
    /// Écrit dans le coffre ce que le fichier porte. Le jour du brouillon est celui de la machine.
    Appliquer {
        /// Fichier d'échange, version 1.
        #[arg(value_name = "FICHIER")]
        fichier: PathBuf,
    },
    /// Écrit le catalogue des séries et de la cadence.
    Ecrire {
        /// Le fichier reçoit les séries et la cadence, dans la forme du document.
        #[arg(long, value_name = "FICHIER")]
        series: PathBuf,
    },
}

pub fn run(
    cmd: EchangeCommand,
    store: &mut Store,
    ctx: &ExecutionContext,
    json: bool,
) -> Result<String, CliError> {
    match cmd {
        EchangeCommand::Verifier { fichier } => {
            let document = read_document(&fichier)?;
            let report = preview_document(store, &document)?;
            Ok(render(&document, &report, json))
        }
        EchangeCommand::Appliquer { fichier } => {
            let document = read_document(&fichier)?;
            let command = ApplyExchange {
                document: document.clone(),
                today: today_local(),
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            Ok(format_outcome_as(&outcome, json, |report| {
                render_account(&document, report)
            }))
        }
        EchangeCommand::Ecrire { series } => write_series(store, &series, json),
    }
}

fn read_document(path: &Path) -> Result<ExchangeDocument, CliError> {
    let raw = std::fs::read_to_string(path).map_err(|error| {
        CliError::Unexpected(format!(
            "lecture de {} impossible : {error}",
            path.display()
        ))
    })?;
    document_from_str(&raw).map_err(CliError::from)
}

fn preview_document(
    store: &Store,
    document: &ExchangeDocument,
) -> Result<ExchangeReport, CliError> {
    griffe_core::exchange::preview(store.connection(), document, today_local())
        .map_err(CliError::from)
}

fn write_series(store: &Store, path: &Path, json: bool) -> Result<String, CliError> {
    let document = series_catalog(store.connection())?;
    let mut bytes = serde_json::to_vec_pretty(&document)
        .map_err(|error| CliError::Unexpected(format!("catalogue illisible : {error}")))?;
    bytes.push(b'\n');
    std::fs::write(path, &bytes).map_err(|error| {
        CliError::Unexpected(format!(
            "écriture de {} impossible : {error}",
            path.display()
        ))
    })?;
    if json {
        Ok(format_json(&serde_json::json!({
            "chemin": path.display().to_string(),
            "series": document.series.len(),
            "cadence": document.cadence.as_ref().map_or(0, Vec::len),
        })))
    } else {
        Ok(format!(
            "✓ séries et cadence écrites dans {}",
            path.display()
        ))
    }
}

fn render(document: &ExchangeDocument, report: &ExchangeReport, json: bool) -> String {
    if json {
        format_json(report)
    } else {
        render_account(document, report)
    }
}

pub(crate) fn render_account(document: &ExchangeDocument, report: &ExchangeReport) -> String {
    griffe_core::exchange::account(document, report)
}
