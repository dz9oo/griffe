//! `griffe courrier` — compte d'envoi, lettres armées, passage au serveur.
//! Le mot de passe ne passe ni par les arguments, ni par le journal.

use std::io::IsTerminal;
use std::path::PathBuf;

use clap::{ArgGroup, Subcommand};
use griffe_core::app::{ExecutionContext, Executor};
use griffe_core::mail::{
    ArmOutbound, CancelOutbound, ClearMailSecret, MailSecret, SaveMailAccount, SaveMailSecret,
    SetAutomaticSend, UNDO_SECS, french_submit_error, hourly_pause, profile, record_deliveries,
    take_due,
};
use griffe_core::store::Store;
use time::OffsetDateTime;
use zeroize::Zeroize;

use crate::error::CliError;
use crate::output::{format_json, format_outcome_as, key_values};

#[derive(Subcommand)]
pub enum MailCommand {
    /// Compte d'envoi, sans le mot de passe.
    Show,
    /// Enregistre le serveur. iCloud remplit l'hôte, le port et le chiffrement.
    Save {
        /// Adresse qui signe les lettres.
        from_address: String,
        /// Identifiant demandé par le serveur.
        username: String,
        /// Nom affiché à côté de l'adresse.
        #[arg(long)]
        from_name: Option<String>,
        /// Hôte SMTP. Ignoré pour le préréglage iCloud.
        #[arg(long)]
        host: Option<String>,
        /// 587 ou 465. Ignoré pour le préréglage iCloud.
        #[arg(long)]
        port: Option<u16>,
        /// `icloud` ou `custom`.
        #[arg(long, default_value = "custom")]
        preset: String,
    },
    /// Pose ou retire le mot de passe. Invite masquée, ou fichier. Jamais en argument.
    Secret {
        /// Fichier UTF-8 contenant le mot de passe, et rien d'autre.
        #[arg(long, conflicts_with = "clear")]
        secret_file: Option<PathBuf>,
        /// Retire le mot de passe du coffre.
        #[arg(long)]
        clear: bool,
    },
    /// Active ou éteint l'envoi des lettres dont la date est aujourd'hui.
    #[command(group(ArgGroup::new("mode").required(true).args(["on", "off"])))]
    Auto {
        #[arg(long)]
        on: bool,
        #[arg(long)]
        off: bool,
    },
    /// Arme un essai vers l'adresse qui signe. `flush` le poste après cinq secondes.
    Essai,
    /// Arme une lettre. `flush` la poste.
    Envoyer {
        to: String,
        subject: String,
        #[arg(long, conflicts_with = "body_file")]
        body: Option<String>,
        #[arg(long)]
        body_file: Option<PathBuf>,
        /// Dossier ou ancre affichée avec la lettre.
        #[arg(long)]
        anchor: Option<String>,
    },
    /// Retient une lettre encore armée.
    Annuler { id: String },
    /// Poste les lettres échues de cette commande. Celles de la fenêtre en cours d'annulation restent.
    Flush,
}

pub fn run(
    command: MailCommand,
    store: &mut Store,
    ctx: &ExecutionContext,
    json: bool,
) -> Result<String, CliError> {
    match command {
        MailCommand::Show => show(store, json),
        MailCommand::Save {
            from_address,
            username,
            from_name,
            host,
            port,
            preset,
        } => save(
            store,
            ctx,
            json,
            SaveFields {
                from_address,
                username,
                from_name,
                host,
                port,
                preset,
            },
        ),
        MailCommand::Secret { secret_file, clear } => {
            if clear {
                let outcome = Executor::new(store).execute(&ClearMailSecret, ctx)?;
                return Ok(format_outcome_as(&outcome, json, |_| {
                    "Le mot de passe est retiré.".to_string()
                }));
            }
            let secret = read_secret(secret_file.as_deref())?;
            let outcome = Executor::new(store).execute(&SaveMailSecret { secret }, ctx)?;
            Ok(format_outcome_as(&outcome, json, |_| {
                "Le mot de passe est dans le coffre.".to_string()
            }))
        }
        MailCommand::Auto { on, .. } => {
            let outcome = Executor::new(store).execute(&SetAutomaticSend { enabled: on }, ctx)?;
            let sentence = if on {
                "L'envoi du jour est actif."
            } else {
                "L'envoi du jour est éteint."
            };
            Ok(format_outcome_as(&outcome, json, |_| sentence.to_string()))
        }
        MailCommand::Essai => essai(store, ctx, json),
        MailCommand::Envoyer {
            to,
            subject,
            body,
            body_file,
            anchor,
        } => envoyer(
            store,
            ctx,
            json,
            LetterFields {
                to,
                subject,
                body,
                body_file,
                anchor,
            },
        ),
        MailCommand::Annuler { id } => {
            let outcome = Executor::new(store).execute(&CancelOutbound { id }, ctx)?;
            Ok(format_outcome_as(&outcome, json, |_| {
                "La lettre reste.".to_string()
            }))
        }
        MailCommand::Flush => flush(store, ctx, json),
    }
}

