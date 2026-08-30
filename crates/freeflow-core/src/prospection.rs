//! Prospection : opportunités, journal d'interactions, pipeline pondéré. Bâti entièrement sur
//! la couche applicative (`Command`/`Executor`, lot 2) — aucune requête SQL directe sur ces
//! tables ne doit exister ailleurs dans l'application.

mod commands;
mod error;
mod queries;
mod row;

pub use commands::{
    AdvanceOpportunity, CreateOpportunity, LogInteraction, LoseOpportunity, WinOpportunity,
};
pub use error::ProspectionError;
pub use queries::{
    StageSummary, late_actions, list_open_opportunities, pipeline_by_stage, weighted_pipeline,
    without_next_action,
};

#[cfg(test)]
mod tests {
    use time::{Date, Month};

    use super::*;
    use crate::app::{Actor, AppError, ExecutionContext, Executor, Outcome};
    use crate::domain::{
        ClientId, InteractionKind, LossReason, MissionKind, Money, OpportunityStage, Probability,
    };
    use crate::store::{Passphrase, Store};

    fn date(year: i32, month: Month, day: u8) -> Date {
        Date::from_calendar_date(year, month, day).unwrap()
    }

    /// Ouvre un coffre de test et y insère un client de référence (les opportunités portent
    /// une contrainte de clé étrangère vers `clients`).
    fn test_store(label: &str) -> (Store, ClientId) {
        let dir = std::env::temp_dir().join(format!(
            "freeflow-prospection-test-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ));
        let store = Store::create(&dir.join("vault.db"), &Passphrase::from("s3cret")).unwrap();
        let client_id = ClientId::new();
        store
            .connection()
            .execute(
                "INSERT INTO clients (id, name, created_at) VALUES (?1, 'Argon Digital', '2026-01-01T00:00:00Z')",
                [client_id.to_string()],
            )
            .unwrap();
        (store, client_id)
    }

    fn human_ctx() -> ExecutionContext {
        ExecutionContext::new(Actor::Human, false)
    }

    fn new_opportunity(
        client_id: ClientId,
        amount_cents: i64,
        probability: u8,
        next_action_at: Date,
    ) -> CreateOpportunity {
        CreateOpportunity {
            client_id,
            name: "Refonte plateforme".into(),
            amount: Money::from_cents(amount_cents),
            probability: Probability::new(probability).unwrap(),
            next_action_at,
            source: Some("recommandation".into()),
        }
    }

    #[test]
    fn creating_an_opportunity_starts_in_qualification_with_its_next_action() {
        let (mut store, client_id) = test_store("create");
        let cmd = new_opportunity(client_id, 7_800_000, 40, date(2026, Month::September, 2));

        let Outcome::Applied(opportunity_id) = Executor::new(&mut store)
            .execute(&cmd, &human_ctx())
            .unwrap()
        else {
            panic!("expected Applied")
        };

        let opportunity = row::opportunity_by_id(store.connection(), opportunity_id)
            .unwrap()
            .unwrap();
        assert_eq!(opportunity.stage, OpportunityStage::Qualification);
        assert_eq!(
            opportunity.next_action_at,
            Some(date(2026, Month::September, 2))
        );
    }

    #[test]
    fn advancing_between_open_stages_updates_stage_and_next_action() {
        let (mut store, client_id) = test_store("advance");
        let create = new_opportunity(client_id, 1_500_000, 60, date(2026, Month::September, 1));
        let Outcome::Applied(opportunity_id) = Executor::new(&mut store)
            .execute(&create, &human_ctx())
            .unwrap()
        else {
            panic!("expected Applied")
        };

        let advance = AdvanceOpportunity {
            opportunity_id,
            to: OpportunityStage::Negotiation,
            next_action_at: date(2026, Month::September, 10),
        };
        Executor::new(&mut store)
            .execute(&advance, &human_ctx())
            .unwrap();

        let opportunity = row::opportunity_by_id(store.connection(), opportunity_id)
            .unwrap()
            .unwrap();
        assert_eq!(opportunity.stage, OpportunityStage::Negotiation);
        assert_eq!(
            opportunity.next_action_at,
            Some(date(2026, Month::September, 10))
        );
    }

    #[test]
    fn advancing_a_closed_opportunity_is_rejected() {
        let (mut store, client_id) = test_store("advance-closed");
        let create = new_opportunity(client_id, 800_000, 80, date(2026, Month::September, 1));
        let Outcome::Applied(opportunity_id) = Executor::new(&mut store)
            .execute(&create, &human_ctx())
            .unwrap()
        else {
            panic!("expected Applied")
        };
        let lose = LoseOpportunity {
            opportunity_id,
            reason: LossReason::Budget,
        };
        Executor::new(&mut store)
            .execute(&lose, &human_ctx())
            .unwrap();

        let advance = AdvanceOpportunity {
            opportunity_id,
            to: OpportunityStage::Negotiation,
            next_action_at: date(2026, Month::September, 10),
        };
        let err = Executor::new(&mut store)
            .execute(&advance, &human_ctx())
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("close")));
    }

