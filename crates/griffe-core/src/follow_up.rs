//! Relances de prospection et d'impayés : journal de faits, file dérivée, brouillons `.eml`.
//!
//! [`PrepareFollowUp`] rend les octets RFC 5322 ; l'adaptateur écrit le fichier et l'ouvre
//! (`xdg-open`). Poster la lettre passe par le courrier (`crate::mail`). « Envoyé » peut
//! aussi être un fait marqué par l'humain.

mod commands;
mod error;
mod queries;
mod row;

pub use commands::{
    ArrangeProspectPhrases, CreateProspectGenre, DropProspectGenre, GenreAssignment,
    GenreWordDraft, KeepGenreWords, KeptWords, MarkFollowUpSent, MomentDraft, PhraseRewrite,
    PrepareFollowUp, PreparedFollowUp, RetractLastFollowUp, RewriteGenreWords,
    RewriteProspectPhrases, SaveGenreLetter, SetDossierGenre, SetFollowUpDate, SetFollowUpSender,
    SkipFollowUpStep, SnoozeFollowUp,
};
pub use error::FollowUpError;
pub use queries::{
    CardStatus, FollowUpCard, HistoryItem, card_for, events_for, follow_up_board, follow_up_queue,
    follow_up_sender, latest_prospect_genre, letter_speaker, phrases_for_genre, prospect_genre_for,
    prospect_genres, prospect_phrases,
};
pub use row::{ProspectGenre, ProspectPhrase};

