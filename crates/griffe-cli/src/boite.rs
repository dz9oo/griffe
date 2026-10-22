//! `griffe boite` — lire le compte déjà réglé dans Le courrier.
//! Le mot de passe ne passe ni par les arguments, ni par le journal.

use std::path::PathBuf;

use clap::Subcommand;
use griffe_core::app::{ExecutionContext, Executor};
use griffe_core::mail::{
    AddressFault, MarkAddress, RECENT_BODIES, ReleaseReplyHold, ReplyDraft, UNDO_SECS, arm_reply,
    file_human, french_submit_error, list_box, note_releve_failure, note_releve_success,
    releve_plan, resolve_message_id,
};
use griffe_core::store::Store;

use crate::error::CliError;
use crate::output::{format_json, format_outcome_as};
use crate::refs;

#[derive(Subcommand)]
pub enum BoiteCommand {
    /// Lettres déjà relevées, sans leur corps.
    Liste,
    /// Relève la boîte de réception et Envoyés.
    Relever,
    /// Range une lettre sur une fiche, et sur un dossier si on le nomme.
    Classer {
        /// Identifiant du message, ou celui du miroir.
        #[arg(long)]
        message: String,
        /// Fiche : nom ou identifiant.
        #[arg(long)]
        fiche: String,
        /// Dossier ouvert. Absent : la fiche seule.
        #[arg(long)]
        dossier: Option<String>,
    },
    /// Dit qu'une adresse en échec est bonne.
    Adresse {
        /// Fiche : nom ou identifiant.
        #[arg(long)]
        fiche: String,
        /// Adresse à confirmer.
        #[arg(long)]
        adresse: String,
    },
    /// Arme une réponse sur le dossier. La relance reste retenue.
    Repondre {
        /// Identifiant du message, ou celui du miroir.
        #[arg(long)]
        message: String,
        /// Dossier : nom ou identifiant.
        #[arg(long)]
        dossier: String,
        /// Sujet de la réponse.
        #[arg(long)]
        sujet: String,
        /// Corps de la réponse.
        #[arg(long, conflicts_with = "corps_file")]
        corps: Option<String>,
        /// Fichier qui contient le corps.
        #[arg(long, conflicts_with = "corps")]
        corps_file: Option<PathBuf>,
    },
    /// Laisse repartir la relance retenue.
    Relance {
        /// Dossier : nom ou identifiant.
        #[arg(long)]
        dossier: String,
    },
}

pub fn run(
    command: BoiteCommand,
    store: &mut Store,
    ctx: &ExecutionContext,
    json: bool,
) -> Result<String, CliError> {
    match command {
        BoiteCommand::Liste => liste(store, json),
        BoiteCommand::Relever => relever(store, ctx, json),
        BoiteCommand::Classer {
            message,
            fiche,
            dossier,
        } => classer(store, ctx, json, &message, &fiche, dossier.as_deref()),
        BoiteCommand::Adresse {
            fiche,
            adresse: address,
        } => adresse(store, ctx, json, &fiche, &address),
        BoiteCommand::Repondre {
            message,
            dossier,
            sujet,
            corps,
            corps_file,
        } => repondre(
            store,
            ctx,
            json,
            ReplyFields {
                message,
                dossier,
                sujet,
                corps,
                corps_file,
            },
        ),
        BoiteCommand::Relance { dossier } => relance(store, ctx, json, &dossier),
    }
}

fn liste(store: &Store, json: bool) -> Result<String, CliError> {
    let mailbox = store.open_mailbox()?;
    let lines = list_box(store, &mailbox)?;
    if json {
        return Ok(format_json(&lines));
    }
    if lines.is_empty() {
        return Ok("La boîte est vide.".to_string());
    }
    let mut out = String::new();
    for line in lines {
        let who = line
            .from_name
            .or(line.from_address)
            .unwrap_or_else(|| "—".to_string());
        let dossier = if line.choice {
            "à classer".to_string()
        } else {
            line.dossier.unwrap_or_else(|| "—".to_string())
        };
        let notice = line.notice.unwrap_or_default();
        out.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\n",
            line.view, who, line.subject, dossier, notice
        ));
    }
    Ok(out)
}

