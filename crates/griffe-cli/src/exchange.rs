//! `griffe echange` — vérifier un fichier, l'appliquer, écrire les séries.

use std::path::{Path, PathBuf};

use clap::Subcommand;
use griffe_core::app::{ExecutionContext, Executor};
use griffe_core::clock::today_local;
use griffe_core::domain::Money;
use griffe_core::exchange::{
    ApplyExchange, CadenceOutcome, DossierFile, DossierOutcome, ExchangeDocument, ExchangeReport,
    ExchangeStatus, document_from_str, parse_montant, series_catalog,
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
    let mut lines = Vec::new();
    push_group(
        &mut lines,
        "Créées",
        rows_for(document, report, ExchangeStatus::Cree),
    );
    push_group(
        &mut lines,
        "Révisées",
        rows_for(document, report, ExchangeStatus::Revise),
    );
    push_group(
        &mut lines,
        "Inchangées",
        rows_for(document, report, ExchangeStatus::Inchange),
    );
    push_group(
        &mut lines,
        "Incomplètes",
        rows_for(document, report, ExchangeStatus::Incomplet),
    );
    push_group(&mut lines, "Montants", montant_rows(document));
    push_group(&mut lines, "Séries à créer", series_to_create(report));
    push_group(&mut lines, "Reprendre écraserait", garder_rows(report));
    lines.join("\n")
}

fn push_group(out: &mut Vec<String>, title: &str, mut rows: Vec<String>) {
    out.push(title.to_string());
    if rows.is_empty() {
        out.push("aucune".to_string());
    } else {
        out.append(&mut rows);
    }
}

fn rows_for(
    document: &ExchangeDocument,
    report: &ExchangeReport,
    statut: ExchangeStatus,
) -> Vec<String> {
    let mut rows = Vec::new();
    for dossier in report.dossiers.iter().filter(|row| row.statut == statut) {
        rows.push(dossier_row(document, dossier));
    }
    for serie in report.series.iter().filter(|row| row.statut == statut) {
        rows.push(format!("série {}", serie.nom));
    }
    if let Some(cadence) = &report.cadence
        && cadence.statut == statut
    {
        rows.push(cadence_row(cadence));
    }
    rows
}

fn dossier_row(document: &ExchangeDocument, outcome: &DossierOutcome) -> String {
    let quoi = document
        .dossiers
        .iter()
        .find(|dossier| dossier.origine.trim() == outcome.origine)
        .map(|dossier| dossier.quoi.trim().to_string())
        .unwrap_or_default();
    let mut line = if quoi.is_empty() {
        outcome.qui.clone()
    } else {
        format!("{} — {quoi}", outcome.qui)
    };
    if let Some(raison) = &outcome.raison {
        line.push_str(" — ");
        line.push_str(raison);
    }
    line
}

fn cadence_row(cadence: &CadenceOutcome) -> String {
    let statut = statut_fr(cadence.statut);
    if cadence.epingle {
        format!("cadence — {statut} — les conversations ouvertes gardent la série d'avant")
    } else if let Some(raison) = &cadence.raison {
        format!("cadence — {statut} — {raison}")
    } else {
        format!("cadence — {statut}")
    }
}

fn statut_fr(statut: ExchangeStatus) -> &'static str {
    match statut {
        ExchangeStatus::Cree => "créée",
        ExchangeStatus::Revise => "révisée",
        ExchangeStatus::Inchange => "inchangée",
        ExchangeStatus::Incomplet => "incomplète",
    }
}

fn montant_rows(document: &ExchangeDocument) -> Vec<String> {
    document
        .dossiers
        .iter()
        .filter_map(|dossier| {
            montant_of(dossier).map(|montant| {
                format!(
                    "{} — {} — {montant}",
                    dossier.qui.trim(),
                    dossier.quoi.trim()
                )
            })
        })
        .collect()
}

fn montant_of(dossier: &DossierFile) -> Option<String> {
    let lines = dossier.lignes.as_ref()?;
    if lines.is_empty() {
        return None;
    }
    let mut total = Money::ZERO;
    for line in lines {
        match parse_montant(&line.montant) {
            Ok(money) => total += money,
            Err(raison) => return Some(raison),
        }
    }
    Some(total.to_string())
}

fn series_to_create(report: &ExchangeReport) -> Vec<String> {
    report
        .series
        .iter()
        .filter(|serie| serie.statut == ExchangeStatus::Cree)
        .map(|serie| serie.nom.clone())
        .collect()
}

fn garder_rows(report: &ExchangeReport) -> Vec<String> {
    let mut rows = Vec::new();
    for dossier in &report.dossiers {
        if !dossier.gardes.is_empty() {
            rows.push(format!("{} — {}", dossier.qui, dossier.gardes.join(", ")));
        }
    }
    for serie in &report.series {
        if !serie.gardes.is_empty() {
            rows.push(format!("série {} — {}", serie.nom, serie.gardes.join(", ")));
        }
    }
    rows
}
