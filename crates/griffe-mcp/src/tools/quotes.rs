//! Outils `quote.*` — miroir de `freeflow quote ...` (CLI, lot 7).
//!
//! Les lignes (`QuoteLine`, polymorphes régie/forfait/récurrent) sont passées en JSON via
//! `lines_json`, comme en CLI : une mini-syntaxe dédiée serait plus complexe à produire, pour
//! un agent, qu'un objet JSON qu'il sait déjà écrire.
//! Exemple : `[{"description":"Acompte","kind":{"Forfait":{"amount":1350000}},"vat_rate":"Standard"}]`
//!
//! Lot 21 : `list`/`show` (un devis créé était jusqu'ici illisible), adressage par référence
//! (`{"quote": "acme"}` — UUID, préfixe, ou nom du client porteur) au lieu d'UUID nus, et
//! remboursement de la dette `dry_run` des cinq outils préexistants qui appelaient encore
//! `self.ctx(false)` en dur.

use griffe_core::app::Executor;
use griffe_core::domain::{Discount, Money, QuoteLine};
use griffe_core::quotes::{self, list_quotes, priced_lines, quote_by_id, quote_references};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::server::FreeflowServer;
use crate::support::{err_text, ok_json, ok_or_return, outcome_json, resolve_quote};

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

/// Total HT net de remise — recalculé par `priced_lines`, jamais réimplémenté par façade.
fn net_total(quote: &griffe_core::domain::Quote) -> Money {
    priced_lines(&quote.lines, quote.discount)
        .iter()
        .map(|(gross, discount)| *gross - *discount)
        .sum()
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct CreateQuoteArgs {
    /// Référence du client : UUID, préfixe d'UUID, ou nom.
    client: String,
    /// Référence de l'opportunité liée : UUID, préfixe d'UUID, ou nom.
    opportunity: Option<String>,
    /// Lignes du devis, au format JSON — voir la description du module.
    lines_json: String,
    /// Remise en dix-millièmes (`1000` = 10 %). Exclusif avec `discount_amount_cents`.
    discount_percent_bps: Option<u32>,
    /// Remise en montant fixe, en centimes. Exclusif avec `discount_percent_bps`.
    discount_amount_cents: Option<i64>,
    terms: Option<String>,
    valid_until: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ReviseQuoteArgs {
    /// Référence de n'importe quelle version de la lignée à réviser (la révision repart
    /// toujours de sa racine) : UUID, préfixe d'UUID, ou nom du client porteur.
    quote: String,
    lines_json: String,
    discount_percent_bps: Option<u32>,
    discount_amount_cents: Option<i64>,
    terms: Option<String>,
    valid_until: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct QuoteRefArgs {
    /// Référence du devis : UUID, préfixe d'UUID, ou nom du client porteur.
    quote: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct QuoteRefMutationArgs {
    quote: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct AcceptQuoteArgs {
    quote: String,
    started_on: String,
    #[serde(default)]
    dry_run: bool,
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
        let mut store = self.store.lock().await;
        let client_id = ok_or_return!(
            "client",
            crate::support::resolve_client(&store, &args.client)
        );
        let opportunity_id = match &args.opportunity {
            Some(reference) => Some(ok_or_return!(
                "opportunity",
                crate::support::resolve_opportunity(&store, reference)
            )),
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
            griffe_core::domain::parse_date(&args.valid_until)
        );
        let cmd = quotes::CreateQuote {
            client_id,
            opportunity_id,
            lines,
            discount,
            terms: args.terms,
            valid_until,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Liste les devis, toutes versions confondues, les plus récents d'abord.
    #[tool(
        name = "quote.list",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn quote_list(&self) -> CallToolResult {
        let store = self.store.lock().await;
        match list_quotes(store.connection()) {
            Ok(all) => ok_json(all),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Affiche un devis : contenu, total HT net de remise, lignée (mission issue de son
    /// acceptation, nombre de versions).
    #[tool(
        name = "quote.show",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn quote_show(&self, Parameters(args): Parameters<QuoteRefArgs>) -> CallToolResult {
        let store = self.store.lock().await;
        let id = ok_or_return!("quote", resolve_quote(&store, &args.quote));
        let quote = match quote_by_id(store.connection(), id) {
            Ok(Some(q)) => q,
            Ok(None) => return err_text(format!("devis introuvable : {id}")),
            Err(e) => return err_text(e.to_string()),
        };
        let references = match quote_references(store.connection(), id) {
            Ok(r) => r,
            Err(e) => return err_text(e.to_string()),
        };
        let total = net_total(&quote);
        ok_json(json!({
            "quote": quote,
            "total_net_ht": total,
            "references": references,
        }))
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
        let mut store = self.store.lock().await;
        let id = ok_or_return!("quote", resolve_quote(&store, &args.quote));
        let root_id = match quote_by_id(store.connection(), id) {
            Ok(Some(q)) => q.root_id,
            Ok(None) => return err_text(format!("devis introuvable : {id}")),
            Err(e) => return err_text(e.to_string()),
        };
        let lines: Vec<QuoteLine> =
            ok_or_return!("lines_json", serde_json::from_str(&args.lines_json));
        let discount = ok_or_return!(
            "discount",
            resolve_discount(args.discount_percent_bps, args.discount_amount_cents)
        );
        let valid_until = ok_or_return!(
            "valid_until",
            griffe_core::domain::parse_date(&args.valid_until)
        );
        let cmd = quotes::ReviseQuote {
            root_id,
            lines,
            discount,
            terms: args.terms,
            valid_until,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
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
    async fn quote_send(
        &self,
        Parameters(args): Parameters<QuoteRefMutationArgs>,
    ) -> CallToolResult {
        let mut store = self.store.lock().await;
        let quote_id = ok_or_return!("quote", resolve_quote(&store, &args.quote));
        match Executor::new(&mut store)
            .execute(&quotes::SendQuote { quote_id }, &self.ctx(args.dry_run))
        {
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
    async fn quote_decline(
        &self,
        Parameters(args): Parameters<QuoteRefMutationArgs>,
    ) -> CallToolResult {
        let mut store = self.store.lock().await;
        let quote_id = ok_or_return!("quote", resolve_quote(&store, &args.quote));
        match Executor::new(&mut store)
            .execute(&quotes::DeclineQuote { quote_id }, &self.ctx(args.dry_run))
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
        let mut store = self.store.lock().await;
        let quote_id = ok_or_return!("quote", resolve_quote(&store, &args.quote));
        let started_on = ok_or_return!(
            "started_on",
            griffe_core::domain::parse_date(&args.started_on)
        );
        match Executor::new(&mut store).execute(
            &quotes::AcceptQuote {
                quote_id,
                started_on,
            },
            &self.ctx(args.dry_run),
        ) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }
}
