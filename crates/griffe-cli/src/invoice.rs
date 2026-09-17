//! `freeflow invoice ...`, `freeflow payment ...`, `freeflow bank ...`
//!
//! Les lignes de facture suivent le même principe que les lignes de devis (lot `quote.rs`) :
//! JSON via `--lines`, ex. `--lines '[{"description":"Sept.","quantity":9.5,"unit_price":65000,"vat_rate":"Standard"}]'`

use std::path::PathBuf;

use clap::{Subcommand, ValueEnum};
use griffe_core::app::{ExecutionContext, Executor};
use griffe_core::billing::{
    self, aged_balance, list_bank_transactions, list_invoices, list_payments, verify_chain,
};
use griffe_core::domain::{
    BankTransactionId, ClientId, InvoiceId, InvoiceLine, InvoiceOrigin, MissionId, Money,
    PaymentId, PaymentMethod,
};
use griffe_core::store::Store;
use time::Date;

use crate::error::CliError;
use crate::output::{format_json, format_outcome, format_outcome_as};
use crate::parsers::{parse_date, parse_money};

fn parse_lines(json: &str) -> Result<Vec<InvoiceLine>, CliError> {
    serde_json::from_str(json).map_err(|e| CliError::InvalidLinesJson(e.to_string()))
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ImportFormat {
    Csv,
    Ofx,
    Xlsx,
}

#[derive(Debug, Subcommand)]
pub enum InvoiceCommand {
    /// Émet une facture — nécessite confirmation humaine quand `--actor agent:...`.
    Emit {
        #[arg(long, value_parser = clap::value_parser!(ClientId))]
        client: ClientId,
        #[arg(long, value_parser = clap::value_parser!(MissionId))]
        mission: Option<MissionId>,
        #[arg(long)]
        lines: String,
        #[arg(long, value_parser = parse_date)]
        issued_on: Date,
        #[arg(long, default_value_t = 30)]
        payment_terms_days: u32,
    },
    /// Enregistre une facture née ailleurs (PA, outil tiers) — conserve son numéro, archive le
    /// fichier. Nécessite confirmation humaine quand `--actor agent:...`.
    Import {
        /// Original produit hors de FreeFlow (PDF de la PA, scan…).
        #[arg(long)]
        file: PathBuf,
        #[arg(long, value_parser = clap::value_parser!(ClientId))]
        client: ClientId,
        #[arg(long, value_parser = clap::value_parser!(MissionId))]
        mission: Option<MissionId>,
        /// Numéro déjà porté par le document, jamais un `FA-` alloué ici.
        #[arg(long)]
        number: String,
        #[arg(long)]
        lines: String,
        #[arg(long, value_parser = parse_date)]
        issued_on: Date,
        #[arg(long, default_value_t = 30)]
        payment_terms_days: u32,
        /// Facture d'origine si cet import est un avoir.
        #[arg(long, value_parser = clap::value_parser!(InvoiceId))]
        credits: Option<InvoiceId>,
    },
    /// Émet un avoir annulant intégralement une facture — même exigence de confirmation.
    CreditNote {
        #[arg(long, value_parser = clap::value_parser!(InvoiceId))]
        id: InvoiceId,
        #[arg(long, value_parser = parse_date)]
        issued_on: Date,
    },
    /// Revérifie la chaîne de hash de toutes les factures.
    VerifyChain,
    /// Balance âgée à une date donnée.
    AgedBalance {
        /// Date du jour (défaut : aujourd'hui, heure locale).
        #[arg(long, value_parser = parse_date)]
        today: Option<Date>,
    },
    /// Rend une facture émise en PDF/A-3b avec le XML CII embarqué (Factur-X). Nécessite un
    /// profil d'entreprise défini (`griffe company set-profile`).
    Render {
        #[arg(long, value_parser = clap::value_parser!(InvoiceId))]
        id: InvoiceId,
        #[arg(long)]
        out: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
pub enum PaymentCommand {
    /// Enregistre un encaissement sur une facture.
    Record {
        #[arg(long, value_parser = clap::value_parser!(InvoiceId))]
        invoice: InvoiceId,
        #[arg(long, value_parser = parse_money)]
        amount: Money,
        #[arg(long, value_parser = parse_date)]
        received_on: Date,
        #[arg(long, value_parser = clap::value_parser!(PaymentMethod))]
        method: PaymentMethod,
    },
    /// Liste les encaissements, annulés compris, les plus récents d'abord.
    List,
    /// Annule un encaissement saisi à tort — contre-écriture, jamais une suppression : il reste
    /// dans l'historique mais sort de tous les calculs. Libère la transaction bancaire liée s'il
    /// était issu d'un rapprochement.
    Void {
        #[arg(long, value_parser = clap::value_parser!(PaymentId))]
        id: PaymentId,
        /// Motif de la correction, journalisé dans l'audit chaîné.
        #[arg(long)]
        reason: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum BankCommand {
    /// Importe un relevé bancaire : l'export CSV de votre banque tel quel, un OFX, ou l'Excel
    /// (xlsx) de Tiime.
    ///
    /// Tout est détecté (lots 38 et 43) : l'encodage (UTF-8 avec ou sans BOM, Windows-1252/latin-1),
    /// le séparateur (`;`, `,`, tabulation), la décimale (`,` ou `.`), les milliers, le format
    /// de date (AAAA-MM-JJ, JJ/MM/AAAA, JJ-MM-AAAA, JJ.MM.AAAA), les guillemets, les lignes de
    /// préambule et de solde, et les colonnes par leur nom — date (d'opération), libellé /
    /// label / intitulé / description, montant / amount, ou débit + crédit séparés, identifiant de
    /// transaction. Exports vérifiés : **Tiime (xlsx, feuille Transactions)** , Qonto, Shine,
    /// Boursorama, Crédit Agricole, BNP, LCL (sans en-tête), La Banque Postale, OFX 1.x et 2.x.
    /// Un fichier sans en-tête reconnu se lit `date;description;montant`. Les doublons
    /// (identifiant de banque, sinon date + montant + libellé) sont ignorés : réimporter ne
    /// duplique rien. `--dry-run` montre ce qui a été compris (dialecte, colonnes, lignes
    /// retenues, doublons, lignes sautées) sans rien écrire.
    Import {
        /// Force le format (`csv`, `ofx`, `xlsx`) si la détection se trompe.
        #[arg(long, value_enum)]
        format: Option<ImportFormat>,
        file: PathBuf,
    },
    /// Supprime une transaction importée par erreur (non rapprochée). Nécessite confirmation
    /// humaine quand `--actor agent:...`.
    Rm {
        #[arg(value_parser = clap::value_parser!(BankTransactionId))]
        transaction: BankTransactionId,
    },
    /// Liste les transactions importées, les plus récentes d'abord (`--unmatched` pour ne voir
    /// que celles restant à rapprocher).
    List {
        #[arg(long)]
        unmatched: bool,
    },
    /// Rapproche un crédit importé avec une facture (crée l'encaissement correspondant) —
    /// pour un débit et une dépense, voir `expense reconcile` / `expense record --transaction`.
    Reconcile {
        #[arg(long, value_parser = clap::value_parser!(BankTransactionId))]
        transaction: BankTransactionId,
        #[arg(long, value_parser = clap::value_parser!(InvoiceId))]
        invoice: InvoiceId,
    },
    /// Défait un rapprochement : libère la transaction et annule l'encaissement qui en était
    /// issu (pour un rapprochement historique sans lignée, seule la transaction est libérée —
    /// annulez le paiement via `payment void`). Pour un débit rapproché d'une dépense, la
    /// dépense reste, seule la transaction est libérée.
    Unreconcile {
        #[arg(long, value_parser = clap::value_parser!(BankTransactionId))]
        transaction: BankTransactionId,
    },
    /// Règle un compte de bilan depuis un mouvement du relevé (lot 37) — ni charge ni
    /// produit : une dette reprise au bilan d'ouverture (401 honoraires, 444 solde d'IS, 4455
    /// TVA), un compte courant d'associé (455), des dividendes (457), un virement entre vos
    /// comptes (580), un emprunt (164). Le libellé vient du plan de comptes ; `--label` pour un
    /// compte hors plan. Nécessite confirmation humaine quand `--actor agent:...`.
    Settle {
        /// Identifiant de la transaction (voir `bank list --unmatched`).
        #[arg(value_parser = clap::value_parser!(BankTransactionId))]
        transaction: BankTransactionId,
        /// Compte de bilan réglé (classes 1 à 5, hors 512), ex. `401000`.
        #[arg(long, value_parser = clap::value_parser!(griffe_core::domain::SettlementAccount))]
        account: griffe_core::domain::SettlementAccount,
        /// Libellé du compte, requis s'il n'est pas dans le plan de comptes de FreeFlow.
        #[arg(long)]
        label: Option<String>,
    },
    /// Défait un règlement de compte de bilan : la transaction redevient « à rapprocher ».
    Unsettle {
        #[arg(value_parser = clap::value_parser!(BankTransactionId))]
        transaction: BankTransactionId,
    },
}

pub fn run_invoice(
    cmd: InvoiceCommand,
    store: &mut Store,
    ctx: &ExecutionContext,
    json: bool,
) -> Result<String, CliError> {
    let output = match cmd {
        InvoiceCommand::Emit {
            client,
            mission,
            lines,
            issued_on,
            payment_terms_days,
        } => {
            let lines = parse_lines(&lines)?;
            let command = billing::EmitInvoice {
                client_id: client,
                mission_id: mission,
                lines,
                issued_on,
                payment_terms_days,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            let rendered = format_outcome(&outcome, json);
            let note = match &outcome {
                griffe_core::app::Outcome::Applied(emitted) => crate::papers::invoice_capture_note(
                    crate::papers::capture_invoice(store, ctx, emitted.id),
                ),
                _ => None,
            };
            crate::papers::append_capture_note(rendered, json, note)
        }
        InvoiceCommand::Import {
            file,
            client,
            mission,
            number,
            lines,
            issued_on,
            payment_terms_days,
            credits,
        } => {
            let bytes = std::fs::read(&file).map_err(|e| {
                CliError::Domain(format!("lecture de {} impossible : {e}", file.display()))
            })?;
            let original_name = file
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| "facture.pdf".to_string());
            let lines = parse_lines(&lines)?;
            let command = billing::ImportIssuedInvoice {
                number,
                client_id: client,
                mission_id: mission,
                lines,
                issued_on,
                payment_terms_days,
                credited_invoice_id: credits,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            let rendered = format_outcome(&outcome, json);
            let note = match &outcome {
                griffe_core::app::Outcome::Applied(emitted) => {
                    crate::papers::invoice_capture_note(crate::papers::capture_imported_invoice(
                        store,
                        ctx,
                        emitted.id,
                        &original_name,
                        &bytes,
                    ))
                }
                _ => None,
            };
            crate::papers::append_capture_note(rendered, json, note)
        }
        InvoiceCommand::CreditNote { id, issued_on } => {
            let command = billing::IssueCreditNote {
                invoice_id: id,
                issued_on,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            let rendered = format_outcome(&outcome, json);
            let note = match &outcome {
                griffe_core::app::Outcome::Applied(emitted) => crate::papers::invoice_capture_note(
                    crate::papers::capture_invoice(store, ctx, emitted.id),
                ),
                _ => None,
            };
            crate::papers::append_capture_note(rendered, json, note)
        }
        InvoiceCommand::VerifyChain => {
            let status = verify_chain(store.connection())?;
            let rendered = match &status {
                billing::ChainStatus::Intact if json => {
                    format_json(&serde_json::json!({"status": "intact"}))
                }
                billing::ChainStatus::Intact => {
                    "✓ chaîne de factures intacte : chaque facture chaîne la précédente".to_string()
                }
                billing::ChainStatus::BrokenAt(id) if json => format_json(
                    &serde_json::json!({"status": "broken", "broken_at": id.to_string()}),
                ),
                billing::ChainStatus::BrokenAt(id) => format!("✗ chaîne rompue à la facture {id}"),
            };
            if matches!(status, billing::ChainStatus::BrokenAt(_)) {
                return Err(CliError::Domain(format!(
                    "{rendered}\nla chaîne de factures est rompue"
                )));
            }
            rendered
        }
        InvoiceCommand::AgedBalance { today } => {
            let today = today.unwrap_or_else(griffe_core::clock::today_local);
            let aged = aged_balance(store.connection(), today)?;
            if json {
                let payload: Vec<_> = aged
                    .iter()
                    .map(|a| {
                        serde_json::json!({
                            "invoice_id": a.invoice_id.to_string(),
                            "client_id": a.client_id.to_string(),
                            "outstanding_cents": a.outstanding.cents(),
                            "days_overdue": a.days_overdue,
                            "bucket": format!("{:?}", a.bucket),
                        })
                    })
                    .collect();
                format_json(&payload)
            } else if aged.is_empty() {
                "aucune facture en attente d'encaissement".to_string()
            } else {
                let bucket = |b: &billing::AgingBucket| match b {
                    billing::AgingBucket::Current => "non échue",
                    billing::AgingBucket::Due1To30 => "1 à 30 j",
                    billing::AgingBucket::Due31To60 => "31 à 60 j",
                    billing::AgingBucket::Due61To90 => "61 à 90 j",
                    billing::AgingBucket::Due91Plus => "plus de 90 j",
                };
                let rows: Vec<Vec<String>> = aged
                    .iter()
                    .map(|a| {
                        vec![
                            a.invoice_id.to_string(),
                            a.client_id.to_string(),
                            a.outstanding.to_string(),
                            a.days_overdue.to_string(),
                            bucket(&a.bucket).to_string(),
                        ]
                    })
                    .collect();
                let total: griffe_core::domain::Money = aged.iter().map(|a| a.outstanding).sum();
                format!(
                    "{}\nEncours total : {total}",
                    crate::table::render(
                        &["Facture", "Client", "Restant dû", "Retard (j)", "Tranche"],
                        &rows
                    )
                )
            }
        }
        InvoiceCommand::Render { id, out } => {
            let invoice = billing::invoice_by_id(store.connection(), id)?
                .ok_or_else(|| CliError::Domain(format!("facture introuvable : {id}")))?;
            if invoice.origin == InvoiceOrigin::Imported {
                return Err(CliError::Domain(
                    "cette facture est née ailleurs : ouvrez le papier importé, ne la re-rendez pas"
                        .to_string(),
                ));
            }
            let client = griffe_core::clients::client_by_id(store.connection(), invoice.client_id)?
                .ok_or_else(|| {
                    CliError::Domain(format!("client introuvable : {}", invoice.client_id))
                })?;
            let profile =
                griffe_core::company::company_profile(store.connection())?.ok_or_else(|| {
                    CliError::Domain(
                        "aucun profil d'entreprise défini : freeflow company set-profile"
                            .to_string(),
                    )
                })?;
            let pdf = griffe_invoice::render_pdf(&invoice, &client, &profile, None)
                .map_err(|e| CliError::Unexpected(e.to_string()))?;
            let size = pdf.len();
            std::fs::write(&out, &pdf).map_err(|e| {
                CliError::Unexpected(format!("écriture de {} impossible : {e}", out.display()))
            })?;
            let _ = crate::papers::capture_invoice(store, ctx, id);
            format!("✓ {} écrit ({size} octets)", out.display())
        }
    };
    Ok(output)
}

pub fn run_payment(
    cmd: PaymentCommand,
    store: &mut Store,
    ctx: &ExecutionContext,
    json: bool,
) -> Result<String, CliError> {
    let output = match cmd {
        PaymentCommand::Record {
            invoice,
            amount,
            received_on,
            method,
        } => {
            let command = billing::RecordPayment {
                invoice_id: invoice,
                amount,
                received_on,
                method,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        PaymentCommand::List => {
            let payments = list_payments(store.connection())?;
            if json {
                format_json(&payments)
            } else {
                payment_table(store, &payments)?
            }
        }
        PaymentCommand::Void { id, reason } => {
            let command = billing::VoidPayment {
                payment_id: id,
                reason,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
    };
    Ok(output)
}

/// Numéro de facture plutôt qu'UUID dans la colonne facture — c'est lui que l'utilisateur
/// connaît (il figure sur le PDF envoyé au client).
fn payment_table(
    store: &Store,
    payments: &[griffe_core::domain::Payment],
) -> Result<String, CliError> {
    let invoices = list_invoices(store.connection())?;
    let number_of = |id: InvoiceId| {
        invoices
            .iter()
            .find(|i| i.id == id)
            .map_or_else(|| "?".to_string(), |i| i.number.clone())
    };
    let rows = payments
        .iter()
        .map(|p| {
            vec![
                p.id.to_string(),
                number_of(p.invoice_id),
                griffe_core::domain::format_date(p.received_on),
                p.amount.to_string(),
                p.method.as_str().to_string(),
                if p.is_voided() {
                    "annulé".to_string()
                } else if p.bank_transaction_id.is_some() {
                    "rapproché".to_string()
                } else {
                    "—".to_string()
                },
            ]
        })
        .collect::<Vec<_>>();
    Ok(crate::table::render(
        &["id", "facture", "reçu le", "montant", "méthode", "statut"],
        &rows,
    ))
}

/// L'aperçu d'un import (`--dry-run`) : ce que la détection a compris, le nombre de lignes
/// retenues et de doublons déjà en base, les cinq premières, les lignes sautées.
fn import_preview(
    store: &Store,
    parsed: &billing::ParsedStatement,
    json: bool,
) -> Result<String, CliError> {
    let fresh = billing::new_transactions_among(store.connection(), &parsed.transactions)?;
    let duplicates = fresh.iter().filter(|f| !**f).count();
    let d = &parsed.dialect;
    if json {
        return Ok(format_json(&serde_json::json!({
            "dialect": d,
            "transactions": parsed.transactions.len(),
            "new": parsed.transactions.len() - duplicates,
            "duplicates": duplicates,
            "skipped": parsed.skipped,
            "preview": parsed.transactions.iter().take(5).collect::<Vec<_>>(),
        })));
    }
    let rows: Vec<Vec<String>> = parsed
        .transactions
        .iter()
        .zip(&fresh)
        .take(5)
        .map(|(t, is_new)| {
            vec![
                griffe_core::domain::format_date(t.occurred_on),
                Money::from_cents(t.amount_cents).to_string(),
                t.description.clone(),
                if *is_new {
                    "nouvelle"
                } else {
                    "déjà importée"
                }
                .to_string(),
            ]
        })
        .collect();
    let mut out = format!(
        "(dry-run) relevé lu comme {} en {}{}{}{}{} — colonnes : {}\n{} transaction(s), {} \
         nouvelle(s), {duplicates} doublon(s) déjà en base\n{}",
        match d.format {
            billing::StatementFormat::Csv => "CSV",
            billing::StatementFormat::Ofx => "OFX",
            billing::StatementFormat::Xlsx => "Excel (xlsx)",
        },
        d.encoding,
        d.separator.map_or(String::new(), |s| format!(
            ", séparateur « {} »",
            if s == '\t' {
                "tab".to_string()
            } else {
                s.to_string()
            }
        )),
        d.decimal
            .map_or(String::new(), |c| format!(", décimale « {c} »")),
        d.date_format
            .as_deref()
            .map_or(String::new(), |f| format!(", dates {f}")),
        d.header_line
            .map_or(String::new(), |l| format!(", en-tête ligne {l}")),
        d.columns,
        parsed.transactions.len(),
        parsed.transactions.len() - duplicates,
        crate::table::render(&["date", "montant", "libellé", "état"], &rows),
    );
    if parsed.transactions.len() > 5 {
        out.push_str(&format!(
            "… et {} autre(s)\n",
            parsed.transactions.len() - 5
        ));
    }
    if !parsed.skipped.is_empty() {
        out.push_str(&format!(
            "{} ligne(s) sautée(s) : {}\n",
            parsed.skipped.len(),
            parsed
                .skipped
                .iter()
                .map(|(l, r)| format!("ligne {l} ({r})"))
                .collect::<Vec<_>>()
                .join(" ; ")
        ));
    }
    Ok(out.trim_end().to_string())
}

fn bank_table(transactions: &[griffe_core::domain::BankTransaction]) -> String {
    let rows = transactions
        .iter()
        .map(|t| {
            vec![
                t.id.to_string(),
                griffe_core::domain::format_date(t.occurred_on),
                Money::from_cents(t.amount_cents).to_string(),
                t.description.clone(),
                if t.matched_invoice_id.is_some() {
                    "rapprochée (facture)".to_string()
                } else if t.matched_expense_id.is_some() {
                    "rapprochée (dépense)".to_string()
                } else if let Some(account) = &t.settlement_account {
                    format!(
                        "réglée ({account} {})",
                        t.settlement_label.as_deref().unwrap_or_default()
                    )
                } else {
                    "à rapprocher".to_string()
                },
            ]
        })
        .collect::<Vec<_>>();
    crate::table::render(&["id", "date", "montant", "libellé", "affectation"], &rows)
}

pub fn run_bank(
    cmd: BankCommand,
    store: &mut Store,
    ctx: &ExecutionContext,
    json: bool,
) -> Result<String, CliError> {
    let output = match cmd {
        BankCommand::Import { format, file } => {
            let bytes = std::fs::read(&file).map_err(|e| {
                CliError::Unexpected(format!("lecture de {} impossible : {e}", file.display()))
            })?;
            let hint = format.map(|f| match f {
                ImportFormat::Csv => billing::StatementFormat::Csv,
                ImportFormat::Ofx => billing::StatementFormat::Ofx,
                ImportFormat::Xlsx => billing::StatementFormat::Xlsx,
            });
            let parsed = billing::parse_bank_statement(&bytes, hint)
                .map_err(|e| CliError::Domain(e.to_string()))?;
            if ctx.dry_run {
                return import_preview(store, &parsed, json);
            }
            let original_name = file
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| "releve.csv".to_string());
            let period = {
                let fye = griffe_core::company::company_profile(store.connection())?
                    .and_then(|p| p.fiscal_year_end)
                    .unwrap_or(griffe_core::domain::FiscalYearEnd::CALENDAR);
                parsed
                    .transactions
                    .first()
                    .map(|t| fye.containing(t.occurred_on).end().year())
            };
            let command = billing::ImportBankTransactions {
                transactions: parsed.transactions,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            if matches!(outcome, griffe_core::app::Outcome::Applied(_)) {
                let _ = crate::papers::capture_bank_statement(
                    store,
                    ctx,
                    &original_name,
                    &bytes,
                    period,
                );
            }
            let rendered = format_outcome(&outcome, json);
            if json || parsed.skipped.is_empty() {
                rendered
            } else {
                format!(
                    "{rendered}\n{} ligne(s) sautée(s) : {}",
                    parsed.skipped.len(),
                    parsed
                        .skipped
                        .iter()
                        .map(|(l, r)| format!("ligne {l} ({r})"))
                        .collect::<Vec<_>>()
                        .join(" ; ")
                )
            }
        }
        BankCommand::Rm { transaction } => {
            let command = billing::DeleteBankTransaction {
                transaction_id: transaction,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome_as(&outcome, json, |()| {
                format!("transaction {transaction} supprimée")
            })
        }
        BankCommand::List { unmatched } => {
            let mut transactions = list_bank_transactions(store.connection())?;
            if unmatched {
                transactions.retain(|t| !t.is_matched());
            }
            if json {
                format_json(&transactions)
            } else {
                bank_table(&transactions)
            }
        }
        BankCommand::Reconcile {
            transaction,
            invoice,
        } => {
            let command = billing::ReconcileTransaction {
                transaction_id: transaction,
                invoice_id: invoice,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        BankCommand::Unreconcile { transaction } => {
            let command = billing::UnreconcileTransaction {
                transaction_id: transaction,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome_as(&outcome, json, |payment| match payment {
                Some(id) => format!("transaction libérée, encaissement {id} annulé"),
                None => "transaction libérée".to_string(),
            })
        }
        BankCommand::Settle {
            transaction,
            account,
            label,
        } => {
            let command = billing::SettleBankTransaction {
                transaction_id: transaction,
                account: account.clone(),
                label,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome_as(&outcome, json, |()| {
                format!("transaction {transaction} réglée sur le compte {account}")
            })
        }
        BankCommand::Unsettle { transaction } => {
            let command = billing::UnsettleBankTransaction {
                transaction_id: transaction,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome_as(&outcome, json, |()| {
                format!("règlement défait : la transaction {transaction} est à rapprocher")
            })
        }
    };
    Ok(output)
}
