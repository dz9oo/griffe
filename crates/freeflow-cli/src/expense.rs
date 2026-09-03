//! `freeflow expense ...` — les références (`<RÉFÉRENCE>`) acceptent un UUID complet, un préfixe
//! d'UUID d'au moins 4 caractères hexadécimaux, ou un libellé de dépense (insensible à la casse
//! et aux accents, exact puis par préfixe) : voir `freeflow_core::reference::resolve_expense`.

use std::path::PathBuf;

use clap::{Args, Subcommand};
use freeflow_core::app::{ExecutionContext, Executor};
use freeflow_core::billing::bank_transaction_by_id;
use freeflow_core::domain::{BankTransactionId, Expense, ExpenseCategory, Money, VatRate};
use freeflow_core::expenses::{
    self, expense_by_id, expense_detail, hash_receipt, list_expenses, reconciled_debits,
};
use freeflow_core::store::Store;
use time::Date;

use crate::error::CliError;
use crate::output::{
    HumanRender, format_json, format_outcome, format_outcome_as, format_value, key_values,
};

impl HumanRender for freeflow_core::expenses::ExpenseDetail {
    fn render_human(&self) -> String {
        let e = &self.expense;
        key_values(&[
            ("Dépense", e.label.clone()),
            ("id", e.id.to_string()),
            ("date", freeflow_core::domain::format_date(e.incurred_on)),
            ("catégorie", e.category.as_str().to_string()),
            ("montant TTC", e.amount.to_string()),
            (
                "TVA déductible",
                format!("{} (taux {})", e.vat_deductible, e.vat_rate.as_str()),
            ),
            (
                "justificatif",
                match (&e.receipt_filename, &e.receipt_hash) {
                    (Some(name), Some(hash)) => format!("{name} ({})", &hash[..hash.len().min(12)]),
                    (None, Some(hash)) => hash[..hash.len().min(12)].to_string(),
                    _ => "aucun".to_string(),
                },
            ),
            (
                "relevé",
                self.bank_transaction.as_ref().map_or_else(
                    || "non rapprochée".to_string(),
                    |t| {
                        format!(
                            "débit du {} — {} ({})",
                            freeflow_core::domain::format_date(t.occurred_on),
                            t.description,
                            t.id
                        )
                    },
                ),
            ),
            ("révision", e.revision.to_string()),
        ])
    }
}
use crate::parsers::{parse_date, parse_money};
use crate::refs;

#[derive(Debug, Args)]
pub struct RecordArgs {
    #[arg(long)]
    label: String,
    /// `software`, `equipment`, `travel`, `meals`, `office`, `professional`, `fees`
    /// (honoraires), `bank_charges` (frais bancaires), `taxes` (impôts et taxes : CFE, CVAE…
    /// — pas l'IS ni la TVA, qui se règlent par `bank settle`) ou `other`.
    #[arg(long, value_parser = clap::value_parser!(ExpenseCategory))]
    category: ExpenseCategory,
    /// Montant TTC — repris du débit du relevé si omis avec `--transaction`.
    #[arg(long, value_parser = parse_money, required_unless_present = "transaction")]
    amount: Option<Money>,
    #[arg(long, value_parser = clap::value_parser!(VatRate))]
    vat_rate: VatRate,
    /// TVA effectivement déductible (peut être inférieure à `amount × taux` — voir la
    /// documentation de `freeflow_core::expenses`).
    #[arg(long, value_parser = parse_money)]
    vat_deductible: Money,
    /// Date d'engagement — reprise de la date du débit du relevé si omise avec `--transaction`.
    #[arg(long, value_parser = parse_date, required_unless_present = "transaction")]
    incurred_on: Option<Date>,
    /// Justificatif à archiver : haché (SHA-256) et copié dans `receipts/` à côté du coffre.
    #[arg(long)]
    receipt: Option<PathBuf>,
    /// Débit du relevé importé (`bank list --unmatched`) que cette dépense paie : elle est
    /// créée rapprochée, au montant exact du débit.
    #[arg(long, value_parser = clap::value_parser!(BankTransactionId))]
    transaction: Option<BankTransactionId>,
}

#[derive(Debug, Subcommand)]
pub enum ExpenseCommand {
    /// Enregistre une dépense professionnelle.
    Record(Box<RecordArgs>),
    /// Liste les dépenses, les plus récentes d'abord.
    List,
    /// Affiche une dépense.
    Show {
        #[arg(value_name = "RÉFÉRENCE")]
        reference: String,
    },
    /// Modifie une dépense existante — seuls les champs fournis changent, le reste est conservé
    /// tel quel. Refusé si sa date tombe dans un exercice déjà clôturé.
    Edit(Box<EditArgs>),
    /// Supprime une dépense pour de bon — le justificatif archivé, lui, reste en place
    /// (adressé par contenu, il peut être partagé par une autre dépense) et le débit du
    /// relevé rapproché, s'il y en a un, est libéré.
    Rm {
        #[arg(value_name = "RÉFÉRENCE")]
        reference: String,
    },
    /// Rapproche une dépense d'un débit du relevé importé, au montant exact (défaire :
    /// `bank unreconcile`).
    Reconcile {
        #[arg(value_name = "RÉFÉRENCE")]
        reference: String,
        #[arg(long, value_parser = clap::value_parser!(BankTransactionId))]
        transaction: BankTransactionId,
    },
}

