//! `freeflow mission ...` — les références (`<RÉFÉRENCE>`) acceptent un UUID complet, un préfixe
//! d'UUID d'au moins 4 caractères hexadécimaux, ou un nom de mission (insensible à la casse et
//! aux accents, exact puis par préfixe) : voir `freeflow_core::reference`.

use clap::{Args, Subcommand};
use freeflow_core::app::{ExecutionContext, Executor};
use freeflow_core::domain::{Milestone, MissionKind, Money, QuoteId, TimeCategory, TimeEntryId};
use freeflow_core::missions::{
    self, MissionFilter, effective_daily_rate, list_missions_with, mission_by_id,
    mission_references, monthly_capacity, time_entry_by_id,
};
use freeflow_core::store::Store;
use time::Date;

use crate::error::CliError;
use crate::output::{HumanRender, format_json, format_outcome, format_value, key_values, or_dash};

impl HumanRender for freeflow_core::domain::Mission {
    fn render_human(&self) -> String {
        let kind = match self.kind {
            MissionKind::Regie { daily_rate } => format!("régie, {daily_rate} / jour"),
            MissionKind::Forfait { budget } => format!("forfait, {budget}"),
            MissionKind::Recurrent { monthly_amount } => {
                format!("récurrent, {monthly_amount} / mois")
            }
        };
        let milestones = if self.milestones.is_empty() {
            "—".to_string()
        } else {
            self.milestones
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(" ; ")
        };
        key_values(&[
            ("Mission", self.name.clone()),
            ("id", self.id.to_string()),
            ("client", self.client_id.to_string()),
            ("type", kind),
            ("jalons", milestones),
            (
                "démarrée",
                freeflow_core::domain::format_date(self.started_on),
            ),
            (
                "terminée",
                or_dash(self.ended_on.map(freeflow_core::domain::format_date)),
            ),
            ("devis", or_dash(self.quote_id)),
            ("opportunité", or_dash(self.opportunity_id)),
            (
                "statut",
                if self.archived_at.is_some() {
                    "archivée"
                } else {
                    "active"
                }
                .to_string(),
            ),
            ("révision", self.revision.to_string()),
        ])
    }
}

impl HumanRender for freeflow_core::missions::MissionReferences {
    fn render_human(&self) -> String {
        format!(
            "{} facture(s), {} saisie(s) de temps{}",
            self.invoices,
            self.time_entries,
            if self.is_empty() {
                " — supprimable"
            } else {
                " — non supprimable (archivable)"
            }
        )
    }
}

/// L'échéancier de facturation en texte.
fn schedule_human(schedule: &missions::BillingSchedule) -> String {
    match schedule {
        missions::BillingSchedule::Regie { daily_rate } => {
            format!("régie : {daily_rate} par jour facturé")
        }
        missions::BillingSchedule::Recurrent { monthly_amount } => {
            format!("récurrent : {monthly_amount} par mois")
        }
        missions::BillingSchedule::Forfait { installments } => {
            let rows: Vec<Vec<String>> = installments
                .iter()
                .map(|(m, amount)| {
                    vec![
                        m.label.clone(),
                        format!("{},{:02} %", m.share_bps / 100, m.share_bps % 100),
                        or_dash(m.due_on.map(freeflow_core::domain::format_date)),
                        amount.to_string(),
                    ]
                })
                .collect();
            format!(
                "forfait, {} jalon(s)\n{}",
                installments.len(),
                crate::table::render(&["Jalon", "Part", "Échéance", "Montant"], &rows)
            )
        }
    }
}
use crate::parsers::{parse_date, parse_money};
use crate::refs;

#[derive(Debug, Clone, clap::ValueEnum)]
pub enum MissionKindArg {
    Regie,
    Forfait,
    Recurrent,
}

#[derive(Debug, Args)]
pub struct CreateArgs {
    #[arg(long, value_name = "RÉFÉRENCE")]
    client: String,
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
    /// Un jalon, répétable : `label:parts_bps[:AAAA-MM-JJ]` (ex. `Acompte:3000:2026-03-31`) —
    /// `parts_bps` en dix-millièmes (`10_000` = 100 %).
    #[arg(long = "milestone", value_parser = clap::value_parser!(Milestone))]
    milestones: Vec<Milestone>,
}

