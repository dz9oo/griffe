//! Relances de prospection et d'impayés : journal de faits, file dérivée, brouillons `.eml`.
//!
//! `FreeFlow` n'envoie jamais le mail. [`PrepareFollowUp`] rend les octets RFC 5322 ; l'adaptateur
//! écrit le fichier et l'ouvre (`xdg-open`). « Envoyé » est un fait marqué par l'humain.

mod commands;
mod error;
mod queries;
mod row;

pub use commands::{
    MarkFollowUpSent, PrepareFollowUp, PreparedFollowUp, RetractLastFollowUp, SetFollowUpDate,
    SetFollowUpSender, SkipFollowUpStep, SnoozeFollowUp,
};
pub use error::FollowUpError;
pub use queries::{
    CardStatus, FollowUpCard, HistoryItem, card_for, events_for, follow_up_board, follow_up_queue,
    follow_up_sender,
};

/// Pose une frontière de cadence sans effacer les lettres déjà classées.
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
    row::insert_event(conn, &event)
}

#[cfg(test)]
mod tests {
    use time::{Date, Month};

    use super::*;
    use crate::app::{Actor, AppError, ExecutionContext, Executor, Outcome};
    use crate::clients::{CreateClient, CreateContact};
    use crate::domain::{
        ClientId, FollowUpFact, FollowUpKind, FollowUpSubject, InteractionKind, Money, Probability,
        VatRate,
    };
    use crate::people::{HistoryKind, person};
    use crate::prospection::{CreateOpportunity, list_interactions, opportunity_by_id};
    use crate::store::Store;
    use crate::store::testing::test_store;

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
                    name: "Acme".into(),
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
        let (_, subject) = seed_opportunity(&mut store, today);
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
        let dossier = person(store.connection(), "Acme", today).unwrap();
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
        let (_, subject) = seed_opportunity(&mut store, today);
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
        let dossier = person(store.connection(), "Acme", today).unwrap();
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
}
