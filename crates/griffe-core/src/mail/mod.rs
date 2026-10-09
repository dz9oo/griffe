//! Courrier sortant. Le cœur décide quand une lettre part et ce qu'elle dit.
//! La crate qui parle SMTP implémente [`OutboundMail`] et reste hors d'ici.

mod commands;
mod error;
mod model;
mod present;
mod secret;
mod store;

pub use commands::{
    ArmOutbound, CancelOutbound, ClearMailSecret, RecordMailProbe, RememberImapUsername,
    ResolveUncertain, RetryOutbound, SaveLetterface, SaveMailAccount, SaveMailSecret,
    SaveMailSignature, SetAutomaticSend,
};
pub use error::MailError;
pub use model::{
    DeliveryOutcome, HOURLY_CAP, ICLOUD_HOST, ICLOUD_IMAP_HOST, ICLOUD_SENT_MAILBOX, IMAP_PORT,
    ImapEndpoint, LetterChrome, LetterInk, MISSING_ADDRESS, MailPreset, MailProbeStatus,
    MailProfile, MailSubmitError, MailTick, OutboundMail, OutboundMessage, OutboundStatus,
    OutboundView, PROBE_OK_SENTENCE, ProbeMaterial, ProbeVerdict, ReadyLetter, RecordingMail,
    SentCopyStatus, SmtpEndpoint, SubmissionBatch, SubmissionReceipt, TlsMode, UNDO_SECS,
};
pub use present::{LINK_HINT, LetterParts, close_letter, letter_card, letter_html, readable_links};
pub use secret::MailSecret;

/// Compte d'envoi, sans le secret.
///
/// # Errors
///
/// Lecture impossible.
pub fn profile(conn: &rusqlite::Connection) -> Result<MailProfile, AppError> {
    store::profile(conn)
}

/// Identifiant, hôte et mot de passe pour un essai de liaison. N'ouvre aucune socket.
/// `None` quand le compte n'est pas encore branché.
///
/// # Errors
///
/// Lecture impossible.
pub fn probe_material(conn: &rusqlite::Connection) -> Result<Option<ProbeMaterial>, AppError> {
    store::probe_material(conn)
}

/// Dernière lettre encore ouverte pour cette ancre (dossier ou relance).
///
/// # Errors
///
/// Lecture impossible.
pub fn outbound_for_anchor(
    conn: &rusqlite::Connection,
    anchor: &str,
    now: OffsetDateTime,
) -> Result<Option<OutboundView>, AppError> {
    store::for_anchor(conn, anchor, now)
}

use time::{Date, OffsetDateTime};

use crate::app::{AppError, ExecutionContext, Executor};
use crate::domain::{TemplateContext, render_template};

/// Lettre d'essai : le premier message, pour un prénom fictif.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrialLetter {
    pub subject: String,
    pub body: String,
}

/// Le premier message vivant, précédé d'une ligne qui dit que c'est un essai.
/// Le prénom est Camille. Aucune fiche n'est lue.
///
/// # Errors
///
/// Lecture impossible.
pub fn trial_letter(conn: &rusqlite::Connection) -> Result<TrialLetter, AppError> {
    let account = profile(conn)?;
    trial_with_closing(conn, &account.signature)
}

/// Même lettre d'essai, fermée par le texte donné. N'écrit rien.
///
/// # Errors
///
/// Lecture impossible.
pub fn trial_with_closing(
    conn: &rusqlite::Connection,
    signature: &str,
) -> Result<TrialLetter, AppError> {
    let phrases = crate::follow_up::prospect_phrases(conn)?;
    let (subject_template, body_template) = phrases.into_iter().next().map_or_else(
        || ("{{sujet}}".to_string(), "{{signature}}\n".to_string()),
        |phrase| (phrase.subject, phrase.body),
    );
    let (moi, societe) = crate::follow_up::letter_speaker(conn)?;
    let ctx = TemplateContext {
        prenom: "Camille".into(),
        sujet: "cette lettre d'essai".into(),
        moi,
        societe,
        signature: signature.to_string(),
        ..TemplateContext::default()
    };
    let rendered_subject = render_template(&subject_template, &ctx);
    let rendered_subject = rendered_subject.trim();
    let subject = if rendered_subject.is_empty() {
        "Essai".to_string()
    } else {
        format!("Essai — {rendered_subject}")
    };
    let rendered = render_template(&body_template, &ctx);
    let body = format!("Cette lettre est un essai. Elle n'attend pas de réponse.\n\n{rendered}");
    Ok(TrialLetter { subject, body })
}

/// Prépare les lettres échues de cette session. Ne contacte aucun serveur.
/// Un agent est refusé : poster est un geste de la fenêtre ou de la CLI humaine.
///
/// `release_stale` annule les lettres encore armées par une autre fenêtre.
/// Le passage `griffe courrier flush` le laisse à faux : il réclame les lettres
/// sans jeton, et celles de son propre jeton, sans couper les cinq secondes
/// encore ouvertes dans la fenêtre.
///
/// # Errors
///
/// Persistance, ou acteur agent.
pub fn take_due(
    store: &mut crate::store::Store,
    ctx: &ExecutionContext,
    session_token: &str,
    now: OffsetDateTime,
    today: Date,
    release_stale: bool,
) -> Result<Option<SubmissionBatch>, AppError> {
    refuse_agent(ctx)?;
    let mut executor = Executor::new(store);
    executor.execute(&commands::FreezeInterrupted { now }, ctx)?;
    if release_stale {
        executor.execute(
            &commands::ReleaseStaleArmed {
                now,
                session_token: session_token.to_string(),
            },
            ctx,
        )?;
    }
    executor.execute(
        &commands::EnrollToday {
            today,
            now,
            session_token: session_token.to_string(),
        },
        ctx,
    )?;
    let claimed = match executor.execute(
        &commands::ClaimDue {
            now,
            session_token: session_token.to_string(),
        },
        ctx,
    )? {
        crate::app::Outcome::Applied(ids) | crate::app::Outcome::AlreadyApplied(ids) => ids,
        crate::app::Outcome::DryRun | crate::app::Outcome::PendingConfirmation(_) => {
            return Ok(None);
        }
    };
    if claimed.is_empty() {
        return Ok(None);
    }
    store::load_batch(store.connection(), &claimed)
}

