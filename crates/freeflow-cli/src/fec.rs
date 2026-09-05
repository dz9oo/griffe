//! `freeflow fec ...` — export et contrôle du Fichier des Écritures Comptables (lot 28).
//!
//! Le cœur construit et rend le fichier (`freeflow_core::fec`) ; l'écriture sur disque est de
//! l'IO d'adaptateur, ici. Contrairement à `invoice render`/`year render` côté MCP, la CLI
//! écrase un fichier existant : c'est l'utilisateur au terminal qui désigne la destination.
//!
//! `check` relit un fichier (sans coffre) ou le FEC d'un exercice (coffre) : même moteur
//! [`freeflow_core::fec::check_fec`].

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use clap::Subcommand;
use freeflow_core::fec::{FecCheck, FecSeverity, build_fec, check_fec, check_fec_of};
use freeflow_core::store::Store;
use serde_json::json;

use crate::error::CliError;
use crate::output::format_json;

#[derive(Debug, Subcommand)]
pub enum FecCommand {
    /// Écrit le Fichier des Écritures Comptables (art. A. 47 A-1 LPF) de l'exercice clos dans
    /// l'année civile PERIOD, dérivé des factures, avoirs, encaissements et dépenses — nommé
    /// `<SIREN>FEC<AAAAMMJJ>.txt`, pour l'expert-comptable.
    Export {
        /// Année civile de la clôture (ex. `2026`) — même désignation que `year show` ; l'exercice
        /// n'a pas besoin d'être clos.
        period: i32,
        /// Fichier à écrire, ou répertoire existant dans lequel écrire le fichier sous son nom
        /// réglementaire.
        #[arg(long)]
        out: PathBuf,
    },
    /// Contrôle la structure d'un FEC (art. A. 47 A-1 LPF). Un fichier se relit **sans coffre** ;
    /// `--period` dérive le FEC du coffre comme `export`, sans l'écrire. Ce n'est pas une
    /// attestation DGFiP.
    Check {
        /// Fichier à relire (`<SIREN>FEC<AAAAMMJJ>.txt`).
        #[arg(value_name = "FICHIER", required_unless_present = "period")]
        file: Option<PathBuf>,
        /// Contrôle le FEC de l'exercice clos dans cette année civile.
        #[arg(long, conflicts_with = "file")]
        period: Option<i32>,
    },
}

/// Un répertoire reçoit le fichier sous son nom réglementaire ; tout autre chemin est le fichier.
#[must_use]
pub fn resolve_out(out: &Path, file_name: &str) -> PathBuf {
    if out.is_dir() {
        out.join(file_name)
    } else {
        out.to_path_buf()
    }
}

/// Relit un fichier sur disque — pas de coffre, appelé depuis `dispatch` avant l'ouverture.
pub fn check_file(path: &Path, json: bool) -> Result<String, CliError> {
    let bytes = std::fs::read(path).map_err(|e| {
        CliError::Unexpected(format!("lecture de {} impossible : {e}", path.display()))
    })?;
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .map(ToOwned::to_owned);
    Ok(format_check(&check_fec(&bytes, name.as_deref()), json))
}

pub fn run(cmd: FecCommand, store: &mut Store, json: bool) -> Result<String, CliError> {
    match cmd {
        FecCommand::Export { period, out } => {
            let fec = build_fec(store.connection(), period)?;
            let path = resolve_out(&out, &fec.file_name());
            let content = fec.render();
            std::fs::write(&path, &content).map_err(|e| {
                CliError::Unexpected(format!("écriture de {} impossible : {e}", path.display()))
            })?;
            let summary = fec.summary();
            if json {
                Ok(format_json(
                    &json!({ "path": path.display().to_string(), "summary": summary }),
                ))
            } else {
                Ok(format!(
                    "✓ {} écrit — exercice {} → {}, {} écriture(s), {} ligne(s), total débit = \
                     total crédit = {}",
                    path.display(),
                    freeflow_core::domain::format_date(summary.starts_on),
                    freeflow_core::domain::format_date(summary.ends_on),
                    summary.entries,
                    summary.lines,
                    fec.total_debit()
                ))
            }
        }
        FecCommand::Check {
            file: Some(path),
            period: None,
        } => check_file(&path, json),
        FecCommand::Check {
            file: None,
            period: Some(period),
        } => Ok(format_check(
            &check_fec_of(store.connection(), period)?,
            json,
        )),
        FecCommand::Check { .. } => Err(CliError::Unexpected(
            "précisez un fichier ou --period".to_string(),
        )),
    }
}

fn format_check(check: &FecCheck, json: bool) -> String {
    if json {
        format_json(check)
    } else {
        render_check(check)
    }
}

fn render_check(check: &FecCheck) -> String {
    let name = check.file_name.as_deref().unwrap_or("FEC");
    let mut out = String::new();
    if check.conformant {
        let _ = writeln!(
            out,
            "✓ {name} — structure conforme à l'art. A. 47 A-1 du LPF"
        );
    } else {
        let _ = writeln!(
            out,
            "✗ {name} — {} erreur(s), {} alerte(s) — structure non conforme à l'art. A. 47 A-1 \
             du LPF",
            check.error_count, check.warning_count
        );
    }
    let debit = freeflow_core::domain::Money::from_cents(check.total_debit_cents);
    let credit = freeflow_core::domain::Money::from_cents(check.total_credit_cents);
    let _ = writeln!(
        out,
        "  {} ligne(s), total débit = {debit}, total crédit = {credit}",
        check.lines
    );
    for finding in &check.findings {
        let sev = match finding.severity {
            FecSeverity::Error => "E",
            FecSeverity::Warning => "A",
        };
        let line = finding
            .line
            .map_or(String::new(), |n| format!(" ligne {n}"));
        let column = finding
            .column
            .as_deref()
            .map_or(String::new(), |c| format!(" [{c}]"));
        let _ = writeln!(out, "  {sev}{line}{column} : {}", finding.message);
    }
    let _ = write!(out, "  {}", check.disclaimer);
    out
}
