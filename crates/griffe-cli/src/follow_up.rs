//! `freeflow follow-up` — file de relances, brouillon `.eml`, jamais d'envoi.

use std::path::{Path, PathBuf};

use clap::Subcommand;
use griffe_core::app::{ExecutionContext, Executor};
use griffe_core::domain::{FollowUpSubject, SnoozePreset, format_date, snooze_date};
use griffe_core::follow_up::{
    CardStatus, FollowUpCard, MarkFollowUpSent, PrepareFollowUp, RetractLastFollowUp,
    SetFollowUpDate, SetFollowUpSender, SkipFollowUpStep, SnoozeFollowUp, card_for,
    follow_up_board, follow_up_queue,
};
use griffe_core::store::Store;
use time::Date;

use crate::error::CliError;
use crate::output::{
    HumanRender, format_outcome, format_outcome_as, format_value, key_values, or_dash,
};
use crate::parsers::parse_date;
use crate::refs;
use crate::table;

impl HumanRender for FollowUpCard {
    fn render_human(&self) -> String {
        let status = match self.status {
            CardStatus::Due => "à faire aujourd'hui",
            CardStatus::Overdue => "en retard",
            CardStatus::Drafted => "brouillon ouvert",
            CardStatus::Later => "plus tard",
            CardStatus::Exhausted => "cadence terminée",
            CardStatus::Blocked => "bloqué",
        };
        let mut pairs = vec![
            ("Dossier", self.title.clone()),
            ("client", self.party.clone()),
            (
                "famille",
                match self.kind {
                    griffe_core::domain::FollowUpKind::Prospect => "prospect".to_string(),
                    griffe_core::domain::FollowUpKind::Invoice => "impayé".to_string(),
                },
            ),
            (
                "étape",
                self.step_label.as_ref().map_or_else(
                    || "—".to_string(),
                    |l| format!("{l} ({}/{})", self.step_index + 1, self.step_count),
                ),
            ),
            ("échéance", or_dash(self.due_on.map(format_date))),
            ("statut", status.to_string()),
            ("montant", self.amount.to_string()),
            ("contact", or_dash(self.contact_email.as_deref())),
        ];
        if let Some(reason) = &self.block_reason {
            pairs.push(("à régler", reason.clone()));
        }
        key_values(&pairs)
    }
}

impl HumanRender for Vec<FollowUpCard> {
    fn render_human(&self) -> String {
        let rows: Vec<Vec<String>> = self
            .iter()
            .map(|c| {
                vec![
                    match c.kind {
                        griffe_core::domain::FollowUpKind::Prospect => "prospect".into(),
                        griffe_core::domain::FollowUpKind::Invoice => "impayé".into(),
                    },
                    c.title.clone(),
                    c.party.clone(),
                    c.step_label.clone().unwrap_or_else(|| "—".into()),
                    or_dash(c.due_on.map(format_date)),
                    match c.status {
                        CardStatus::Due => "aujourd'hui".into(),
                        CardStatus::Overdue => "retard".into(),
                        CardStatus::Drafted => "brouillon".into(),
                        CardStatus::Later => "plus tard".into(),
                        CardStatus::Exhausted => "finie".into(),
                        CardStatus::Blocked => "bloqué".into(),
                    },
                ]
            })
            .collect();
        table::render(
            &[
                "famille",
                "dossier",
                "client",
                "étape",
                "échéance",
                "statut",
            ],
            &rows,
        )
    }
}

