//! `freeflow invoice ...`, `freeflow payment ...`, `freeflow bank ...`
//!
//! Les lignes de facture suivent le même principe que les lignes de devis (lot `quote.rs`) :
//! JSON via `--lines`, ex. `--lines '[{"description":"Sept.","quantity":9.5,"unit_price":65000,"vat_rate":"Standard"}]'`

use std::path::PathBuf;

use clap::{Subcommand, ValueEnum};
use freeflow_core::app::{ExecutionContext, Executor};
use freeflow_core::billing::{
    self, aged_balance, list_bank_transactions, list_invoices, list_payments,
    parse_csv_bank_statement, parse_ofx_bank_statement, verify_chain,
};
use freeflow_core::domain::{
    BankTransactionId, ClientId, InvoiceId, InvoiceLine, MissionId, Money, PaymentId, PaymentMethod,
};
use freeflow_core::store::Store;
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
    /// profil d'entreprise défini (`freeflow company set-profile`).
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
    /// Importe un relevé bancaire (CSV ou OFX).
    Import {
        #[arg(long, value_enum)]
        format: ImportFormat,
        file: PathBuf,
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
        #[arg(long, value_parser = clap::value_parser!(freeflow_core::domain::SettlementAccount))]
        account: freeflow_core::domain::SettlementAccount,
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
            format_outcome(&outcome, json)
        }
        InvoiceCommand::CreditNote { id, issued_on } => {
            let command = billing::IssueCreditNote {
                invoice_id: id,
                issued_on,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
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
            let today = today.unwrap_or_else(freeflow_core::clock::today_local);
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
                let total: freeflow_core::domain::Money = aged.iter().map(|a| a.outstanding).sum();
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
            let client =
                freeflow_core::clients::client_by_id(store.connection(), invoice.client_id)?
                    .ok_or_else(|| {
                        CliError::Domain(format!("client introuvable : {}", invoice.client_id))
                    })?;
            let profile =
                freeflow_core::company::company_profile(store.connection())?.ok_or_else(|| {
                    CliError::Domain(
                        "aucun profil d'entreprise défini : freeflow company set-profile"
                            .to_string(),
                    )
                })?;
            let pdf = freeflow_invoice::render_pdf(&invoice, &client, &profile, None)
                .map_err(|e| CliError::Unexpected(e.to_string()))?;
            let size = pdf.len();
            std::fs::write(&out, pdf).map_err(|e| {
                CliError::Unexpected(format!("écriture de {} impossible : {e}", out.display()))
            })?;
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
    payments: &[freeflow_core::domain::Payment],
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
                freeflow_core::domain::format_date(p.received_on),
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

fn bank_table(transactions: &[freeflow_core::domain::BankTransaction]) -> String {
    let rows = transactions
        .iter()
        .map(|t| {
            vec![
                t.id.to_string(),
                freeflow_core::domain::format_date(t.occurred_on),
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
            let content = std::fs::read_to_string(&file).map_err(|e| {
                CliError::Unexpected(format!("lecture de {} impossible : {e}", file.display()))
            })?;
            let transactions = match format {
                ImportFormat::Csv => parse_csv_bank_statement(&content),
                ImportFormat::Ofx => parse_ofx_bank_statement(&content),
            }
            .map_err(|e| CliError::Domain(e.to_string()))?;
            let command = billing::ImportBankTransactions { transactions };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
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