/// Engage une lettre tenue vers le serveur. `false` : Annuler a gagné, rien ne part.
///
/// # Errors
///
/// Persistance, ou acteur agent.
pub fn commit_outbound(
    store: &mut crate::store::Store,
    ctx: &ExecutionContext,
    id: &str,
) -> Result<bool, AppError> {
    refuse_agent(ctx)?;
    match Executor::new(store).execute(&commands::CommitOutbound { id: id.to_string() }, ctx)? {
        crate::app::Outcome::Applied(engaged) | crate::app::Outcome::AlreadyApplied(engaged) => {
            Ok(engaged)
        }
        crate::app::Outcome::DryRun | crate::app::Outcome::PendingConfirmation(_) => Ok(false),
    }
}

/// Annule les lettres tenues dont l'envoi n'a pas abouti. Elles ne repartent pas.
///
/// # Errors
///
/// Persistance, ou acteur agent.
pub fn abandon_held(
    store: &mut crate::store::Store,
    ctx: &ExecutionContext,
) -> Result<(), AppError> {
    refuse_agent(ctx)?;
    Executor::new(store).execute(&commands::AbandonHeld, ctx)?;
    Ok(())
}

/// Inscrit le résultat de chaque soumission. Une réussite avance la relance liée.
///
/// # Errors
///
/// Persistance, ou acteur agent.
pub fn record_deliveries(
    store: &mut crate::store::Store,
    ctx: &ExecutionContext,
    today: Date,
    outcomes: &[DeliveryOutcome],
) -> Result<(), AppError> {
    refuse_agent(ctx)?;
    let icloud = store::profile(store.connection())?.preset == MailPreset::Icloud;
    for outcome in outcomes {
        let detail = match &outcome.result {
            Ok(receipt) => commands::DeliveryDetail::Sent {
                response: receipt.smtp_response.clone(),
            },
            Err(error) => commands::DeliveryDetail::Failed {
                message: french_submit_error(error, icloud),
            },
        };
        Executor::new(store).execute(
            &commands::RecordDelivery {
                id: outcome.id.clone(),
                today,
                detail,
            },
            ctx,
        )?;
        if outcome.result.is_ok() {
            record_one_copy(store, ctx, outcome)?;
        }
    }
    Ok(())
}

/// Inscrit le dépôt d'une copie, sans retoucher au statut d'envoi.
///
/// # Errors
///
/// Persistance, ou acteur agent.
pub fn record_copies(
    store: &mut crate::store::Store,
    ctx: &ExecutionContext,
    outcomes: &[DeliveryOutcome],
) -> Result<(), AppError> {
    refuse_agent(ctx)?;
    for outcome in outcomes {
        record_one_copy(store, ctx, outcome)?;
    }
    Ok(())
}

fn record_one_copy(
    store: &mut crate::store::Store,
    ctx: &ExecutionContext,
    outcome: &DeliveryOutcome,
) -> Result<(), AppError> {
    let (status, detail, imap_username) = match &outcome.copy {
        SentCopyStatus::Saved { username } => ("saved", String::new(), Some(username.clone())),
        SentCopyStatus::Failed(message) => ("failed", message.clone(), None),
        SentCopyStatus::Skipped => ("skipped", String::new(), None),
    };
    Executor::new(store).execute(
        &commands::RecordSentCopy {
            id: outcome.id.clone(),
            status: status.to_string(),
            detail,
            imap_username,
        },
        ctx,
    )?;
    Ok(())
}

/// Lettres déjà parties dont la copie manque encore. N'ouvre aucune socket
/// et ne les remet pas dans la file SMTP.
///
/// # Errors
///
/// Persistance, ou acteur agent.
pub fn take_copies(
    store: &mut crate::store::Store,
    ctx: &ExecutionContext,
    now: OffsetDateTime,
) -> Result<Option<SubmissionBatch>, AppError> {
    refuse_agent(ctx)?;
    store::load_copies(store.connection(), now)
}

/// Phrase montrée à la personne. Le texte du serveur est déjà raccourci, sans secret.
#[must_use]
pub fn french_submit_error(error: &MailSubmitError, icloud: bool) -> String {
    match error {
        MailSubmitError::Auth if icloud => {
            "Le serveur a refusé le mot de passe. Pour iCloud, c'est un mot de passe d'application."
                .to_string()
        }
        MailSubmitError::Auth => "Le serveur a refusé le mot de passe.".to_string(),
        MailSubmitError::Tls => "La liaison chiffrée a échoué.".to_string(),
        MailSubmitError::Timeout => "Le serveur n'a pas répondu.".to_string(),
        MailSubmitError::Refused(message) | MailSubmitError::Other(message) => message.clone(),
    }
}

/// Lettres à signaler sur Le jour : parties, restées, incertaines, échouées.
///
/// # Errors
///
/// Lecture impossible.
pub fn day_notes(conn: &rusqlite::Connection, today: Date) -> Result<Vec<String>, AppError> {
    store::day_notes(conn, today)
}

/// Vrai quand le plafond de l'heure est atteint et qu'une lettre armée attend encore.
///
/// # Errors
///
/// Lecture impossible.
pub fn hourly_pause(conn: &rusqlite::Connection, now: OffsetDateTime) -> Result<bool, AppError> {
    store::hourly_pause(conn, now)
}

/// Ce que l'horloge a à faire, sans écrire. `send` passe par les commandes
/// d'envoi. `copy` ne relit que les lettres déjà parties.
///
/// # Errors
///
/// Lecture impossible.
pub fn mail_tick(
    conn: &rusqlite::Connection,
    session_token: &str,
    now: OffsetDateTime,
    today: Date,
    release_stale: bool,
) -> Result<MailTick, AppError> {
    store::mail_tick(conn, session_token, now, today, release_stale)
}

/// Vrai quand un passage de l'horloge changerait le coffre. La fenêtre s'en sert
/// pour ne pas écrire dans l'audit à chaque seconde.
///
/// # Errors
///
/// Lecture impossible.
pub fn needs_tick(
    conn: &rusqlite::Connection,
    session_token: &str,
    now: OffsetDateTime,
    today: Date,
    release_stale: bool,
) -> Result<bool, AppError> {
    Ok(mail_tick(conn, session_token, now, today, release_stale)?.pending())
}

fn refuse_agent(ctx: &ExecutionContext) -> Result<(), AppError> {
    if matches!(ctx.actor, crate::app::Actor::Agent { .. }) {
        return Err(MailError::AgentCannotPost.into());
    }
    Ok(())
}

