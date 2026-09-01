//! `freeflow quote ...`
//!
//! Les lignes (`QuoteLine`, polymorphes régie/forfait/récurrent) sont passées en JSON via
//! `--lines` : une mini-syntaxe de flags pour un tableau d'objets hétérogènes serait plus
//! complexe à utiliser, en CLI comme pour un agent, qu'un objet JSON qu'il sait déjà produire.
//! Exemple : `--lines '[{"description":"Acompte","kind":{"Forfait":{"amount":1350000}},"vat_rate":"Standard"}]'`
//!
//! Lot 21 : `send`/`decline`/`accept`/`revise` prennent une `RÉFÉRENCE` positionnelle (UUID,
//! préfixe d'UUID, ou nom du client porteur — voir `freeflow_core::reference::resolve_quote`) au
//! lieu d'un `--id`/`--root` UUID nu — même rupture de contrat assumée que le lot 16 sur
//! `prospect`/`mission`, pour la même raison de cohérence avec `list`/`show` sur le même écran.

use clap::{Args, Subcommand};
use freeflow_core::app::{ExecutionContext, Executor};
use freeflow_core::domain::{Discount, Money, Quote, QuoteLine};
use freeflow_core::quotes::{self, list_quotes, priced_lines, quote_by_id, quote_references};
use freeflow_core::store::Store;
use serde::Serialize;
use time::Date;

use crate::error::CliError;
use crate::output::{format_outcome, format_value};
use crate::parsers::{parse_date, parse_money};
use crate::refs;

#[derive(Debug, Args)]
pub struct DiscountArgs {
    /// Remise en pourcentage, en dix-millièmes (`1000` = 10 %). Exclusif avec `--discount-amount`.
    #[arg(long)]
    discount_percent: Option<u32>,
    /// Remise en montant fixe. Exclusif avec `--discount-percent`.
    #[arg(long, value_parser = parse_money)]
    discount_amount: Option<Money>,
}

impl DiscountArgs {
    fn resolve(&self) -> Result<Option<Discount>, CliError> {
        match (self.discount_percent, self.discount_amount) {
            (Some(_), Some(_)) => Err(CliError::Domain(
                "--discount-percent et --discount-amount sont exclusifs".to_string(),
            )),
            (Some(bps), None) => Ok(Some(Discount::Percentage(bps))),
            (None, Some(amount)) => Ok(Some(Discount::FixedAmount(amount))),
            (None, None) => Ok(None),
        }
    }
}

fn parse_lines(json: &str) -> Result<Vec<QuoteLine>, CliError> {
    serde_json::from_str(json).map_err(|e| CliError::InvalidLinesJson(e.to_string()))
}

#[derive(Debug, Subcommand)]
pub enum QuoteCommand {
    /// Crée un devis (version 1).
    Create {
        /// Client porteur (référence : UUID, préfixe, ou nom).
        #[arg(long, value_name = "RÉFÉRENCE")]
        client: String,
        /// Opportunité liée (référence : UUID, préfixe, ou nom).
        #[arg(long, value_name = "RÉFÉRENCE")]
        opportunity: Option<String>,
        /// Lignes au format JSON — voir l'aide du module pour un exemple.
        #[arg(long)]
        lines: String,
        #[command(flatten)]
        discount: DiscountArgs,
        #[arg(long)]
        terms: Option<String>,
        #[arg(long, value_parser = parse_date)]
        valid_until: Date,
    },
    /// Liste les devis, toutes versions confondues, les plus récents d'abord.
    List,
    /// Affiche un devis : contenu, montants, lignée (mission issue, nombre de versions).
    Show {
        #[arg(value_name = "RÉFÉRENCE")]
        reference: String,
    },
    /// Crée une nouvelle version d'un devis existant (la référence désigne n'importe quelle
    /// version de la lignée : la révision part toujours de sa racine).
    Revise {
        #[arg(value_name = "RÉFÉRENCE")]
        reference: String,
        #[arg(long)]
        lines: String,
        #[command(flatten)]
        discount: DiscountArgs,
        #[arg(long)]
        terms: Option<String>,
        #[arg(long, value_parser = parse_date)]
        valid_until: Date,
    },
    /// Marque un devis comme envoyé.
    Send {
        #[arg(value_name = "RÉFÉRENCE")]
        reference: String,
    },
    /// Décline un devis envoyé.
    Decline {
        #[arg(value_name = "RÉFÉRENCE")]
        reference: String,
    },
    /// Accepte un devis envoyé : crée la mission et l'échéancier correspondants.
    Accept {
        #[arg(value_name = "RÉFÉRENCE")]
        reference: String,
        #[arg(long, value_parser = parse_date)]
        started_on: Date,
    },
}

