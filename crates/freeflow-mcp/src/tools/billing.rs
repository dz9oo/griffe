//! Outils `invoice.*`, `payment.*`, `bank.*` — miroir de `freeflow invoice/payment/bank ...`
//! (CLI, lot 7). `invoice.emit` et `invoice.credit_note` sont marqués `destructive_hint` : la
//! politique de confirmation (lot 2) dépose une `PendingAction` plutôt que d'appliquer l'effet
//! quand l'acteur est un agent — ce que ces outils sont, systématiquement.

use freeflow_core::app::Executor;
use freeflow_core::billing::{
    self, aged_balance, parse_csv_bank_statement, parse_ofx_bank_statement, verify_chain,
};
use freeflow_core::domain::{
    BankTransactionId, ClientId, InvoiceId, InvoiceLine, MissionId, Money, PaymentMethod,
};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::server::FreeflowServer;
use crate::support::{err_text, ok_json, ok_or_return, outcome_json};

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct EmitInvoiceArgs {
    client_id: String,
    mission_id: Option<String>,
    /// Lignes de facture, au format JSON. Ex. :
    /// `[{"description":"Sept.","quantity":9.5,"unit_price":65000,"vat_rate":"Standard"}]`
    lines_json: String,
    issued_on: String,
    /// Délai de paiement en jours (défaut : 30).
    payment_terms_days: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct CreditNoteArgs {
    /// Identifiant de la facture annulée.
    id: String,
    issued_on: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct AgedBalanceArgs {
    today: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct RecordPaymentArgs {
    invoice_id: String,
    /// Montant encaissé, en centimes.
    amount_cents: i64,
    received_on: String,
    /// `bank_transfer`, `check`, `card`, ou `other`.
    method: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct BankImportArgs {
    /// `csv` ou `ofx`.
    format: String,
    /// Contenu brut du relevé bancaire.
    content: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct BankReconcileArgs {
    transaction_id: String,
    invoice_id: String,
}

#[tool_router(router = billing_router, vis = "pub(crate)")]
impl FreeflowServer {
    /// Émet une facture. Nécessite confirmation humaine — l'appel dépose une action en attente,
    /// il n'applique jamais l'émission directement.
    #[tool(
        name = "invoice.emit",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false
        )
    )]
    async fn invoice_emit(&self, Parameters(args): Parameters<EmitInvoiceArgs>) -> CallToolResult {
        let client_id: ClientId = ok_or_return!("client_id", args.client_id.parse());
        let mission_id: Option<MissionId> = match &args.mission_id {
            Some(s) => Some(ok_or_return!("mission_id", s.parse())),
            None => None,
        };
        let lines: Vec<InvoiceLine> =
            ok_or_return!("lines_json", serde_json::from_str(&args.lines_json));
        let issued_on = ok_or_return!(
            "issued_on",
            freeflow_core::domain::parse_date(&args.issued_on)
        );
        let cmd = billing::EmitInvoice {
            client_id,
            mission_id,
            lines,
            issued_on,
            payment_terms_days: args.payment_terms_days.unwrap_or(30),
        };
        let mut store = self.store.lock().await;
        match Executor::new(&mut store).execute(&cmd, &self.ctx()) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Émet un avoir annulant intégralement une facture. Même exigence de confirmation que
    /// `invoice.emit`.
    #[tool(
        name = "invoice.credit_note",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false
        )
    )]
    async fn invoice_credit_note(
        &self,
        Parameters(args): Parameters<CreditNoteArgs>,
    ) -> CallToolResult {
        let invoice_id: InvoiceId = ok_or_return!("id", args.id.parse());
        let issued_on = ok_or_return!(
            "issued_on",
            freeflow_core::domain::parse_date(&args.issued_on)
        );
        let cmd = billing::IssueCreditNote {
            invoice_id,
            issued_on,
        };
        let mut store = self.store.lock().await;
        match Executor::new(&mut store).execute(&cmd, &self.ctx()) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Revérifie la chaîne de hash de toutes les factures.
    #[tool(
        name = "invoice.verify_chain",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn invoice_verify_chain(&self) -> CallToolResult {
        let store = self.store.lock().await;
        match verify_chain(store.connection()) {
            Ok(billing::ChainStatus::Intact) => ok_json(json!({"status": "intact"})),
            Ok(billing::ChainStatus::BrokenAt(id)) => {
                ok_json(json!({"status": "broken", "broken_at": id.to_string()}))
            }
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Balance âgée des factures impayées à une date donnée.
    #[tool(
        name = "invoice.aged_balance",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn invoice_aged_balance(
        &self,
        Parameters(args): Parameters<AgedBalanceArgs>,
    ) -> CallToolResult {
        let today = ok_or_return!("today", freeflow_core::domain::parse_date(&args.today));
        let store = self.store.lock().await;
        match aged_balance(store.connection(), today) {
            Ok(aged) => ok_json(
                aged.iter()
                    .map(|a| {
                        json!({
                            "invoice_id": a.invoice_id.to_string(),
                            "client_id": a.client_id.to_string(),
                            "outstanding_cents": a.outstanding.cents(),
                            "days_overdue": a.days_overdue,
                            "bucket": format!("{:?}", a.bucket),
                        })
                    })
                    .collect::<Vec<_>>(),
            ),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Enregistre un encaissement sur une facture.
    #[tool(
        name = "payment.record",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn payment_record(
        &self,
        Parameters(args): Parameters<RecordPaymentArgs>,
    ) -> CallToolResult {
        let invoice_id: InvoiceId = ok_or_return!("invoice_id", args.invoice_id.parse());
        let received_on = ok_or_return!(
            "received_on",
            freeflow_core::domain::parse_date(&args.received_on)
        );
        let method: PaymentMethod = ok_or_return!("method", args.method.parse());
        let cmd = billing::RecordPayment {
            invoice_id,
            amount: Money::from_cents(args.amount_cents),
            received_on,
            method,
        };
        let mut store = self.store.lock().await;
        match Executor::new(&mut store).execute(&cmd, &self.ctx()) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Importe un relevé bancaire (CSV ou OFX) fourni en texte brut.
    #[tool(
        name = "bank.import",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn bank_import(&self, Parameters(args): Parameters<BankImportArgs>) -> CallToolResult {
        let transactions = match args.format.as_str() {
            "csv" => parse_csv_bank_statement(&args.content),
            "ofx" => parse_ofx_bank_statement(&args.content),
            other => {
                return err_text(format!("format invalide : {other} (attendu csv ou ofx)"));
            }
        };
        let transactions = ok_or_return!("content", transactions);
        let cmd = billing::ImportBankTransactions { transactions };
        let mut store = self.store.lock().await;
        match Executor::new(&mut store).execute(&cmd, &self.ctx()) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Rapproche une transaction bancaire importée avec une facture (crée l'encaissement
    /// correspondant).
    #[tool(
        name = "bank.reconcile",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn bank_reconcile(
        &self,
        Parameters(args): Parameters<BankReconcileArgs>,
    ) -> CallToolResult {
        let transaction_id: BankTransactionId =
            ok_or_return!("transaction_id", args.transaction_id.parse());
        let invoice_id: InvoiceId = ok_or_return!("invoice_id", args.invoice_id.parse());
        let cmd = billing::ReconcileTransaction {
            transaction_id,
            invoice_id,
        };
        let mut store = self.store.lock().await;
        match Executor::new(&mut store).execute(&cmd, &self.ctx()) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }
}