fn build_mission_kind(
    kind: &MissionKindArg,
    daily_rate: Option<Money>,
    budget: Option<Money>,
    monthly_amount: Option<Money>,
) -> Result<MissionKind, CliError> {
    match kind {
        MissionKindArg::Regie => {
            let daily_rate = daily_rate.ok_or_else(|| {
                CliError::Domain("--daily-rate est requis pour --kind regie".to_string())
            })?;
            Ok(MissionKind::Regie { daily_rate })
        }
        MissionKindArg::Forfait => {
            let budget = budget.ok_or_else(|| {
                CliError::Domain("--budget est requis pour --kind forfait".to_string())
            })?;
            Ok(MissionKind::Forfait { budget })
        }
        MissionKindArg::Recurrent => {
            let monthly_amount = monthly_amount.ok_or_else(|| {
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
    /// Affiche une mission.
    Show {
        #[arg(value_name = "RÉFÉRENCE")]
        reference: String,
    },
    /// Liste les missions en cours et non archivées (ajoutez `--ended`/`--archived` pour
    /// élargir).
    List {
        #[arg(long)]
        ended: bool,
        #[arg(long)]
        archived: bool,
    },
    /// Modifie une mission existante — seuls les champs fournis changent. Ne peut ni clore la
    /// mission (utilisez `close`) ni changer son devis d'origine.
    Edit {
        #[arg(value_name = "RÉFÉRENCE")]
        reference: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long, value_enum)]
        kind: Option<MissionKindArg>,
        #[arg(long, value_parser = parse_money)]
        daily_rate: Option<Money>,
        #[arg(long, value_parser = parse_money)]
        budget: Option<Money>,
        #[arg(long, value_parser = parse_money)]
        monthly_amount: Option<Money>,
        #[arg(long, value_parser = parse_date)]
        started_on: Option<Date>,
        /// Répété, remplace toute la collection de jalons actuelle.
        #[arg(long = "milestone", value_parser = clap::value_parser!(Milestone))]
        milestones: Vec<Milestone>,
        /// Vide la collection de jalons, sans en fournir une nouvelle.
        #[arg(long, conflicts_with = "milestones")]
        clear_milestones: bool,
    },
    /// Clôt une mission à une date donnée — fait métier daté et réversible (`reopen`), distinct
    /// de `archive` (classement sans date, pour une mission déjà facturée par exemple).
    Close {
        #[arg(value_name = "RÉFÉRENCE")]
        reference: String,
        #[arg(long, value_parser = parse_date)]
        ended_on: Date,
    },
    /// Rouvre une mission clôturée.
    Reopen {
        #[arg(value_name = "RÉFÉRENCE")]
        reference: String,
    },
    /// Retire une mission des listes actives sans la clore ni la supprimer.
    Archive {
        #[arg(value_name = "RÉFÉRENCE")]
        reference: String,
    },
    /// Réintègre une mission archivée dans les listes actives.
    Unarchive {
        #[arg(value_name = "RÉFÉRENCE")]
        reference: String,
    },
    /// Supprime une mission pour de bon — refusée si une facture ou une saisie de temps la
    /// référence (archivez-la, ou videz ses saisies de temps, dans ce cas).
    Rm {
        #[arg(value_name = "RÉFÉRENCE")]
        reference: String,
    },
    /// Ce qui empêche une mission d'être supprimée.
    References {
        #[arg(value_name = "RÉFÉRENCE")]
        reference: String,
    },
    /// Échéancier de facturation dérivé du type de mission et de ses jalons.
    Schedule {
        #[arg(value_name = "RÉFÉRENCE")]
        reference: String,
    },
    /// Saisit du temps sur une mission.
    LogTime {
        #[arg(value_name = "RÉFÉRENCE")]
        reference: String,
        #[arg(long, value_parser = parse_date)]
        worked_on: Date,
        #[arg(long)]
        days: f64,
        #[arg(long, value_parser = clap::value_parser!(TimeCategory))]
        category: TimeCategory,
        #[arg(long)]
        note: Option<String>,
    },
    /// Saisies de temps d'une mission : modifier ou supprimer une entrée déjà saisie (`log-time`
    /// reste le verbe de création).
    #[command(subcommand)]
    Time(TimeCommand),
    /// TJM effectif d'une mission (CA ÷ jours facturables consommés).
    Rate {
        #[arg(value_name = "RÉFÉRENCE")]
        reference: String,
    },
    /// Capacité vendue pour un mois donné, tous clients confondus.
    Capacity {
        /// Année et mois, ex. `2026-09`.
        #[arg(long)]
        month: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum TimeCommand {
    /// Liste les saisies de temps d'une mission.
    List {
        #[arg(value_name = "RÉFÉRENCE")]
        reference: String,
    },
    /// Modifie une saisie de temps existante.
    Edit {
        #[arg(value_parser = clap::value_parser!(TimeEntryId))]
        id: TimeEntryId,
        #[arg(long, value_parser = parse_date)]
        worked_on: Option<Date>,
        #[arg(long)]
        days: Option<f64>,
        #[arg(long, value_parser = clap::value_parser!(TimeCategory))]
        category: Option<TimeCategory>,
        #[arg(long, conflicts_with = "clear_note")]
        note: Option<String>,
        #[arg(long)]
        clear_note: bool,
    },
    /// Supprime une saisie de temps.
    Rm {
        #[arg(value_parser = clap::value_parser!(TimeEntryId))]
        id: TimeEntryId,
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

fn mission_or_not_found(
    store: &Store,
    reference: &str,
) -> Result<freeflow_core::domain::Mission, CliError> {
    let id = refs::resolve_mission(store, reference)?;
    mission_by_id(store.connection(), id)?
        .ok_or_else(|| CliError::Domain(format!("mission introuvable : {id}")))
}

fn time_entry_or_not_found(
    store: &Store,
    id: TimeEntryId,
) -> Result<freeflow_core::domain::TimeEntry, CliError> {
    time_entry_by_id(store.connection(), id)?
        .ok_or_else(|| CliError::Domain(format!("saisie de temps introuvable : {id}")))
}

fn mission_table(missions: &[freeflow_core::domain::Mission]) -> String {
    let rows = missions
        .iter()
        .map(|m| {
            let kind = match m.kind {
                MissionKind::Regie { .. } => "régie",
                MissionKind::Forfait { .. } => "forfait",
                MissionKind::Recurrent { .. } => "récurrent",
            };
            let status = match (m.ended_on, m.archived_at) {
                (Some(_), _) => "clôturée".to_string(),
                (None, Some(_)) => "archivée".to_string(),
                (None, None) => "active".to_string(),
            };
            vec![
                m.id.to_string(),
                m.name.clone(),
                kind.to_string(),
                freeflow_core::domain::format_date(m.started_on),
                status,
            ]
        })
        .collect::<Vec<_>>();
    crate::table::render(&["id", "nom", "type", "démarrée", "statut"], &rows)
}

fn time_entry_table(entries: &[freeflow_core::domain::TimeEntry]) -> String {
    let rows = entries
        .iter()
        .map(|e| {
            vec![
                e.id.to_string(),
                freeflow_core::domain::format_date(e.worked_on),
                e.days.to_string(),
                e.category.as_str().to_string(),
                e.note.clone().unwrap_or_else(|| "—".to_string()),
            ]
        })
        .collect::<Vec<_>>();
    crate::table::render(&["id", "date", "jours", "catégorie", "note"], &rows)
}

pub fn run(
    cmd: MissionCommand,
    store: &mut Store,
    ctx: &ExecutionContext,
    json: bool,
) -> Result<String, CliError> {
    let output = match cmd {
        MissionCommand::Create(args) => {
            let client_id = refs::resolve_client(store, &args.client)?;
            let kind = build_mission_kind(
                &args.kind,
                args.daily_rate,
                args.budget,
                args.monthly_amount,
            )?;
            let command = missions::CreateMission {
                client_id,
                quote_id: args.quote,
                name: args.name.clone(),
                kind,
                milestones: args.milestones.clone(),
                started_on: args.started_on,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        MissionCommand::Show { reference } => {
            let mission = mission_or_not_found(store, &reference)?;
            format_value(&mission, json)
        }
        MissionCommand::List { ended, archived } => {
            let filter = MissionFilter {
                include_ended: ended,
                include_archived: archived,
            };
            let missions = list_missions_with(store.connection(), filter)?;
            if json {
                format_json(&missions)
            } else {
                mission_table(&missions)
            }
        }
        MissionCommand::Edit {
            reference,
            name,
            kind,
            daily_rate,
            budget,
            monthly_amount,
            started_on,
            milestones,
            clear_milestones,
        } => {
            let current = mission_or_not_found(store, &reference)?;
            let new_kind = match kind {
                Some(kind) => build_mission_kind(&kind, daily_rate, budget, monthly_amount)?,
                None => current.kind.clone(),
            };
            let new_milestones = if clear_milestones {
                Vec::new()
            } else if milestones.is_empty() {
                current.milestones.clone()
            } else {
                milestones
            };
            let command = missions::UpdateMission {
                id: current.id,
                revision: current.revision,
                name: name.unwrap_or(current.name),
                kind: new_kind,
                milestones: new_milestones,
                started_on: started_on.unwrap_or(current.started_on),
                quote_id: current.quote_id,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        MissionCommand::Close {
            reference,
            ended_on,
        } => {
            let current = mission_or_not_found(store, &reference)?;
            let command = missions::CloseMission {
                id: current.id,
                revision: current.revision,
                ended_on,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        MissionCommand::Reopen { reference } => {
            let current = mission_or_not_found(store, &reference)?;
            let command = missions::ReopenMission {
                id: current.id,
                revision: current.revision,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        MissionCommand::Archive { reference } => {
            let current = mission_or_not_found(store, &reference)?;
            let command = missions::ArchiveMission {
                id: current.id,
                revision: current.revision,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        MissionCommand::Unarchive { reference } => {
            let current = mission_or_not_found(store, &reference)?;
            let command = missions::UnarchiveMission {
                id: current.id,
                revision: current.revision,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        MissionCommand::Rm { reference } => {
            let current = mission_or_not_found(store, &reference)?;
            let command = missions::DeleteMission {
                id: current.id,
                revision: current.revision,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        MissionCommand::References { reference } => {
            let current = mission_or_not_found(store, &reference)?;
            let refs = mission_references(store.connection(), current.id)?;
            format_value(&refs, json)
        }
        MissionCommand::Schedule { reference } => {
            let current = mission_or_not_found(store, &reference)?;
            let schedule = missions::billing_schedule(&current);
            if json {
                format_json(&schedule_json(&schedule))
            } else {
                schedule_human(&schedule)
            }
        }
        MissionCommand::LogTime {
            reference,
            worked_on,
            days,
            category,
            note,
        } => {
            let mission_id = refs::resolve_mission(store, &reference)?;
            let command = missions::LogTime {
                mission_id,
                worked_on,
                days,
                category,
                note,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        MissionCommand::Time(cmd) => run_time(cmd, store, ctx, json)?,
        MissionCommand::Rate { reference } => {
            let mission = mission_or_not_found(store, &reference)?;
            let rate = effective_daily_rate(store.connection(), mission.id)?;
            if json {
                format_json(&rate.map(freeflow_core::domain::Money::cents))
            } else {
                rate.map_or_else(
                    || "aucun temps saisi : TJM effectif indéterminé".to_string(),
                    |r| format!("TJM effectif : {r} par jour"),
                )
            }
        }
        MissionCommand::Capacity { month } => {
            let month = parse_month(&month)?;
            let capacity = monthly_capacity(store.connection(), month)?;
            if json {
                format_json(&serde_json::json!({
                    "month": month.to_string(),
                    "available_business_days": capacity.available_business_days,
                    "billable_days": capacity.billable_days,
                    "utilization_percent": capacity.utilization_percent(),
                }))
            } else {
                format!(
                    "{month} : {:.1} jour(s) facturé(s) sur {} jour(s) ouvré(s), soit {:.0} % \
                     d'occupation",
                    capacity.billable_days,
                    capacity.available_business_days,
                    capacity.utilization_percent()
                )
            }
        }
    };
    Ok(output)
}

fn schedule_json(schedule: &missions::BillingSchedule) -> serde_json::Value {
    match schedule {
        missions::BillingSchedule::Regie { daily_rate } => serde_json::json!({
            "kind": "regie",
            "daily_rate_cents": daily_rate.cents(),
        }),
        missions::BillingSchedule::Recurrent { monthly_amount } => serde_json::json!({
            "kind": "recurrent",
            "monthly_amount_cents": monthly_amount.cents(),
        }),
        missions::BillingSchedule::Forfait { installments } => serde_json::json!({
            "kind": "forfait",
            "installments": installments.iter().map(|(m, amount)| serde_json::json!({
                "label": m.label,
                "share_bps": m.share_bps,
                "due_on": m.due_on.map(freeflow_core::domain::format_date),
                "amount_cents": amount.cents(),
            })).collect::<Vec<_>>(),
        }),
    }
}

fn run_time(
    cmd: TimeCommand,
    store: &mut Store,
    ctx: &ExecutionContext,
    json: bool,
) -> Result<String, CliError> {
    let output = match cmd {
        TimeCommand::List { reference } => {
            let mission_id = refs::resolve_mission(store, &reference)?;
            let entries = missions::list_time_entries(store.connection(), mission_id)?;
            if json {
                format_json(&entries)
            } else {
                time_entry_table(&entries)
            }
        }
        TimeCommand::Edit {
            id,
            worked_on,
            days,
            category,
            note,
            clear_note,
        } => {
            let current = time_entry_or_not_found(store, id)?;
            let note = if clear_note {
                None
            } else {
                note.or(current.note)
            };
            let command = missions::UpdateTimeEntry {
                id,
                revision: current.revision,
                worked_on: worked_on.unwrap_or(current.worked_on),
                days: days.unwrap_or(current.days),
                category: category.unwrap_or(current.category),
                note,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        TimeCommand::Rm { id } => {
            let current = time_entry_or_not_found(store, id)?;
            let command = missions::DeleteTimeEntry {
                id,
                revision: current.revision,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
    };
    Ok(output)
}
