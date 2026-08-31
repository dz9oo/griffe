//! `freeflow expense ...`

use std::path::PathBuf;

use clap::{Args, Subcommand};
use freeflow_core::app::{ExecutionContext, Executor};
use freeflow_core::domain::{ExpenseCategory, Money, VatRate};
use freeflow_core::expenses::{self, hash_receipt, list_expenses};
use freeflow_core::store::Store;
use time::Date;

use crate::error::CliError;
use crate::output::{format_outcome, format_value};
use crate::parsers::{parse_date, parse_money};

#[derive(Debug, Args)]
pub struct RecordArgs {
    #[arg(long)]
    label: String,
    #[arg(long, value_parser = clap::value_parser!(ExpenseCategory))]
    category: ExpenseCategory,
    #[arg(long, value_parser = parse_money)]
    amount: Money,
    #[arg(long, value_parser = clap::value_parser!(VatRate))]
    vat_rate: VatRate,
    /// TVA effectivement déductible (peut être inférieure à `amount × taux` — voir la
    /// documentation de `freeflow_core::expenses`).
    #[arg(long, value_parser = parse_money)]
    vat_deductible: Money,
    #[arg(long, value_parser = parse_date)]
    incurred_on: Date,
    /// Justificatif à archiver : haché (SHA-256) et copié dans `receipts/` à côté du coffre.
    #[arg(long)]
    receipt: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
pub enum ExpenseCommand {
    /// Enregistre une dépense professionnelle.
    Record(Box<RecordArgs>),
    /// Liste les dépenses, les plus récentes d'abord.
    List,
}

/// Copie `receipt` dans `<répertoire du coffre>/receipts/<hash>-<nom d'origine>` (stockage
/// adressé par contenu : un même fichier importé deux fois écrase le même chemin, sans le
/// dupliquer) et renvoie `(hash, nom de fichier archivé)`.
fn archive_receipt(
    db_path: &std::path::Path,
    receipt: &std::path::Path,
) -> Result<(String, String), CliError> {
    let content = std::fs::read(receipt).map_err(|e| {
        CliError::Unexpected(format!("lecture de {} impossible : {e}", receipt.display()))
    })?;
    let hash = hash_receipt(&content);
    let original_name = receipt.file_name().map_or_else(
        || "justificatif".to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let archived_name = format!("{hash}-{original_name}");

    let receipts_dir = db_path
        .parent()
        .map_or_else(|| PathBuf::from("receipts"), |p| p.join("receipts"));
    std::fs::create_dir_all(&receipts_dir).map_err(|e| {
        CliError::Unexpected(format!(
            "création de {} impossible : {e}",
            receipts_dir.display()
        ))
    })?;
    // Le justificatif (facture fournisseur, note de frais…) vit à côté du coffre chiffré mais
    // n'est pas lui-même chiffré : au minimum, on le rend illisible aux autres utilisateurs de la
    // machine — le répertoire en 0700 et le fichier en 0600 — pour ne pas laisser en clair, en
    // 0644 (umask par défaut), des données que tout le reste du produit protège.
    tighten_dir_permissions(&receipts_dir);
    let archived_path = receipts_dir.join(&archived_name);
    std::fs::write(&archived_path, &content)
        .map_err(|e| CliError::Unexpected(format!("écriture du justificatif impossible : {e}")))?;
    tighten_file_permissions(&archived_path);

    Ok((hash, archived_name))
}

#[cfg(unix)]
fn tighten_dir_permissions(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
}

#[cfg(unix)]
fn tighten_file_permissions(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn tighten_dir_permissions(_path: &std::path::Path) {}

#[cfg(not(unix))]
fn tighten_file_permissions(_path: &std::path::Path) {}

pub fn run(
    cmd: ExpenseCommand,
    store: &mut Store,
    ctx: &ExecutionContext,
    json: bool,
) -> Result<String, CliError> {
    let output = match cmd {
        ExpenseCommand::Record(args) => {
            // En dry-run, aucune écriture ne doit avoir lieu — pas même la copie du justificatif
            // sur disque. On calcule quand même le hash (lecture seule) pour que la prévisualisation
            // reste fidèle, mais sans rien archiver.
            let (receipt_hash, receipt_filename) = match &args.receipt {
                Some(path) if ctx.dry_run => {
                    let content = std::fs::read(path).map_err(|e| {
                        CliError::Unexpected(format!(
                            "lecture de {} impossible : {e}",
                            path.display()
                        ))
                    })?;
                    (Some(hash_receipt(&content)), None)
                }
                Some(path) => {
                    let (hash, filename) = archive_receipt(store.db_path(), path)?;
                    (Some(hash), Some(filename))
                }
                None => (None, None),
            };
            let command = expenses::RecordExpense {
                label: args.label,
                category: args.category,
                amount: args.amount,
                vat_rate: args.vat_rate,
                vat_deductible: args.vat_deductible,
                incurred_on: args.incurred_on,
                receipt_hash,
                receipt_filename,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        ExpenseCommand::List => {
            let expenses = list_expenses(store.connection())?;
            format_value(&expenses, json)
        }
    };
    Ok(output)
}