fn show(store: &Store, json: bool) -> Result<String, CliError> {
    let account = profile(store.connection())?;
    if json {
        return Ok(format_json(&serde_json::json!({
            "from_name": account.from_name,
            "from_address": account.from_address,
            "host": account.host,
            "port": account.port,
            "tls": account.tls.as_str(),
            "username": account.username,
            "preset": account.preset.as_str(),
            "has_secret": account.has_secret,
            "auto_send": account.auto_send,
            "ready": account.ready,
        })));
    }
    Ok(key_values(&[
        ("adresse", account.from_address),
        ("nom", or_empty(account.from_name)),
        ("serveur", account.host),
        ("port", account.port.to_string()),
        ("chiffrement", account.tls.as_str().to_string()),
        ("identifiant", account.username),
        ("préréglage", account.preset.as_str().to_string()),
        (
            "mot de passe",
            if account.has_secret {
                "présent".to_string()
            } else {
                "absent".to_string()
            },
        ),
        (
            "envoi du jour",
            if account.auto_send {
                "actif".to_string()
            } else {
                "éteint".to_string()
            },
        ),
    ]))
}

struct SaveFields {
    from_address: String,
    username: String,
    from_name: Option<String>,
    host: Option<String>,
    port: Option<u16>,
    preset: String,
}

fn save(
    store: &mut Store,
    ctx: &ExecutionContext,
    json: bool,
    fields: SaveFields,
) -> Result<String, CliError> {
    let SaveFields {
        from_address,
        username,
        from_name,
        host,
        port,
        preset,
    } = fields;
    let icloud = preset == "icloud";
    let port = if icloud { 587 } else { port.unwrap_or(587) };
    let tls = match port {
        587 => "starttls",
        465 => "implicit",
        _ => {
            return Err(CliError::Domain("Le port est 587 ou 465.".into()));
        }
    };
    let host = if icloud {
        griffe_core::mail::ICLOUD_HOST.to_string()
    } else {
        host.filter(|value| !value.trim().is_empty())
            .ok_or_else(|| CliError::Domain("L'hôte du serveur manque.".into()))?
    };
    let outcome = Executor::new(store).execute(
        &SaveMailAccount {
            from_name,
            from_address,
            host,
            port,
            tls: tls.to_string(),
            username,
            preset,
        },
        ctx,
    )?;
    Ok(format_outcome_as(&outcome, json, |_| {
        "Le serveur est enregistré.".to_string()
    }))
}

fn essai(store: &mut Store, ctx: &ExecutionContext, json: bool) -> Result<String, CliError> {
    let account = profile(store.connection())?;
    if !account.ready {
        return Err(CliError::Domain(
            "Le courrier n'est pas encore branché.".into(),
        ));
    }
    let outcome = Executor::new(store).execute(
        &ArmOutbound {
            kind: "trial".into(),
            anchor: Some("essai".into()),
            to_address: account.from_address,
            subject: "Essai".into(),
            body: "Ceci est un essai envoyé depuis Griffe.".into(),
            delay_secs: UNDO_SECS,
            session_token: Some("cli".into()),
            follow_subject: None,
            follow_subject_id: None,
            follow_cycle: None,
            follow_step: None,
        },
        ctx,
    )?;
    Ok(format_outcome_as(&outcome, json, |_| {
        "L'essai est armé. `griffe courrier flush` le poste après cinq secondes.".to_string()
    }))
}

