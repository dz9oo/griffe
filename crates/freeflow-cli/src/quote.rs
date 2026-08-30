//! `freeflow quote ...`
//!
//! Les lignes (`QuoteLine`, polymorphes régie/forfait/récurrent) sont passées en JSON via
//! `--lines` : une mini-syntaxe de flags pour un tableau d'objets hétérogènes serait plus
//! complexe à utiliser, en CLI comme pour un agent, qu'un objet JSON qu'il sait déjà produire.
//! Exemple : `--lines '[{"description":"Acompte","kind":{"Forfait":{"amount":1350000}},"vat_rate":"Standard"}]'`

use clap::{Args, Subcommand};
use freeflow_core::app::{ExecutionContext, Executor};
use freeflow_core::domain::{ClientId, Discount, Money, OpportunityId, QuoteId, QuoteLine};
use freeflow_core::quotes;
use freeflow_core::store::Store;
use time::Date;

use crate::error::CliError;
use crate::output::format_outcome;
use crate::parsers::{parse_date, parse_money};

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
        #[arg(long, value_parser = clap::value_parser!(ClientId))]
        client: ClientId,
        #[arg(long, value_parser = clap::value_parser!(OpportunityId))]
        opportunity: Option<OpportunityId>,
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
    /// Crée une nouvelle version d'un devis existant.
    Revise {
        #[arg(long, value_parser = clap::value_parser!(QuoteId))]
        root: QuoteId,
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
        #[arg(long, value_parser = clap::value_parser!(QuoteId))]
        id: QuoteId,
    },
    /// Décline un devis envoyé.
    Decline {
        #[arg(long, value_parser = clap::value_parser!(QuoteId))]
        id: QuoteId,
    },
    /// Accepte un devis envoyé : crée la mission et l'échéancier correspondants.
    Accept {
        #[arg(long, value_parser = clap::value_parser!(QuoteId))]
        id: QuoteId,
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
            let lines = parse_lines(&lines)?;
            let discount = discount.resolve()?;
            let command = quotes::CreateQuote {
                client_id: client,
                opportunity_id: opportunity,
                lines,
                discount,
                terms,
                valid_until,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        QuoteCommand::Revise {
            root,
            lines,
            discount,
            terms,
            valid_until,
        } => {
            let lines = parse_lines(&lines)?;
            let discount = discount.resolve()?;
            let command = quotes::ReviseQuote {
                root_id: root,
                lines,
                discount,
                terms,
                valid_until,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        QuoteCommand::Send { id } => {
            let outcome = Executor::new(store).execute(&quotes::SendQuote { quote_id: id }, ctx)?;
            format_outcome(&outcome, json)
        }
        QuoteCommand::Decline { id } => {
            let outcome =
                Executor::new(store).execute(&quotes::DeclineQuote { quote_id: id }, ctx)?;
            format_outcome(&outcome, json)
        }
        QuoteCommand::Accept { id, started_on } => {
            let outcome = Executor::new(store).execute(
                &quotes::AcceptQuote {
                    quote_id: id,
                    started_on,
                },
                ctx,
            )?;
            format_outcome(&outcome, json)
        }
    };
    Ok(output)
}
