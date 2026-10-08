//! Courrier sortant. Le cœur décide quand une lettre part et ce qu'elle dit.
//! La crate qui parle SMTP implémente [`OutboundMail`] et reste hors d'ici.

mod commands;
mod error;
mod model;
mod secret;
mod store;

pub use commands::{
    ArmOutbound, CancelOutbound, ClearMailSecret, ResolveUncertain, RetryOutbound, SaveMailAccount,
    SaveMailSecret, SetAutomaticSend,
};
pub use error::MailError;
pub use model::{
    DeliveryOutcome, HOURLY_CAP, ICLOUD_HOST, MailPreset, MailProfile, MailSubmitError,
    OutboundMail, OutboundMessage, OutboundStatus, OutboundView, RecordingMail, SmtpEndpoint,
    SubmissionBatch, SubmissionReceipt, TlsMode, UNDO_SECS,
};
pub use secret::MailSecret;

/// Compte d'envoi, sans le secret.
///
/// # Errors
///
/// Lecture impossible.
pub fn profile(conn: &rusqlite::Connection) -> Result<MailProfile, AppError> {
    store::profile(conn)
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
    }
    Ok(())
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
    store::needs_tick(conn, session_token, now, today, release_stale)
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
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use time::{Date, Duration, Month, OffsetDateTime, Time};

    use super::{
        ArmOutbound, ClearMailSecret, MailError, MailSubmitError, RecordingMail, SaveMailAccount,
        SaveMailSecret, SetAutomaticSend, deliver_with, profile, record_deliveries, take_due,
    };
    use crate::app::{Actor, ExecutionContext, Executor, Outcome, recent_audit_entries};
    use crate::clients::{CreateClient, CreateContact};
    use crate::domain::{ClientId, FollowUpSubject, Money, Probability};
    use crate::mail::MailSecret;
    use crate::prospection::CreateOpportunity;
    use crate::store::testing::test_store;

    fn human() -> ExecutionContext {
        ExecutionContext::new(Actor::Human, false)
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
            .expect("réclamée");
        assert_eq!(batch.letters.len(), 1);
        let cancel =
            Executor::new(&mut store).execute(&super::CancelOutbound { id: id.clone() }, &human());
        assert!(matches!(
            cancel,
            Err(crate::app::AppError::Domain(message)) if message == MailError::NotArmed.to_string()
        ));
        let recorder = RecordingMail::default();
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
                },
                &human(),
            )
            .unwrap();
        let batch = take_due(&mut store, &human(), "nouvelle", now, now.date(), true)
            .unwrap()
            .expect("vivante");
        assert_eq!(batch.letters[0].message.subject, "Vive");
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
        assert_eq!(status, "uncertain");
        let agent_post = take_due(&mut store, &agent(), "nouvelle", now, now.date(), true);
        assert!(agent_post.is_err());
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
                },
                &human(),
            )
            .unwrap();
        let now = OffsetDateTime::now_utc() + Duration::seconds(1);
        let batch = take_due(&mut store, &human(), "fenetre", now, now.date(), true)
            .unwrap()
            .unwrap();
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
            .execute(&ClearMailSecret, &human())
            .unwrap();
        assert!(!profile(store.connection()).unwrap().has_secret);
    }
}
