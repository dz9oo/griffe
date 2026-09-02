//! `freeflow fec ...` — export du Fichier des Écritures Comptables (lot 28).
//!
//! Le cœur construit et rend le fichier (`freeflow_core::fec`) ; l'écriture sur disque est de
//! l'IO d'adaptateur, ici. Contrairement à `invoice render`/`year render` côté MCP, la CLI
//! écrase un fichier existant : c'est l'utilisateur au terminal qui désigne la destination.

use std::path::{Path, PathBuf};

use clap::Subcommand;
use freeflow_core::fec::build_fec;
use freeflow_core::store::Store;
use serde_json::json;

use crate::error::CliError;
use crate::output::format_value;

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
                Ok(format_value(
                    &json!({ "path": path.display().to_string(), "summary": summary }),
                    true,
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
    }
}
