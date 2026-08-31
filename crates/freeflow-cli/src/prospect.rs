//! `freeflow prospect ...` — les références (`<RÉFÉRENCE>`) acceptent un UUID complet, un
//! préfixe d'UUID d'au moins 4 caractères hexadécimaux, ou un nom d'opportunité (insensible à la
//! casse et aux accents, exact puis par préfixe) : voir `freeflow_core::reference`.

use clap::Subcommand;
use freeflow_core::app::{ExecutionContext, Executor};
use freeflow_core::domain::{InteractionId, InteractionKind, OpportunityStage, Probability};
use freeflow_core::prospection::{
    self, OpportunityFilter, late_actions, list_opportunities_with, opportunity_by_id,
    opportunity_references, pipeline_by_stage, weighted_pipeline, without_next_action,
};
use freeflow_core::store::Store;
use time::{Date, OffsetDateTime};

use crate::error::CliError;
use crate::output::{format_outcome, format_value};
use crate::parsers::{parse_date, parse_loss_reason, parse_money, parse_probability};
use crate::refs;

#[derive(Debug, Subcommand)]
pub enum ProspectCommand {
    /// Crée une nouvelle opportunité, en étape Qualification.
    Create {
        #[arg(long, value_name = "RÉFÉRENCE")]
        client: String,
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
    /// Affiche une opportunité.
    Show {
        #[arg(value_name = "RÉFÉRENCE")]
        reference: String,
    },
    /// Liste les opportunités ouvertes et non archivées (ajoutez `--closed`/`--archived` pour
    /// élargir).
    List {
        #[arg(long)]
        closed: bool,
        #[arg(long)]
        archived: bool,
    },
    /// Modifie une opportunité ouverte — seuls les champs fournis changent. Ne peut pas changer
    /// l'étape (utilisez `advance`/`win`/`lose`) ni le motif de perte.
    Edit {
        #[arg(value_name = "RÉFÉRENCE")]
        reference: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long, value_parser = parse_money)]
        amount: Option<freeflow_core::domain::Money>,
        #[arg(long, value_parser = parse_probability)]
        probability: Option<Probability>,
        #[arg(long, value_parser = parse_date)]
        next_action: Option<Date>,
        #[arg(long, conflicts_with = "clear_source")]
        source: Option<String>,
        /// Efface la source, sans en fournir une nouvelle.
        #[arg(long)]
        clear_source: bool,
    },
    /// Fait avancer une opportunité vers une autre étape ouverte.
    Advance {
        #[arg(value_name = "RÉFÉRENCE")]
        reference: String,
        #[arg(long, value_parser = clap::value_parser!(OpportunityStage))]
        to: OpportunityStage,
        #[arg(long, value_parser = parse_date)]
        next_action: Date,
    },
    /// Gagne une opportunité : crée la mission forfait correspondante.
    Win {
        #[arg(value_name = "RÉFÉRENCE")]
        reference: String,
        #[arg(long, value_parser = parse_date)]
        started_on: Date,
    },
    /// Perd une opportunité, avec un motif structuré.
    Lose {
        #[arg(value_name = "RÉFÉRENCE")]
        reference: String,
        #[arg(long, value_parser = parse_loss_reason)]
        reason: freeflow_core::domain::LossReason,
    },
    /// Retire une opportunité des listes actives sans la supprimer — un axe distinct de
    /// gagnée/perdue : une opportunité archivée n'est ni l'une ni l'autre, juste sans objet.
    Archive {
        #[arg(value_name = "RÉFÉRENCE")]
        reference: String,
    },
    /// Réintègre une opportunité archivée dans les listes actives.
    Unarchive {
        #[arg(value_name = "RÉFÉRENCE")]
        reference: String,
    },
    /// Supprime une opportunité pour de bon — refusée si un devis la référence (archivez-la dans
    /// ce cas).
    Rm {
        #[arg(value_name = "RÉFÉRENCE")]
        reference: String,
    },
    /// Ce qui empêche une opportunité d'être supprimée.
    References {
        #[arg(value_name = "RÉFÉRENCE")]
        reference: String,
    },
    /// Journalise une interaction (appel, email, réunion, note).
    LogInteraction {
        #[arg(value_name = "RÉFÉRENCE")]
        reference: String,
        #[arg(long, value_parser = clap::value_parser!(InteractionKind))]
        kind: InteractionKind,
        #[arg(long)]
        note: String,
        /// Par défaut, l'instant présent — utile pour journaliser un échange survenu plus tôt.
        #[arg(long, value_parser = parse_date)]
        occurred_on: Option<Date>,
    },
    /// Interactions d'une opportunité : modifier ou supprimer une entrée déjà journalisée
    /// (`log-interaction` reste le verbe de création).
    #[command(subcommand)]
    Interaction(InteractionCommand),
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