fn relever(store: &mut Store, ctx: &ExecutionContext, json: bool) -> Result<String, CliError> {
    if ctx.dry_run {
        return Ok("La relève n'écrit rien.".to_string());
    }
    let Some(plan) = releve_plan(store)? else {
        return Err(CliError::Domain(
            "Le courrier ne permet pas encore de lire la boîte.".into(),
        ));
    };
    let icloud = plan.endpoint.icloud;
    let pulled = match griffe_mail::fetch_new(&plan.endpoint, &plan.secret, &plan.cursors) {
        Ok(pulled) => pulled,
        Err(error) => {
            let sentence = french_submit_error(&error, icloud);
            let _ = note_releve_failure(store, &sentence);
            return Err(CliError::Domain(sentence));
        }
    };
    let mut mailbox = store.open_mailbox()?;
    let mut report = griffe_core::mail::absorb(store, &mut mailbox, &pulled, ctx)?;
    let pending = mailbox.pending_bodies(RECENT_BODIES)?;
    if let Ok(bodies) = griffe_mail::fetch_pending(&plan.endpoint, &plan.secret, &pending) {
        let filled = griffe_core::mail::absorb(store, &mut mailbox, &bodies, ctx)?;
        report.stored = report.stored.saturating_add(filled.stored);
        report.held = report.held.saturating_add(filled.held);
        report.failed = report.failed.saturating_add(filled.failed);
        report.choices = report.choices.saturating_add(filled.choices);
        report.on_dossier = report.on_dossier.saturating_add(filled.on_dossier);
    }
    let stamp = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();
    let _ = note_releve_success(store, &stamp);
    if json {
        return Ok(format_json(&report));
    }
    Ok(format!(
        "{} lettres rangées. {} relances retenues. {} adresses ne reçoivent pas. {} à classer.",
        report.stored, report.held, report.failed, report.choices
    ))
}

fn classer(
    store: &mut Store,
    ctx: &ExecutionContext,
    json: bool,
    message: &str,
    fiche: &str,
    dossier: Option<&str>,
) -> Result<String, CliError> {
    let message_id = resolve_message_id(store, message)?;
    let client = refs::resolve_client(store, fiche)?;
    let opportunity = dossier
        .map(|name| refs::resolve_opportunity(store, name).map(|id| id.to_string()))
        .transpose()?;
    let outcome = file_human(store, ctx, &message_id, &client.to_string(), opportunity)?;
    Ok(format_outcome_as(&outcome, json, |_| {
        "La lettre est rangée.".to_string()
    }))
}

fn adresse(
    store: &mut Store,
    ctx: &ExecutionContext,
    json: bool,
    fiche: &str,
    adresse: &str,
) -> Result<String, CliError> {
    let client = refs::resolve_client(store, fiche)?;
    let outcome = Executor::new(store).execute(
        &MarkAddress {
            client_id: client.to_string(),
            address: adresse.to_string(),
            status: AddressFault::Confirmed,
            message_id: None,
        },
        ctx,
    )?;
    Ok(format_outcome_as(&outcome, json, |_| {
        "L'adresse est bonne.".to_string()
    }))
}

struct ReplyFields {
    message: String,
    dossier: String,
    sujet: String,
    corps: Option<String>,
    corps_file: Option<PathBuf>,
}

fn repondre(
    store: &mut Store,
    ctx: &ExecutionContext,
    json: bool,
    fields: ReplyFields,
) -> Result<String, CliError> {
    let ReplyFields {
        message,
        dossier,
        sujet,
        corps,
        corps_file,
    } = fields;
    let body = match (corps, corps_file) {
        (Some(text), None) => text,
        (None, Some(path)) => std::fs::read_to_string(&path).map_err(|error| {
            CliError::Domain(format!("lecture de {} : {error}", path.display()))
        })?,
        _ => {
            return Err(CliError::Domain(
                "Le corps se donne avec --corps ou --corps-file.".into(),
            ));
        }
    };
    let opportunity = refs::resolve_opportunity(store, &dossier)?;
    let outcome = arm_reply(
        store,
        ctx,
        &ReplyDraft {
            opportunity_id: opportunity.to_string(),
            message,
            subject: sujet,
            body,
            delay_secs: UNDO_SECS,
            session_token: None,
        },
    )?;
    Ok(format_outcome_as(&outcome, json, |_| {
        "La réponse est armée.".to_string()
    }))
}

fn relance(
    store: &mut Store,
    ctx: &ExecutionContext,
    json: bool,
    dossier: &str,
) -> Result<String, CliError> {
    let opportunity = refs::resolve_opportunity(store, dossier)?;
    let outcome = Executor::new(store).execute(
        &ReleaseReplyHold {
            opportunity_id: opportunity.to_string(),
        },
        ctx,
    )?;
    Ok(format_outcome_as(&outcome, json, |_| {
        "La relance peut partir.".to_string()
    }))
}