#[derive(Debug, Args)]
pub struct EditArgs {
    #[arg(value_name = "RÉFÉRENCE")]
    reference: String,
    #[arg(long)]
    label: Option<String>,
    #[arg(long, value_parser = clap::value_parser!(ExpenseCategory))]
    category: Option<ExpenseCategory>,
    #[arg(long, value_parser = parse_money)]
    amount: Option<Money>,
    #[arg(long, value_parser = clap::value_parser!(VatRate))]
    vat_rate: Option<VatRate>,
    #[arg(long, value_parser = parse_money)]
    vat_deductible: Option<Money>,
    #[arg(long, value_parser = parse_date)]
    incurred_on: Option<Date>,
    /// Remplace le justificatif : le nouveau fichier est haché et archivé, l'ancien reste en
    /// place dans `receipts/`. Exclusif avec `--clear-receipt`.
    #[arg(long, conflicts_with = "clear_receipt")]
    receipt: Option<PathBuf>,
    /// Détache le justificatif de la dépense (sans supprimer le fichier archivé).
    #[arg(long)]
    clear_receipt: bool,
}

/// Copie `receipt` dans `<répertoire du coffre>/receipts/<hash>-<nom d'origine>` (stockage
/// adressé par contenu : un même fichier importé deux fois écrase le même chemin, sans le
/// dupliquer) et renvoie `(hash, nom de fichier archivé)`.
/// Résultat de l'archivage d'un justificatif : son hash d'intégrité et le nom du fichier copié
/// dans `receipts/`, tels que la commande du cœur les persiste.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchivedReceipt {
    pub hash: String,
    pub filename: String,
}

