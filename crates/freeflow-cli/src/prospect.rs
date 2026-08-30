//! `freeflow prospect ...`

use clap::Subcommand;
use freeflow_core::app::{ExecutionContext, Executor};
use freeflow_core::domain::{
    ClientId, InteractionKind, OpportunityId, OpportunityStage, Probability,
};
use freeflow_core::prospection::{
    self, late_actions, pipeline_by_stage, weighted_pipeline, without_next_action,
};
use freeflow_core::store::Store;
use time::Date;

use crate::error::CliError;
use crate::output::{format_outcome, format_value};
use crate::parsers::{parse_date, parse_loss_reason, parse_money, parse_probability};

#[derive(Debug, Subcommand)]
pub enum ProspectCommand {
    /// Crée une nouvelle opportunité, en étape Qualification.
    Create {
        #[arg(long, value_parser = clap::value_parser!(ClientId))]
        client: ClientId,
        #[arg(long)]
        name: String,
        #[arg(long, value_parser = parse_money)]
        amount: freeflow_core::domain::Money,
        #[arg(long, value_parser = parse_probability)]
        probability: Probability,
        #[arg(long, value_parser = parse_date)]
        next_action: Date,
        #[arg(long)]
        source: Option<String>,
    },
    /// Fait avancer une opportunité vers une autre étape ouverte.
    Advance {
        #[arg(long, value_parser = clap::value_parser!(OpportunityId))]
        id: OpportunityId,
        #[arg(long, value_parser = clap::value_parser!(OpportunityStage))]
        to: OpportunityStage,
        #[arg(long, value_parser = parse_date)]
        next_action: Date,
    },
    /// Gagne une opportunité : crée la mission forfait correspondante.
    Win {
        #[arg(long, value_parser = clap::value_parser!(OpportunityId))]
        id: OpportunityId,
        #[arg(long, value_parser = parse_date)]
        started_on: Date,
    },
    /// Perd une opportunité, avec un motif structuré.
    Lose {
        #[arg(long, value_parser = clap::value_parser!(OpportunityId))]
        id: OpportunityId,
        #[arg(long, value_parser = parse_loss_reason)]
        reason: freeflow_core::domain::LossReason,
    },
    /// Journalise une interaction (appel, email, réunion, note).
    LogInteraction {
        #[arg(long, value_parser = clap::value_parser!(OpportunityId))]
        id: OpportunityId,
        #[arg(long, value_parser = clap::value_parser!(InteractionKind))]
        kind: InteractionKind,
        #[arg(long)]
        note: String,
    },
    /// Opportunités ouvertes dont la prochaine action est en retard.
    Late {
        #[arg(long, value_parser = parse_date)]
        today: Date,
    },
    /// Opportunités ouvertes sans prochaine action (filet de sécurité).
    Orphans,
    /// Pipeline pondéré et répartition par étape.
    Pipeline,
}

pub fn run(
    cmd: ProspectCommand,
    store: &mut Store,
    ctx: &ExecutionContext,
    json: bool,
) -> Result<String, CliError> {
    let output = match cmd {
        ProspectCommand::Create {
            client,
            name,
            amount,
            probability,
            next_action,
            source,
        } => {
            let command = prospection::CreateOpportunity {
                client_id: client,
                name,
                amount,
                probability,
                next_action_at: next_action,
                source,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        ProspectCommand::Advance {
            id,
            to,
            next_action,
        } => {
            let command = prospection::AdvanceOpportunity {
                opportunity_id: id,
                to,
                next_action_at: next_action,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        ProspectCommand::Win { id, started_on } => {
            let command = prospection::WinOpportunity {
                opportunity_id: id,
                started_on,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        ProspectCommand::Lose { id, reason } => {
            let command = prospection::LoseOpportunity {
                opportunity_id: id,
                reason,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        ProspectCommand::LogInteraction { id, kind, note } => {
            let command = prospection::LogInteraction {
                opportunity_id: id,
                kind,
                note,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        ProspectCommand::Late { today } => {
            let opportunities = late_actions(store.connection(), today)?;
            format_value(&opportunities, json)
        }
        ProspectCommand::Orphans => {
            let opportunities = without_next_action(store.connection())?;
            format_value(&opportunities, json)
        }
        ProspectCommand::Pipeline => {
            let weighted = weighted_pipeline(store.connection())?;
            let by_stage = pipeline_by_stage(store.connection())?;
            let payload = serde_json::json!({
                "weighted_total_cents": weighted.cents(),
                "by_stage": by_stage.iter().map(|s| serde_json::json!({
                    "stage": s.stage.as_str(),
                    "count": s.count,
                    "weighted_amount_cents": s.weighted_amount.cents(),
                })).collect::<Vec<_>>(),
            });
            format_value(&payload, json)
        }
    };
    Ok(output)
}