#[derive(Debug, Subcommand)]
pub enum FollowUpCommand {
    /// La file du jour : en retard, dû aujourd'hui, brouillon ouvert.
    Queue {
        #[arg(long, value_parser = parse_date)]
        today: Option<Date>,
    },
    /// Vue d'ensemble : toutes les relances actives.
    Board {
        #[arg(long, value_parser = parse_date)]
        today: Option<Date>,
    },
    /// Fiche d'une relance (piste, historique, aperçu du mail).
    Show {
        #[arg(value_name = "RÉFÉRENCE")]
        reference: String,
        #[arg(long, value_parser = parse_date)]
        today: Option<Date>,
    },
    /// Email d'expédition des brouillons.
    From {
        #[arg(long)]
        email: String,
        #[arg(long)]
        name: Option<String>,
    },
    /// Prépare un brouillon `.eml` et l'ouvre dans le client mail (`xdg-open`).
    Draft {
        #[arg(value_name = "RÉFÉRENCE")]
        reference: String,
        #[arg(long, value_parser = parse_date)]
        today: Option<Date>,
        /// Sujet libre (sinon le modèle de cadence).
        #[arg(long)]
        subject: Option<String>,
        /// Corps libre (sinon le modèle de cadence).
        #[arg(long)]
        body: Option<String>,
        /// Corps lu depuis un fichier (UTF-8). Exclusif avec `--body`.
        #[arg(long, conflicts_with = "body")]
        body_file: Option<PathBuf>,
        /// Écrit le fichier sans l'ouvrir.
        #[arg(long)]
        no_open: bool,
    },
    /// Atteste que le mail est parti — avance la cadence.
    Sent {
        #[arg(value_name = "RÉFÉRENCE")]
        reference: String,
        #[arg(long, value_parser = parse_date)]
        today: Option<Date>,
        /// Rectifie le sujet au classement (sinon le dernier brouillon).
        #[arg(long)]
        subject: Option<String>,
        /// Rectifie le corps au classement (sinon le dernier brouillon).
        #[arg(long)]
        body: Option<String>,
        #[arg(long, conflicts_with = "body")]
        body_file: Option<PathBuf>,
    },
    /// Saute l'étape courante sans envoyer.
    Skip {
        #[arg(value_name = "RÉFÉRENCE")]
        reference: String,
        #[arg(long, value_parser = parse_date)]
        today: Option<Date>,
    },
    /// Reporte à une date, sans changer d'étape.
    Snooze {
        #[arg(value_name = "RÉFÉRENCE")]
        reference: String,
        #[arg(long, value_parser = parse_date)]
        until: Option<Date>,
        #[arg(long)]
        tomorrow: bool,
        #[arg(long)]
        next_week: bool,
        #[arg(long)]
        monday: bool,
        #[arg(long, value_parser = parse_date)]
        today: Option<Date>,
    },
    /// Pose la prochaine date à la main (même étape).
    Schedule {
        #[arg(value_name = "RÉFÉRENCE")]
        reference: String,
        #[arg(long, value_parser = parse_date, required = true)]
        on: Date,
        #[arg(long, value_parser = parse_date)]
        today: Option<Date>,
    },
    /// Annule le dernier geste (envoi, saut, report, brouillon).
    Retract {
        #[arg(value_name = "RÉFÉRENCE")]
        reference: String,
        #[arg(long, value_parser = parse_date)]
        today: Option<Date>,
    },
}

