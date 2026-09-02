//! Outils `invoice.*`, `payment.*`, `bank.*` — miroir de `freeflow invoice/payment/bank ...`
//! (CLI, lot 7). `invoice.emit` et `invoice.credit_note` sont marqués `destructive_hint` : la
//! politique de confirmation (lot 2) dépose une `PendingAction` plutôt que d'appliquer l'effet
//! quand l'acteur est un agent — ce que ces outils sont, systématiquement.
//!
//! Lot 22 : `payment.list`/`payment.void` et `bank.list`/`bank.unreconcile` (corrections
//! d'encaissement, contre-écriture), et remboursement de la dette `dry_run` des cinq outils
//! mutants qui appelaient encore `self.ctx(false)` en dur.

use freeflow_core::app::Executor;
use freeflow_core::billing::{
    self, aged_balance, list_bank_transactions, list_payments, parse_csv_bank_statement,
    parse_ofx_bank_statement, verify_chain,
};
use freeflow_core::domain::{
    BankTransactionId, ClientId, InvoiceId, InvoiceLine, MissionId, Money, PaymentId, PaymentMethod,
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
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct CreditNoteArgs {
    /// Identifiant de la facture annulée.
    id: String,
    issued_on: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct AgedBalanceArgs {
    today: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct RenderInvoiceArgs {
    /// Identifiant de la facture (UUID).
    id: String,
    /// Chemin du fichier PDF à écrire — refusé s'il existe déjà.
    out: String,
    /// Délai de paiement en jours, pour la mention sur le document (défaut : 30).
    payment_terms_days: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct RecordPaymentArgs {
    invoice_id: String,
    /// Montant encaissé, en centimes.
    amount_cents: i64,
    received_on: String,
    /// `bank_transfer`, `check`, `card`, ou `other`.
    method: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct BankImportArgs {
    /// `csv` ou `ofx`.
    format: String,
    /// Contenu brut du relevé bancaire.
    content: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct BankReconcileArgs {
    transaction_id: String,
    invoice_id: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct VoidPaymentArgs {
    payment_id: String,
    /// Motif de la correction, journalisé dans l'audit chaîné.
    reason: Option<String>,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct BankUnreconcileArgs {
    transaction_id: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct BankListArgs {
    /// Ne renvoie que les transactions restant à rapprocher si vrai (défaut : faux).
    #[serde(default)]
    unmatched: bool,
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
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
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
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Rend une facture émise en PDF Factur-X (PDF/A-3b + XML CII embarqué) dans `out` — refuse
    /// d'écraser un fichier existant. Comme en CLI, le rendu (`typst`) et l'écriture disque sont
    /// de l'IO d'adaptateur : rien n'est modifié en base, mais l'appel écrit un fichier sur le
    /// poste. Nécessite un profil d'entreprise (`company.set_profile`).
    #[tool(
        name = "invoice.render",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn invoice_render(
        &self,
        Parameters(args): Parameters<RenderInvoiceArgs>,
    ) -> CallToolResult {
        let invoice_id: InvoiceId = ok_or_return!("id", args.id.parse());
        let store = self.store.lock().await;
        let invoice = match billing::invoice_by_id(store.connection(), invoice_id) {
            Ok(Some(invoice)) => invoice,
            Ok(None) => return err_text(format!("facture introuvable : {invoice_id}")),
            Err(e) => return err_text(e.to_string()),
        };
        let client =
            match freeflow_core::clients::client_by_id(store.connection(), invoice.client_id) {
                Ok(Some(client)) => client,
                Ok(None) => return err_text(format!("client introuvable : {}", invoice.client_id)),
                Err(e) => return err_text(e.to_string()),
            };
        let profile = match freeflow_core::company::company_profile(store.connection()) {
            Ok(Some(p)) => p,
            Ok(None) => {
                return err_text("aucun profil d'entreprise défini : company.set_profile");
            }
            Err(e) => return err_text(e.to_string()),
        };
        let pdf = ok_or_return!(
            "render",
            freeflow_invoice::render_pdf(&invoice, &client, &profile, args.payment_terms_days)
        );
        match crate::tools::fiscal::write_new_document(&args.out, &pdf) {
            Ok(msg) => ok_json(json!({ "written": msg })),
            Err(e) => err_text(e),
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
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
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
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Rapproche un crédit importé avec une facture (crée l'encaissement correspondant) — pour
    /// un débit et une dépense, voir `expense.reconcile` / `expense.record` avec
    /// `bank_transaction_id`.
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
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Liste les encaissements, annulés compris, les plus récents d'abord.
    #[tool(
        name = "payment.list",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn payment_list(&self) -> CallToolResult {
        let store = self.store.lock().await;
        match list_payments(store.connection()) {
            Ok(payments) => ok_json(payments),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Annule un encaissement saisi à tort — contre-écriture, jamais une suppression : il reste
    /// dans l'historique mais sort de tous les calculs, et la transaction bancaire liée est
    /// libérée s'il était issu d'un rapprochement. Action sensible : un agent la propose, seul
    /// un humain (`freeflow confirm`) peut l'appliquer.
    #[tool(
        name = "payment.void",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false
        )
    )]
    async fn payment_void(&self, Parameters(args): Parameters<VoidPaymentArgs>) -> CallToolResult {
        let payment_id: PaymentId = ok_or_return!("payment_id", args.payment_id.parse());
        let cmd = billing::VoidPayment {
            payment_id,
            reason: args.reason,
        };
        let mut store = self.store.lock().await;
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Liste les transactions bancaires importées, les plus récentes d'abord (`unmatched` :
    /// celles rapprochées ni d'une facture ni d'une dépense).
    #[tool(
        name = "bank.list",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn bank_list(&self, Parameters(args): Parameters<BankListArgs>) -> CallToolResult {
        let store = self.store.lock().await;
        match list_bank_transactions(store.connection()) {
            Ok(mut transactions) => {
                if args.unmatched {
                    transactions.retain(|t| !t.is_matched());
                }
                ok_json(transactions)
            }
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Défait un rapprochement : libère la transaction et annule l'encaissement qui en était
    /// issu (rapprochement historique sans lignée : seule la transaction est libérée, le
    /// paiement s'annule via `payment.void` ; débit rapproché d'une dépense : la dépense reste,
    /// seule la transaction est libérée). Même exigence de confirmation que `payment.void`.
    #[tool(
        name = "bank.unreconcile",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false
        )
    )]
    async fn bank_unreconcile(
        &self,
        Parameters(args): Parameters<BankUnreconcileArgs>,
    ) -> CallToolResult {
        let transaction_id: BankTransactionId =
            ok_or_return!("transaction_id", args.transaction_id.parse());
        let cmd = billing::UnreconcileTransaction { transaction_id };
        let mut store = self.store.lock().await;
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }
}