/// Pose la reprise. Les lettres d'avant restent, le compteur repart.
///
/// Si une série a déjà été copiée, cette reprise entre sur la série vivante
/// et en garde une copie. Avant le premier changement de structure, elle lit
/// la série vivante, comme tout le monde.
///
/// # Errors
pub(crate) fn open_cycle(
    conn: &rusqlite::Connection,
    id: crate::domain::OpportunityId,
    today: time::Date,
) -> Result<(), crate::app::AppError> {
    use crate::domain::{FollowUpEvent, FollowUpEventId, FollowUpFact, FollowUpSubject};

    let events = row::events_for(conn, crate::domain::FollowUpSubject::Opportunity(id))?;
    let event = FollowUpEvent {
        id: FollowUpEventId::new(),
        subject: FollowUpSubject::Opportunity(id),
        fact: FollowUpFact::CycleOpened,
        at: commands::stamp_after(today, &events),
        until: None,
        rendered_subject: None,
        rendered_body: None,
        retracts: None,
        interaction_id: None,
    };
    row::insert_event(conn, &event)?;
    if row::any_copied_series(conn)? {
        let living = row::list_phrases(conn)?;
        row::copy_series(conn, id, &event.id.to_string(), &living)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use time::{Date, Month};

    use super::*;
    use crate::app::{Actor, AppError, Command, ExecutionContext, Executor, Outcome};
    use crate::clients::{CreateClient, CreateContact};
    use crate::day::{GestureSource, GestureVerb, day_gestures};
    use crate::domain::{
        ClientId, FollowUpFact, FollowUpKind, FollowUpSubject, InteractionKind, LossReason, Money,
        PROSPECT_CADENCE, Probability, VatRate,
    };
    use crate::people::{HistoryKind, person};
    use crate::prospection::{
        CreateOpportunity, LoseOpportunity, ReopenOpportunity, list_interactions, opportunity_by_id,
    };
    use crate::store::Store;
    use crate::store::testing::test_store;

    fn reapply_signature_migration(conn: &rusqlite::Connection) {
        let sql = include_str!("store/migrations/0046_mail_signature_up.sql");
        for chunk in sql.split(';') {
            let code = chunk
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with("--"))
                .collect::<Vec<_>>()
                .join("\n");
            if code.starts_with("UPDATE") {
                conn.execute_batch(&code).unwrap();
            }
        }
    }

    fn date(year: i32, month: Month, day: u8) -> Date {
        Date::from_calendar_date(year, month, day).unwrap()
    }

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

    fn seed_opportunity(store: &mut Store, next: Date) -> (ClientId, FollowUpSubject) {
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
            other => panic!("expected Applied, got {other:?}"),
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
        let oid = match Executor::new(store)
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
            Outcome::Applied(id) => id,
            other => panic!("expected Applied, got {other:?}"),
        };
        (client_id, FollowUpSubject::Opportunity(oid))
    }

    fn set_sender(store: &mut Store) {
        let Outcome::Applied(()) = Executor::new(store)
            .execute(
                &SetFollowUpSender {
                    email: "nicolas@lumen.test".into(),
                    name: Some("Nicolas".into()),
                },
                &human(),
            )
            .unwrap()
        else {
            panic!("expected Applied");
        };
    }

    #[test]
    fn an_open_opportunity_is_already_in_the_queue_on_its_next_action_date() {
        let mut store = test_store("enrolled");
        let today = date(2026, Month::September, 5);
        let (_, subject) = seed_opportunity(&mut store, today);
        let queue = follow_up_queue(store.connection(), today).unwrap();
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].subject, subject);
        assert_eq!(queue[0].kind, FollowUpKind::Prospect);
        assert_eq!(queue[0].step_key.as_deref(), Some("hello"));
        assert_eq!(queue[0].status, CardStatus::Blocked);
    }

    #[test]
    fn preparing_a_draft_returns_rfc5322_and_marks_the_card_drafted() {
        let mut store = test_store("draft");
        let today = date(2026, Month::September, 5);
        let (_, subject) = seed_opportunity(&mut store, today);
        set_sender(&mut store);
        let Outcome::Applied(prepared) = Executor::new(&mut store)
            .execute(
                &PrepareFollowUp {
                    subject,
                    today,
                    subject_line: None,
                    body: None,
                },
                &human(),
            )
            .unwrap()
        else {
            panic!("expected Applied");
        };
        assert!(prepared.draft.rfc5322.contains("X-Unsent: 1"));
        assert!(prepared.draft.rfc5322.contains("marie@acme.test"));
        assert!(prepared.to_email.contains("marie@acme.test"));
        let card = card_for(store.connection(), subject, today).unwrap();
        assert_eq!(card.status, CardStatus::Drafted);
    }

    #[test]
    fn marking_sent_advances_next_action_at_and_logs_an_email() {
        let mut store = test_store("sent");
        let today = date(2026, Month::September, 5);
        let (_, subject) = seed_opportunity(&mut store, today);
        set_sender(&mut store);
        Executor::new(&mut store)
            .execute(
                &PrepareFollowUp {
                    subject,
                    today,
                    subject_line: None,
                    body: None,
                },
                &human(),
            )
            .unwrap();
        let Outcome::Applied(card) = Executor::new(&mut store)
            .execute(
                &MarkFollowUpSent {
                    subject,
                    today,
                    subject_line: None,
                    body: None,
                },
                &human(),
            )
            .unwrap()
        else {
            panic!("expected Applied");
        };
        assert_eq!(card.step_key.as_deref(), Some("bump"));
        assert_eq!(card.due_on, Some(date(2026, Month::September, 8)));
        let FollowUpSubject::Opportunity(oid) = subject else {
            panic!("opportunity");
        };
        let opp = opportunity_by_id(store.connection(), oid).unwrap().unwrap();
        assert_eq!(opp.next_action_at, Some(date(2026, Month::September, 8)));
        let interactions = list_interactions(store.connection(), oid).unwrap();
        assert_eq!(interactions.len(), 1);
        assert_eq!(interactions[0].kind, InteractionKind::Email);
    }

    #[test]
    fn a_fresh_vault_seeds_the_four_prospect_phrases() {
        let store = test_store("seed-phrases");
        let phrases = prospect_phrases(store.connection()).unwrap();
        assert_eq!(phrases.len(), PROSPECT_CADENCE.len());
        for (phrase, step) in phrases.iter().zip(PROSPECT_CADENCE) {
            assert_eq!(phrase.key, step.key);
            assert_eq!(phrase.label, step.label);
            assert_eq!(phrase.offset_days, step.offset_days);
            assert_eq!(phrase.subject, step.subject);
            assert_eq!(phrase.body, step.body);
            assert_eq!(phrase.revision, 1);
        }
    }

    #[test]
    fn the_signature_migration_rewrites_only_the_stock_closing() {
        let store = test_store("signature-migration");
        let phrases = prospect_phrases(store.connection()).unwrap();
        assert!(
            phrases
                .iter()
                .all(|phrase| phrase.body.contains("{{signature}}"))
        );
        assert!(
            phrases
                .iter()
                .all(|phrase| !phrase.body.contains("Bien à vous"))
        );

        let old_two = "Bonjour,\n\nBien à vous,\n{{moi}}\n";
        let old_three = "Bonjour,\n\nBien à vous,\n{{moi}}\n{{societe}}\n";
        let custom = "Bonjour,\n\nBien à vous,\n{{moi}}\nle studio\n";
        let conn = store.connection();
        conn.execute(
            "UPDATE prospect_phrases SET body = ?1 WHERE id = 'bump'",
            [old_two],
        )
        .unwrap();
        conn.execute(
            "UPDATE prospect_phrases SET body = ?1 WHERE id = 'close'",
            [custom],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO prospect_genres (id, name, touched_at)
             VALUES ('g1', 'Ateliers', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO prospect_genre_words (genre_id, phrase_key, subject, body)
             VALUES ('g1', 'hello', '{{sujet}}', ?1)",
            [old_three],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO prospect_series (id, opportunity_id, cycle_key) VALUES ('s1', 'opp', '')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO prospect_series_steps (
                series_id, position, phrase_key, label, offset_days, subject, body
             ) VALUES ('s1', 0, 'hello', 'Premier message', 0, '{{sujet}}', ?1)",
            [old_three],
        )
        .unwrap();

        reapply_signature_migration(conn);
        reapply_signature_migration(conn);

        let body = |sql: &str| -> String { conn.query_row(sql, [], |row| row.get(0)).unwrap() };
        assert_eq!(
            body("SELECT body FROM prospect_phrases WHERE id = 'bump'"),
            "Bonjour,\n\n{{signature}}\n"
        );
        assert_eq!(
            body("SELECT body FROM prospect_phrases WHERE id = 'close'"),
            custom
        );
        assert_eq!(
            body("SELECT body FROM prospect_genre_words WHERE genre_id = 'g1'"),
            "Bonjour,\n\n{{signature}}\n"
        );
        assert_eq!(
            body("SELECT body FROM prospect_series_steps WHERE series_id = 's1'"),
            old_three
        );
        let hello = body("SELECT body FROM prospect_phrases WHERE id = 'hello'");
        assert_eq!(hello.matches("{{signature}}").count(), 1);
    }

    #[test]
    fn rewriting_a_phrase_changes_the_next_unopened_letter_only() {
        let mut store = test_store("rewrite-phrase");
        let today = date(2026, Month::September, 5);
        let (_, filed_subject) = seed_opportunity(&mut store, today);
        let (_, drafted_subject) = seed_opportunity(&mut store, today);
        let (_, fresh_subject) = seed_opportunity(&mut store, today);
        set_sender(&mut store);
        Executor::new(&mut store)
            .execute(
                &PrepareFollowUp {
                    subject: filed_subject,
                    today,
                    subject_line: None,
                    body: None,
                },
                &human(),
            )
            .unwrap();
        Executor::new(&mut store)
            .execute(
                &MarkFollowUpSent {
                    subject: filed_subject,
                    today,
                    subject_line: None,
                    body: None,
                },
                &human(),
            )
            .unwrap();
        let filed_before = events_for(store.connection(), filed_subject)
            .unwrap()
            .into_iter()
            .find(|event| event.fact == FollowUpFact::MarkedSent)
            .unwrap()
            .rendered_body
            .unwrap();
        assert!(filed_before.contains("Je me permets de revenir"));
        Executor::new(&mut store)
            .execute(
                &PrepareFollowUp {
                    subject: drafted_subject,
                    today,
                    subject_line: None,
                    body: None,
                },
                &human(),
            )
            .unwrap();
        let hello = prospect_phrases(store.connection())
            .unwrap()
            .into_iter()
            .find(|phrase| phrase.key == "hello")
            .unwrap();
        let Outcome::Applied(_) = Executor::new(&mut store)
            .execute(
                &RewriteProspectPhrases {
                    phrases: vec![PhraseRewrite {
                        key: hello.key,
                        label: hello.label,
                        subject: hello.subject,
                        body: "Bonjour {{prenom}},\n\nUn premier mot réécrit.\n".into(),
                        revision: hello.revision,
                    }],
                },
                &human(),
            )
            .unwrap()
        else {
            panic!("expected Applied");
        };
        let filed_after = events_for(store.connection(), filed_subject)
            .unwrap()
            .into_iter()
            .find(|event| event.fact == FollowUpFact::MarkedSent)
            .unwrap()
            .rendered_body
            .unwrap();
        assert_eq!(filed_after, filed_before);
        let drafted = card_for(store.connection(), drafted_subject, today).unwrap();
        let drafted_body = drafted.preview_body.unwrap();
        assert!(drafted_body.contains("Je me permets de revenir"));
        assert!(!drafted_body.contains("réécrit"));
        let fresh = card_for(store.connection(), fresh_subject, today).unwrap();
        let fresh_body = fresh.preview_body.unwrap();
        assert!(fresh_body.contains("Un premier mot réécrit"));
        assert!(!fresh_body.contains("Je me permets de revenir"));
    }

    fn hello_phrase(store: &Store) -> ProspectPhrase {
        prospect_phrases(store.connection())
            .unwrap()
            .into_iter()
            .find(|phrase| phrase.key == "hello")
            .unwrap()
    }

    fn rewrite_hello(
        store: &mut Store,
        hello: &ProspectPhrase,
        label: &str,
        body: &str,
        revision: i64,
    ) -> Result<Outcome<Vec<ProspectPhrase>>, AppError> {
        Executor::new(store).execute(
            &RewriteProspectPhrases {
                phrases: vec![PhraseRewrite {
                    key: hello.key.clone(),
                    label: label.to_string(),
                    subject: hello.subject.clone(),
                    body: body.to_string(),
                    revision,
                }],
            },
            &human(),
        )
    }

    #[test]
    fn a_stale_revision_is_refused_unless_the_words_are_the_same() {
        let mut store = test_store("stale-phrase");
        let hello = hello_phrase(&store);
        let stale = rewrite_hello(&mut store, &hello, "Autre nom", &hello.body, 0).unwrap_err();
        assert!(matches!(
            stale,
            AppError::Domain(msg) if msg == "Ces phrases ont changé entre-temps. Relis avant d'enregistrer."
        ));
        assert_eq!(hello_phrase(&store).revision, 1);

        let Outcome::Applied(_) =
            rewrite_hello(&mut store, &hello, &hello.label, &hello.body, 99).unwrap()
        else {
            panic!("expected Applied");
        };
        assert_eq!(hello_phrase(&store).revision, 1);

        let body = "Un mot.\n";
        let Outcome::Applied(_) =
            rewrite_hello(&mut store, &hello, "Premier contact", body, hello.revision).unwrap()
        else {
            panic!("expected Applied");
        };
        let rewritten = hello_phrase(&store);
        assert_eq!(rewritten.revision, 2);
        assert_eq!(rewritten.label, "Premier contact");
        assert_eq!(rewritten.body, body);

        let Outcome::Applied(_) =
            rewrite_hello(&mut store, &rewritten, "Premier contact", body, 1).unwrap()
        else {
            panic!("expected Applied");
        };
        assert_eq!(hello_phrase(&store).revision, 2);

        let empty = rewrite_hello(&mut store, &rewritten, "  ", body, 2).unwrap_err();
        assert!(matches!(
            empty,
            AppError::Domain(msg) if msg == "Ce moment n'a pas de nom."
        ));
        let unknown = Executor::new(&mut store)
            .execute(
                &RewriteProspectPhrases {
                    phrases: vec![PhraseRewrite {
                        key: "absent".into(),
                        label: "Nom".into(),
                        subject: "Sujet".into(),
                        body: "Lettre".into(),
                        revision: 1,
                    }],
                },
                &human(),
            )
            .unwrap_err();
        assert!(matches!(
            unknown,
            AppError::Domain(msg) if msg == "Ce moment n'existe pas."
        ));
    }

    #[test]
    fn preparing_a_draft_with_a_free_body_stores_that_text_not_the_template() {
        let mut store = test_store("free-body");
        let today = date(2026, Month::September, 5);
        let (_, subject) = seed_opportunity(&mut store, today);
        set_sender(&mut store);
        let body = "Camille,\n\nTrois phrases qui ne sont pas le modèle.\n\nNicolas\n";
        let Outcome::Applied(prepared) = Executor::new(&mut store)
            .execute(
                &PrepareFollowUp {
                    subject,
                    today,
                    subject_line: Some("Au sujet de la refonte".into()),
                    body: Some(body.into()),
                },
                &human(),
            )
            .unwrap()
        else {
            panic!("expected Applied");
        };
        assert!(
            prepared
                .draft
                .rfc5322
                .contains("Trois phrases qui ne sont pas le modèle")
        );
        assert!(!prepared.draft.rfc5322.contains("Je me permets de revenir"));
        assert_eq!(prepared.subject_line, "Au sujet de la refonte");
        let events = events_for(store.connection(), subject).unwrap();
        let draft = events
            .iter()
            .find(|e| e.fact == FollowUpFact::DraftPrepared)
            .unwrap();
        assert_eq!(draft.rendered_body.as_deref(), Some(body));
        assert_eq!(
            draft.rendered_subject.as_deref(),
            Some("Au sujet de la refonte")
        );
    }

    #[test]
    fn marking_sent_copies_the_last_draft_as_the_carbon_copy() {
        let mut store = test_store("carbon");
        let today = date(2026, Month::September, 5);
        let (client_id, subject) = seed_opportunity(&mut store, today);
        set_sender(&mut store);
        let body = "Camille,\n\nC'est le double.\n";
        Executor::new(&mut store)
            .execute(
                &PrepareFollowUp {
                    subject,
                    today,
                    subject_line: Some("Refonte".into()),
                    body: Some(body.into()),
                },
                &human(),
            )
            .unwrap();
        Executor::new(&mut store)
            .execute(
                &MarkFollowUpSent {
                    subject,
                    today,
                    subject_line: None,
                    body: None,
                },
                &human(),
            )
            .unwrap();
        let events = events_for(store.connection(), subject).unwrap();
        let sent = events
            .iter()
            .find(|e| e.fact == FollowUpFact::MarkedSent)
            .unwrap();
        assert_eq!(sent.rendered_body.as_deref(), Some(body));
        assert_eq!(sent.rendered_subject.as_deref(), Some("Refonte"));
        let dossier = person(store.connection(), &client_id.to_string(), today).unwrap();
        assert!(
            dossier.history.iter().any(|h| matches!(
                &h.kind,
                HistoryKind::Letter { subject, body: b }
                    if subject == "Refonte" && b == body
            )),
            "{:?}",
            dossier.history
        );
    }

    #[test]
    fn marking_sent_can_rectify_the_letter() {
        let mut store = test_store("rectify");
        let today = date(2026, Month::September, 5);
        let (_, subject) = seed_opportunity(&mut store, today);
        set_sender(&mut store);
        Executor::new(&mut store)
            .execute(
                &PrepareFollowUp {
                    subject,
                    today,
                    subject_line: Some("Brouillon".into()),
                    body: Some("texte du brouillon".into()),
                },
                &human(),
            )
            .unwrap();
        Executor::new(&mut store)
            .execute(
                &MarkFollowUpSent {
                    subject,
                    today,
                    subject_line: Some("Parti".into()),
                    body: Some("texte vraiment envoyé".into()),
                },
                &human(),
            )
            .unwrap();
        let events = events_for(store.connection(), subject).unwrap();
        let sent = events
            .iter()
            .find(|e| e.fact == FollowUpFact::MarkedSent)
            .unwrap();
        assert_eq!(sent.rendered_subject.as_deref(), Some("Parti"));
        assert_eq!(sent.rendered_body.as_deref(), Some("texte vraiment envoyé"));
    }

    #[test]
    fn retracting_a_send_hides_the_carbon_copy() {
        let mut store = test_store("retract-letter");
        let today = date(2026, Month::September, 5);
        let (client_id, subject) = seed_opportunity(&mut store, today);
        set_sender(&mut store);
        Executor::new(&mut store)
            .execute(
                &PrepareFollowUp {
                    subject,
                    today,
                    subject_line: Some("Refonte".into()),
                    body: Some("un double".into()),
                },
                &human(),
            )
            .unwrap();
        Executor::new(&mut store)
            .execute(
                &MarkFollowUpSent {
                    subject,
                    today,
                    subject_line: None,
                    body: None,
                },
                &human(),
            )
            .unwrap();
        // Le brouillon est horodaté `now`, l'envoi à midi de `today` : le dernier
        // fait est le brouillon. On rétracte les deux pour viser l'envoi.
        Executor::new(&mut store)
            .execute(&RetractLastFollowUp { subject, today }, &human())
            .unwrap();
        Executor::new(&mut store)
            .execute(&RetractLastFollowUp { subject, today }, &human())
            .unwrap();
        let dossier = person(store.connection(), &client_id.to_string(), today).unwrap();
        assert!(
            !dossier
                .history
                .iter()
                .any(|h| matches!(h.kind, HistoryKind::Letter { .. })),
            "{:?}",
            dossier.history
        );
    }

    #[test]
    fn an_agent_cannot_mark_sent_without_confirmation() {
        let mut store = test_store("agent-sent");
        let today = date(2026, Month::September, 5);
        let (_, subject) = seed_opportunity(&mut store, today);
        set_sender(&mut store);
        let outcome = Executor::new(&mut store)
            .execute(
                &MarkFollowUpSent {
                    subject,
                    today,
                    subject_line: None,
                    body: None,
                },
                &agent(),
            )
            .unwrap();
        assert!(matches!(outcome, Outcome::PendingConfirmation(_)));
    }

    #[test]
    fn snooze_keeps_the_step_and_moves_the_date() {
        let mut store = test_store("snooze");
        let today = date(2026, Month::September, 5);
        let (_, subject) = seed_opportunity(&mut store, today);
        set_sender(&mut store);
        let until = date(2026, Month::September, 14);
        let Outcome::Applied(card) = Executor::new(&mut store)
            .execute(
                &SnoozeFollowUp {
                    subject,
                    until,
                    today,
                },
                &human(),
            )
            .unwrap()
        else {
            panic!("expected Applied");
        };
        assert_eq!(card.step_key.as_deref(), Some("hello"));
        assert_eq!(card.due_on, Some(until));
        assert_eq!(card.status, CardStatus::Later);
        let queue = follow_up_queue(store.connection(), today).unwrap();
        assert!(queue.is_empty());
    }

    #[test]
    fn retract_undoes_the_last_send() {
        let mut store = test_store("retract");
        let today = date(2026, Month::September, 5);
        let (_, subject) = seed_opportunity(&mut store, today);
        set_sender(&mut store);
        Executor::new(&mut store)
            .execute(
                &MarkFollowUpSent {
                    subject,
                    today,
                    subject_line: None,
                    body: None,
                },
                &human(),
            )
            .unwrap();
        let Outcome::Applied(card) = Executor::new(&mut store)
            .execute(&RetractLastFollowUp { subject, today }, &human())
            .unwrap()
        else {
            panic!("expected Applied");
        };
        assert_eq!(card.step_key.as_deref(), Some("hello"));
        let FollowUpSubject::Opportunity(oid) = subject else {
            panic!("opportunity");
        };
        assert!(
            list_interactions(store.connection(), oid)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn prepare_without_sender_is_refused() {
        let mut store = test_store("no-sender");
        let today = date(2026, Month::September, 5);
        let (_, subject) = seed_opportunity(&mut store, today);
        let err = Executor::new(&mut store)
            .execute(
                &PrepareFollowUp {
                    subject,
                    today,
                    subject_line: None,
                    body: None,
                },
                &human(),
            )
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("email avec lequel")));
    }

    #[test]
    fn an_unpaid_invoice_due_today_joins_the_queue() {
        let mut store = test_store("invoice");
        let today = date(2026, Month::September, 5);
        let (client_id, _) = seed_opportunity(&mut store, date(2026, Month::October, 1));
        let emitted = match Executor::new(&mut store)
            .execute(
                &crate::billing::EmitInvoice {
                    client_id,
                    mission_id: None,
                    lines: vec![crate::domain::InvoiceLine {
                        description: "Mission".into(),
                        quantity: 1.0,
                        unit_price: Money::from_cents(120_000),
                        vat_rate: VatRate::Standard,
                    }],
                    issued_on: date(2026, Month::August, 6),
                    payment_terms_days: 30,
                },
                &human(),
            )
            .unwrap()
        {
            Outcome::Applied(e) => e,
            other => panic!("expected Applied, got {other:?}"),
        };
        let queue = follow_up_queue(store.connection(), today).unwrap();
        assert!(
            queue
                .iter()
                .any(|c| c.subject == FollowUpSubject::Invoice(emitted.id)
                    && c.kind == FollowUpKind::Invoice),
            "{queue:?}"
        );
    }

    /// Classe la lettre du moment, sans brouillon ouvert : l'aperçu suivant est la phrase.
    fn file_letter(store: &mut Store, subject: FollowUpSubject, today: Date) {
        set_sender(store);
        let Outcome::Applied(_) = Executor::new(store)
            .execute(
                &MarkFollowUpSent {
                    subject,
                    today,
                    subject_line: None,
                    body: None,
                },
                &human(),
            )
            .unwrap()
        else {
            panic!("expected Applied");
        };
    }

    fn phrase_by_key(store: &Store, key: &str) -> ProspectPhrase {
        prospect_phrases(store.connection())
            .unwrap()
            .into_iter()
            .find(|phrase| phrase.key == key)
            .unwrap()
    }

    fn arrange_three(store: &mut Store) {
        let hello = phrase_by_key(store, "hello");
        let bump = phrase_by_key(store, "bump");
        let close = phrase_by_key(store, "close");
        let Outcome::Applied(_) = Executor::new(store)
            .execute(
                &ArrangeProspectPhrases {
                    moments: vec![
                        MomentDraft {
                            key: Some(hello.key),
                            label: "Premier contact".into(),
                            offset_days: 0,
                            subject: hello.subject,
                            body: hello.body,
                            revision: Some(hello.revision),
                        },
                        MomentDraft {
                            key: Some(bump.key),
                            label: "Première relance".into(),
                            offset_days: 10,
                            subject: bump.subject,
                            body: bump.body,
                            revision: Some(bump.revision),
                        },
                        MomentDraft {
                            key: Some(close.key),
                            label: "Dernière relance".into(),
                            offset_days: 21,
                            subject: close.subject,
                            body: close.body,
                            revision: Some(close.revision),
                        },
                    ],
                },
                &human(),
            )
            .unwrap()
        else {
            panic!("expected Applied");
        };
    }

    fn on_the_day(store: &Store, today: Date, title: &str) -> bool {
        day_gestures(store.connection(), today)
            .unwrap()
            .iter()
            .any(|geste| {
                matches!(
                    (&geste.verb, &geste.source),
                    (
                        GestureVerb::Write,
                        GestureSource::FollowUp {
                            title: gesture_title,
                            due_on: Some(due),
                            ..
                        }
                    ) if gesture_title == title && *due == today
                )
            })
    }

    #[test]
    fn an_engaged_dossier_finishes_its_series_and_stays_on_the_day() {
        let mut store = test_store("engaged-series");
        let sent_on = date(2026, Month::September, 2);
        let today = date(2026, Month::September, 5);
        let (_, engaged) = seed_opportunity(&mut store, sent_on);
        file_letter(&mut store, engaged, sent_on);
        let before = card_for(store.connection(), engaged, today).unwrap();
        assert_eq!(before.step_label.as_deref(), Some("Petit rappel"));
        assert_eq!(before.due_on, Some(today));
        assert_eq!(before.step_count, 4);
        assert!(on_the_day(&store, today, "Refonte"));

        arrange_three(&mut store);

        let kept = card_for(store.connection(), engaged, today).unwrap();
        assert_eq!(kept.step_label.as_deref(), Some("Petit rappel"));
        assert_eq!(kept.step_count, 4);
        assert_eq!(kept.due_on, Some(today));
        assert!(kept.preview_body.unwrap().contains("court rappel"));
        assert!(on_the_day(&store, today, "Refonte"));

        let bump = phrase_by_key(&store, "bump");
        let Outcome::Applied(_) = Executor::new(&mut store)
            .execute(
                &RewriteProspectPhrases {
                    phrases: vec![PhraseRewrite {
                        key: bump.key.clone(),
                        label: bump.label.clone(),
                        subject: bump.subject.clone(),
                        body: "Un écart nouveau.".into(),
                        revision: bump.revision,
                    }],
                },
                &human(),
            )
            .unwrap()
        else {
            panic!("expected Applied");
        };
        let still = card_for(store.connection(), engaged, today).unwrap();
        assert!(still.preview_body.unwrap().contains("court rappel"));
        assert_eq!(still.due_on, Some(today));

        let (_, fresh) = seed_opportunity(&mut store, today);
        file_letter(&mut store, fresh, today);
        let next = card_for(store.connection(), fresh, today).unwrap();
        assert_eq!(next.step_count, 3);
        assert_eq!(next.step_label.as_deref(), Some("Première relance"));
        assert_eq!(next.due_on, Some(date(2026, Month::September, 15)));
        assert!(next.preview_body.unwrap().contains("Un écart nouveau."));
        assert!(on_the_day(&store, today, "Refonte"));
    }

    #[test]
    fn a_reopened_conversation_takes_the_living_series() {
        let mut store = test_store("reopened-series");
        let sent_on = date(2026, Month::September, 2);
        let today = date(2026, Month::September, 5);
        let (_, engaged) = seed_opportunity(&mut store, sent_on);
        file_letter(&mut store, engaged, sent_on);
        arrange_three(&mut store);
        let FollowUpSubject::Opportunity(id) = engaged else {
            panic!("opportunity");
        };
        Executor::new(&mut store)
            .execute(
                &LoseOpportunity {
                    opportunity_id: id,
                    reason: LossReason::Timing,
                },
                &human(),
            )
            .unwrap();
        Executor::new(&mut store)
            .execute(
                &ReopenOpportunity {
                    opportunity_id: id,
                    next_action_at: today,
                },
                &human(),
            )
            .unwrap();
        let resumed = card_for(store.connection(), engaged, today).unwrap();
        assert_eq!(resumed.step_count, 3);
        assert_eq!(resumed.step_label.as_deref(), Some("Premier contact"));
        assert!(
            resumed
                .preview_body
                .unwrap()
                .contains("Je me permets de revenir")
        );

        let hello = phrase_by_key(&store, "hello");
        Executor::new(&mut store)
            .execute(
                &RewriteProspectPhrases {
                    phrases: vec![PhraseRewrite {
                        key: hello.key,
                        label: hello.label,
                        subject: hello.subject,
                        body: "Mot repris.".into(),
                        revision: hello.revision,
                    }],
                },
                &human(),
            )
            .unwrap();
        let frozen = card_for(store.connection(), engaged, today).unwrap();
        assert!(
            frozen
                .preview_body
                .unwrap()
                .contains("Je me permets de revenir")
        );
        assert_eq!(frozen.step_count, 3);

        let (_, fresh) = seed_opportunity(&mut store, today);
        let fresh_card = card_for(store.connection(), fresh, today).unwrap();
        assert!(fresh_card.preview_body.unwrap().contains("Mot repris."));
        assert_eq!(fresh_card.step_count, 3);
    }

    #[test]
    fn before_a_structure_change_a_reopening_reads_the_living_words() {
        let mut store = test_store("reopen-before-structure");
        let today = date(2026, Month::September, 5);
        let (_, subject) = seed_opportunity(&mut store, today);
        let FollowUpSubject::Opportunity(id) = subject else {
            panic!("opportunity");
        };
        Executor::new(&mut store)
            .execute(
                &LoseOpportunity {
                    opportunity_id: id,
                    reason: LossReason::Timing,
                },
                &human(),
            )
            .unwrap();
        Executor::new(&mut store)
            .execute(
                &ReopenOpportunity {
                    opportunity_id: id,
                    next_action_at: today,
                },
                &human(),
            )
            .unwrap();
        let hello = phrase_by_key(&store, "hello");
        Executor::new(&mut store)
            .execute(
                &RewriteProspectPhrases {
                    phrases: vec![PhraseRewrite {
                        key: hello.key,
                        label: hello.label,
                        subject: hello.subject,
                        body: "Avant tout changement.".into(),
                        revision: hello.revision,
                    }],
                },
                &human(),
            )
            .unwrap();
        let reading = card_for(store.connection(), subject, today).unwrap();
        assert_eq!(reading.step_count, 4);
        assert!(
            reading
                .preview_body
                .unwrap()
                .contains("Avant tout changement.")
        );

        arrange_three(&mut store);
        let pinned = card_for(store.connection(), subject, today).unwrap();
        assert_eq!(pinned.step_count, 4);
        assert!(
            pinned
                .preview_body
                .unwrap()
                .contains("Avant tout changement.")
        );
        assert_eq!(pinned.step_label.as_deref(), Some("Premier message"));
    }

    #[test]
    fn moving_a_moment_changes_only_the_living_order() {
        let mut store = test_store("move-moment");
        let sent_on = date(2026, Month::September, 2);
        let today = date(2026, Month::September, 5);
        let (_, engaged) = seed_opportunity(&mut store, sent_on);
        file_letter(&mut store, engaged, sent_on);
        let hello = phrase_by_key(&store, "hello");
        let bump = phrase_by_key(&store, "bump");
        let close = phrase_by_key(&store, "close");
        let Outcome::Applied(_) = Executor::new(&mut store)
            .execute(
                &ArrangeProspectPhrases {
                    moments: vec![
                        MomentDraft::from_phrase(&hello),
                        MomentDraft::from_phrase(&close),
                        MomentDraft::from_phrase(&bump),
                    ],
                },
                &human(),
            )
            .unwrap()
        else {
            panic!("expected Applied");
        };
        let kept = card_for(store.connection(), engaged, today).unwrap();
        assert_eq!(kept.step_label.as_deref(), Some("Petit rappel"));
        assert_eq!(kept.step_count, 4);

        let (_, fresh) = seed_opportunity(&mut store, today);
        file_letter(&mut store, fresh, today);
        let next = card_for(store.connection(), fresh, today).unwrap();
        assert_eq!(next.step_label.as_deref(), Some("Dernier mot"));
        assert_eq!(next.step_count, 3);
    }

    #[test]
    fn arranging_words_alone_does_not_copy_the_series() {
        let mut store = test_store("words-only");
        let today = date(2026, Month::September, 5);
        let (_, subject) = seed_opportunity(&mut store, today);
        let mut moments: Vec<_> = prospect_phrases(store.connection())
            .unwrap()
            .iter()
            .map(MomentDraft::from_phrase)
            .collect();
        moments[0].label = "Premier contact".into();
        moments[0].body = "Un premier mot réécrit.".into();
        let Outcome::Applied(_) = Executor::new(&mut store)
            .execute(&ArrangeProspectPhrases { moments }, &human())
            .unwrap()
        else {
            panic!("expected Applied");
        };
        let card = card_for(store.connection(), subject, today).unwrap();
        assert_eq!(card.step_label.as_deref(), Some("Premier contact"));
        assert!(
            card.preview_body
                .unwrap()
                .contains("Un premier mot réécrit.")
        );
        assert_eq!(card.step_count, 4);
    }

    #[test]
    fn the_last_moment_stays_and_the_first_gap_is_refused() {
        let mut store = test_store("moment-guards");
        let only = MomentDraft::from_phrase(&phrase_by_key(&store, "hello"));
        let err = Executor::new(&mut store)
            .execute(&ArrangeProspectPhrases { moments: vec![] }, &human())
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg == "Il reste au moins un moment."));
        let mut shifted = only.clone();
        shifted.offset_days = 4;
        let err = Executor::new(&mut store)
            .execute(
                &ArrangeProspectPhrases {
                    moments: vec![shifted],
                },
                &human(),
            )
            .unwrap_err();
        assert!(matches!(
            err,
            AppError::Domain(msg) if msg == "Le premier moment est le jour déjà posé sur le dossier."
        ));
        assert_eq!(prospect_phrases(store.connection()).unwrap().len(), 4);
        let _ = only;
    }

    #[test]
    fn invoice_reminders_keep_their_own_gaps() {
        let mut store = test_store("invoice-gaps");
        let today = date(2026, Month::September, 5);
        let (client_id, _) = seed_opportunity(&mut store, date(2026, Month::October, 1));
        let emitted = match Executor::new(&mut store)
            .execute(
                &crate::billing::EmitInvoice {
                    client_id,
                    mission_id: None,
                    lines: vec![crate::domain::InvoiceLine {
                        description: "Mission".into(),
                        quantity: 1.0,
                        unit_price: Money::from_cents(120_000),
                        vat_rate: VatRate::Standard,
                    }],
                    issued_on: date(2026, Month::August, 6),
                    payment_terms_days: 30,
                },
                &human(),
            )
            .unwrap()
        {
            Outcome::Applied(emitted) => emitted,
            other => panic!("expected Applied, got {other:?}"),
        };
        arrange_three(&mut store);
        let card = card_for(
            store.connection(),
            FollowUpSubject::Invoice(emitted.id),
            today,
        )
        .unwrap();
        assert_eq!(card.step_count, 4);
        assert_eq!(card.step_key.as_deref(), Some("due"));
        assert_eq!(card.step_label.as_deref(), Some("Échéance"));
        assert_eq!(card.due_on, Some(today));
    }

    /// # Panics
    /// Si la commande n'est pas appliquée.
    fn applied<C: Command>(store: &mut Store, command: &C) -> C::Output {
        match Executor::new(store).execute(command, &human()).unwrap() {
            Outcome::Applied(value) => value,
            Outcome::DryRun | Outcome::AlreadyApplied(_) | Outcome::PendingConfirmation(_) => {
                panic!("expected Applied")
            }
        }
    }

    fn count_rows(store: &Store, sql: &str) -> i64 {
        store
            .connection()
            .query_row(sql, [], |row| row.get(0))
            .unwrap()
    }

    fn genre_subject(store: &Store, genre_id: &str, key: &str) -> String {
        store
            .connection()
            .query_row(
                "SELECT subject FROM prospect_genre_words WHERE genre_id = ?1 AND phrase_key = ?2",
                [genre_id, key],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn stored_letter(
        store: &Store,
        subject: FollowUpSubject,
        fact: FollowUpFact,
    ) -> (Option<String>, Option<String>) {
        let event = events_for(store.connection(), subject)
            .unwrap()
            .into_iter()
            .find(|event| event.fact == fact)
            .unwrap();
        (event.rendered_subject, event.rendered_body)
    }

    #[test]
    fn two_dossiers_of_the_same_rank_do_not_share_a_subject() {
        let mut store = test_store("genre-subjects");
        let today = date(2026, Month::September, 5);
        let (_, mairie) = seed_opportunity(&mut store, today);
        let (_, commerce) = seed_opportunity(&mut store, today);
        let (_, nu) = seed_opportunity(&mut store, today);
        let services = applied(
            &mut store,
            &CreateProspectGenre {
                name: "Services publics".into(),
                copy_from: None,
            },
        );
        let shops = applied(
            &mut store,
            &CreateProspectGenre {
                name: "Commerces".into(),
                copy_from: None,
            },
        );
        applied(
            &mut store,
            &RewriteGenreWords {
                genre_id: services.id.clone(),
                words: vec![GenreWordDraft {
                    key: "hello".into(),
                    subject: "Pour la mairie".into(),
                    body: "Bonjour {{prenom}},\n\nLa mairie.\n".into(),
                }],
            },
        );
        applied(
            &mut store,
            &RewriteGenreWords {
                genre_id: shops.id,
                words: vec![GenreWordDraft {
                    key: "hello".into(),
                    subject: "Pour le commerce".into(),
                    body: "Bonjour {{prenom}},\n\nLe commerce.\n".into(),
                }],
            },
        );
        applied(
            &mut store,
            &SetDossierGenre {
                subject: mairie,
                name: "Services publics".into(),
            },
        );
        applied(
            &mut store,
            &SetDossierGenre {
                subject: commerce,
                name: "Commerces".into(),
            },
        );

        let town = card_for(store.connection(), mairie, today).unwrap();
        let shop = card_for(store.connection(), commerce, today).unwrap();
        let plain = card_for(store.connection(), nu, today).unwrap();
        assert_eq!(town.due_on, shop.due_on);
        assert_eq!(town.due_on, plain.due_on);
        assert_eq!(town.step_index, shop.step_index);
        assert_eq!(town.step_key, shop.step_key);
        assert_eq!(town.preview_subject.as_deref(), Some("Pour la mairie"));
        assert_eq!(shop.preview_subject.as_deref(), Some("Pour le commerce"));
        assert_eq!(plain.preview_subject.as_deref(), Some("Refonte"));
        assert!(
            plain
                .preview_body
                .unwrap()
                .contains("Je me permets de revenir")
        );
        assert_eq!(phrase_by_key(&store, "hello").subject, "{{sujet}}");
    }

    #[test]
    fn changing_the_genre_keeps_the_cursor_and_the_due_date() {
        let mut store = test_store("genre-cursor");
        let today = date(2026, Month::September, 5);
        let (_, subject) = seed_opportunity(&mut store, today);
        set_sender(&mut store);
        applied(
            &mut store,
            &MarkFollowUpSent {
                subject,
                today,
                subject_line: Some("Première lettre classée".into()),
                body: Some("Le texte classé.".into()),
            },
        );
        applied(
            &mut store,
            &MarkFollowUpSent {
                subject,
                today,
                subject_line: Some("Deuxième lettre classée".into()),
                body: Some("Le second texte classé.".into()),
            },
        );
        let before = card_for(store.connection(), subject, today).unwrap();
        assert_eq!(before.step_index, 2);
        assert_eq!(before.step_key.as_deref(), Some("value"));
        let services = applied(
            &mut store,
            &CreateProspectGenre {
                name: "Services publics".into(),
                copy_from: None,
            },
        );
        applied(
            &mut store,
            &RewriteGenreWords {
                genre_id: services.id,
                words: vec![GenreWordDraft {
                    key: "value".into(),
                    subject: "Troisième des services".into(),
                    body: "Bonjour {{prenom}},\n\nLe troisième moment.\n".into(),
                }],
            },
        );
        applied(
            &mut store,
            &SetDossierGenre {
                subject,
                name: "Services publics".into(),
            },
        );
        let after = card_for(store.connection(), subject, today).unwrap();
        assert_eq!(after.step_index, before.step_index);
        assert_eq!(after.step_key, before.step_key);
        assert_eq!(after.due_on, before.due_on);
        assert_eq!(
            after.preview_subject.as_deref(),
            Some("Troisième des services")
        );
        assert_eq!(
            count_rows(&store, "SELECT COUNT(*) FROM prospect_series"),
            0
        );
        let (filed_subject, filed_body) = stored_letter(&store, subject, FollowUpFact::MarkedSent);
        assert_eq!(filed_subject.as_deref(), Some("Première lettre classée"));
        assert_eq!(filed_body.as_deref(), Some("Le texte classé."));
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn keeping_words_updates_only_that_genre_and_that_moment() {
        let mut store = test_store("keep-words");
        let today = date(2026, Month::September, 5);
        let (_, mairie) = seed_opportunity(&mut store, today);
        let (_, commerce) = seed_opportunity(&mut store, today);
        let (_, ailleurs) = seed_opportunity(&mut store, today);
        let (_, classe) = seed_opportunity(&mut store, today);
        set_sender(&mut store);
        let services = applied(
            &mut store,
            &CreateProspectGenre {
                name: "Services publics".into(),
                copy_from: None,
            },
        );
        let shops = applied(
            &mut store,
            &CreateProspectGenre {
                name: "Commerces".into(),
                copy_from: None,
            },
        );
        applied(
            &mut store,
            &SetDossierGenre {
                subject: mairie,
                name: "Services publics".into(),
            },
        );
        applied(
            &mut store,
            &SetDossierGenre {
                subject: commerce,
                name: "Commerces".into(),
            },
        );
        let before = card_for(store.connection(), mairie, today).unwrap();
        applied(
            &mut store,
            &PrepareFollowUp {
                subject: ailleurs,
                today,
                subject_line: Some("Brouillon déjà préparé".into()),
                body: Some("Ce brouillon reste.".into()),
            },
        );
        applied(
            &mut store,
            &MarkFollowUpSent {
                subject: classe,
                today,
                subject_line: Some("Lettre déjà classée".into()),
                body: Some("Ce classement reste.".into()),
            },
        );
        let shops_hello = genre_subject(&store, &shops.id, "hello");
        let services_bump = genre_subject(&store, &services.id, "bump");
        let kept = applied(
            &mut store,
            &KeepGenreWords {
                subject: mairie,
                today,
                subject_line: Some(before.preview_subject.unwrap()),
                body: Some(before.preview_body.unwrap()),
                genre_name: Some("Ignoré".into()),
            },
        );
        assert_eq!(kept.key, "hello");
        assert_eq!(kept.genre_id, services.id);
        assert_eq!(kept.subject, "{{sujet}}");
        assert!(kept.body.contains("{{prenom}}"));
        assert_eq!(genre_subject(&store, &shops.id, "hello"), shops_hello);
        assert_eq!(genre_subject(&store, &services.id, "bump"), services_bump);
        assert_eq!(phrase_by_key(&store, "hello").subject, "{{sujet}}");
        let still = card_for(store.connection(), mairie, today).unwrap();
        assert_eq!(still.step_key.as_deref(), Some("hello"));
        assert_eq!(still.due_on, before.due_on);
        assert_eq!(still.step_index, before.step_index);
        assert_eq!(
            stored_letter(&store, ailleurs, FollowUpFact::DraftPrepared),
            (
                Some("Brouillon déjà préparé".into()),
                Some("Ce brouillon reste.".into())
            )
        );
        assert_eq!(
            stored_letter(&store, classe, FollowUpFact::MarkedSent),
            (
                Some("Lettre déjà classée".into()),
                Some("Ce classement reste.".into())
            )
        );
        let written = applied(
            &mut store,
            &KeepGenreWords {
                subject: mairie,
                today,
                subject_line: Some("Bonjour <prénom>".into()),
                body: Some("Un mot pour la mairie.".into()),
                genre_name: None,
            },
        );
        assert_eq!(written.subject, "Bonjour {{prenom}}");
        assert_eq!(written.body, "Un mot pour la mairie.");
        assert_eq!(genre_subject(&store, &shops.id, "hello"), shops_hello);
        assert_eq!(
            card_for(store.connection(), commerce, today)
                .unwrap()
                .preview_subject
                .as_deref(),
            Some("Refonte")
        );
        assert_eq!(
            count_rows(&store, "SELECT COUNT(*) FROM prospect_series"),
            0
        );

        let (_, neuf) = seed_opportunity(&mut store, today);
        let created = applied(
            &mut store,
            &KeepGenreWords {
                subject: neuf,
                today,
                subject_line: Some("Pour les ateliers".into()),
                body: Some("Le mot des ateliers.".into()),
                genre_name: Some("Ateliers".into()),
            },
        );
        assert_eq!(created.genre_name, "Ateliers");
        assert_eq!(created.key, "hello");
        assert_eq!(created.subject, "Pour les ateliers");
        assert_eq!(
            prospect_genre_for(
                store.connection(),
                match neuf {
                    FollowUpSubject::Opportunity(id) => id,
                    FollowUpSubject::Invoice(_) => panic!("conversation"),
                }
            )
            .unwrap()
            .unwrap()
            .name,
            "Ateliers"
        );
        assert_eq!(
            genre_subject(&store, &created.genre_id, "bump"),
            phrase_by_key(&store, "bump").subject
        );
        applied(
            &mut store,
            &KeepGenreWords {
                subject: neuf,
                today,
                subject_line: None,
                body: None,
                genre_name: None,
            },
        );
        let (_, sans) = seed_opportunity(&mut store, today);
        let missing = Executor::new(&mut store)
            .execute(
                &KeepGenreWords {
                    subject: sans,
                    today,
                    subject_line: None,
                    body: None,
                    genre_name: None,
                },
                &human(),
            )
            .unwrap_err();
        assert!(matches!(missing, AppError::Domain(msg) if msg == "Pour qui ?"));
    }

    #[test]
    fn rewriting_genre_words_does_not_copy_a_series() {
        let mut store = test_store("genre-words");
        let today = date(2026, Month::September, 5);
        let (_, subject) = seed_opportunity(&mut store, today);
        let services = applied(
            &mut store,
            &CreateProspectGenre {
                name: "Services publics".into(),
                copy_from: None,
            },
        );
        let defaults = prospect_phrases(store.connection()).unwrap();
        let mut moments: Vec<_> = defaults.iter().map(MomentDraft::from_phrase).collect();
        moments[0].subject = "Pour la mairie".into();
        moments[0].body = "Bonjour {{prenom}},\n\nLa mairie seulement.\n".into();
        applied(
            &mut store,
            &SaveGenreLetter {
                genre_id: services.id.clone(),
                moments,
            },
        );
        applied(
            &mut store,
            &SetDossierGenre {
                subject,
                name: "Services publics".into(),
            },
        );
        assert_eq!(
            count_rows(&store, "SELECT COUNT(*) FROM prospect_series"),
            0
        );
        assert_eq!(phrase_by_key(&store, "hello").subject, "{{sujet}}");
        assert_eq!(phrase_by_key(&store, "bump").offset_days, 3);
        assert_eq!(phrase_by_key(&store, "value").offset_days, 7);
        assert_eq!(phrase_by_key(&store, "close").offset_days, 14);
        let card = card_for(store.connection(), subject, today).unwrap();
        assert_eq!(card.preview_subject.as_deref(), Some("Pour la mairie"));
        assert!(card.preview_body.unwrap().contains("La mairie seulement."));
        let shown = phrases_for_genre(store.connection(), &services.id).unwrap();
        assert_eq!(shown[0].subject, "Pour la mairie");
        assert_eq!(shown[1].subject, defaults[1].subject);
        assert_eq!(shown[1].offset_days, 3);
    }

    #[test]
    fn a_structure_change_still_pins_and_drops_the_moment_from_living_genres() {
        let mut store = test_store("genre-pin");
        let sent_on = date(2026, Month::September, 2);
        let today = date(2026, Month::September, 5);
        let (_, engaged) = seed_opportunity(&mut store, sent_on);
        file_letter(&mut store, engaged, sent_on);
        let services = applied(
            &mut store,
            &CreateProspectGenre {
                name: "Services publics".into(),
                copy_from: None,
            },
        );
        applied(
            &mut store,
            &RewriteGenreWords {
                genre_id: services.id.clone(),
                words: vec![GenreWordDraft {
                    key: "value".into(),
                    subject: "Valeur du genre".into(),
                    body: "Ce mot ne doit pas rester sur la copie.\n".into(),
                }],
            },
        );
        applied(
            &mut store,
            &SetDossierGenre {
                subject: engaged,
                name: "Services publics".into(),
            },
        );
        let phrases = prospect_phrases(store.connection()).unwrap();
        let draft =
            |key: &str| MomentDraft::from_phrase(phrases.iter().find(|p| p.key == key).unwrap());
        let mut hello = draft("hello");
        hello.subject = "Pour la mairie".into();
        applied(
            &mut store,
            &SaveGenreLetter {
                genre_id: services.id.clone(),
                moments: vec![hello, draft("bump"), draft("close")],
            },
        );
        assert_eq!(
            count_rows(&store, "SELECT COUNT(*) FROM prospect_series"),
            1
        );
        assert_eq!(
            count_rows(
                &store,
                "SELECT COUNT(*) FROM prospect_genre_words WHERE phrase_key = 'value'"
            ),
            0
        );
        assert!(
            prospect_phrases(store.connection())
                .unwrap()
                .iter()
                .all(|p| p.key != "value")
        );
        assert_eq!(
            genre_subject(&store, &services.id, "hello"),
            "Pour la mairie"
        );
        assert_eq!(phrase_by_key(&store, "hello").subject, "{{sujet}}");
        let pinned = card_for(store.connection(), engaged, today).unwrap();
        assert_eq!(pinned.step_count, 4);
        assert_eq!(pinned.step_key.as_deref(), Some("bump"));
        assert_eq!(pinned.due_on, Some(today));
        let copy_subject: String = store
            .connection()
            .query_row(
                "SELECT subject FROM prospect_series_steps WHERE phrase_key = 'value'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(copy_subject, "{{sujet}}");
        let (_, fresh) = seed_opportunity(&mut store, today);
        let fresh_card = card_for(store.connection(), fresh, today).unwrap();
        assert_eq!(fresh_card.step_count, 3);
        assert_ne!(fresh_card.step_key.as_deref(), Some("value"));
    }

    #[test]
    fn dropping_a_genre_returns_the_next_letter_to_the_default_words() {
        let mut store = test_store("drop-genre");
        let today = date(2026, Month::September, 5);
        let (_, subject) = seed_opportunity(&mut store, today);
        set_sender(&mut store);
        let services = applied(
            &mut store,
            &CreateProspectGenre {
                name: "Services publics".into(),
                copy_from: None,
            },
        );
        applied(
            &mut store,
            &RewriteGenreWords {
                genre_id: services.id.clone(),
                words: vec![GenreWordDraft {
                    key: "bump".into(),
                    subject: "Rappel des services".into(),
                    body: "Le rappel du genre.\n".into(),
                }],
            },
        );
        applied(
            &mut store,
            &SetDossierGenre {
                subject,
                name: "Services publics".into(),
            },
        );
        applied(
            &mut store,
            &MarkFollowUpSent {
                subject,
                today,
                subject_line: Some("Lettre classée".into()),
                body: Some("Elle reste.".into()),
            },
        );
        let before = card_for(store.connection(), subject, today).unwrap();
        assert_eq!(
            before.preview_subject.as_deref(),
            Some("Rappel des services")
        );
        applied(&mut store, &DropProspectGenre { id: services.id });
        let after = card_for(store.connection(), subject, today).unwrap();
        assert_eq!(after.step_index, before.step_index);
        assert_eq!(after.step_key, before.step_key);
        assert_eq!(after.due_on, before.due_on);
        assert_eq!(
            after.preview_subject.as_deref(),
            Some("Refonte — je me permets un rappel")
        );
        assert_eq!(
            stored_letter(&store, subject, FollowUpFact::MarkedSent)
                .0
                .as_deref(),
            Some("Lettre classée")
        );
        assert!(prospect_genres(store.connection()).unwrap().is_empty());
        let FollowUpSubject::Opportunity(id) = subject else {
            panic!("conversation");
        };
        assert!(
            prospect_genre_for(store.connection(), id)
                .unwrap()
                .is_none()
        );
    }
}