/// Écrit le `.eml` à côté du coffre (`<coffre>.drafts/`, 0700/0600) et l'ouvre. Partagé avec
/// la fenêtre.
///
/// # Errors
pub fn write_and_open_draft(
    db_path: &Path,
    filename: &str,
    rfc5322: &str,
    open: bool,
) -> Result<PathBuf, CliError> {
    let dir = drafts_dir(db_path);
    std::fs::create_dir_all(&dir).map_err(|e| {
        CliError::Unexpected(format!("création de {} impossible : {e}", dir.display()))
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    let path = dir.join(filename);
    std::fs::write(&path, rfc5322.as_bytes()).map_err(|e| {
        CliError::Unexpected(format!("écriture de {} impossible : {e}", path.display()))
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    if open {
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        let _ = std::process::Command::new(opener)
            .arg(&path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
    Ok(path)
}

fn drafts_dir(db_path: &Path) -> PathBuf {
    let mut name = db_path
        .file_name()
        .map_or_else(|| "freeflow.db".into(), std::ffi::OsStr::to_os_string);
    name.push(".drafts");
    db_path.with_file_name(name)
}

fn today_or(today: Option<Date>) -> Date {
    today.unwrap_or_else(griffe_core::clock::today_local)
}

fn read_letter_body(
    body: Option<String>,
    body_file: Option<PathBuf>,
) -> Result<Option<String>, CliError> {
    match body_file {
        Some(path) => {
            let text = std::fs::read_to_string(&path).map_err(|e| {
                CliError::Unexpected(format!("lecture de {} impossible : {e}", path.display()))
            })?;
            Ok(Some(text))
        }
        None => Ok(body),
    }
}

fn resolve_subject(store: &Store, needle: &str) -> Result<FollowUpSubject, CliError> {
    refs::resolve_follow_up(store, needle)
}

pub fn run(
    cmd: FollowUpCommand,
    store: &mut Store,
    ctx: &ExecutionContext,
    json: bool,
) -> Result<String, CliError> {
    let db_path = store.db_path().to_path_buf();
    match cmd {
        FollowUpCommand::Queue { today } => {
            let cards = follow_up_queue(store.connection(), today_or(today))?;
            Ok(format_value(&cards, json))
        }
        FollowUpCommand::Board { today } => {
            let cards = follow_up_board(store.connection(), today_or(today))?;
            Ok(format_value(&cards, json))
        }
        FollowUpCommand::Show { reference, today } => {
            let subject = resolve_subject(store, &reference)?;
            let card = card_for(store.connection(), subject, today_or(today))?;
            Ok(format_value(&card, json))
        }
        FollowUpCommand::From { email, name } => {
            let outcome = Executor::new(store).execute(&SetFollowUpSender { email, name }, ctx)?;
            Ok(format_outcome_as(&outcome, json, |_| {
                "expéditeur enregistré".to_string()
            }))
        }
        FollowUpCommand::Draft {
            reference,
            today,
            subject,
            body,
            body_file,
            no_open,
        } => {
            let today = today_or(today);
            let follow_up = resolve_subject(store, &reference)?;
            let body = read_letter_body(body, body_file)?;
            let outcome = Executor::new(store).execute(
                &PrepareFollowUp {
                    subject: follow_up,
                    today,
                    subject_line: subject,
                    body,
                },
                ctx,
            )?;
            match &outcome {
                griffe_core::app::Outcome::Applied(prepared)
                | griffe_core::app::Outcome::AlreadyApplied(prepared) => {
                    let path = write_and_open_draft(
                        &db_path,
                        &prepared.draft.filename,
                        &prepared.draft.rfc5322,
                        !no_open && !ctx.dry_run,
                    )?;
                    Ok(format_outcome_as(&outcome, json, |_| {
                        format!("brouillon ouvert : {}", path.display())
                    }))
                }
                _ => Ok(format_outcome_as(&outcome, json, |_| {
                    "brouillon préparé".to_string()
                })),
            }
        }
        FollowUpCommand::Sent {
            reference,
            today,
            subject,
            body,
            body_file,
        } => {
            let today = today_or(today);
            let follow_up = resolve_subject(store, &reference)?;
            let body = read_letter_body(body, body_file)?;
            let outcome = Executor::new(store).execute(
                &MarkFollowUpSent {
                    subject: follow_up,
                    today,
                    subject_line: subject,
                    body,
                },
                ctx,
            )?;
            Ok(format_outcome_as(&outcome, json, |card| {
                format!(
                    "marqué envoyé — prochaine : {}",
                    or_dash(card.due_on.map(format_date))
                )
            }))
        }
        FollowUpCommand::Skip { reference, today } => {
            let today = today_or(today);
            let subject = resolve_subject(store, &reference)?;
            let outcome =
                Executor::new(store).execute(&SkipFollowUpStep { subject, today }, ctx)?;
            Ok(format_outcome(&outcome, json))
        }
        FollowUpCommand::Snooze {
            reference,
            until,
            tomorrow,
            next_week,
            monday,
            today,
        } => {
            let today = today_or(today);
            let until = if let Some(d) = until {
                d
            } else if tomorrow {
                snooze_date(today, SnoozePreset::Tomorrow)
            } else if next_week {
                snooze_date(today, SnoozePreset::NextWeek)
            } else if monday {
                snooze_date(today, SnoozePreset::NextMonday)
            } else {
                return Err(CliError::Domain(
                    "précisez --until AAAA-MM-JJ, --tomorrow, --next-week ou --monday".into(),
                ));
            };
            let subject = resolve_subject(store, &reference)?;
            let outcome = Executor::new(store).execute(
                &SnoozeFollowUp {
                    subject,
                    until,
                    today,
                },
                ctx,
            )?;
            Ok(format_outcome_as(&outcome, json, |_| {
                format!("reporté au {}", format_date(until))
            }))
        }
        FollowUpCommand::Schedule {
            reference,
            on,
            today,
        } => {
            let today = today_or(today);
            let subject = resolve_subject(store, &reference)?;
            let outcome =
                Executor::new(store).execute(&SetFollowUpDate { subject, on, today }, ctx)?;
            Ok(format_outcome_as(&outcome, json, |_| {
                format!("prochaine relance le {}", format_date(on))
            }))
        }
        FollowUpCommand::Retract { reference, today } => {
            let today = today_or(today);
            let subject = resolve_subject(store, &reference)?;
            let outcome =
                Executor::new(store).execute(&RetractLastFollowUp { subject, today }, ctx)?;
            Ok(format_outcome_as(&outcome, json, |_| {
                "dernier geste annulé".to_string()
            }))
        }
    }
}
