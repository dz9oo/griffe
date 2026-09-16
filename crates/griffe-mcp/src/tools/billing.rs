//! Outils `invoice.*`, `payment.*`, `bank.*` — miroir de `freeflow invoice/payment/bank ...`
//! (CLI, lot 7). `invoice.emit` et `invoice.credit_note` sont marqués `destructive_hint` : la
//! politique de confirmation (lot 2) dépose une `PendingAction` plutôt que d'appliquer l'effet
//! quand l'acteur est un agent — ce que ces outils sont, systématiquement.
//!
//! Lot 22 : `payment.list`/`payment.void` et `bank.list`/`bank.unreconcile` (corrections
//! d'encaissement, contre-écriture), et remboursement de la dette `dry_run` des cinq outils
//! mutants qui appelaient encore `self.ctx(false)` en dur.

use griffe_core::app::Executor;
use griffe_core::billing::{
    self, aged_balance, list_bank_transactions, list_payments, verify_chain,
};
use griffe_core::domain::{
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
    /// `csv`, `ofx` ou `xlsx` — facultatif, détecté sinon.
    format: Option<String>,
    /// Contenu du relevé en texte (UTF-8). Pour un export en latin-1 ou un fichier binaire,
    /// préférez `content_base64` ou `path`.
    content: Option<String>,
    /// Contenu du relevé encodé en base64 (octets exacts du fichier : l'encodage est détecté).
    content_base64: Option<String>,
    /// Chemin local du fichier de relevé — lu par le serveur (IO d'adaptateur).
    path: Option<String>,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct BankDeleteArgs {
    transaction_id: String,
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
pub(crate) struct BankSettleArgs {
    transaction_id: String,
    /// Compte de bilan réglé (classes 1 à 5, hors 512), ex. `401000` (fournisseurs / honoraires
    /// repris au bilan), `444000` (solde d'IS repris), `445510` (TVA à décaisser), `445670`
    /// (crédit de TVA remboursé), `455000` (compte courant d'associé), `457000` (dividendes
    /// payés), `580000` (virement entre les comptes de la société), `164000` (emprunt).
    account: String,
    /// Libellé du compte, requis seulement s'il n'est pas dans le plan de comptes de FreeFlow.
    label: Option<String>,
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
            griffe_core::domain::parse_date(&args.issued_on)
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
            griffe_core::domain::parse_date(&args.issued_on)
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
        let mut store = self.store.lock().await;
        let invoice = match billing::invoice_by_id(store.connection(), invoice_id) {
            Ok(Some(invoice)) => invoice,
            Ok(None) => return err_text(format!("facture introuvable : {invoice_id}")),
            Err(e) => return err_text(e.to_string()),
        };
        let client = match griffe_core::clients::client_by_id(store.connection(), invoice.client_id)
        {
            Ok(Some(client)) => client,
            Ok(None) => return err_text(format!("client introuvable : {}", invoice.client_id)),
            Err(e) => return err_text(e.to_string()),
        };
        let profile = match griffe_core::company::company_profile(store.connection()) {
            Ok(Some(p)) => p,
            Ok(None) => {
                return err_text("aucun profil d'entreprise défini : company.set_profile");
            }
            Err(e) => return err_text(e.to_string()),
        };
        let pdf = ok_or_return!(
            "render",
            griffe_invoice::render_pdf(&invoice, &client, &profile, args.payment_terms_days)
        );
        let written = match crate::tools::fiscal::write_new_document(&args.out, &pdf) {
            Ok(msg) => msg,
            Err(e) => return err_text(e),
        };
        let _ = griffe_cli::capture_invoice(&mut store, &self.ctx(false), invoice_id);
        ok_json(json!({ "written": written }))
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
        let today = ok_or_return!("today", griffe_core::domain::parse_date(&args.today));
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
            griffe_core::domain::parse_date(&args.received_on)
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

    /// Importe un relevé bancaire — l'export CSV de la banque tel quel (Qonto, Shine,
    /// Boursorama, Crédit Agricole, BNP, LCL, La Banque Postale…), un OFX, ou l'Excel (xlsx)
    /// de Tiime (feuille Transactions) : encodage, séparateur, décimale, format de date et
    /// colonnes sont détectés ; `format` force `csv`/`ofx`/`xlsx`. Fournir `content` (texte),
    /// `content_base64` (octets) ou `path` (fichier local). Les doublons (identifiant de
    /// banque, sinon date + montant + libellé) sont ignorés. `dry_run` renvoie l'aperçu :
    /// dialecte détecté, colonnes, nombre de lignes nouvelles et de doublons, lignes sautées.
    #[tool(
        name = "bank.import",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn bank_import(&self, Parameters(args): Parameters<BankImportArgs>) -> CallToolResult {
        use base64::Engine as _;
        let bytes: Vec<u8> = match (&args.content, &args.content_base64, &args.path) {
            (Some(text), _, _) => text.clone().into_bytes(),
            (None, Some(b64), _) => {
                ok_or_return!(
                    "content_base64",
                    base64::engine::general_purpose::STANDARD.decode(b64.trim())
                )
            }
            (None, None, Some(path)) => ok_or_return!("path", std::fs::read(path)),
            (None, None, None) => {
                return err_text("fournissez content, content_base64 ou path");
            }
        };
        let hint = match args.format.as_deref() {
            None => None,
            Some(raw) => Some(ok_or_return!(
                "format",
                raw.parse::<billing::StatementFormat>()
            )),
        };
        let parsed = ok_or_return!("content", billing::parse_bank_statement(&bytes, hint));
        let mut store = self.store.lock().await;
        if args.dry_run {
            let fresh = ok_or_return!(
                "content",
                billing::new_transactions_among(store.connection(), &parsed.transactions)
            );
            let duplicates = fresh.iter().filter(|f| !**f).count();
            return ok_json(json!({
                "status": "dry_run",
                "dialect": parsed.dialect,
                "transactions": parsed.transactions.len(),
                "new": parsed.transactions.len() - duplicates,
                "duplicates": duplicates,
                "skipped": parsed.skipped,
                "preview": parsed.transactions.iter().take(5).collect::<Vec<_>>(),
            }));
        }
        let original_name = args
            .path
            .as_deref()
            .and_then(|p| std::path::Path::new(p).file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "releve.csv".to_string());
        let period = {
            let fye = griffe_core::company::company_profile(store.connection())
                .ok()
                .flatten()
                .and_then(|p| p.fiscal_year_end)
                .unwrap_or(griffe_core::domain::FiscalYearEnd::CALENDAR);
            parsed
                .transactions
                .first()
                .map(|t| fye.containing(t.occurred_on).end().year())
        };
        let cmd = billing::ImportBankTransactions {
            transactions: parsed.transactions,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(false)) {
            Ok(outcome) => {
                if matches!(outcome, griffe_core::app::Outcome::Applied(_)) {
                    let _ = griffe_cli::capture_bank_statement(
                        &mut store,
                        &self.ctx(false),
                        &original_name,
                        &bytes,
                        period,
                    );
                }
                let mut body = outcome_json(&outcome);
                body["skipped"] = json!(parsed.skipped);
                ok_json(body)
            }
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Supprime une transaction importée par erreur — non rapprochée seulement (défaire
    /// d'abord). Action sensible : un agent la propose, seul un humain (`freeflow confirm`)
    /// l'applique.
    #[tool(
        name = "bank.delete",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false
        )
    )]
    async fn bank_delete(&self, Parameters(args): Parameters<BankDeleteArgs>) -> CallToolResult {
        let transaction_id: BankTransactionId =
            ok_or_return!("transaction_id", args.transaction_id.parse());
        let cmd = billing::DeleteBankTransaction { transaction_id };
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

    /// Règle un compte de bilan depuis un mouvement du relevé (lot 37) — ni charge ni produit,
    /// là où `expense.record` compterait une charge : dette reprise au bilan d'ouverture (401
    /// honoraires, 444 solde d'IS, 4455 TVA), compte courant d'associé (455), dividendes (457),
    /// virement entre les comptes de la société (580), emprunt (164). Le libellé vient du plan
    /// de comptes ; `label` pour un compte hors plan. Action sensible : un agent la propose,
    /// seul un humain (`freeflow confirm`) l'applique.
    #[tool(
        name = "bank.settle",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn bank_settle(&self, Parameters(args): Parameters<BankSettleArgs>) -> CallToolResult {
        let transaction_id: BankTransactionId =
            ok_or_return!("transaction_id", args.transaction_id.parse());
        let account: griffe_core::domain::SettlementAccount =
            ok_or_return!("account", args.account.parse());
        let cmd = billing::SettleBankTransaction {
            transaction_id,
            account,
            label: args.label,
        };
        let mut store = self.store.lock().await;
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Défait un règlement de compte de bilan (`bank.settle`) : la transaction redevient « à
    /// rapprocher ». Même exigence de confirmation.
    #[tool(
        name = "bank.unsettle",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false
        )
    )]
    async fn bank_unsettle(
        &self,
        Parameters(args): Parameters<BankUnreconcileArgs>,
    ) -> CallToolResult {
        let transaction_id: BankTransactionId =
            ok_or_return!("transaction_id", args.transaction_id.parse());
        let cmd = billing::UnsettleBankTransaction { transaction_id };
        let mut store = self.store.lock().await;
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
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