fn archive_receipt(
    db_path: &std::path::Path,
    receipt: &std::path::Path,
) -> Result<(String, String), CliError> {
    let content = std::fs::read(receipt).map_err(|e| {
        CliError::Unexpected(format!("lecture de {} impossible : {e}", receipt.display()))
    })?;
    let original_name = receipt.file_name().map_or_else(
        || "justificatif".to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let archived =
        archive_receipt_bytes(db_path, &original_name, &content).map_err(CliError::Unexpected)?;
    Ok((archived.hash, archived.filename))
}

/// Archive le contenu d'un justificatif à côté du coffre (`<répertoire du coffre>/receipts/
/// <hash>-<nom d'origine>`) et renvoie ce qu'il faut persister sur la dépense.
///
/// C'est l'IO annexe que `CLAUDE.md` réserve à l'adaptateur appelant, jamais à `Command::apply` :
/// la CLI l'invoque pour `--receipt <fichier>`, la fenêtre (`freeflow-web`) pour un fichier reçu
/// en multipart — une seule implémentation, jamais réécrite par façade. `original_name` est
/// réduit à son composant final : un nom venu d'un navigateur ne doit pas pouvoir sortir de
/// `receipts/` (« ../x »).
///
/// # Errors
///
/// Un message lisible si le répertoire ne peut être créé ou le fichier écrit.
pub fn archive_receipt_bytes(
    db_path: &std::path::Path,
    original_name: &str,
    content: &[u8],
) -> Result<ArchivedReceipt, String> {
    let hash = hash_receipt(content);
    let original_name = std::path::Path::new(original_name)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty() && n != "." && n != "..")
        .unwrap_or_else(|| "justificatif".to_string());
    let archived_name = format!("{hash}-{original_name}");

    let receipts_dir = db_path
        .parent()
        .map_or_else(|| PathBuf::from("receipts"), |p| p.join("receipts"));
    std::fs::create_dir_all(&receipts_dir)
        .map_err(|e| format!("création de {} impossible : {e}", receipts_dir.display()))?;
    // Le justificatif (facture fournisseur, note de frais…) vit à côté du coffre chiffré mais
    // n'est pas lui-même chiffré : au minimum, on le rend illisible aux autres utilisateurs de la
    // machine — le répertoire en 0700 et le fichier en 0600 — pour ne pas laisser en clair, en
    // 0644 (umask par défaut), des données que tout le reste du produit protège.
    tighten_dir_permissions(&receipts_dir);
    let archived_path = receipts_dir.join(&archived_name);
    std::fs::write(&archived_path, content)
        .map_err(|e| format!("écriture du justificatif impossible : {e}"))?;
    tighten_file_permissions(&archived_path);

    Ok(ArchivedReceipt {
        hash,
        filename: archived_name,
    })
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
            // Avec `--transaction`, le montant et la date manquants viennent du débit lui-même
            // — la commande du cœur, elle, exige toujours le montant exact.
            let debit = match args.transaction {
                Some(id) => Some(bank_transaction_by_id(store.connection(), id)?.ok_or_else(
                    || CliError::Domain(format!("transaction bancaire introuvable : {id}")),
                )?),
                None => None,
            };
            let (amount, incurred_on) = match (&debit, args.amount, args.incurred_on) {
                (Some(debit), amount, incurred_on) => (
                    amount.unwrap_or_else(|| Money::from_cents(-debit.amount_cents)),
                    incurred_on.unwrap_or(debit.occurred_on),
                ),
                // `required_unless_present` garantit les deux sans `--transaction`.
                (None, Some(amount), Some(incurred_on)) => (amount, incurred_on),
                (None, _, _) => {
                    return Err(CliError::Domain(
                        "--amount et --incurred-on sont requis sans --transaction".to_string(),
                    ));
                }
            };
            let command = expenses::RecordExpense {
                label: args.label,
                category: args.category,
                amount,
                vat_rate: args.vat_rate,
                vat_deductible: args.vat_deductible,
                incurred_on,
                receipt_hash,
                receipt_filename,
                bank_transaction_id: args.transaction,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        ExpenseCommand::List => {
            let expenses = list_expenses(store.connection())?;
            if json {
                format_json(&expenses)
            } else {
                let reconciled = reconciled_debits(store.connection())?;
                expense_table(&expenses, &reconciled)
            }
        }
        ExpenseCommand::Show { reference } => {
            let id = refs::resolve_expense(store, &reference)?;
            let detail = expense_detail(store.connection(), id)?
                .ok_or_else(|| CliError::Domain(format!("dépense introuvable : {id}")))?;
            format_value(&detail, json)
        }
        ExpenseCommand::Edit(args) => {
            let id = refs::resolve_expense(store, &args.reference)?;
            let current = expense_or_not_found(store, id)?;
            // Même logique de justificatif que `Record` : hash seul en dry-run (lecture sans
            // archivage), hash + copie sinon ; `--clear-receipt` détache sans toucher au fichier.
            let (receipt_hash, receipt_filename) = if args.clear_receipt {
                (None, None)
            } else {
                match &args.receipt {
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
                    None => (
                        current.receipt_hash.clone(),
                        current.receipt_filename.clone(),
                    ),
                }
            };
            let command = expenses::UpdateExpense {
                id,
                revision: current.revision,
                label: args.label.unwrap_or(current.label),
                category: args.category.unwrap_or(current.category),
                amount: args.amount.unwrap_or(current.amount),
                vat_rate: args.vat_rate.unwrap_or(current.vat_rate),
                vat_deductible: args.vat_deductible.unwrap_or(current.vat_deductible),
                incurred_on: args.incurred_on.unwrap_or(current.incurred_on),
                receipt_hash,
                receipt_filename,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        ExpenseCommand::Rm { reference } => {
            let id = refs::resolve_expense(store, &reference)?;
            let current = expense_or_not_found(store, id)?;
            let command = expenses::DeleteExpense {
                id,
                revision: current.revision,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        ExpenseCommand::Reconcile {
            reference,
            transaction,
        } => {
            let id = refs::resolve_expense(store, &reference)?;
            let command = expenses::ReconcileExpense {
                transaction_id: transaction,
                expense_id: id,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome_as(&outcome, json, |()| {
                format!("dépense {id} rapprochée du débit {transaction}")
            })
        }
    };
    Ok(output)
}

fn expense_or_not_found(
    store: &Store,
    id: freeflow_core::domain::ExpenseId,
) -> Result<Expense, CliError> {
    expense_by_id(store.connection(), id)?
        .ok_or_else(|| CliError::Domain(format!("dépense introuvable : {id}")))
}

fn expense_table(
    expenses: &[Expense],
    reconciled: &std::collections::HashMap<
        freeflow_core::domain::ExpenseId,
        freeflow_core::domain::BankTransaction,
    >,
) -> String {
    let rows = expenses
        .iter()
        .map(|e| {
            vec![
                e.id.to_string(),
                freeflow_core::domain::format_date(e.incurred_on),
                e.label.clone(),
                e.category.as_str().to_string(),
                e.amount.to_string(),
                e.vat_deductible.to_string(),
                if e.receipt_hash.is_some() {
                    "oui".to_string()
                } else {
                    "—".to_string()
                },
                reconciled.get(&e.id).map_or_else(
                    || "—".to_string(),
                    |t| freeflow_core::domain::format_date(t.occurred_on),
                ),
            ]
        })
        .collect::<Vec<_>>();
    crate::table::render(
        &[
            "id",
            "date",
            "libellé",
            "catégorie",
            "montant ttc",
            "tva déductible",
            "justificatif",
            "relevé",
        ],
        &rows,
    )
}
