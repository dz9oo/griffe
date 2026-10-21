//! `griffe boite` — lire le compte déjà réglé dans Le courrier.
//! Le mot de passe ne passe ni par les arguments, ni par le journal.

use clap::Subcommand;
use griffe_core::app::{ExecutionContext, Executor};
use griffe_core::mail::{
    AddressFault, MarkAddress, file_human, french_submit_error, list_box, releve_plan,
    resolve_message_id,
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
    let pulled = griffe_mail::fetch_new(&plan.endpoint, &plan.secret, &plan.cursors)
        .map_err(|error| CliError::Domain(french_submit_error(&error, icloud)))?;
    let mut mailbox = store.open_mailbox()?;
    let report = griffe_core::mail::absorb(store, &mut mailbox, &pulled, ctx)?;
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