struct LetterFields {
    to: String,
    subject: String,
    body: Option<String>,
    body_file: Option<PathBuf>,
    anchor: Option<String>,
}

fn envoyer(
    store: &mut Store,
    ctx: &ExecutionContext,
    json: bool,
    fields: LetterFields,
) -> Result<String, CliError> {
    let LetterFields {
        to,
        subject,
        body,
        body_file,
        anchor,
    } = fields;
    let body = match (body, body_file) {
        (Some(text), None) => text,
        (None, Some(path)) => std::fs::read_to_string(&path).map_err(|error| {
            CliError::Domain(format!("lecture de {} : {error}", path.display()))
        })?,
        _ => {
            return Err(CliError::Domain(
                "La lettre se donne avec --body ou --body-file.".into(),
            ));
        }
    };
    let outcome = Executor::new(store).execute(
        &ArmOutbound {
            kind: "letter".into(),
            anchor,
            to_address: to,
            subject,
            body,
            delay_secs: 0,
            session_token: None,
            follow_subject: None,
            follow_subject_id: None,
            follow_cycle: None,
            follow_step: None,
        },
        ctx,
    )?;
    Ok(format_outcome_as(&outcome, json, |_| {
        "La lettre est armée. `griffe courrier flush` la poste.".to_string()
    }))
}

fn flush(store: &mut Store, ctx: &ExecutionContext, json: bool) -> Result<String, CliError> {
    let now = OffsetDateTime::now_utc();
    let today = crate::today();
    let Some(batch) = take_due(store, ctx, "cli", now, today, false)? else {
        if hourly_pause(store.connection(), now)? {
            return Ok(done(
                json,
                "Trop de lettres cette heure. Celles qui restent attendent.",
                &serde_json::json!({ "posted": 0, "paused": true }),
            ));
        }
        return Ok(done(
            json,
            "Rien à poster.",
            &serde_json::json!({ "posted": 0 }),
        ));
    };
    let icloud = profile(store.connection())?.preset == griffe_core::mail::MailPreset::Icloud;
    let subjects: Vec<(String, String)> = batch
        .letters
        .iter()
        .map(|letter| (letter.id.clone(), letter.message.subject.clone()))
        .collect();
    let outcomes = griffe_mail::submit_batch(batch);
    let lines: Vec<String> = outcomes
        .iter()
        .map(|outcome| {
            let subject = subjects
                .iter()
                .find(|(id, _)| id == &outcome.id)
                .map(|(_, subject)| subject.as_str())
                .unwrap_or("Une lettre");
            match &outcome.result {
                Ok(_) => format!("« {subject} » est partie."),
                Err(error) => format!(
                    "« {subject} » n'est pas partie. {}",
                    french_submit_error(error, icloud)
                ),
            }
        })
        .collect();
    let posted = outcomes
        .iter()
        .filter(|outcome| outcome.result.is_ok())
        .count();
    record_deliveries(store, ctx, today, &outcomes)?;
    let text = format!("{posted} lettre(s) postée(s).\n{}", lines.join("\n"));
    Ok(done(
        json,
        &text,
        &serde_json::json!({ "posted": posted, "lines": lines }),
    ))
}

fn done(json: bool, text: &str, value: &serde_json::Value) -> String {
    if json {
        format_json(value)
    } else {
        text.to_string()
    }
}

fn or_empty(value: String) -> String {
    if value.is_empty() {
        "—".to_string()
    } else {
        value
    }
}

fn read_secret(path: Option<&std::path::Path>) -> Result<MailSecret, CliError> {
    let mut raw = if let Some(path) = path {
        std::fs::read_to_string(path)
            .map_err(|error| CliError::Domain(format!("lecture de {} : {error}", path.display())))?
    } else if std::io::stderr().is_terminal() {
        rpassword::prompt_password("Mot de passe du serveur : ")
            .map_err(|error| CliError::Domain(format!("lecture du mot de passe : {error}")))?
    } else {
        return Err(CliError::Domain(
            "Le mot de passe se donne avec --secret-file, ou dans un terminal.".into(),
        ));
    };
    let secret = MailSecret::new(raw.trim()).map_err(|error| CliError::Domain(error.to_string()));
    raw.zeroize();
    secret
}