    #[test]
    fn advancing_to_won_or_lost_is_rejected_in_favor_of_the_dedicated_commands() {
        let (mut store, client_id) = test_store("advance-to-closed");
        let create = new_opportunity(client_id, 800_000, 80, date(2026, Month::September, 1));
        let Outcome::Applied(opportunity_id) = Executor::new(&mut store)
            .execute(&create, &human_ctx())
            .unwrap()
        else {
            panic!("expected Applied")
        };

        let advance = AdvanceOpportunity {
            opportunity_id,
            to: OpportunityStage::Won,
            next_action_at: date(2026, Month::September, 10),
        };
        let err = Executor::new(&mut store)
            .execute(&advance, &human_ctx())
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(_)));
    }

    #[test]
    fn winning_an_opportunity_creates_exactly_one_coherent_mission() {
        let (mut store, client_id) = test_store("win");
        let create = new_opportunity(client_id, 4_500_000, 70, date(2026, Month::September, 1));
        let Outcome::Applied(opportunity_id) = Executor::new(&mut store)
            .execute(&create, &human_ctx())
            .unwrap()
        else {
            panic!("expected Applied")
        };

        let win = WinOpportunity {
            opportunity_id,
            started_on: date(2026, Month::October, 1),
        };
        let Outcome::Applied(mission_id) = Executor::new(&mut store)
            .execute(&win, &human_ctx())
            .unwrap()
        else {
            panic!("expected Applied")
        };

        assert_eq!(
            crate::missions::row::mission_count_for_client(store.connection(), client_id).unwrap(),
            1
        );
        assert_eq!(
            crate::missions::row::mission_id_for_client(store.connection(), client_id).unwrap(),
            mission_id
        );
        assert_eq!(
            crate::missions::row::mission_kind_for_client(store.connection(), client_id).unwrap(),
            MissionKind::Forfait {
                budget: Money::from_cents(4_500_000)
            }
        );

        let opportunity = row::opportunity_by_id(store.connection(), opportunity_id)
            .unwrap()
            .unwrap();
        assert_eq!(opportunity.stage, OpportunityStage::Won);
        assert_eq!(opportunity.next_action_at, None);
    }

    #[test]
    fn winning_a_closed_opportunity_is_rejected() {
        let (mut store, client_id) = test_store("win-closed");
        let create = new_opportunity(client_id, 800_000, 80, date(2026, Month::September, 1));
        let Outcome::Applied(opportunity_id) = Executor::new(&mut store)
            .execute(&create, &human_ctx())
            .unwrap()
        else {
            panic!("expected Applied")
        };
        Executor::new(&mut store)
            .execute(
                &WinOpportunity {
                    opportunity_id,
                    started_on: date(2026, Month::October, 1),
                },
                &human_ctx(),
            )
            .unwrap();

        let err = Executor::new(&mut store)
            .execute(
                &WinOpportunity {
                    opportunity_id,
                    started_on: date(2026, Month::October, 2),
                },
                &human_ctx(),
            )
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(_)));
        assert_eq!(
            crate::missions::row::mission_count_for_client(store.connection(), client_id).unwrap(),
            1,
            "pas de seconde mission créée"
        );
    }

    #[test]
    fn losing_an_opportunity_records_the_structured_reason_and_clears_next_action() {
        let (mut store, client_id) = test_store("lose");
        let create = new_opportunity(client_id, 800_000, 20, date(2026, Month::September, 1));
        let Outcome::Applied(opportunity_id) = Executor::new(&mut store)
            .execute(&create, &human_ctx())
            .unwrap()
        else {
            panic!("expected Applied")
        };

        Executor::new(&mut store)
            .execute(
                &LoseOpportunity {
                    opportunity_id,
                    reason: LossReason::Competitor,
                },
                &human_ctx(),
            )
            .unwrap();

        let opportunity = row::opportunity_by_id(store.connection(), opportunity_id)
            .unwrap()
            .unwrap();
        assert_eq!(opportunity.stage, OpportunityStage::Lost);
        assert_eq!(opportunity.next_action_at, None);
        assert_eq!(opportunity.loss_reason, Some(LossReason::Competitor));
    }

    #[test]
    fn weighted_pipeline_sums_amount_times_probability_over_open_opportunities_only() {
        let (mut store, client_id) = test_store("weighted-pipeline");
        // 78 000 € à 40 % -> 31 200 € (cas Kappa Software de la maquette).
        let a = new_opportunity(client_id, 7_800_000, 40, date(2026, Month::September, 1));
        // 15 000 € à 60 % -> 9 000 €.
        let b = new_opportunity(client_id, 1_500_000, 60, date(2026, Month::September, 1));
        Executor::new(&mut store).execute(&a, &human_ctx()).unwrap();
        Executor::new(&mut store).execute(&b, &human_ctx()).unwrap();

        // Une opportunité gagnée ne doit plus compter dans le pipeline (elle n'est plus "en cours").
        let c = new_opportunity(client_id, 99_999_900, 100, date(2026, Month::September, 1));
        let Outcome::Applied(won_id) = Executor::new(&mut store).execute(&c, &human_ctx()).unwrap()
        else {
            panic!("expected Applied")
        };
        Executor::new(&mut store)
            .execute(
                &WinOpportunity {
                    opportunity_id: won_id,
                    started_on: date(2026, Month::September, 1),
                },
                &human_ctx(),
            )
            .unwrap();

        let pipeline = weighted_pipeline(store.connection()).unwrap();
        assert_eq!(
            pipeline,
            Money::from_cents(3_120_000) + Money::from_cents(900_000)
        );
    }

    #[test]
    fn late_actions_detects_the_today_yesterday_boundary() {
        let (mut store, client_id) = test_store("late-actions");
        let today = date(2026, Month::September, 2);
        let yesterday = date(2026, Month::September, 1);

        let due_today = new_opportunity(client_id, 1_000_000, 50, today);
        let overdue = new_opportunity(client_id, 2_000_000, 50, yesterday);
        Executor::new(&mut store)
            .execute(&due_today, &human_ctx())
            .unwrap();
        let Outcome::Applied(overdue_id) = Executor::new(&mut store)
            .execute(&overdue, &human_ctx())
            .unwrap()
        else {
            panic!("expected Applied")
        };

        let late = late_actions(store.connection(), today).unwrap();
        assert_eq!(
            late.len(),
            1,
            "une action prévue aujourd'hui n'est pas en retard, seule celle d'hier l'est"
        );
        assert_eq!(late[0].id, overdue_id);
    }

    #[test]
    fn logging_an_interaction_on_an_unknown_opportunity_fails_cleanly() {
        let (mut store, _client_id) = test_store("log-interaction-unknown");
        let cmd = LogInteraction {
            opportunity_id: crate::domain::OpportunityId::new(),
            kind: InteractionKind::Call,
            note: "test".into(),
        };
        let err = Executor::new(&mut store)
            .execute(&cmd, &human_ctx())
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("introuvable")));
    }

    #[test]
    fn list_open_opportunities_excludes_won_and_lost() {
        let (mut store, client_id) = test_store("list-open");
        let open = new_opportunity(client_id, 3_200_000, 20, date(2026, Month::September, 5));
        let Outcome::Applied(open_id) = Executor::new(&mut store)
            .execute(&open, &human_ctx())
            .unwrap()
        else {
            panic!("expected Applied")
        };

        let to_win = new_opportunity(client_id, 7_800_000, 40, date(2026, Month::September, 2));
        let Outcome::Applied(won_id) = Executor::new(&mut store)
            .execute(&to_win, &human_ctx())
            .unwrap()
        else {
            panic!("expected Applied")
        };
        Executor::new(&mut store)
            .execute(
                &WinOpportunity {
                    opportunity_id: won_id,
                    started_on: date(2026, Month::September, 3),
                },
                &human_ctx(),
            )
            .unwrap();

        let to_lose = new_opportunity(client_id, 1_500_000, 60, date(2026, Month::September, 1));
        let Outcome::Applied(lost_id) = Executor::new(&mut store)
            .execute(&to_lose, &human_ctx())
            .unwrap()
        else {
            panic!("expected Applied")
        };
        Executor::new(&mut store)
            .execute(
                &LoseOpportunity {
                    opportunity_id: lost_id,
                    reason: LossReason::Budget,
                },
                &human_ctx(),
            )
            .unwrap();

        let listed = list_open_opportunities(store.connection()).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, open_id);
    }
}
