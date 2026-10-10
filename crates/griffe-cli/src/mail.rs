//! `griffe courrier` — compte d'envoi, lettres armées, passage au serveur.
//! Le mot de passe ne passe ni par les arguments, ni par le journal.

use std::io::IsTerminal;
use std::path::PathBuf;

use clap::{ArgGroup, Subcommand};
use griffe_core::app::{ExecutionContext, Executor};
use griffe_core::mail::{
    ArmOutbound, CancelOutbound, ClearMailSecret, DeliveryOutcome, MailPreset, MailSecret,
    RecordMailProbe, RememberImapUsername, SaveLetterface, SaveMailAccount, SaveMailSecret,
    SaveMailSignature, SentCopyStatus, SetAutomaticSend, SubmissionBatch, UNDO_SECS, abandon_held,
    commit_outbound, french_submit_error, hourly_pause, probe_material, profile, record_copies,
    record_deliveries, take_copies, take_due, trial_letter,
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
        /// Hôte IMAP des copies. Ignoré pour iCloud. Vide : pas de copie.
        #[arg(long)]
        imap_host: Option<String>,
        /// Port des copies. 993. Ignoré pour iCloud, et quand l'hôte est vide.
        #[arg(long, default_value_t = 993)]
        imap_port: u16,
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
    /// Couleur, métier et site de la lettre. Sans drapeau, montre l'allure.
    Apparence {
        /// `vert`, `encre` ou `sceau`. Absent : inchangé.
        #[arg(long)]
        encre: Option<String>,
        /// Ligne sous le nom. Chaîne vide : retire la ligne.
        #[arg(long)]
        metier: Option<String>,
        /// Adresse en bas de la lettre. Chaîne vide : retire la ligne.
        #[arg(long)]
        site: Option<String>,
    },
    /// Montre la formule, ou la pose depuis l'entrée standard.
    Signature {
        /// Lit la formule sur l'entrée standard, retours à la ligne compris.
        #[arg(long)]
        set: bool,
    },
    /// Essaie la liaison, sans envoyer de lettre.
    Essayer,
    /// Arme la lettre d'essai. `flush` la poste après cinq secondes.
    Essai {
        /// Destinataire. Sans lui, l'adresse qui signe.
        #[arg(long)]
        to: Option<String>,
    },
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
        MailCommand::Apparence {
            encre,
            metier,
            site,
        } => apparence(store, ctx, json, encre, metier, site),
        MailCommand::Save {
            from_address,
            username,
            from_name,
            host,
            port,
            imap_host,
            imap_port,
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
                imap_host,
                imap_port,
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
        MailCommand::Signature { set } => signature(store, ctx, json, set),
        MailCommand::Essayer => essayer(store, ctx, json),
        MailCommand::Essai { to } => essai(store, ctx, json, to),
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
    let value = account_json(&account);
    let mut text = key_values(&[
        ("adresse", account.from_address.clone()),
        ("nom", or_empty(account.from_name.clone())),
        ("serveur", account.host.clone()),
        ("port", account.port.to_string()),
        ("chiffrement", account.tls.as_str().to_string()),
        ("identifiant", account.username.clone()),
        ("préréglage", account.preset.as_str().to_string()),
        ("copie", copy_line(&account)),
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
        ("liaison", liaison_line(&account)),
        ("encre", account.link_ink.as_str().to_string()),
        ("métier", or_empty(account.metier.clone())),
        ("site", or_empty(account.site.clone())),
    ]);
    if account.signature.is_empty() {
        text.push_str("\nformule : vide");
    } else {
        text.push_str("\nformule :\n");
        text.push_str(&account.signature);
    }
    Ok(format_json_or(json, &value, text))
}

struct SaveFields {
    from_address: String,
    username: String,
    from_name: Option<String>,
    host: Option<String>,
    port: Option<u16>,
    imap_host: Option<String>,
    imap_port: u16,
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
        imap_host,
        imap_port,
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
            imap_host: imap_host.unwrap_or_default(),
            imap_port,
        },
        ctx,
    )?;
    Ok(format_outcome_as(&outcome, json, |_| {
        "Le serveur est enregistré.".to_string()
    }))
}

fn signature(
    store: &mut Store,
    ctx: &ExecutionContext,
    json: bool,
    set: bool,
) -> Result<String, CliError> {
    if !set {
        let text = profile(store.connection())?.signature;
        return Ok(format_json_or(
            json,
            &serde_json::json!({ "signature": text }),
            if text.is_empty() {
                "vide".to_string()
            } else {
                text
            },
        ));
    }
    let raw = read_signature()?;
    let empty = raw.replace('\r', "");
    let empty = empty.trim().is_empty();
    let outcome = Executor::new(store).execute(&SaveMailSignature { signature: raw }, ctx)?;
    let sentence = if empty {
        "La lettre se ferme par Bien à vous, le nom, la société."
    } else {
        "La formule est enregistrée."
    };
    Ok(format_outcome_as(&outcome, json, |_| sentence.to_string()))
}