#[derive(Debug, Subcommand)]
pub enum InteractionCommand {
    /// Liste les interactions d'une opportunité.
    List {
        #[arg(value_name = "RÉFÉRENCE")]
        reference: String,
    },
    /// Modifie une interaction existante.
    Edit {
        #[arg(value_parser = clap::value_parser!(InteractionId))]
        id: InteractionId,
        #[arg(long, value_parser = clap::value_parser!(InteractionKind))]
        kind: Option<InteractionKind>,
        #[arg(long)]
        note: Option<String>,
        #[arg(long, value_parser = parse_date)]
        occurred_on: Option<Date>,
    },
    /// Supprime une interaction.
    Rm {
        #[arg(value_parser = clap::value_parser!(InteractionId))]
        id: InteractionId,
    },
}

fn midnight_utc(date: Date) -> OffsetDateTime {
    date.with_hms(0, 0, 0)
        .expect("minuit est toujours une heure valide")
        .assume_utc()
}

fn opportunity_table(opportunities: &[freeflow_core::domain::Opportunity]) -> String {
    let rows = opportunities
        .iter()
        .map(|o| {
            vec![
                o.id.to_string(),
                o.name.clone(),
                o.stage.as_str().to_string(),
                o.amount.to_string(),
                format!("{}%", o.probability.percent()),
                o.next_action_at
                    .map_or_else(|| "—".to_string(), freeflow_core::domain::format_date),
                if o.archived_at.is_some() {
                    "archivée".to_string()
                } else {
                    "active".to_string()
                },
            ]
        })
        .collect::<Vec<_>>();
    crate::table::render(
        &[
            "id",
            "nom",
            "étape",
            "montant",
            "proba",
            "prochaine action",
            "statut",
        ],
        &rows,
    )
}

