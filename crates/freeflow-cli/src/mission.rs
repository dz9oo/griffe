//! `freeflow mission ...`

use clap::{Args, Subcommand};
use freeflow_core::app::{ExecutionContext, Executor};
use freeflow_core::domain::{
    ClientId, Milestone, MissionId, MissionKind, Money, QuoteId, TimeCategory,
};
use freeflow_core::missions::{self, effective_daily_rate, monthly_capacity};
use freeflow_core::store::Store;
use time::Date;

use crate::error::CliError;
use crate::output::{format_outcome, format_value};
use crate::parsers::{parse_date, parse_money};

#[derive(Debug, Clone, clap::ValueEnum)]
pub enum MissionKindArg {
    Regie,
    Forfait,
    Recurrent,
}

#[derive(Debug, Args)]
pub struct CreateArgs {
    #[arg(long, value_parser = clap::value_parser!(ClientId))]
    client: ClientId,
    #[arg(long, value_parser = clap::value_parser!(QuoteId))]
    quote: Option<QuoteId>,
    #[arg(long)]
    name: String,
    #[arg(long, value_enum)]
    kind: MissionKindArg,
    /// TJM, obligatoire pour `--kind regie`.
    #[arg(long, value_parser = parse_money)]
    daily_rate: Option<Money>,
    /// Budget total, obligatoire pour `--kind forfait`.
    #[arg(long, value_parser = parse_money)]
    budget: Option<Money>,
    /// Montant mensuel, obligatoire pour `--kind recurrent`.
    #[arg(long, value_parser = parse_money)]
    monthly_amount: Option<Money>,
    #[arg(long, value_parser = parse_date)]
    started_on: Date,
}

fn build_mission_kind(args: &CreateArgs) -> Result<MissionKind, CliError> {
    match args.kind {
        MissionKindArg::Regie => {
            let daily_rate = args.daily_rate.ok_or_else(|| {
                CliError::Domain("--daily-rate est requis pour --kind regie".to_string())
            })?;
            Ok(MissionKind::Regie { daily_rate })
        }
        MissionKindArg::Forfait => {
            let budget = args.budget.ok_or_else(|| {
                CliError::Domain("--budget est requis pour --kind forfait".to_string())
            })?;
            Ok(MissionKind::Forfait { budget })
        }
        MissionKindArg::Recurrent => {
            let monthly_amount = args.monthly_amount.ok_or_else(|| {
                CliError::Domain("--monthly-amount est requis pour --kind recurrent".to_string())
            })?;
            Ok(MissionKind::Recurrent { monthly_amount })
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum MissionCommand {
    /// Crée une mission directement (sans passer par le gain d'une opportunité).
    Create(Box<CreateArgs>),
    /// Saisit du temps sur une mission.
    LogTime {
        #[arg(long, value_parser = clap::value_parser!(MissionId))]
        mission: MissionId,
        #[arg(long, value_parser = parse_date)]
        worked_on: Date,
        #[arg(long)]
        days: f64,
        #[arg(long, value_parser = clap::value_parser!(TimeCategory))]
        category: TimeCategory,
        #[arg(long)]
        note: Option<String>,
    },
    /// TJM effectif d'une mission (CA ÷ jours facturables consommés).
    Rate {
        #[arg(long, value_parser = clap::value_parser!(MissionId))]
        id: MissionId,
    },
    /// Capacité vendue pour un mois donné, tous clients confondus.
    Capacity {
        /// Année et mois, ex. `2026-09`.
        #[arg(long)]
        month: String,
    },
}

fn parse_month(s: &str) -> Result<freeflow_core::domain::Month, CliError> {
    let (year_str, month_str) = s
        .split_once('-')
        .ok_or_else(|| CliError::Domain(format!("mois invalide : {s} (attendu AAAA-MM)")))?;
    let year: i32 = year_str
        .parse()
        .map_err(|_| CliError::Domain(format!("mois invalide : {s}")))?;
    let month: u8 = month_str
        .parse()
        .map_err(|_| CliError::Domain(format!("mois invalide : {s}")))?;
    freeflow_core::domain::Month::new(year, month).map_err(|e| CliError::Domain(e.to_string()))
}

pub fn run(
    cmd: MissionCommand,
    store: &mut Store,
    ctx: &ExecutionContext,
    json: bool,
) -> Result<String, CliError> {
    let output = match cmd {
        MissionCommand::Create(args) => {
            let kind = build_mission_kind(&args)?;
            let command = missions::CreateMission {
                client_id: args.client,
                quote_id: args.quote,
                name: args.name.clone(),
                kind,
                milestones: Vec::<Milestone>::new(),
                started_on: args.started_on,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        MissionCommand::LogTime {
            mission,
            worked_on,
            days,
            category,
            note,
        } => {
            let command = missions::LogTime {
                mission_id: mission,
                worked_on,
                days,
                category,
                note,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        MissionCommand::Rate { id } => {
            let rate = effective_daily_rate(store.connection(), id)?;
            format_value(&rate.map(freeflow_core::domain::Money::cents), json)
        }
        MissionCommand::Capacity { month } => {
            let month = parse_month(&month)?;
            let capacity = monthly_capacity(store.connection(), month)?;
            let payload = serde_json::json!({
                "month": month.to_string(),
                "available_business_days": capacity.available_business_days,
                "billable_days": capacity.billable_days,
                "utilization_percent": capacity.utilization_percent(),
            });
            format_value(&payload, json)
        }
    };
    Ok(output)
}