pub fn run(
    cmd: QuoteCommand,
    store: &mut Store,
    ctx: &ExecutionContext,
    json: bool,
) -> Result<String, CliError> {
    let output = match cmd {
        QuoteCommand::Create {
            client,
            opportunity,
            lines,
            discount,
            terms,
            valid_until,
        } => {
            let client_id = refs::resolve_client(store, &client)?;
            let opportunity_id = opportunity
                .map(|reference| refs::resolve_opportunity(store, &reference))
                .transpose()?;
            let lines = parse_lines(&lines)?;
            let discount = discount.resolve()?;
            let command = quotes::CreateQuote {
                client_id,
                opportunity_id,
                lines,
                discount,
                terms,
                valid_until,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        QuoteCommand::List => {
            let all = list_quotes(store.connection())?;
            if json {
                format_value(&all, json)
            } else {
                quote_table(store, &all)?
            }
        }
        QuoteCommand::Show { reference } => {
            let id = refs::resolve_quote(store, &reference)?;
            let quote = quote_or_not_found(store, id)?;
            let references = quote_references(store.connection(), id)?;
            let total_net_ht = net_total(&quote);
            #[derive(Debug, Serialize)]
            struct QuoteView {
                #[serde(flatten)]
                quote: Quote,
                total_net_ht: Money,
                references: quotes::QuoteReferences,
            }
            format_value(
                &QuoteView {
                    quote,
                    total_net_ht,
                    references,
                },
                json,
            )
        }
        QuoteCommand::Revise {
            reference,
            lines,
            discount,
            terms,
            valid_until,
        } => {
            let id = refs::resolve_quote(store, &reference)?;
            // N'importe quelle version de la lignée peut servir de référence : la révision
            // repart toujours de la racine partagée.
            let root_id = quote_or_not_found(store, id)?.root_id;
            let lines = parse_lines(&lines)?;
            let discount = discount.resolve()?;
            let command = quotes::ReviseQuote {
                root_id,
                lines,
                discount,
                terms,
                valid_until,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        QuoteCommand::Send { reference } => {
            let quote_id = refs::resolve_quote(store, &reference)?;
            let outcome = Executor::new(store).execute(&quotes::SendQuote { quote_id }, ctx)?;
            format_outcome(&outcome, json)
        }
        QuoteCommand::Decline { reference } => {
            let quote_id = refs::resolve_quote(store, &reference)?;
            let outcome = Executor::new(store).execute(&quotes::DeclineQuote { quote_id }, ctx)?;
            format_outcome(&outcome, json)
        }
        QuoteCommand::Accept {
            reference,
            started_on,
        } => {
            let quote_id = refs::resolve_quote(store, &reference)?;
            let outcome = Executor::new(store).execute(
                &quotes::AcceptQuote {
                    quote_id,
                    started_on,
                },
                ctx,
            )?;
            format_outcome(&outcome, json)
        }
    };
    Ok(output)
}

fn quote_or_not_found(
    store: &Store,
    id: freeflow_core::domain::QuoteId,
) -> Result<Quote, CliError> {
    quote_by_id(store.connection(), id)?
        .ok_or_else(|| CliError::Domain(format!("devis introuvable : {id}")))
}

/// Total HT net de remise — recalculé par `priced_lines`, la seule source de vérité de
/// l'arithmétique de devis (jamais réimplémenté par façade).
fn net_total(quote: &Quote) -> Money {
    priced_lines(&quote.lines, quote.discount)
        .iter()
        .map(|(gross, discount)| *gross - *discount)
        .sum()
}

fn quote_table(store: &Store, all: &[Quote]) -> Result<String, CliError> {
    let clients = freeflow_core::clients::list_clients(store.connection())?;
    let client_name = |id: freeflow_core::domain::ClientId| {
        clients
            .iter()
            .find(|c| c.id == id)
            .map_or_else(|| "?".to_string(), |c| c.name.clone())
    };
    let rows = all
        .iter()
        .map(|q| {
            vec![
                q.id.to_string(),
                client_name(q.client_id),
                format!("v{}", q.version),
                q.status.as_str().to_string(),
                net_total(q).to_string(),
                freeflow_core::domain::format_date(q.valid_until),
            ]
        })
        .collect::<Vec<_>>();
    Ok(crate::table::render(
        &[
            "id",
            "client",
            "version",
            "statut",
            "total ht",
            "valide jusqu'au",
        ],
        &rows,
    ))
}
