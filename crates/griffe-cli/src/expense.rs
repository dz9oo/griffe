//! `freeflow expense ...` — les références (`<RÉFÉRENCE>`) acceptent un UUID complet, un préfixe
//! d'UUID d'au moins 4 caractères hexadécimaux, ou un libellé de dépense (insensible à la casse
//! et aux accents, exact puis par préfixe) : voir `griffe_core::reference::resolve_expense`.

use std::path::PathBuf;

use clap::{Args, Subcommand, ValueEnum};
use griffe_core::app::{ExecutionContext, Executor};
use griffe_core::billing::bank_transaction_by_id;
use griffe_core::domain::{
    BankTransactionId, Expense, ExpenseCategory, ExpensePaidBy, Money, VatRate,
};
use griffe_core::expenses::{
    self, expense_by_id, expense_detail, hash_receipt, list_expenses, reconciled_debits,
};
use griffe_core::store::Store;
use time::Date;

use crate::error::CliError;
use crate::output::{
    HumanRender, format_json, format_outcome, format_outcome_as, format_value, key_values,
};

impl HumanRender for griffe_core::expenses::ExpenseDetail {
    fn render_human(&self) -> String {
        let e = &self.expense;
        key_values(&[
            ("Dépense", e.label.clone()),
            ("id", e.id.to_string()),
            ("date", griffe_core::domain::format_date(e.incurred_on)),
            ("catégorie", e.category.as_str().to_string()),
            (
                "bénéficiaire",
                crate::output::or_dash(e.supplier.as_deref()),
            ),
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
                "payée par",
                paid_by_label(&self.expense, self.bank_transaction.is_some()),
            ),
            (
                "relevé",
                self.bank_transaction.as_ref().map_or_else(
                    || "non rapprochée".to_string(),
                    |t| {
                        format!(
                            "débit du {} — {} ({})",
                            griffe_core::domain::format_date(t.occurred_on),
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
    /// documentation de `griffe_core::expenses`).
    #[arg(long, value_parser = parse_money)]
    vat_deductible: Money,
    /// Date d'engagement — reprise de la date du débit du relevé si omise avec `--transaction`.
    #[arg(long, value_parser = parse_date, required_unless_present = "transaction")]
    incurred_on: Option<Date>,
    /// Bénéficiaire (fournisseur) — pour les honoraires, c'est ce nom qui cumule par année
    /// civile sur la DAS2 (seuil 2 400 € par bénéficiaire).
    #[arg(long)]
    supplier: Option<String>,
    /// Justificatif à archiver : haché (SHA-256) et copié **chiffré** dans `<coffre>.receipts/`.
    #[arg(long)]
    receipt: Option<PathBuf>,
    /// Débit du relevé importé (`bank list --unmatched`) que cette dépense paie : elle est
    /// créée rapprochée, au montant exact du débit.
    #[arg(long, value_parser = clap::value_parser!(BankTransactionId))]
    transaction: Option<BankTransactionId>,
    /// Qui a payé : `company` (la société, défaut) ou `me` (compte perso — la société te doit
    /// cette somme, la banque ne bouge pas). Incompatible avec `--transaction`.
    #[arg(long, value_enum, default_value_t = PaidByArg::Company, conflicts_with = "transaction")]
    paid_by: PaidByArg,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum PaidByArg {
    Company,
    Me,
}

impl PaidByArg {
    fn to_domain(self) -> ExpensePaidBy {
        match self {
            Self::Company => ExpensePaidBy::Company,
            Self::Me => ExpensePaidBy::Associate,
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum ExpenseCommand {
    /// Enregistre une dépense professionnelle.
    Record(Box<RecordArgs>),
    /// Joint (ou remplace) le justificatif d'une dépense — **même dans un exercice clôturé** :
    /// la pièce ne change ni le montant ni la date, et la facture du cabinet arrive souvent
    /// après la clôture. Le fichier est archivé chiffré dans `<coffre>.receipts/`.
    Attach {
        #[arg(value_name = "RÉFÉRENCE")]
        reference: String,
        #[arg(value_name = "FICHIER")]
        file: PathBuf,
    },
    /// Déchiffre le justificatif d'une dépense : dans `--out`, ou dans un fichier temporaire
    /// (0600) ouvert par le visualiseur par défaut.
    Receipt {
        #[arg(value_name = "RÉFÉRENCE")]
        reference: String,
        #[arg(long, value_name = "FICHIER")]
        out: Option<PathBuf>,
    },
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
    /// Bénéficiaire (fournisseur) — pour les honoraires, c'est ce nom qui cumule sur la DAS2.
    #[arg(long, conflicts_with = "clear_supplier")]
    supplier: Option<String>,
    /// Efface le bénéficiaire.
    #[arg(long)]
    clear_supplier: bool,
    /// Remplace le justificatif : le nouveau fichier est haché et archivé chiffré, l'ancien
    /// reste en place. Exclusif avec `--clear-receipt` — voir aussi `expense attach`.
    #[arg(long, conflicts_with = "clear_receipt")]
    receipt: Option<PathBuf>,
    /// Détache le justificatif de la dépense (sans supprimer le fichier archivé).
    #[arg(long)]
    clear_receipt: bool,
    /// Qui a payé : `company` ou `me` (compte perso). Inchangé si omis.
    #[arg(long, value_enum)]
    paid_by: Option<PaidByArg>,
}

/// Lit `receipt` et l'archive **chiffré** dans `<coffre>.receipts/` (lot 39 —
/// `griffe_core::receipts`, la même implémentation que la fenêtre et le serveur MCP) ;
/// renvoie `(hash, nom de fichier archivé)`, ce que la commande du cœur persiste.
fn archive_receipt(store: &Store, receipt: &std::path::Path) -> Result<(String, String), CliError> {
    let content = std::fs::read(receipt).map_err(|e| {
        CliError::Unexpected(format!("lecture de {} impossible : {e}", receipt.display()))
    })?;
    let original_name = receipt.file_name().map_or_else(
        || "justificatif".to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let archived = griffe_core::receipts::archive(store, &original_name, &content)
        .map_err(|e| CliError::Unexpected(e.to_string()))?;
    Ok((archived.hash, archived.filename))
}

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
                    let (hash, filename) = archive_receipt(store, path)?;
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
            let captured_receipt = receipt_filename
                .as_deref()
                .zip(receipt_hash.as_deref())
                .map(|(f, h)| (f.to_string(), h.to_string()));
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
                supplier: args.supplier,
                paid_by: args.paid_by.to_domain(),
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            if let (griffe_core::app::Outcome::Applied(id), Some((filename, hash))) =
                (&outcome, captured_receipt)
            {
                let _ = crate::papers::capture_expense_receipt(store, ctx, *id, &filename, &hash);
            }
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
                        let (hash, filename) = archive_receipt(store, path)?;
                        (Some(hash), Some(filename))
                    }
                    None => (
                        current.receipt_hash.clone(),
                        current.receipt_filename.clone(),
                    ),
                }
            };
            let new_receipt = args.receipt.is_some();
            let captured_receipt = receipt_filename
                .as_deref()
                .zip(receipt_hash.as_deref())
                .map(|(f, h)| (f.to_string(), h.to_string()));
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
                supplier: if args.clear_supplier {
                    None
                } else {
                    args.supplier.or(current.supplier)
                },
                paid_by: args.paid_by.map_or(current.paid_by, PaidByArg::to_domain),
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            if new_receipt && let Some((filename, hash)) = captured_receipt {
                let _ = crate::papers::capture_expense_receipt(store, ctx, id, &filename, &hash);
            }
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
        ExpenseCommand::Attach { reference, file } => {
            let id = refs::resolve_expense(store, &reference)?;
            let current = expense_or_not_found(store, id)?;
            let (receipt_hash, receipt_filename) = if ctx.dry_run {
                let content = std::fs::read(&file).map_err(|e| {
                    CliError::Unexpected(format!("lecture de {} impossible : {e}", file.display()))
                })?;
                (hash_receipt(&content), String::new())
            } else {
                archive_receipt(store, &file)?
            };
            let command = expenses::AttachReceipt {
                id,
                revision: current.revision,
                receipt_hash: receipt_hash.clone(),
                receipt_filename: receipt_filename.clone(),
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            if matches!(outcome, griffe_core::app::Outcome::Applied(_))
                && !receipt_filename.is_empty()
            {
                let _ = crate::papers::capture_expense_receipt(
                    store,
                    ctx,
                    id,
                    &receipt_filename,
                    &receipt_hash,
                );
            }
            format_outcome_as(&outcome, json, |revision| {
                format!("justificatif joint (révision {revision})")
            })
        }
        ExpenseCommand::Receipt { reference, out } => {
            let id = refs::resolve_expense(store, &reference)?;
            let current = expense_or_not_found(store, id)?;
            let filename = current.receipt_filename.ok_or_else(|| {
                CliError::Domain(format!("la dépense {id} n'a pas de justificatif archivé"))
            })?;
            let content = griffe_core::receipts::read(store, &filename)
                .map_err(|e| CliError::Unexpected(e.to_string()))?;
            let original = filename
                .split_once('-')
                .map_or(filename.as_str(), |(_, n)| n);
            let open_viewer = out.is_none();
            let target = match out {
                Some(path) => path,
                None => std::env::temp_dir()
                    .join(format!("freeflow-{}-{original}", &id.to_string()[..8])),
            };
            write_private(&target, &content)?;
            if json {
                format_json(&serde_json::json!({ "path": target.display().to_string() }))
            } else {
                // `--out` : l'utilisateur a choisi la destination, on n'ouvre pas le
                // visualiseur (un test ou un script y écrit souvent un fichier qui n'est
                // pas un vrai PDF). Sans `--out`, on ouvre le fichier temporaire.
                let opened = open_viewer && open_with_default_viewer(&target);
                format!(
                    "✓ justificatif déchiffré dans {}{}",
                    target.display(),
                    if opened { " (ouvert)" } else { "" }
                )
            }
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

fn paid_by_label(expense: &Expense, reconciled: bool) -> String {
    match expense.paid_by {
        ExpensePaidBy::Associate => "toi".into(),
        ExpensePaidBy::Company if reconciled => "au relevé".into(),
        ExpensePaidBy::Company => "la société, pas au relevé".into(),
    }
}

fn expense_or_not_found(
    store: &Store,
    id: griffe_core::domain::ExpenseId,
) -> Result<Expense, CliError> {
    expense_by_id(store.connection(), id)?
        .ok_or_else(|| CliError::Domain(format!("dépense introuvable : {id}")))
}

fn expense_table(
    expenses: &[Expense],
    reconciled: &std::collections::HashMap<
        griffe_core::domain::ExpenseId,
        griffe_core::domain::BankTransaction,
    >,
) -> String {
    let rows = expenses
        .iter()
        .map(|e| {
            vec![
                e.id.to_string(),
                griffe_core::domain::format_date(e.incurred_on),
                e.label.clone(),
                e.category.as_str().to_string(),
                e.amount.to_string(),
                e.vat_deductible.to_string(),
                if e.receipt_hash.is_some() {
                    "oui".to_string()
                } else {
                    "—".to_string()
                },
                paid_by_label(e, reconciled.contains_key(&e.id)),
                reconciled.get(&e.id).map_or_else(
                    || "—".to_string(),
                    |t| griffe_core::domain::format_date(t.occurred_on),
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
            "payée par",
            "relevé",
        ],
        &rows,
    )
}

/// Écrit `content` dans `path` en 0600 (Unix) : une pièce déchiffrée ne doit pas être lisible
/// par les autres utilisateurs de la machine.
fn write_private(path: &std::path::Path, content: &[u8]) -> Result<(), CliError> {
    std::fs::write(path, content).map_err(|e| {
        CliError::Unexpected(format!("écriture de {} impossible : {e}", path.display()))
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Ouvre `path` avec le visualiseur par défaut (`xdg-open` sous Linux, `open` sous macOS) —
/// best-effort : `false` si aucun ouvreur n'est disponible, le chemin est de toute façon
/// affiché.
fn open_with_default_viewer(path: &std::path::Path) -> bool {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(opener)
        .arg(path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .is_ok()
}