fn interaction_table(interactions: &[freeflow_core::domain::Interaction]) -> String {
    let rows = interactions
        .iter()
        .map(|i| {
            vec![
                i.id.to_string(),
                i.kind.as_str().to_string(),
                i.note.clone(),
                i.occurred_at.date().to_string(),
            ]
        })
        .collect::<Vec<_>>();
    crate::table::render(&["id", "type", "note", "date"], &rows)
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
            let client_id = refs::resolve_client(store, &client)?;
            let command = prospection::CreateOpportunity {
                client_id,
                name,
                amount,
                probability,
                next_action_at: next_action,
                source,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        ProspectCommand::Show { reference } => {
            let id = refs::resolve_opportunity(store, &reference)?;
            let opportunity = opportunity_by_id(store.connection(), id)?
                .ok_or_else(|| CliError::Domain(format!("opportunité introuvable : {id}")))?;
            format_value(&opportunity, json)
        }
        ProspectCommand::List { closed, archived } => {
            let filter = OpportunityFilter {
                include_closed: closed,
                include_archived: archived,
            };
            let opportunities = list_opportunities_with(store.connection(), filter)?;
            if json {
                format_value(&opportunities, json)
            } else {
                opportunity_table(&opportunities)
            }
        }
        ProspectCommand::Edit {
            reference,
            name,
            amount,
            probability,
            next_action,
            source,
            clear_source,
        } => {
            let id = refs::resolve_opportunity(store, &reference)?;
            let current = opportunity_by_id(store.connection(), id)?
                .ok_or_else(|| CliError::Domain(format!("opportunité introuvable : {id}")))?;
            let source = if clear_source {
                None
            } else {
                source.or(current.source)
            };
            let command = prospection::UpdateOpportunity {
                id,
                revision: current.revision,
                name: name.unwrap_or(current.name),
                amount: amount.unwrap_or(current.amount),
                probability: probability.unwrap_or(current.probability),
                next_action_at: next_action.or(current.next_action_at),
                source,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        ProspectCommand::Advance {
            reference,
            to,
            next_action,
        } => {
            let opportunity_id = refs::resolve_opportunity(store, &reference)?;
            let command = prospection::AdvanceOpportunity {
                opportunity_id,
                to,
                next_action_at: next_action,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        ProspectCommand::Win {
            reference,
            started_on,
        } => {
            let opportunity_id = refs::resolve_opportunity(store, &reference)?;
            let command = prospection::WinOpportunity {
                opportunity_id,
                started_on,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        ProspectCommand::Lose { reference, reason } => {
            let opportunity_id = refs::resolve_opportunity(store, &reference)?;
            let command = prospection::LoseOpportunity {
                opportunity_id,
                reason,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        ProspectCommand::Archive { reference } => {
            let id = refs::resolve_opportunity(store, &reference)?;
            let current = opportunity_by_id(store.connection(), id)?
                .ok_or_else(|| CliError::Domain(format!("opportunité introuvable : {id}")))?;
            let command = prospection::ArchiveOpportunity {
                id,
                revision: current.revision,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        ProspectCommand::Unarchive { reference } => {
            let id = refs::resolve_opportunity(store, &reference)?;
            let current = opportunity_by_id(store.connection(), id)?
                .ok_or_else(|| CliError::Domain(format!("opportunité introuvable : {id}")))?;
            let command = prospection::UnarchiveOpportunity {
                id,
                revision: current.revision,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        ProspectCommand::Rm { reference } => {
            let id = refs::resolve_opportunity(store, &reference)?;
            let current = opportunity_by_id(store.connection(), id)?
                .ok_or_else(|| CliError::Domain(format!("opportunité introuvable : {id}")))?;
            let command = prospection::DeleteOpportunity {
                id,
                revision: current.revision,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        ProspectCommand::References { reference } => {
            let id = refs::resolve_opportunity(store, &reference)?;
            let refs = opportunity_references(store.connection(), id)?;
            format_value(&refs, json)
        }
        ProspectCommand::LogInteraction {
            reference,
            kind,
            note,
            occurred_on,
        } => {
            let opportunity_id = refs::resolve_opportunity(store, &reference)?;
            let command = prospection::LogInteraction {
                opportunity_id,
                kind,
                note,
                occurred_at: occurred_on.map(midnight_utc),
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        ProspectCommand::Interaction(cmd) => run_interaction(cmd, store, ctx, json)?,
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

fn interaction_or_not_found(
    store: &Store,
    id: InteractionId,
) -> Result<freeflow_core::domain::Interaction, CliError> {
    prospection::interaction_by_id(store.connection(), id)?
        .ok_or_else(|| CliError::Domain(format!("interaction introuvable : {id}")))
}

fn run_interaction(
    cmd: InteractionCommand,
    store: &mut Store,
    ctx: &ExecutionContext,
    json: bool,
) -> Result<String, CliError> {
    let output = match cmd {
        InteractionCommand::List { reference } => {
            let opportunity_id = refs::resolve_opportunity(store, &reference)?;
            let interactions = prospection::list_interactions(store.connection(), opportunity_id)?;
            if json {
                format_value(&interactions, json)
            } else {
                interaction_table(&interactions)
            }
        }
        InteractionCommand::Edit {
            id,
            kind,
            note,
            occurred_on,
        } => {
            let current = interaction_or_not_found(store, id)?;
            let command = prospection::UpdateInteraction {
                id,
                revision: current.revision,
                kind: kind.unwrap_or(current.kind),
                note: note.unwrap_or(current.note),
                occurred_at: occurred_on.map_or(current.occurred_at, midnight_utc),
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        InteractionCommand::Rm { id } => {
            let current = interaction_or_not_found(store, id)?;
            let command = prospection::DeleteInteraction {
                id,
                revision: current.revision,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
    };
    Ok(output)
}
