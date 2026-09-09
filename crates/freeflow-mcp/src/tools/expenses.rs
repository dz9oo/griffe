//! Outils `expense.*` — miroir de `freeflow expense ...` (lot 21 : première apparition des
//! dépenses côté MCP, qui étaient jusqu'ici la principale dette de parité du serveur).
//!
//! Pas de justificatif ici : l'archivage d'un fichier (lecture, hash, copie en `receipts/`) est
//! de l'IO d'adaptateur au poste de l'utilisateur — voir `freeflow-cli::expense::archive_receipt`
//! — et un agent MCP n'a pas à manipuler des chemins locaux. `expense.update` conserve donc tel
//! quel le justificatif existant ; l'attacher, le remplacer ou le détacher se fait en CLI.
//!
//! Lot 33 : `expense.reconcile` et `bank_transaction_id` sur `expense.record` rapprochent une
//! dépense d'un débit du relevé (`bank.list` avec `unmatched`) — actions de rapprochement
//! bancaire, derrière la même confirmation humaine que `bank.reconcile`.

use freeflow_core::app::Executor;
use freeflow_core::billing::bank_transaction_by_id;
use freeflow_core::domain::{BankTransactionId, ExpenseCategory, Money, VatRate};
use freeflow_core::expenses::{self, expense_by_id, expense_detail, list_expenses};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::server::FreeflowServer;
use crate::support::{err_text, ok_json, ok_or_return, outcome_json, resolve_expense};

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct RecordExpenseArgs {
    label: String,
    /// `software`, `equipment`, `travel`, `meals`, `office`, `professional`, `fees`
    /// (honoraires), `bank_charges` (frais bancaires) ou `other`.
    category: String,
    /// Montant total TTC, en centimes — repris du débit si omis avec `bank_transaction_id`.
    amount_cents: Option<i64>,
    /// `standard`, `intermediate`, `reduced`, `super_reduced`, ou `zero`.
    vat_rate: String,
    /// TVA effectivement déductible, en centimes (peut être inférieure à `amount × taux`).
    vat_deductible_cents: i64,
    /// Date d'engagement — reprise de la date du débit si omise avec `bank_transaction_id`.
    incurred_on: Option<String>,
    /// Débit du relevé importé que cette dépense paie : elle est créée rapprochée, au montant
    /// exact du débit. Action de rapprochement : dépose une action en attente de confirmation
    /// humaine.
    bank_transaction_id: Option<String>,
    /// Bénéficiaire (fournisseur) — pour les honoraires, le nom qui cumule sur la DAS2
    /// (seuil 2 400 € par bénéficiaire et par année civile).
    supplier: Option<String>,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ReconcileExpenseArgs {
    /// Référence de la dépense : UUID, préfixe d'UUID, ou libellé.
    expense: String,
    /// Débit du relevé importé (`bank.list` avec `unmatched`), au montant exact de la dépense.
    transaction_id: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct AttachReceiptArgs {
    /// Référence de la dépense (libellé, préfixe d'UUID).
    expense: String,
    /// Chemin local du justificatif — lu et archivé chiffré par le serveur (IO d'adaptateur).
    path: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ExpenseRefArgs {
    /// Référence de la dépense : UUID, préfixe d'UUID, ou libellé.
    expense: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ExpenseRefMutationArgs {
    expense: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct UpdateExpenseArgs {
    expense: String,
    label: Option<String>,
    category: Option<String>,
    amount_cents: Option<i64>,
    vat_rate: Option<String>,
    vat_deductible_cents: Option<i64>,
    incurred_on: Option<String>,
    /// Bénéficiaire (fournisseur), pour la DAS2 ; inchangé si absent.
    supplier: Option<String>,
    /// Efface le bénéficiaire (défaut : faux).
    #[serde(default)]
    clear_supplier: bool,
    #[serde(default)]
    dry_run: bool,
}

#[tool_router(router = expenses_router, vis = "pub(crate)")]
impl FreeflowServer {
    /// Enregistre une dépense professionnelle. Refusé si sa date tombe dans un exercice déjà
    /// clôturé.
    #[tool(
        name = "expense.record",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn expense_record(
        &self,
        Parameters(args): Parameters<RecordExpenseArgs>,
    ) -> CallToolResult {
        let category: ExpenseCategory = ok_or_return!("category", args.category.parse());
        let vat_rate: VatRate = ok_or_return!("vat_rate", args.vat_rate.parse());
        let incurred_on = match &args.incurred_on {
            Some(s) => Some(ok_or_return!(
                "incurred_on",
                freeflow_core::domain::parse_date(s)
            )),
            None => None,
        };
        let bank_transaction_id: Option<BankTransactionId> = match &args.bank_transaction_id {
            Some(s) => Some(ok_or_return!("bank_transaction_id", s.parse())),
            None => None,
        };
        let mut store = self.store.lock().await;
        // Comme la CLI : le montant et la date manquants viennent du débit lui-même.
        let debit = match bank_transaction_id {
            Some(id) => match bank_transaction_by_id(store.connection(), id) {
                Ok(Some(tx)) => Some(tx),
                Ok(None) => return err_text(format!("transaction bancaire introuvable : {id}")),
                Err(e) => return err_text(e.to_string()),
            },
            None => None,
        };
        let (amount, incurred_on) = match (&debit, args.amount_cents, incurred_on) {
            (Some(debit), amount, incurred_on) => (
                Money::from_cents(amount.unwrap_or(-debit.amount_cents)),
                incurred_on.unwrap_or(debit.occurred_on),
            ),
            (None, Some(amount), Some(incurred_on)) => (Money::from_cents(amount), incurred_on),
            (None, _, _) => {
                return err_text(
                    "amount_cents et incurred_on sont requis sans bank_transaction_id",
                );
            }
        };
        let cmd = expenses::RecordExpense {
            label: args.label,
            category,
            amount,
            vat_rate,
            vat_deductible: Money::from_cents(args.vat_deductible_cents),
            incurred_on,
            receipt_hash: None,
            receipt_filename: None,
            supplier: args.supplier,
            bank_transaction_id,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Liste les dépenses, les plus récentes d'abord.
    #[tool(
        name = "expense.list",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn expense_list(&self) -> CallToolResult {
        let store = self.store.lock().await;
        match list_expenses(store.connection()) {
            Ok(all) => ok_json(all),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Affiche une dépense, avec le débit du relevé rapproché (`bank_transaction`, `null` si
    /// non rapprochée).
    #[tool(
        name = "expense.show",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn expense_show(&self, Parameters(args): Parameters<ExpenseRefArgs>) -> CallToolResult {
        let store = self.store.lock().await;
        let id = ok_or_return!("expense", resolve_expense(&store, &args.expense));
        match expense_detail(store.connection(), id) {
            Ok(Some(detail)) => ok_json(detail),
            Ok(None) => err_text(format!("dépense introuvable : {id}")),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Rapproche une dépense existante d'un débit du relevé importé, au montant exact — le
    /// pendant de `bank.reconcile` pour les sorties d'argent. Action de rapprochement bancaire :
    /// un agent la propose, seul un humain (`freeflow confirm`) l'applique. Défaire :
    /// `bank.unreconcile` (la dépense reste, la transaction est libérée).
    #[tool(
        name = "expense.reconcile",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn expense_reconcile(
        &self,
        Parameters(args): Parameters<ReconcileExpenseArgs>,
    ) -> CallToolResult {
        let transaction_id: BankTransactionId =
            ok_or_return!("transaction_id", args.transaction_id.parse());
        let mut store = self.store.lock().await;
        let expense_id = ok_or_return!("expense", resolve_expense(&store, &args.expense));
        let cmd = expenses::ReconcileExpense {
            transaction_id,
            expense_id,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Modifie une dépense existante — seuls les champs fournis changent, le justificatif
    /// existant est conservé tel quel (voir la description du module). Refusé si sa date tombe
    /// dans un exercice déjà clôturé.
    #[tool(
        name = "expense.update",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn expense_update(
        &self,
        Parameters(args): Parameters<UpdateExpenseArgs>,
    ) -> CallToolResult {
        let mut store = self.store.lock().await;
        let id = ok_or_return!("expense", resolve_expense(&store, &args.expense));
        let current = match expense_by_id(store.connection(), id) {
            Ok(Some(e)) => e,
            Ok(None) => return err_text(format!("dépense introuvable : {id}")),
            Err(e) => return err_text(e.to_string()),
        };
        let category: ExpenseCategory = match &args.category {
            Some(s) => ok_or_return!("category", s.parse()),
            None => current.category,
        };
        let vat_rate: VatRate = match &args.vat_rate {
            Some(s) => ok_or_return!("vat_rate", s.parse()),
            None => current.vat_rate,
        };
        let incurred_on = match &args.incurred_on {
            Some(s) => ok_or_return!("incurred_on", freeflow_core::domain::parse_date(s)),
            None => current.incurred_on,
        };
        let cmd = expenses::UpdateExpense {
            id,
            revision: current.revision,
            label: args.label.unwrap_or(current.label),
            category,
            amount: args.amount_cents.map_or(current.amount, Money::from_cents),
            vat_rate,
            vat_deductible: args
                .vat_deductible_cents
                .map_or(current.vat_deductible, Money::from_cents),
            incurred_on,
            receipt_hash: current.receipt_hash,
            receipt_filename: current.receipt_filename,
            supplier: if args.clear_supplier {
                None
            } else {
                args.supplier.or(current.supplier)
            },
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Joint (ou remplace) le justificatif d'une dépense depuis un fichier local — même dans un
    /// exercice clôturé (une pièce ne change ni le montant ni la date). Le fichier est archivé
    /// chiffré dans `<coffre>.receipts/` ; la dépense garde son hash SHA-256.
    #[tool(
        name = "expense.attach_receipt",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true
        )
    )]
    async fn expense_attach_receipt(
        &self,
        Parameters(args): Parameters<AttachReceiptArgs>,
    ) -> CallToolResult {
        let content = ok_or_return!("path", std::fs::read(&args.path));
        let original = std::path::Path::new(&args.path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "justificatif".to_string());
        let mut store = self.store.lock().await;
        let id = ok_or_return!("expense", resolve_expense(&store, &args.expense));
        let current = match expense_by_id(store.connection(), id) {
            Ok(Some(e)) => e,
            Ok(None) => return err_text(format!("dépense introuvable : {id}")),
            Err(e) => return err_text(e.to_string()),
        };
        let (receipt_hash, receipt_filename) = if args.dry_run {
            (expenses::hash_receipt(&content), String::new())
        } else {
            let archived = ok_or_return!(
                "path",
                freeflow_core::receipts::archive(&store, &original, &content)
            );
            (archived.hash, archived.filename)
        };
        let cmd = expenses::AttachReceipt {
            id,
            revision: current.revision,
            receipt_hash: receipt_hash.clone(),
            receipt_filename: receipt_filename.clone(),
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => {
                if matches!(outcome, freeflow_core::app::Outcome::Applied(_))
                    && !args.dry_run
                    && !receipt_filename.is_empty()
                {
                    let _ = freeflow_cli::capture_expense_receipt(
                        &mut store,
                        &self.ctx(false),
                        id,
                        &receipt_filename,
                        &receipt_hash,
                    );
                }
                ok_json(outcome_json(&outcome))
            }
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Supprime une dépense pour de bon — action sensible : un agent la propose, seul un humain
    /// (`freeflow confirm`) peut l'appliquer. Refusé si sa date tombe dans un exercice clôturé.
    #[tool(
        name = "expense.delete",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false
        )
    )]
    async fn expense_delete(
        &self,
        Parameters(args): Parameters<ExpenseRefMutationArgs>,
    ) -> CallToolResult {
        let mut store = self.store.lock().await;
        let id = ok_or_return!("expense", resolve_expense(&store, &args.expense));
        let current = match expense_by_id(store.connection(), id) {
            Ok(Some(e)) => e,
            Ok(None) => return err_text(format!("dépense introuvable : {id}")),
            Err(e) => return err_text(e.to_string()),
        };
        let cmd = expenses::DeleteExpense {
            id,
            revision: current.revision,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }
}
