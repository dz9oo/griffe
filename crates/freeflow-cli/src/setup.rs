//! `freeflow setup status` — où en est la configuration du coffre (lot 39). Les prérequis et le
//! prochain geste viennent du cœur (`freeflow_core::setup`) ; cette façade ne fait que les
//! afficher et, avec `--new-company`, enregistrer le choix « société nouvelle ».

use clap::Subcommand;
use freeflow_core::app::{ExecutionContext, Executor};
use freeflow_core::setup::{DeclareNewCompany, Prerequisite, SetupStep, setup_json, setup_status};
use freeflow_core::store::Store;

use crate::error::CliError;
use crate::output::{format_json, format_outcome_as};

#[derive(Debug, Subcommand)]
pub enum SetupCommand {
    /// Liste à cocher : profil, point de départ (bilan d'ouverture ou société nouvelle), relevé
    /// importé — et le prochain geste.
    Status,
    /// Déclare que la société vient d'être créée : pas de bilan de cabinet à reprendre, la
    /// chaîne part de zéro en connaissance de cause (`--undo` pour revenir dessus).
    NewCompany {
        #[arg(long)]
        undo: bool,
    },
}

fn glyph(p: &Prerequisite) -> &'static str {
    match p {
        Prerequisite::Done => "✓",
        Prerequisite::Missing => "✗",
        Prerequisite::Incomplete { .. } => "!",
    }
}

fn detail(p: &Prerequisite) -> String {
    match p {
        Prerequisite::Done => String::new(),
        Prerequisite::Missing => " — manquant".to_string(),
        Prerequisite::Incomplete { fields } => format!(" — à compléter : {}", fields.join(", ")),
    }
}

pub fn run(
    cmd: SetupCommand,
    store: &mut Store,
    ctx: &ExecutionContext,
    json: bool,
) -> Result<String, CliError> {
    match cmd {
        SetupCommand::Status => {
            let status = setup_status(store.connection())?;
            if json {
                return Ok(format_json(&setup_json(&status)));
            }
            let origin = if status.declared_new_company {
                " (société nouvelle, pas de bilan à reprendre)"
            } else {
                ""
            };
            let hint = match status.next_step {
                SetupStep::Profile => "freeflow company set-profile …",
                SetupStep::Origin => "freeflow year opening set … (ou freeflow setup new-company)",
                SetupStep::Bank => "freeflow bank import <export de votre banque>",
                SetupStep::Done => "freeflow year checklist <année>",
            };
            Ok(format!(
                "Premier lancement\n  {} profil de la société{}\n  {} point de départ{}{}\n  {} \
                 relevé bancaire{}\n\nProchain geste : {}\n  ↳ {hint}",
                glyph(&status.profile),
                detail(&status.profile),
                glyph(&status.origin),
                detail(&status.origin),
                origin,
                glyph(&status.bank),
                detail(&status.bank),
                status.next_step.text(),
            ))
        }
        SetupCommand::NewCompany { undo } => {
            let outcome =
                Executor::new(store).execute(&DeclareNewCompany { declared: !undo }, ctx)?;
            format_outcome_as(&outcome, json, |()| {
                if undo {
                    "choix « société nouvelle » annulé".to_string()
                } else {
                    "société déclarée nouvelle : la chaîne part de zéro".to_string()
                }
            })
            .pipe_ok()
        }
    }
}

trait PipeOk {
    fn pipe_ok(self) -> Result<String, CliError>;
}

impl PipeOk for String {
    fn pipe_ok(self) -> Result<String, CliError> {
        Ok(self)
    }
}
