//! Outils `expense.*` — miroir de `freeflow expense ...` (lot 21 : première apparition des
//! dépenses côté MCP, qui étaient jusqu'ici la principale dette de parité du serveur).
//!
//! Pas de justificatif ici : l'archivage d'un fichier (lecture, hash, copie en `receipts/`) est
//! de l'IO d'adaptateur au poste de l'utilisateur — voir `freeflow-cli::expense::archive_receipt`
//! — et un agent MCP n'a pas à manipuler des chemins locaux. `expense.update` conserve donc tel
//! quel le justificatif existant ; l'attacher, le remplacer ou le détacher se fait en CLI.

use freeflow_core::app::Executor;
use freeflow_core::domain::{ExpenseCategory, Money, VatRate};
use freeflow_core::expenses::{self, expense_by_id, list_expenses};
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
    /// `software`, `equipment`, `travel`, `meals`, `office`, `professional`, ou `other`.
    category: String,
    /// Montant total TTC, en centimes.
    amount_cents: i64,
    /// `standard`, `intermediate`, `reduced`, `super_reduced`, ou `zero`.
    vat_rate: String,
    /// TVA effectivement déductible, en centimes (peut être inférieure à `amount × taux`).
    vat_deductible_cents: i64,
    incurred_on: String,
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
        let incurred_on = ok_or_return!(
            "incurred_on",
            freeflow_core::domain::parse_date(&args.incurred_on)
        );
        let cmd = expenses::RecordExpense {
            label: args.label,
            category,
            amount: Money::from_cents(args.amount_cents),
            vat_rate,
            vat_deductible: Money::from_cents(args.vat_deductible_cents),
            incurred_on,
            receipt_hash: None,
            receipt_filename: None,
        };
        let mut store = self.store.lock().await;
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

    /// Affiche une dépense.
    #[tool(
        name = "expense.show",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn expense_show(&self, Parameters(args): Parameters<ExpenseRefArgs>) -> CallToolResult {
        let store = self.store.lock().await;
        let id = ok_or_return!("expense", resolve_expense(&store, &args.expense));
        match expense_by_id(store.connection(), id) {
            Ok(Some(expense)) => ok_json(expense),
            Ok(None) => err_text(format!("dépense introuvable : {id}")),
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
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
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
