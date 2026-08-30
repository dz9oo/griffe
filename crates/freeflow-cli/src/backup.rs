//! `freeflow backup ...`
//!
//! Une sauvegarde automatique et silencieuse tourne déjà à chaque commande (voir
//! [`crate::dispatch`]) : ces sous-commandes ne sont utiles que pour en forcer une
//! immédiatement, ou pour restaurer.

use std::path::PathBuf;

use clap::Subcommand;
use freeflow_core::store::Store;

use crate::error::CliError;
use crate::vault::{self, PassphraseOpts};

#[derive(Debug, Subcommand)]
pub enum BackupCommand {
    /// Écrit une sauvegarde immédiatement, sans tenir compte de la fraîcheur d'une précédente.
    Create {
        #[arg(long)]
        out: PathBuf,
    },
    /// Restaure une sauvegarde vers un nouveau coffre — ne touche jamais le coffre d'origine.
    Restore {
        #[arg(long)]
        from: PathBuf,
        #[arg(long)]
        to: PathBuf,
    },
}

/// `Restore` ne touche jamais le coffre par défaut (`--db`/`FREEFLOW_DB`) : elle est traitée à
/// part dans [`crate::dispatch`], *avant* toute tentative de l'ouvrir — restaurer doit rester
/// possible même quand ce coffre-là est justement celui qui est cassé.
///
/// # Errors
pub fn restore(from: PathBuf, to: PathBuf, opts: &PassphraseOpts) -> Result<String, CliError> {
    let passphrase = vault::resolve_passphrase(opts, "Passphrase de la sauvegarde : ")?;
    Store::restore_from(&from, &to, &passphrase)?;
    Ok(format!("✓ coffre restauré : {}", to.display()))
}

/// # Errors
pub fn run(cmd: BackupCommand, store: &Store) -> Result<String, CliError> {
    match cmd {
        BackupCommand::Create { out } => {
            store.backup_to(&out)?;
            Ok(format!("✓ sauvegarde écrite : {}", out.display()))
        }
        BackupCommand::Restore { .. } => {
            unreachable!("`backup restore` est interceptée par crate::dispatch avant ce point")
        }
    }
}