fn essai(
    store: &mut Store,
    ctx: &ExecutionContext,
    json: bool,
    to: Option<String>,
) -> Result<String, CliError> {
    let account = profile(store.connection())?;
    if !account.ready {
        return Err(CliError::Domain(
            "Le courrier n'est pas encore branché.".into(),
        ));
    }
    let letter = trial_letter(store.connection())?;
    let outcome = Executor::new(store).execute(
        &ArmOutbound {
            kind: "trial".into(),
            anchor: Some("essai".into()),
            to_address: to.unwrap_or(account.from_address),
            subject: letter.subject,
            body: letter.body,
            delay_secs: UNDO_SECS,
            session_token: Some("cli".into()),
            follow_subject: None,
            follow_subject_id: None,
            follow_cycle: None,
            follow_step: None,
            client_id: None,
        },
        ctx,
    )?;
    Ok(format_outcome_as(&outcome, json, |_| {
        "L'essai est armé. `griffe courrier flush` le poste après cinq secondes.".to_string()
    }))
}

fn account_json(account: &griffe_core::mail::MailProfile) -> serde_json::Value {
    serde_json::json!({
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
        "signature": account.signature,
        "link_ink": account.link_ink.as_str(),
        "metier": account.metier,
        "site": account.site,
        "probe_ok": account.probe.as_ref().map(|probe| probe.ok),
        "probe_detail": account.probe.as_ref().map(|probe| probe.detail.clone()),
        "copy_host": account.copy_target().as_ref().map(|target| target.host.clone()),
        "copy_port": account.copy_target().map(|target| target.port),
        "imap_username": account.imap_username.clone(),
    })
}

fn copy_line(account: &griffe_core::mail::MailProfile) -> String {
    match account.copy_target() {
        Some(target) if target.icloud => "Envoyés, via iCloud".to_string(),
        Some(target) => format!("Envoyés, via {}:{}", target.host, target.port),
        None => "pas de copie".to_string(),
    }
}

fn liaison_line(account: &griffe_core::mail::MailProfile) -> String {
    account.probe.as_ref().map_or_else(
        || "pas encore essayée".to_string(),
        |probe| probe.detail.clone(),
    )
}

fn essayer(store: &mut Store, ctx: &ExecutionContext, json: bool) -> Result<String, CliError> {
    let account = profile(store.connection())?;
    if account.from_address.is_empty() || account.host.is_empty() || account.username.is_empty() {
        return Err(CliError::Domain(
            "Le courrier n'est pas encore branché.".into(),
        ));
    }
    if !account.has_secret {
        return Err(CliError::Domain("Le mot de passe manque.".into()));
    }
    let Some(material) = probe_material(store.connection())? else {
        return Err(CliError::Domain(
            "Le courrier n'est pas encore branché.".into(),
        ));
    };
    let verdict =
        griffe_mail::probe_account(&material.endpoint, material.copy.as_ref(), &material.secret);
    drop(material);
    if let Some(username) = verdict.imap_username.clone() {
        Executor::new(store).execute(&RememberImapUsername { username }, ctx)?;
    }
    let detail = verdict.detail.clone();
    let outcome = Executor::new(store).execute(
        &RecordMailProbe {
            ok: verdict.ok,
            detail: detail.clone(),
        },
        ctx,
    )?;
    Ok(format_outcome_as(&outcome, json, |_| detail.clone()))
}

fn format_json_or(json: bool, value: &serde_json::Value, text: String) -> String {
    if json { format_json(value) } else { text }
}

fn read_signature() -> Result<String, CliError> {
    if std::io::stderr().is_terminal() && std::io::stdin().is_terminal() {
        eprintln!("Formule, puis Ctrl-D :");
    }
    let mut raw = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut raw)
        .map_err(|error| CliError::Domain(format!("lecture de la formule : {error}")))?;
    Ok(raw)
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
            client_id: None,
        },
        ctx,
    )?;
    Ok(format_outcome_as(&outcome, json, |_| {
        "La lettre est armée. `griffe courrier flush` la poste.".to_string()
    }))
}

