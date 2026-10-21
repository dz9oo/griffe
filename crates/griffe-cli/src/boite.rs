//! `griffe boite` — lire le compte déjà réglé dans Le courrier.
//! Le mot de passe ne passe ni par les arguments, ni par le journal.

use clap::Subcommand;
use griffe_core::app::ExecutionContext;
use griffe_core::mail::{french_submit_error, list_box, releve_plan};
use griffe_core::store::Store;

use crate::error::CliError;
use crate::output::format_json;

#[derive(Subcommand)]
pub enum BoiteCommand {
    /// Lettres déjà relevées, sans leur corps.
    Liste,
    /// Relève la boîte de réception et Envoyés.
    Relever,
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
