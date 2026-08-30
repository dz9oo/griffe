//! `freeflow invoice ...`, `freeflow payment ...`, `freeflow bank ...`
//!
//! Les lignes de facture suivent le même principe que les lignes de devis (lot `quote.rs`) :
//! JSON via `--lines`, ex. `--lines '[{"description":"Sept.","quantity":9.5,"unit_price":65000,"vat_rate":"Standard"}]'`

use std::path::PathBuf;

use clap::{Subcommand, ValueEnum};
use freeflow_core::app::{ExecutionContext, Executor};
use freeflow_core::billing::{
    self, aged_balance, parse_csv_bank_statement, parse_ofx_bank_statement, verify_chain,
};
use freeflow_core::domain::{
    BankTransactionId, ClientId, InvoiceId, InvoiceLine, MissionId, Money, PaymentMethod,
};
use freeflow_core::store::Store;
use time::Date;

use crate::error::CliError;
use crate::output::{format_outcome, format_value};
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
        #[arg(long, value_parser = parse_date)]
        today: Date,
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
}

#[derive(Debug, Subcommand)]
pub enum BankCommand {
    /// Importe un relevé bancaire (CSV ou OFX).
    Import {
        #[arg(long, value_enum)]
        format: ImportFormat,
        file: PathBuf,
    },
    /// Rapproche une transaction importée avec une facture (crée l'encaissement correspondant).
    Reconcile {
        #[arg(long, value_parser = clap::value_parser!(BankTransactionId))]
        transaction: BankTransactionId,
        #[arg(long, value_parser = clap::value_parser!(InvoiceId))]
        invoice: InvoiceId,
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
            let payload = match status {
                billing::ChainStatus::Intact => serde_json::json!({"status": "intact"}),
                billing::ChainStatus::BrokenAt(id) => {
                    serde_json::json!({"status": "broken", "broken_at": id.to_string()})
                }
            };
            let rendered = format_value(&payload, json);
            if matches!(status, billing::ChainStatus::BrokenAt(_)) {
                return Err(CliError::Domain(format!(
                    "{rendered}\nla chaîne de factures est rompue"
                )));
            }
            rendered
        }
        InvoiceCommand::AgedBalance { today } => {
            let aged = aged_balance(store.connection(), today)?;
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
            format_value(&payload, json)
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
    let PaymentCommand::Record {
        invoice,
        amount,
        received_on,
        method,
    } = cmd;
    let command = billing::RecordPayment {
        invoice_id: invoice,
        amount,
        received_on,
        method,
    };
    let outcome = Executor::new(store).execute(&command, ctx)?;
    Ok(format_outcome(&outcome, json))
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
    };
    Ok(output)
}