fn apparence(
    store: &mut Store,
    ctx: &ExecutionContext,
    json: bool,
    encre: Option<String>,
    metier: Option<String>,
    site: Option<String>,
) -> Result<String, CliError> {
    let account = profile(store.connection())?;
    if encre.is_none() && metier.is_none() && site.is_none() {
        let text = key_values(&[
            ("encre", account.link_ink.as_str().to_string()),
            ("métier", or_empty(account.metier.clone())),
            ("site", or_empty(account.site.clone())),
        ]);
        return Ok(format_json_or(
            json,
            &serde_json::json!({
                "link_ink": account.link_ink.as_str(),
                "metier": account.metier,
                "site": account.site,
            }),
            text,
        ));
    }
    let outcome = Executor::new(store).execute(
        &SaveLetterface {
            ink: encre.unwrap_or_else(|| account.link_ink.as_str().to_string()),
            metier: metier.unwrap_or(account.metier),
            site: site.unwrap_or(account.site),
        },
        ctx,
    )?;
    Ok(format_outcome_as(&outcome, json, |_| {
        "L'allure est enregistrée.".to_string()
    }))
}

fn flush(store: &mut Store, ctx: &ExecutionContext, json: bool) -> Result<String, CliError> {
    let now = OffsetDateTime::now_utc();
    let today = crate::today();
    abandon_held(store, ctx)?;
    let batch = take_due(store, ctx, "cli", now, today, false)?;
    let mut lines = Vec::new();
    let mut posted = 0;
    if let Some(batch) = batch {
        let icloud = profile(store.connection())?.preset == MailPreset::Icloud;
        let SubmissionBatch {
            endpoint,
            secret,
            imap,
            letters,
        } = batch;
        match griffe_mail::LettreMail::new(endpoint, secret) {
            Err(error) => {
                let outcomes = letters
                    .into_iter()
                    .map(|letter| {
                        lines.push(format!(
                            "« {} » n'est pas partie. {}",
                            letter.message.subject,
                            french_submit_error(&error, icloud)
                        ));
                        DeliveryOutcome {
                            id: letter.id,
                            result: Err(error.clone()),
                            copy: SentCopyStatus::Skipped,
                        }
                    })
                    .collect::<Vec<_>>();
                record_deliveries(store, ctx, today, &outcomes)?;
            }
            Ok(mail) => {
                let mut outcomes = Vec::new();
                for letter in letters {
                    let id = letter.id.clone();
                    let subject = letter.message.subject.clone();
                    if !commit_outbound(store, ctx, &id)? {
                        continue;
                    }
                    let outcome = griffe_mail::submit_letter(&mail, imap.as_ref(), letter);
                    lines.push(match &outcome.result {
                        Ok(_) => {
                            posted += 1;
                            format!("« {subject} » est partie.")
                        }
                        Err(error) => format!(
                            "« {subject} » n'est pas partie. {}",
                            french_submit_error(error, icloud)
                        ),
                    });
                    outcomes.push(outcome);
                }
                if !outcomes.is_empty() {
                    record_deliveries(store, ctx, today, &outcomes)?;
                }
            }
        }
    }
    let copy_lines = file_pending_copies(store, ctx)?;
    if lines.is_empty() && copy_lines.is_empty() {
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
    }
    let mut text = String::new();
    if !lines.is_empty() {
        text.push_str(&format!("{posted} lettre(s) postée(s)."));
        if hourly_pause(store.connection(), now)? && posted == 0 {
            text.push_str(" Trop de lettres cette heure. Celles qui restent attendent.");
        }
        text.push('\n');
        text.push_str(&lines.join("\n"));
    } else if hourly_pause(store.connection(), now)? {
        text.push_str("Trop de lettres cette heure. Celles qui restent attendent.");
    }
    if !copy_lines.is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&copy_lines.join("\n"));
    }
    lines.extend(copy_lines.iter().cloned());
    Ok(done(
        json,
        &text,
        &serde_json::json!({ "posted": posted, "lines": lines }),
    ))
}

fn file_pending_copies(store: &mut Store, ctx: &ExecutionContext) -> Result<Vec<String>, CliError> {
    let Some(batch) = take_copies(store, ctx, OffsetDateTime::now_utc())? else {
        return Ok(Vec::new());
    };
    let subjects: Vec<(String, String)> = batch
        .letters
        .iter()
        .map(|letter| (letter.id.clone(), letter.message.subject.clone()))
        .collect();
    let outcomes = griffe_mail::file_copies(batch);
    let lines = outcomes
        .iter()
        .map(|outcome| {
            let subject = subjects
                .iter()
                .find(|(id, _)| id == &outcome.id)
                .map(|(_, subject)| subject.as_str())
                .unwrap_or("Une lettre");
            match &outcome.copy {
                SentCopyStatus::Saved { .. } => format!("« {subject} » est dans Envoyés."),
                SentCopyStatus::Failed(message) => format!("« {subject} » est partie. {message}"),
                SentCopyStatus::Skipped => {
                    format!("« {subject} » est partie. La copie n'ira pas dans Envoyés.")
                }
            }
        })
        .collect();
    record_copies(store, ctx, &outcomes)?;
    Ok(lines)
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
