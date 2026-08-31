//! Outils `quote.*` — miroir de `freeflow quote ...` (CLI, lot 7).
//!
//! Les lignes (`QuoteLine`, polymorphes régie/forfait/récurrent) sont passées en JSON via
//! `lines_json`, comme en CLI : une mini-syntaxe dédiée serait plus complexe à produire, pour
//! un agent, qu'un objet JSON qu'il sait déjà écrire.
//! Exemple : `[{"description":"Acompte","kind":{"Forfait":{"amount":1350000}},"vat_rate":"Standard"}]`

use freeflow_core::app::Executor;
use freeflow_core::domain::{ClientId, Discount, Money, OpportunityId, QuoteId, QuoteLine};
use freeflow_core::quotes;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::server::FreeflowServer;
use crate::support::{err_text, ok_json, ok_or_return, outcome_json};

fn resolve_discount(
    percent_bps: Option<u32>,
    amount_cents: Option<i64>,
) -> Result<Option<Discount>, String> {
    match (percent_bps, amount_cents) {
        (Some(_), Some(_)) => {
            Err("discount_percent_bps et discount_amount_cents sont exclusifs".to_string())
        }
        (Some(bps), None) => Ok(Some(Discount::Percentage(bps))),
        (None, Some(cents)) => Ok(Some(Discount::FixedAmount(Money::from_cents(cents)))),
        (None, None) => Ok(None),
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct CreateQuoteArgs {
    client_id: String,
    opportunity_id: Option<String>,
    /// Lignes du devis, au format JSON — voir la description du module.
    lines_json: String,
    /// Remise en dix-millièmes (`1000` = 10 %). Exclusif avec `discount_amount_cents`.
    discount_percent_bps: Option<u32>,
    /// Remise en montant fixe, en centimes. Exclusif avec `discount_percent_bps`.
    discount_amount_cents: Option<i64>,
    terms: Option<String>,
    valid_until: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ReviseQuoteArgs {
    /// Identifiant racine du devis à réviser (`root_id`).
    root_id: String,
    lines_json: String,
    discount_percent_bps: Option<u32>,
    discount_amount_cents: Option<i64>,
    terms: Option<String>,
    valid_until: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct QuoteIdArgs {
    id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct AcceptQuoteArgs {
    id: String,
    started_on: String,
}

#[tool_router(router = quotes_router, vis = "pub(crate)")]
impl FreeflowServer {
    /// Crée un devis (version 1).
    #[tool(
        name = "quote.create",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn quote_create(&self, Parameters(args): Parameters<CreateQuoteArgs>) -> CallToolResult {
        let client_id: ClientId = ok_or_return!("client_id", args.client_id.parse());
        let opportunity_id: Option<OpportunityId> = match &args.opportunity_id {
            Some(s) => Some(ok_or_return!("opportunity_id", s.parse())),
            None => None,
        };
        let lines: Vec<QuoteLine> =
            ok_or_return!("lines_json", serde_json::from_str(&args.lines_json));
        let discount = ok_or_return!(
            "discount",
            resolve_discount(args.discount_percent_bps, args.discount_amount_cents)
        );
        let valid_until = ok_or_return!(
            "valid_until",
            freeflow_core::domain::parse_date(&args.valid_until)
        );
        let cmd = quotes::CreateQuote {
            client_id,
            opportunity_id,
            lines,
            discount,
            terms: args.terms,
            valid_until,
        };
        let mut store = self.store.lock().await;
        match Executor::new(&mut store).execute(&cmd, &self.ctx(false)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Crée une nouvelle version d'un devis existant.
    #[tool(
        name = "quote.revise",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn quote_revise(&self, Parameters(args): Parameters<ReviseQuoteArgs>) -> CallToolResult {
        let root_id: QuoteId = ok_or_return!("root_id", args.root_id.parse());
        let lines: Vec<QuoteLine> =
            ok_or_return!("lines_json", serde_json::from_str(&args.lines_json));
        let discount = ok_or_return!(
            "discount",
            resolve_discount(args.discount_percent_bps, args.discount_amount_cents)
        );
        let valid_until = ok_or_return!(
            "valid_until",
            freeflow_core::domain::parse_date(&args.valid_until)
        );
        let cmd = quotes::ReviseQuote {
            root_id,
            lines,
            discount,
            terms: args.terms,
            valid_until,
        };
        let mut store = self.store.lock().await;
        match Executor::new(&mut store).execute(&cmd, &self.ctx(false)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Marque un devis comme envoyé.
    #[tool(
        name = "quote.send",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn quote_send(&self, Parameters(args): Parameters<QuoteIdArgs>) -> CallToolResult {
        let quote_id: QuoteId = ok_or_return!("id", args.id.parse());
        let mut store = self.store.lock().await;
        match Executor::new(&mut store).execute(&quotes::SendQuote { quote_id }, &self.ctx(false)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Décline un devis envoyé.
    #[tool(
        name = "quote.decline",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn quote_decline(&self, Parameters(args): Parameters<QuoteIdArgs>) -> CallToolResult {
        let quote_id: QuoteId = ok_or_return!("id", args.id.parse());
        let mut store = self.store.lock().await;
        match Executor::new(&mut store)
            .execute(&quotes::DeclineQuote { quote_id }, &self.ctx(false))
        {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Accepte un devis envoyé : crée la mission et l'échéancier de facturation correspondants.
    #[tool(
        name = "quote.accept",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn quote_accept(&self, Parameters(args): Parameters<AcceptQuoteArgs>) -> CallToolResult {
        let quote_id: QuoteId = ok_or_return!("id", args.id.parse());
        let started_on = ok_or_return!(
            "started_on",
            freeflow_core::domain::parse_date(&args.started_on)
        );
        let mut store = self.store.lock().await;
        match Executor::new(&mut store).execute(
            &quotes::AcceptQuote {
                quote_id,
                started_on,
            },
            &self.ctx(false),
        ) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }
}