/// Soumet chaque lettre déjà réclamée. Le secret du lot est effacé en sortant.
#[must_use]
pub fn deliver_with<T: OutboundMail + ?Sized>(
    mail: &T,
    batch: SubmissionBatch,
) -> Vec<DeliveryOutcome> {
    let SubmissionBatch {
        letters, secret, ..
    } = batch;
    drop(secret);
    letters
        .into_iter()
        .map(|letter| DeliveryOutcome {
            id: letter.id,
            result: mail.submit(&letter.message),
            copy: SentCopyStatus::Skipped,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use time::{Date, Duration, Month, OffsetDateTime, Time};

    use super::{
        ArmOutbound, ClearMailSecret, DeliveryOutcome, LetterInk, MISSING_ADDRESS, MailError,
        MailSubmitError, PROBE_OK_SENTENCE, RecordMailProbe, RecordingMail, RetryOutbound,
        SaveLetterface, SaveMailAccount, SaveMailSecret, SaveMailSignature, SentCopyStatus,
        SetAutomaticSend, SubmissionReceipt, abandon_held, commit_outbound, deliver_with,
        mail_tick, profile, record_deliveries, take_copies, take_due, trial_letter,
    };
    use crate::app::{Actor, AppError, ExecutionContext, Executor, Outcome, recent_audit_entries};
    use crate::clients::{CreateClient, CreateContact, DeleteContact, UpdateContact};
    use crate::domain::{ClientId, FollowUpSubject, Money, Probability};
    use crate::follow_up::SetFollowUpSender;
    use crate::mail::MailSecret;
    use crate::prospection::CreateOpportunity;
    use crate::store::testing::test_store;

    fn human() -> ExecutionContext {
        ExecutionContext::new(Actor::Human, false)
    }

    fn engage(store: &mut crate::store::Store, id: &str) {
        assert!(commit_outbound(store, &human(), id).unwrap());
    }

    fn agent() -> ExecutionContext {
        ExecutionContext::new(
            Actor::Agent {
                session: "mcp-test".into(),
            },
            false,
        )
    }

    fn at(day: u8, hour: u8, minute: u8, second: u8) -> OffsetDateTime {
        OffsetDateTime::new_utc(
            Date::from_calendar_date(2026, Month::September, day).unwrap(),
            Time::from_hms(hour, minute, second).unwrap(),
        )
    }

    fn save_ready(store: &mut crate::store::Store) {
        let Outcome::Applied(()) = Executor::new(store)
            .execute(
                &SaveMailAccount {
                    from_name: Some("Camille".into()),
                    from_address: "camille@studio.test".into(),
                    host: "smtp.mail.me.com".into(),
                    port: 25,
                    tls: "plain".into(),
                    username: "camille@icloud.test".into(),
                    preset: "icloud".into(),
                    imap_host: String::new(),
                    imap_port: 993,
                },
                &human(),
            )
            .unwrap()
        else {
            panic!("compte");
        };
        let Outcome::Applied(()) = Executor::new(store)
            .execute(
                &SaveMailSecret {
                    secret: MailSecret::new("mot-de-passe-application-xyz").unwrap(),
                },
                &human(),
            )
            .unwrap()
        else {
            panic!("secret");
        };
    }

    fn seed(store: &mut crate::store::Store, next: Date) -> FollowUpSubject {
        let client_id = match Executor::new(store)
            .execute(
                &CreateClient {
                    name: format!("Acme {}", ClientId::new()),
                    siren: None,
                    vat_number: None,
                    address: None,
                },
                &human(),
            )
            .unwrap()
        {
            Outcome::Applied(id) => id,
            other => panic!("client {other:?}"),
        };
        Executor::new(store)
            .execute(
                &CreateContact {
                    client_id,
                    name: "Marie Curie".into(),
                    email: Some("marie@acme.test".into()),
                    phone: None,
                    role: None,
                },
                &human(),
            )
            .unwrap();
        match Executor::new(store)
            .execute(
                &CreateOpportunity {
                    client_id,
                    name: "Refonte".into(),
                    amount: Money::from_cents(4_500_000),
                    probability: Probability::new(40).unwrap(),
                    next_action_at: next,
                    source: None,
                },
                &human(),
            )
            .unwrap()
        {
            Outcome::Applied(id) => FollowUpSubject::Opportunity(id),
            other => panic!("opportunité {other:?}"),
        }
    }

    #[test]
    fn the_secret_stays_out_of_the_audit_and_of_debug() {
        let mut store = test_store("mail-secret");
        save_ready(&mut store);
        let secret = "mot-de-passe-application-xyz";
        let shown = format!("{:?}", MailSecret::new(secret).unwrap());
        assert!(!shown.contains(secret));
        let entries = recent_audit_entries(store.connection(), 0, 20).unwrap();
        let saved = entries
            .iter()
            .find(|entry| entry.command_name == "mail.save_secret")
            .unwrap();
        assert!(!saved.command_json.contains(secret));
        assert_eq!(saved.command_json, "{}");
        let account = profile(store.connection()).unwrap();
        assert!(account.has_secret);
        assert!(account.ready);
        assert!(!format!("{account:?}").contains(secret));
    }

    #[test]
    fn a_signature_is_kept_without_a_server_and_refuses_its_own_token() {
        let mut store = test_store("signature-save");
        let Outcome::Applied(()) = Executor::new(&mut store)
            .execute(
                &SaveMailSignature {
                    signature: "Nicolas\r\nAtelier\n".into(),
                },
                &human(),
            )
            .unwrap()
        else {
            panic!("formule");
        };
        let account = profile(store.connection()).unwrap();
        assert_eq!(account.signature, "Nicolas\nAtelier");
        assert!(!account.ready);
        let entries = recent_audit_entries(store.connection(), 0, 20).unwrap();
        let saved = entries
            .iter()
            .find(|entry| entry.command_name == "mail.save_signature")
            .unwrap();
        assert!(saved.command_json.contains("Nicolas"));

        let token = Executor::new(&mut store).execute(
            &SaveMailSignature {
                signature: "voir <signature>".into(),
            },
            &human(),
        );
        assert!(token.is_err());
        let long = Executor::new(&mut store).execute(
            &SaveMailSignature {
                signature: "é".repeat(2_001),
            },
            &human(),
        );
        assert!(long.is_err());
        assert_eq!(
            profile(store.connection()).unwrap().signature,
            "Nicolas\nAtelier"
        );

        let Outcome::Applied(()) = Executor::new(&mut store)
            .execute(
                &SaveMailSignature {
                    signature: "  \n".into(),
                },
                &human(),
            )
            .unwrap()
        else {
            panic!("retrait");
        };
        assert!(profile(store.connection()).unwrap().signature.is_empty());
    }

    #[test]
    fn the_letterface_survives_the_server_and_refuses_a_bad_ink() {
        let mut store = test_store("letterface");
        let Outcome::Applied(()) = Executor::new(&mut store)
            .execute(
                &SaveLetterface {
                    ink: "sceau".into(),
                    metier: "Ingénieur logiciel".into(),
                    site: "https://atelier.example".into(),
                },
                &human(),
            )
            .unwrap()
        else {
            panic!("face");
        };
        let account = profile(store.connection()).unwrap();
        assert_eq!(account.link_ink, LetterInk::Sceau);
        assert_eq!(account.metier, "Ingénieur logiciel");
        assert_eq!(account.site, "https://atelier.example");
        assert!(!account.ready);
        assert!(
            Executor::new(&mut store)
                .execute(
                    &SaveLetterface {
                        ink: "rouge".into(),
                        metier: String::new(),
                        site: String::new(),
                    },
                    &human(),
                )
                .is_err()
        );
        assert_eq!(
            profile(store.connection()).unwrap().link_ink,
            LetterInk::Sceau
        );
        assert!(
            Executor::new(&mut store)
                .execute(
                    &SaveLetterface {
                        ink: "vert".into(),
                        metier: "é".repeat(81),
                        site: String::new(),
                    },
                    &human(),
                )
                .is_err()
        );
        assert!(
            Executor::new(&mut store)
                .execute(
                    &SaveLetterface {
                        ink: "encre".into(),
                        metier: "une\nligne".into(),
                        site: "atelier.example".into(),
                    },
                    &human(),
                )
                .is_err()
        );
        save_ready(&mut store);
        let account = profile(store.connection()).unwrap();
        assert_eq!(account.link_ink, LetterInk::Sceau);
        assert_eq!(account.metier, "Ingénieur logiciel");
        let Outcome::Applied(()) = Executor::new(&mut store)
            .execute(
                &SaveLetterface {
                    ink: "vert".into(),
                    metier: "  ".into(),
                    site: String::new(),
                },
                &human(),
            )
            .unwrap()
        else {
            panic!("retrait");
        };
        let account = profile(store.connection()).unwrap();
        assert!(account.metier.is_empty());
        assert!(account.site.is_empty());
        assert_eq!(account.link_ink, LetterInk::Vert);
    }

    #[test]
    fn the_trial_letter_is_the_first_phrase_for_camille() {
        let mut store = test_store("trial-letter");
        let _ = seed(
            &mut store,
            Date::from_calendar_date(2026, Month::September, 5).unwrap(),
        );
        let Outcome::Applied(()) = Executor::new(&mut store)
            .execute(
                &SaveMailSignature {
                    signature: "Nicolas Collier\nAtelier Nord".into(),
                },
                &human(),
            )
            .unwrap()
        else {
            panic!("formule");
        };
        let letter = trial_letter(store.connection()).unwrap();
        assert_eq!(letter.subject, "Essai — cette lettre d'essai");
        assert!(letter.body.starts_with(
            "Cette lettre est un essai. Elle n'attend pas de réponse.\n\nBonjour Camille,"
        ));
        assert!(letter.body.contains("cette lettre d'essai"));
        assert!(letter.body.contains("Nicolas Collier\nAtelier Nord"));
        assert!(!letter.body.contains("marie@"));
        assert!(!letter.body.contains("Marie"));
        assert!(!letter.body.contains("Refonte"));
        assert!(!letter.body.contains("Bien à vous"));
    }

    #[test]
    fn arming_closes_the_letter_with_the_formula_once() {
        let mut store = test_store("arm-formula");
        save_ready(&mut store);
        let Outcome::Applied(()) = Executor::new(&mut store)
            .execute(
                &SaveMailSignature {
                    signature: "Nicolas\nAtelier".into(),
                },
                &human(),
            )
            .unwrap()
        else {
            panic!("formule");
        };
        let Outcome::Applied(id) = Executor::new(&mut store)
            .execute(
                &ArmOutbound {
                    kind: "trial".into(),
                    anchor: Some("essai".into()),
                    to_address: "ada@atelier.test".into(),
                    subject: "Essai".into(),
                    body: "Bonjour.".into(),
                    delay_secs: 0,
                    session_token: None,
                    follow_subject: None,
                    follow_subject_id: None,
                    follow_cycle: None,
                    follow_step: None,
                    client_id: None,
                },
                &human(),
            )
            .unwrap()
        else {
            panic!("armement");
        };
        let body: String = store
            .connection()
            .query_row(
                "SELECT body FROM outbound_mail WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(body, "Bonjour.\n\nNicolas\nAtelier");
    }

    #[test]
    fn an_empty_signature_closes_the_trial_with_the_formula() {
        let mut store = test_store("trial-formula");
        let Outcome::Applied(()) = Executor::new(&mut store)
            .execute(
                &SetFollowUpSender {
                    email: "nicolas@lumen.test".into(),
                    name: Some("Nicolas".into()),
                },
                &human(),
            )
            .unwrap()
        else {
            panic!("expéditeur");
        };
        let letter = trial_letter(store.connection()).unwrap();
        assert!(letter.body.contains("Bien à vous,\nNicolas"));
        assert!(letter.body.contains("Bonjour Camille"));
        assert!(!letter.body.contains("marie@"));
    }

    #[test]
    fn cleartext_is_refused_and_icloud_fills_the_server() {
        let mut store = test_store("mail-clear");
        let refused = Executor::new(&mut store).execute(
            &SaveMailAccount {
                from_name: None,
                from_address: "camille@studio.test".into(),
                host: "smtp.example.test".into(),
                port: 25,
                tls: "plain".into(),
                username: "camille@studio.test".into(),
                preset: "custom".into(),
                imap_host: String::new(),
                imap_port: 993,
            },
            &human(),
        );
        assert!(refused.is_err());
        save_ready(&mut store);
        let account = profile(store.connection()).unwrap();
        assert_eq!(account.host, "smtp.mail.me.com");
        assert_eq!(account.port, 587);
        assert!(!account.auto_send);
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn cancel_wins_before_the_claim_and_loses_after() {
        let mut store = test_store("mail-cancel");
        save_ready(&mut store);
        let id = match Executor::new(&mut store)
            .execute(
                &ArmOutbound {
                    kind: "letter".into(),
                    anchor: Some("camille".into()),
                    to_address: "marie@acme.test".into(),
                    subject: "Bonjour".into(),
                    body: "Une ligne.".into(),
                    delay_secs: 5,
                    session_token: Some("fenetre".into()),
                    follow_subject: None,
                    follow_subject_id: None,
                    follow_cycle: None,
                    follow_step: None,
                    client_id: None,
                },
                &human(),
            )
            .unwrap()
        {
            Outcome::Applied(id) => id,
            other => panic!("{other:?}"),
        };
        Executor::new(&mut store)
            .execute(&super::CancelOutbound { id: id.clone() }, &human())
            .unwrap();
        let now = at(5, 8, 0, 0);
        let batch = take_due(&mut store, &human(), "fenetre", now, now.date(), true).unwrap();
        assert!(batch.is_none());

        let id = match Executor::new(&mut store)
            .execute(
                &ArmOutbound {
                    kind: "letter".into(),
                    anchor: Some("camille".into()),
                    to_address: "marie@acme.test".into(),
                    subject: "Bonjour".into(),
                    body: "Une ligne.".into(),
                    delay_secs: 0,
                    session_token: Some("fenetre".into()),
                    follow_subject: None,
                    follow_subject_id: None,
                    follow_cycle: None,
                    follow_step: None,
                    client_id: None,
                },
                &human(),
            )
            .unwrap()
        {
            Outcome::Applied(id) => id,
            other => panic!("{other:?}"),
        };
        let later = OffsetDateTime::now_utc() + Duration::seconds(1);
        let batch = take_due(&mut store, &human(), "fenetre", later, later.date(), true)
            .unwrap()
            .expect("tenue");
        assert_eq!(batch.letters.len(), 1);
        Executor::new(&mut store)
            .execute(&super::CancelOutbound { id: id.clone() }, &human())
            .unwrap();
        let recorder = RecordingMail::default();
        assert!(recorder.sent().is_empty());
        let status: String = store
            .connection()
            .query_row(
                "SELECT status FROM outbound_mail WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(status, "cancelled");

        let id = match Executor::new(&mut store)
            .execute(
                &ArmOutbound {
                    kind: "letter".into(),
                    anchor: Some("camille".into()),
                    to_address: "marie@acme.test".into(),
                    subject: "Encore".into(),
                    body: "Une ligne.".into(),
                    delay_secs: 0,
                    session_token: Some("fenetre".into()),
                    follow_subject: None,
                    follow_subject_id: None,
                    follow_cycle: None,
                    follow_step: None,
                    client_id: None,
                },
                &human(),
            )
            .unwrap()
        {
            Outcome::Applied(id) => id,
            other => panic!("{other:?}"),
        };
        let batch = take_due(&mut store, &human(), "fenetre", later, later.date(), true)
            .unwrap()
            .expect("réclamée");
        engage(&mut store, &id);
        let cancel =
            Executor::new(&mut store).execute(&super::CancelOutbound { id: id.clone() }, &human());
        assert!(matches!(
            cancel,
            Err(crate::app::AppError::Domain(message)) if message == MailError::NotArmed.to_string()
        ));
        let outcomes = deliver_with(&recorder, batch);
        record_deliveries(&mut store, &human(), later.date(), &outcomes).unwrap();
        assert_eq!(recorder.sent().len(), 1);
        assert!(!recorder.sent()[0].text.contains("mot-de-passe"));
    }

    #[test]
    fn a_second_claim_finds_nothing() {
        let mut store = test_store("mail-once");
        save_ready(&mut store);
        Executor::new(&mut store)
            .execute(
                &ArmOutbound {
                    kind: "trial".into(),
                    anchor: None,
                    to_address: "camille@studio.test".into(),
                    subject: "Essai".into(),
                    body: "Une ligne.".into(),
                    delay_secs: 0,
                    session_token: None,
                    follow_subject: None,
                    follow_subject_id: None,
                    follow_cycle: None,
                    follow_step: None,
                    client_id: None,
                },
                &human(),
            )
            .unwrap();
        let now = OffsetDateTime::now_utc() + Duration::seconds(1);
        let first = take_due(&mut store, &human(), "fenetre", now, now.date(), true)
            .unwrap()
            .expect("première");
        let second = take_due(&mut store, &human(), "fenetre", now, now.date(), true).unwrap();
        assert!(second.is_none());
        assert_eq!(first.letters.len(), 1);
    }

    #[test]
    fn automatic_stays_off_and_skips_what_is_late() {
        let mut store = test_store("mail-auto");
        let today = Date::from_calendar_date(2026, Month::September, 5).unwrap();
        save_ready(&mut store);
        seed(&mut store, today);
        seed(
            &mut store,
            Date::from_calendar_date(2026, Month::September, 4).unwrap(),
        );
        let now = at(5, 9, 0, 0);
        assert!(
            take_due(&mut store, &human(), "fenetre", now, today, true)
                .unwrap()
                .is_none()
        );
        let pending = Executor::new(&mut store)
            .execute(&SetAutomaticSend { enabled: true }, &agent())
            .unwrap();
        assert!(matches!(pending, Outcome::PendingConfirmation(_)));
        assert!(!profile(store.connection()).unwrap().auto_send);
        Executor::new(&mut store)
            .execute(&SetAutomaticSend { enabled: true }, &human())
            .unwrap();
        let batch = take_due(&mut store, &human(), "fenetre", now, today, true)
            .unwrap()
            .expect("la lettre du jour");
        assert_eq!(batch.letters.len(), 1);
        assert_eq!(batch.letters[0].message.to_address, "marie@acme.test");
        let id = batch.letters[0].id.clone();
        engage(&mut store, &id);
        let recorder = RecordingMail::default();
        let outcomes = deliver_with(&recorder, batch);
        record_deliveries(&mut store, &human(), today, &outcomes).unwrap();
        let again = take_due(&mut store, &human(), "fenetre", now, today, true).unwrap();
        assert!(again.is_none());
    }

    #[test]
    fn an_abandoned_undo_is_not_sent_and_a_crash_stays_uncertain() {
        let mut store = test_store("mail-stale");
        save_ready(&mut store);
        Executor::new(&mut store)
            .execute(
                &ArmOutbound {
                    kind: "letter".into(),
                    anchor: Some("reste".into()),
                    to_address: "marie@acme.test".into(),
                    subject: "Restée".into(),
                    body: "Pas partie.".into(),
                    delay_secs: 0,
                    session_token: Some("ancienne".into()),
                    follow_subject: None,
                    follow_subject_id: None,
                    follow_cycle: None,
                    follow_step: None,
                    client_id: None,
                },
                &human(),
            )
            .unwrap();
        let now = OffsetDateTime::now_utc() + Duration::seconds(2);
        assert!(
            take_due(&mut store, &human(), "nouvelle", now, now.date(), true)
                .unwrap()
                .is_none()
        );
        Executor::new(&mut store)
            .execute(
                &ArmOutbound {
                    kind: "letter".into(),
                    anchor: Some("vive".into()),
                    to_address: "marie@acme.test".into(),
                    subject: "Vive".into(),
                    body: "Elle part.".into(),
                    delay_secs: 0,
                    session_token: Some("nouvelle".into()),
                    follow_subject: None,
                    follow_subject_id: None,
                    follow_cycle: None,
                    follow_step: None,
                    client_id: None,
                },
                &human(),
            )
            .unwrap();
        let batch = take_due(&mut store, &human(), "nouvelle", now, now.date(), true)
            .unwrap()
            .expect("vivante");
        assert_eq!(batch.letters[0].message.subject, "Vive");
        let id = batch.letters[0].id.clone();
        drop(batch);
        let later = take_due(
            &mut store,
            &human(),
            "nouvelle",
            now + Duration::seconds(1),
            now.date(),
            true,
        )
        .unwrap();
        assert!(later.is_none());
        let status: String = store
            .connection()
            .query_row(
                "SELECT status FROM outbound_mail WHERE subject = 'Vive'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(status, "held");
        engage(&mut store, &id);
        let frozen = take_due(
            &mut store,
            &human(),
            "nouvelle",
            now + Duration::seconds(2),
            now.date(),
            true,
        )
        .unwrap();
        assert!(frozen.is_none());
        let status: String = store
            .connection()
            .query_row(
                "SELECT status FROM outbound_mail WHERE subject = 'Vive'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(status, "uncertain");
        let agent_post = take_due(&mut store, &agent(), "nouvelle", now, now.date(), true);
        assert!(agent_post.is_err());
    }

    #[test]
    fn a_held_letter_left_behind_is_cancelled() {
        let mut store = test_store("mail-held-left");
        save_ready(&mut store);
        Executor::new(&mut store)
            .execute(
                &ArmOutbound {
                    kind: "letter".into(),
                    anchor: None,
                    to_address: "marie@acme.test".into(),
                    subject: "Tenue".into(),
                    body: "Pas partie.".into(),
                    delay_secs: 0,
                    session_token: Some("fenetre".into()),
                    follow_subject: None,
                    follow_subject_id: None,
                    follow_cycle: None,
                    follow_step: None,
                    client_id: None,
                },
                &human(),
            )
            .unwrap();
        let now = OffsetDateTime::now_utc() + Duration::seconds(1);
        let batch = take_due(&mut store, &human(), "fenetre", now, now.date(), true)
            .unwrap()
            .expect("tenue");
        let id = batch.letters[0].id.clone();
        drop(batch);
        let pending = mail_tick(store.connection(), "fenetre", now, now.date(), true).unwrap();
        assert!(pending.send);
        abandon_held(&mut store, &human()).unwrap();
        let quiet = mail_tick(store.connection(), "fenetre", now, now.date(), true).unwrap();
        assert!(!quiet.send);
        let (status, error): (String, String) = store
            .connection()
            .query_row(
                "SELECT status, error FROM outbound_mail WHERE id = ?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "cancelled");
        assert_eq!(error, "restée");
        assert!(
            take_due(&mut store, &human(), "fenetre", now, now.date(), true)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn the_hourly_cap_stops_a_further_claim() {
        let mut store = test_store("mail-cap");
        save_ready(&mut store);
        let now = OffsetDateTime::now_utc();
        let stamp = super::store::stamp(now).unwrap();
        for index in 0..30 {
            store
                .connection_mut()
                .execute(
                    "INSERT INTO outbound_mail (
                        id, kind, status, from_address, to_address, subject, body, message_id,
                        send_after, created_at, updated_at
                     ) VALUES (?1, 'trial', 'sent', 'camille@studio.test', 'marie@acme.test',
                               'Déjà', 'corps', ?2, ?3, ?3, ?3)",
                    rusqlite::params![
                        format!("sent-{index}"),
                        format!("<sent-{index}@griffe.local>"),
                        stamp
                    ],
                )
                .unwrap();
        }
        Executor::new(&mut store)
            .execute(
                &ArmOutbound {
                    kind: "trial".into(),
                    anchor: None,
                    to_address: "marie@acme.test".into(),
                    subject: "Trop".into(),
                    body: "Encore.".into(),
                    delay_secs: 0,
                    session_token: None,
                    follow_subject: None,
                    follow_subject_id: None,
                    follow_cycle: None,
                    follow_step: None,
                    client_id: None,
                },
                &human(),
            )
            .unwrap();
        let batch = take_due(
            &mut store,
            &human(),
            "fenetre",
            now + Duration::seconds(1),
            now.date(),
            true,
        )
        .unwrap();
        assert!(batch.is_none());
    }

    #[test]
    fn a_refused_server_keeps_the_letter() {
        let mut store = test_store("mail-fail");
        save_ready(&mut store);
        Executor::new(&mut store)
            .execute(
                &ArmOutbound {
                    kind: "trial".into(),
                    anchor: None,
                    to_address: "camille@studio.test".into(),
                    subject: "Essai".into(),
                    body: "Une ligne.".into(),
                    delay_secs: 0,
                    session_token: None,
                    follow_subject: None,
                    follow_subject_id: None,
                    follow_cycle: None,
                    follow_step: None,
                    client_id: None,
                },
                &human(),
            )
            .unwrap();
        let now = OffsetDateTime::now_utc() + Duration::seconds(1);
        let batch = take_due(&mut store, &human(), "fenetre", now, now.date(), true)
            .unwrap()
            .unwrap();
        let id = batch.letters[0].id.clone();
        engage(&mut store, &id);
        let recorder = RecordingMail::failing(MailSubmitError::Auth);
        let outcomes = deliver_with(&recorder, batch);
        record_deliveries(&mut store, &human(), now.date(), &outcomes).unwrap();
        let error: String = store
            .connection()
            .query_row(
                "SELECT error FROM outbound_mail WHERE subject = 'Essai'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(error.contains("mot de passe d'application"));
    }

    #[test]
    fn clearing_the_secret_removes_it() {
        let mut store = test_store("mail-clear-secret");
        save_ready(&mut store);
        Executor::new(&mut store)
            .execute(
                &RecordMailProbe {
                    ok: true,
                    detail: PROBE_OK_SENTENCE.to_string(),
                },
                &human(),
            )
            .unwrap();
        Executor::new(&mut store)
            .execute(&ClearMailSecret, &human())
            .unwrap();
        let account = profile(store.connection()).unwrap();
        assert!(!account.has_secret);
        assert!(account.probe.is_none());
    }

    #[test]
    fn a_new_account_forgets_the_previous_probe_and_the_verdict_stays_short() {
        let mut store = test_store("mail-probe");
        save_ready(&mut store);
        let sentence = PROBE_OK_SENTENCE.to_string();
        Executor::new(&mut store)
            .execute(
                &RecordMailProbe {
                    ok: true,
                    detail: sentence.clone(),
                },
                &human(),
            )
            .unwrap();
        let account = profile(store.connection()).unwrap();
        let probe = account.probe.expect("verdict");
        assert!(probe.ok);
        assert_eq!(probe.detail, sentence);
        let entries = recent_audit_entries(store.connection(), 0, 20).unwrap();
        let recorded = entries
            .iter()
            .find(|entry| entry.command_name == "mail.record_probe")
            .unwrap();
        assert!(recorded.command_json.contains("identifiant est accepté"));
        assert!(!recorded.command_json.contains("mot-de-passe"));

        save_ready(&mut store);
        assert!(profile(store.connection()).unwrap().probe.is_none());

        let long = format!("x{}", "y".repeat(400));
        Executor::new(&mut store)
            .execute(
                &RecordMailProbe {
                    ok: false,
                    detail: format!("refus\n{long}"),
                },
                &human(),
            )
            .unwrap();
        let probe = profile(store.connection()).unwrap().probe.expect("refus");
        assert!(!probe.ok);
        assert_eq!(probe.detail.chars().count(), 160);
        assert!(!probe.detail.contains('\n'));
    }

    #[test]
    fn icloud_ignores_a_posted_copy_host() {
        let mut store = test_store("mail-icloud-copy");
        let Outcome::Applied(()) = Executor::new(&mut store)
            .execute(
                &SaveMailAccount {
                    from_name: None,
                    from_address: "camille@studio.test".into(),
                    host: "ailleurs.test".into(),
                    port: 25,
                    tls: "plain".into(),
                    username: "camille@icloud.test".into(),
                    preset: "icloud".into(),
                    imap_host: "evil.test".into(),
                    imap_port: 143,
                },
                &human(),
            )
            .unwrap()
        else {
            panic!("compte");
        };
        let account = profile(store.connection()).unwrap();
        assert!(account.imap_host.is_empty());
        let target = account.copy_target().expect("iCloud a une copie");
        assert_eq!(target.host, "imap.mail.me.com");
        assert_eq!(target.port, 993);
    }

    #[test]
    fn a_custom_copy_port_other_than_993_is_refused() {
        let mut store = test_store("mail-copy-port");
        let err = Executor::new(&mut store)
            .execute(
                &SaveMailAccount {
                    from_name: None,
                    from_address: "camille@studio.test".into(),
                    host: "smtp.exemple.test".into(),
                    port: 587,
                    tls: "starttls".into(),
                    username: "camille@studio.test".into(),
                    preset: "custom".into(),
                    imap_host: "imap.exemple.test".into(),
                    imap_port: 143,
                },
                &human(),
            )
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("993")));
    }

    fn new_client(store: &mut crate::store::Store, name: &str) -> crate::domain::ClientId {
        match Executor::new(store)
            .execute(
                &CreateClient {
                    name: name.to_string(),
                    siren: None,
                    vat_number: None,
                    address: None,
                },
                &human(),
            )
            .unwrap()
        {
            Outcome::Applied(id) => id,
            other => panic!("{other:?}"),
        }
    }

    fn new_contact(
        store: &mut crate::store::Store,
        client_id: crate::domain::ClientId,
        name: &str,
        email: &str,
    ) -> crate::domain::ContactId {
        match Executor::new(store)
            .execute(
                &CreateContact {
                    client_id,
                    name: name.to_string(),
                    email: Some(email.to_string()),
                    phone: None,
                    role: None,
                },
                &human(),
            )
            .unwrap()
        {
            Outcome::Applied(id) => id,
            other => panic!("{other:?}"),
        }
    }

    fn arm_fiche(
        store: &mut crate::store::Store,
        client_id: crate::domain::ClientId,
        to: &str,
        subject: &str,
    ) -> String {
        match Executor::new(store)
            .execute(
                &ArmOutbound {
                    kind: "letter".into(),
                    anchor: Some(subject.to_string()),
                    to_address: to.to_string(),
                    subject: subject.to_string(),
                    body: "Une ligne.".into(),
                    delay_secs: 0,
                    session_token: None,
                    follow_subject: None,
                    follow_subject_id: None,
                    follow_cycle: None,
                    follow_step: None,
                    client_id: Some(client_id.to_string()),
                },
                &human(),
            )
            .unwrap()
        {
            Outcome::Applied(id) => id,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_claim_uses_the_correspondent_after_the_fiche_changes() {
        let mut store = test_store("mail-fiche-live");
        save_ready(&mut store);
        let client_id = new_client(&mut store, "Atelier Nord");
        let contact_id = new_contact(&mut store, client_id, "Marie", "marie@exemple.fr");
        new_contact(&mut store, client_id, "Aaron", "aaron@exemple.fr");
        Executor::new(&mut store)
            .execute(
                &UpdateContact {
                    id: contact_id,
                    revision: 1,
                    name: "Aline".into(),
                    email: Some("aline@exemple.fr".into()),
                    phone: None,
                    role: None,
                },
                &human(),
            )
            .unwrap();
        arm_fiche(&mut store, client_id, "marie@exemple.fr", "Bonjour");
        let now = OffsetDateTime::now_utc() + Duration::seconds(1);
        let batch = take_due(&mut store, &human(), "fenetre", now, now.date(), true)
            .unwrap()
            .expect("réclamée");
        assert_eq!(batch.letters[0].message.to_address, "aline@exemple.fr");
    }

    #[test]
    fn a_missing_fiche_address_fails_the_letter_and_a_retry_reads_it_again() {
        let mut store = test_store("mail-fiche-missing");
        save_ready(&mut store);
        let client_id = new_client(&mut store, "Atelier Sud");
        let contact_id = new_contact(&mut store, client_id, "Marie", "marie@exemple.fr");
        Executor::new(&mut store)
            .execute(
                &UpdateContact {
                    id: contact_id,
                    revision: 1,
                    name: "Marie".into(),
                    email: Some("pas une adresse".into()),
                    phone: None,
                    role: None,
                },
                &human(),
            )
            .unwrap();
        let armed = arm_fiche(&mut store, client_id, "oublie@exemple.fr", "Bonjour");
        let now = OffsetDateTime::now_utc() + Duration::seconds(1);
        assert!(
            take_due(&mut store, &human(), "fenetre", now, now.date(), true)
                .unwrap()
                .is_none()
        );
        let (status, to_address, error): (String, String, String) = store
            .connection()
            .query_row(
                "SELECT status, to_address, error FROM outbound_mail WHERE id = ?1",
                [armed.clone()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(status, "failed");
        assert_eq!(to_address, "oublie@exemple.fr");
        assert_eq!(error, MISSING_ADDRESS);
        let refused = Executor::new(&mut store)
            .execute(
                &RetryOutbound {
                    id: armed.clone(),
                    session_token: None,
                },
                &human(),
            )
            .unwrap_err();
        assert!(matches!(
            refused,
            AppError::Domain(message) if message == MailError::MissingAddress.to_string()
        ));
        let armed_count: i64 = store
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM outbound_mail WHERE status = 'armed'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(armed_count, 0);

        Executor::new(&mut store)
            .execute(
                &UpdateContact {
                    id: contact_id,
                    revision: 2,
                    name: "Marie".into(),
                    email: Some("aline@exemple.fr".into()),
                    phone: None,
                    role: None,
                },
                &human(),
            )
            .unwrap();
        Executor::new(&mut store)
            .execute(
                &RetryOutbound {
                    id: armed,
                    session_token: None,
                },
                &human(),
            )
            .unwrap();
        let fresh: String = store
            .connection()
            .query_row(
                "SELECT to_address FROM outbound_mail WHERE status = 'armed'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(fresh, "aline@exemple.fr");
    }

    #[test]
    fn deleting_the_correspondent_does_not_send_to_someone_else() {
        let mut store = test_store("mail-fiche-deleted");
        save_ready(&mut store);
        let client_id = new_client(&mut store, "Atelier Ouest");
        let contact_id = new_contact(&mut store, client_id, "Marie", "marie@exemple.fr");
        new_contact(&mut store, client_id, "Aaron", "aaron@exemple.fr");
        Executor::new(&mut store)
            .execute(
                &DeleteContact {
                    id: contact_id,
                    revision: 1,
                },
                &human(),
            )
            .unwrap();
        arm_fiche(&mut store, client_id, "marie@exemple.fr", "Orpheline");
        let now = OffsetDateTime::now_utc() + Duration::seconds(1);
        assert!(
            take_due(&mut store, &human(), "fenetre", now, now.date(), true)
                .unwrap()
                .is_none()
        );
        let error: String = store
            .connection()
            .query_row(
                "SELECT error FROM outbound_mail WHERE subject = 'Orpheline'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(error, MISSING_ADDRESS);
    }

    #[test]
    fn a_failed_copy_leaves_the_letter_sent_and_waits_a_minute() {
        let mut store = test_store("mail-copy-retry");
        save_ready(&mut store);
        Executor::new(&mut store)
            .execute(
                &ArmOutbound {
                    kind: "trial".into(),
                    anchor: None,
                    to_address: "camille@studio.test".into(),
                    subject: "Essai".into(),
                    body: "Une ligne.".into(),
                    delay_secs: 0,
                    session_token: None,
                    follow_subject: None,
                    follow_subject_id: None,
                    follow_cycle: None,
                    follow_step: None,
                    client_id: None,
                },
                &human(),
            )
            .unwrap();
        let now = OffsetDateTime::now_utc() + Duration::seconds(1);
        let batch = take_due(&mut store, &human(), "fenetre", now, now.date(), true)
            .unwrap()
            .expect("réclamée");
        let id = batch.letters[0].id.clone();
        drop(batch);
        engage(&mut store, &id);
        let outcomes = vec![DeliveryOutcome {
            id,
            result: Ok(SubmissionReceipt {
                message_id: "<essai@griffe.local>".into(),
                smtp_response: "250".into(),
            }),
            copy: SentCopyStatus::Failed("La copie n'a pas pu être déposée.".into()),
        }];
        record_deliveries(&mut store, &human(), now.date(), &outcomes).unwrap();
        let (status, sent_copy): (String, String) = store
            .connection()
            .query_row(
                "SELECT status, sent_copy FROM outbound_mail WHERE subject = 'Essai'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "sent");
        assert_eq!(sent_copy, "failed");
        let tick = mail_tick(store.connection(), "fenetre", now, now.date(), true).unwrap();
        assert!(!tick.copy);
        assert!(take_copies(&mut store, &human(), now).unwrap().is_none());
        let later = now + Duration::seconds(60);
        let due = mail_tick(store.connection(), "fenetre", later, later.date(), true).unwrap();
        assert!(due.copy);
        assert!(!due.send);
        let copies = take_copies(&mut store, &human(), later)
            .unwrap()
            .expect("copie à retenter");
        assert_eq!(copies.letters.len(), 1);
        assert_eq!(copies.letters[0].message.to_address, "camille@studio.test");
    }

    #[test]
    fn a_copy_without_a_date_or_without_a_host_is_not_retried() {
        let mut store = test_store("mail-copy-quiet");
        save_ready(&mut store);
        let now = OffsetDateTime::now_utc();
        let stamp = super::store::stamp(now).unwrap();
        insert_sent(&mut store, "historique", &stamp, None, None);
        let quiet = mail_tick(store.connection(), "fenetre", now, now.date(), true).unwrap();
        assert!(!quiet.copy);
        assert!(!quiet.send);

        insert_sent(&mut store, "immediate", &stamp, Some(&stamp), None);
        let soon = mail_tick(store.connection(), "fenetre", now, now.date(), true).unwrap();
        assert!(soon.copy);
        assert!(!soon.send);

        let mut custom = test_store("mail-copy-no-host");
        let Outcome::Applied(()) = Executor::new(&mut custom)
            .execute(
                &SaveMailAccount {
                    from_name: None,
                    from_address: "camille@studio.test".into(),
                    host: "smtp.exemple.test".into(),
                    port: 587,
                    tls: "starttls".into(),
                    username: "camille@studio.test".into(),
                    preset: "custom".into(),
                    imap_host: String::new(),
                    imap_port: 993,
                },
                &human(),
            )
            .unwrap()
        else {
            panic!("compte");
        };
        insert_sent(&mut custom, "sans-hote", &stamp, Some(&stamp), None);
        let nowhere = mail_tick(custom.connection(), "fenetre", now, now.date(), true).unwrap();
        assert!(!nowhere.copy);
    }

    fn insert_sent(
        store: &mut crate::store::Store,
        id: &str,
        when: &str,
        copy_at: Option<&str>,
        sent_copy: Option<&str>,
    ) {
        store
            .connection_mut()
            .execute(
                "INSERT INTO outbound_mail (
                    id, kind, status, from_address, to_address, subject, body, message_id,
                    send_after, created_at, updated_at, copy_at, sent_copy
                 ) VALUES (?1, 'trial', 'sent', 'camille@studio.test', 'marie@acme.test',
                           'Déjà', 'corps', ?2, ?3, ?3, ?3, ?4, ?5)",
                rusqlite::params![id, format!("<{id}@griffe.local>"), when, copy_at, sent_copy],
            )
            .unwrap();
    }
}
