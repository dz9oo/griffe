//! Prospection : opportunités, journal d'interactions, pipeline pondéré. Bâti entièrement sur
//! la couche applicative (`Command`/`Executor`, lot 2) — aucune requête SQL directe sur ces
//! tables ne doit exister ailleurs dans l'application.

mod commands;
mod error;
mod queries;
mod row;

pub use commands::{
    AdvanceOpportunity, ArchiveOpportunity, CreateOpportunity, CreateProspect, DeleteInteraction,
    DeleteOpportunity, EstimationLineInput, LogInteraction, LoseOpportunity, ReopenOpportunity,
    SetEstimation, UnarchiveOpportunity, UpdateInteraction, UpdateOpportunity, UpdateProspect,
    WinOpportunity,
};
pub use error::ProspectionError;
pub use queries::{
    OpportunityFilter, OpportunityReferences, StageSummary, estimation_lines, interaction_by_id,
    late_actions, list_interactions, list_open_opportunities, list_opportunities,
    list_opportunities_with, opportunity_by_id, opportunity_references, pipeline_by_stage,
    weighted_pipeline, without_next_action,
};

/// Voir [`row::set_next_action_at`].
pub(crate) fn set_next_action_at(
    conn: &rusqlite::Connection,
    id: crate::domain::OpportunityId,
    next_action_at: Option<time::Date>,
) -> Result<(), crate::app::AppError> {
    row::set_next_action_at(conn, id, next_action_at)
}

#[cfg(test)]
mod tests {
    use rusqlite::params;
    use time::{Date, Month, OffsetDateTime};

    use super::*;
    use crate::app::{Actor, AppError, ExecutionContext, Executor, Outcome};
    use crate::domain::{
        ClientId, InteractionKind, LossReason, MissionKind, Money, OpportunityId, OpportunityStage,
        Probability,
    };
    use crate::store::Store;
    use crate::store::testing::test_store as empty_store;

    fn date(year: i32, month: Month, day: u8) -> Date {
        Date::from_calendar_date(year, month, day).unwrap()
    }

    /// Ouvre un coffre de test et y insère un client de référence (les opportunités portent
    /// une contrainte de clé étrangère vers `clients`).
    fn test_store(label: &str) -> (Store, ClientId) {
        let store = empty_store(label);
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

        let mission = crate::missions::row::mission_by_id(store.connection(), mission_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            mission.opportunity_id,
            Some(opportunity_id),
            "la lignée du gain doit être persistée (migration 0012)"
        );
    }

    #[test]
    fn a_won_opportunity_with_its_mission_can_no_longer_be_deleted() {
        let (mut store, client_id) = test_store("delete-won");
        let create = new_opportunity(client_id, 4_500_000, 70, date(2026, Month::September, 1));
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

        let refs = queries::opportunity_references(store.connection(), opportunity_id).unwrap();
        assert_eq!(refs.missions, 1);
        assert!(!refs.is_empty());

        let opportunity = row::opportunity_by_id(store.connection(), opportunity_id)
            .unwrap()
            .unwrap();
        let err = Executor::new(&mut store)
            .execute(
                &DeleteOpportunity {
                    id: opportunity_id,
                    revision: opportunity.revision,
                },
                &human_ctx(),
            )
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("mission")));
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
            occurred_at: None,
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

    fn create_opportunity(store: &mut Store, client_id: ClientId) -> OpportunityId {
        let create = new_opportunity(client_id, 1_000_000, 50, date(2026, Month::September, 1));
        let Outcome::Applied(id) = Executor::new(store).execute(&create, &human_ctx()).unwrap()
        else {
            panic!("expected Applied")
        };
        id
    }

    fn update_opportunity(id: OpportunityId, revision: i64) -> UpdateOpportunity {
        UpdateOpportunity {
            id,
            revision,
            name: "Refonte plateforme v2".into(),
            amount: Money::from_cents(2_000_000),
            probability: Probability::new(70).unwrap(),
            next_action_at: Some(date(2026, Month::October, 1)),
            source: Some("recommandation".into()),
        }
    }

    #[test]
    fn updating_bumps_the_revision() {
        let (mut store, client_id) = test_store("update-bumps-revision");
        let id = create_opportunity(&mut store, client_id);

        let Outcome::Applied(new_revision) = Executor::new(&mut store)
            .execute(&update_opportunity(id, 1), &human_ctx())
            .unwrap()
        else {
            panic!("expected Applied")
        };
        assert_eq!(new_revision, 2);

        let opportunity = opportunity_by_id(store.connection(), id).unwrap().unwrap();
        assert_eq!(opportunity.revision, 2);
        assert_eq!(opportunity.name, "Refonte plateforme v2");
        assert_eq!(opportunity.amount, Money::from_cents(2_000_000));
    }

    #[test]
    fn stale_revision_is_a_conflict_and_writes_nothing() {
        let (mut store, client_id) = test_store("stale-revision");
        let id = create_opportunity(&mut store, client_id);
        Executor::new(&mut store)
            .execute(&update_opportunity(id, 1), &human_ctx())
            .unwrap();

        let err = Executor::new(&mut store)
            .execute(&update_opportunity(id, 1), &human_ctx())
            .unwrap_err();
        assert!(matches!(
            err,
            AppError::Conflict {
                entity: "opportunité",
                ..
            }
        ));

        let opportunity = opportunity_by_id(store.connection(), id).unwrap().unwrap();
        assert_eq!(opportunity.revision, 2, "rien n'a dû être réécrit");
    }

    #[test]
    fn updating_a_closed_opportunity_is_refused() {
        let (mut store, client_id) = test_store("update-closed");
        let id = create_opportunity(&mut store, client_id);
        Executor::new(&mut store)
            .execute(
                &WinOpportunity {
                    opportunity_id: id,
                    started_on: date(2026, Month::October, 1),
                },
                &human_ctx(),
            )
            .unwrap();

        let err = Executor::new(&mut store)
            .execute(&update_opportunity(id, 2), &human_ctx())
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("close")));
    }

    #[test]
    fn updating_an_unknown_opportunity_is_not_found() {
        let (mut store, _client_id) = test_store("update-unknown");
        let err = Executor::new(&mut store)
            .execute(&update_opportunity(OpportunityId::new(), 1), &human_ctx())
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("introuvable")));
    }

    #[test]
    fn archiving_removes_it_from_the_open_list_and_the_weighted_pipeline_but_not_from_the_full_list()
     {
        let (mut store, client_id) = test_store("archive-open-list");
        let id = create_opportunity(&mut store, client_id);

        Executor::new(&mut store)
            .execute(&ArchiveOpportunity { id, revision: 1 }, &human_ctx())
            .unwrap();

        assert!(
            list_open_opportunities(store.connection())
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            weighted_pipeline(store.connection()).unwrap(),
            Money::from_cents(0)
        );
        assert_eq!(list_opportunities(store.connection()).unwrap().len(), 1);
        let opportunity = opportunity_by_id(store.connection(), id).unwrap().unwrap();
        assert!(opportunity.archived_at.is_some());
        assert_eq!(
            opportunity.stage,
            OpportunityStage::Qualification,
            "won|lost reste un axe distinct de l'archivage"
        );
    }

    #[test]
    fn unarchiving_round_trips() {
        let (mut store, client_id) = test_store("unarchive-round-trip");
        let id = create_opportunity(&mut store, client_id);
        Executor::new(&mut store)
            .execute(&ArchiveOpportunity { id, revision: 1 }, &human_ctx())
            .unwrap();
        Executor::new(&mut store)
            .execute(&UnarchiveOpportunity { id, revision: 2 }, &human_ctx())
            .unwrap();

        assert_eq!(
            list_open_opportunities(store.connection()).unwrap().len(),
            1
        );
        let opportunity = opportunity_by_id(store.connection(), id).unwrap().unwrap();
        assert_eq!(opportunity.archived_at, None);
        assert_eq!(opportunity.revision, 3);
    }

    #[test]
    fn deleting_an_unreferenced_opportunity_removes_it_and_its_interactions() {
        let (mut store, client_id) = test_store("delete-unreferenced");
        let id = create_opportunity(&mut store, client_id);
        Executor::new(&mut store)
            .execute(
                &LogInteraction {
                    opportunity_id: id,
                    kind: InteractionKind::Call,
                    note: "premier contact".into(),
                    occurred_at: None,
                },
                &human_ctx(),
            )
            .unwrap();

        Executor::new(&mut store)
            .execute(&DeleteOpportunity { id, revision: 1 }, &human_ctx())
            .unwrap();

        assert_eq!(opportunity_by_id(store.connection(), id).unwrap(), None);
        assert_eq!(list_interactions(store.connection(), id).unwrap().len(), 0);
    }

    #[test]
    fn deleting_a_quoted_opportunity_is_refused_and_names_the_quote() {
        let (mut store, client_id) = test_store("delete-quoted");
        let id = create_opportunity(&mut store, client_id);
        store
            .connection()
            .execute(
                "INSERT INTO quotes (id, client_id, opportunity_id, version, status, valid_until, created_at)
                 VALUES (?1, ?2, ?3, 1, 'draft', '2026-12-31', '2026-01-01T00:00:00Z')",
                params![
                    crate::domain::QuoteId::new().to_string(),
                    client_id.to_string(),
                    id.to_string(),
                ],
            )
            .unwrap();

        let err = Executor::new(&mut store)
            .execute(&DeleteOpportunity { id, revision: 1 }, &human_ctx())
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("devis")));
        assert!(opportunity_by_id(store.connection(), id).unwrap().is_some());
    }

    #[test]
    fn advancing_then_updating_with_the_pre_advance_revision_is_a_conflict() {
        let (mut store, client_id) = test_store("advance-then-update");
        let id = create_opportunity(&mut store, client_id);

        Executor::new(&mut store)
            .execute(
                &AdvanceOpportunity {
                    opportunity_id: id,
                    to: OpportunityStage::Negotiation,
                    next_action_at: date(2026, Month::September, 10),
                },
                &human_ctx(),
            )
            .unwrap();

        // La révision lue avant l'avancée (1) est désormais périmée : l'avancée l'a bumpée à 2.
        let err = Executor::new(&mut store)
            .execute(&update_opportunity(id, 1), &human_ctx())
            .unwrap_err();
        assert!(matches!(
            err,
            AppError::Conflict {
                entity: "opportunité",
                ..
            }
        ));
    }

    #[test]
    fn winning_and_losing_also_bump_the_revision() {
        let (mut store, client_id) = test_store("win-bumps-revision");
        let won = create_opportunity(&mut store, client_id);
        Executor::new(&mut store)
            .execute(
                &WinOpportunity {
                    opportunity_id: won,
                    started_on: date(2026, Month::October, 1),
                },
                &human_ctx(),
            )
            .unwrap();
        assert_eq!(
            opportunity_by_id(store.connection(), won)
                .unwrap()
                .unwrap()
                .revision,
            2
        );

        let lost = create_opportunity(&mut store, client_id);
        Executor::new(&mut store)
            .execute(
                &LoseOpportunity {
                    opportunity_id: lost,
                    reason: LossReason::Budget,
                },
                &human_ctx(),
            )
            .unwrap();
        assert_eq!(
            opportunity_by_id(store.connection(), lost)
                .unwrap()
                .unwrap()
                .revision,
            2
        );
    }

    #[test]
    fn interaction_lifecycle_create_update_delete_with_revisions() {
        let (mut store, client_id) = test_store("interaction-lifecycle");
        let opportunity_id = create_opportunity(&mut store, client_id);

        let Outcome::Applied(interaction_id) = Executor::new(&mut store)
            .execute(
                &LogInteraction {
                    opportunity_id,
                    kind: InteractionKind::Call,
                    note: "premier contact".into(),
                    occurred_at: None,
                },
                &human_ctx(),
            )
            .unwrap()
        else {
            panic!("expected Applied")
        };
        let interaction = interaction_by_id(store.connection(), interaction_id)
            .unwrap()
            .unwrap();
        assert_eq!(interaction.revision, 1);

        let Outcome::Applied(new_revision) = Executor::new(&mut store)
            .execute(
                &UpdateInteraction {
                    id: interaction_id,
                    revision: 1,
                    kind: InteractionKind::Meeting,
                    note: "rendez-vous".into(),
                    occurred_at: interaction.occurred_at,
                },
                &human_ctx(),
            )
            .unwrap()
        else {
            panic!("expected Applied")
        };
        assert_eq!(new_revision, 2);
        let updated = interaction_by_id(store.connection(), interaction_id)
            .unwrap()
            .unwrap();
        assert_eq!(updated.kind, InteractionKind::Meeting);
        assert_eq!(updated.note, "rendez-vous");

        Executor::new(&mut store)
            .execute(
                &DeleteInteraction {
                    id: interaction_id,
                    revision: 2,
                },
                &human_ctx(),
            )
            .unwrap();
        assert_eq!(
            interaction_by_id(store.connection(), interaction_id).unwrap(),
            None
        );
    }

    #[test]
    fn occurred_at_round_trips_through_log_and_update() {
        let (mut store, client_id) = test_store("occurred-at-round-trip");
        let opportunity_id = create_opportunity(&mut store, client_id);
        let occurred_at = OffsetDateTime::parse(
            "2026-08-15T09:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap();

        let Outcome::Applied(interaction_id) = Executor::new(&mut store)
            .execute(
                &LogInteraction {
                    opportunity_id,
                    kind: InteractionKind::Email,
                    note: "relance".into(),
                    occurred_at: Some(occurred_at),
                },
                &human_ctx(),
            )
            .unwrap()
        else {
            panic!("expected Applied")
        };
        let interaction = interaction_by_id(store.connection(), interaction_id)
            .unwrap()
            .unwrap();
        assert_eq!(interaction.occurred_at, occurred_at);
    }

    fn new_prospect(name: &str) -> CreateProspect {
        CreateProspect {
            prospect_name: name.into(),
            address: None,
            representative: None,
            email: None,
            phone: None,
            name: "Refonte plateforme".into(),
            amount: Money::from_cents(7_800_000),
            probability: Probability::new(40).unwrap(),
            next_action_at: date(2026, Month::September, 2),
            source: Some("recommandation".into()),
        }
    }

    #[test]
    fn creating_a_prospect_does_not_require_an_existing_client_and_stays_off_the_client_list() {
        let mut store = empty_store("create-prospect");
        let Outcome::Applied(opportunity_id) = Executor::new(&mut store)
            .execute(&new_prospect("Lumen Conseil"), &human_ctx())
            .unwrap()
        else {
            panic!("expected Applied")
        };

        let opportunity = row::opportunity_by_id(store.connection(), opportunity_id)
            .unwrap()
            .unwrap();
        let party = crate::clients::client_by_id(store.connection(), opportunity.client_id)
            .unwrap()
            .unwrap();
        assert_eq!(party.name, "Lumen Conseil");
        assert_eq!(
            crate::clients::list_clients_with(
                store.connection(),
                crate::clients::ClientFilter::ActiveOnly
            )
            .unwrap()
            .len(),
            0,
            "un prospect sans devis ni facture n'apparaît pas dans Clients"
        );
        assert_eq!(
            crate::clients::list_clients(store.connection())
                .unwrap()
                .len(),
            1,
            "list_clients reste inclusive pour la résolution de références"
        );
    }

    #[test]
    fn creating_a_prospect_records_optional_contact_and_address() {
        let mut store = empty_store("create-prospect-contact");
        let mut cmd = new_prospect("Nova Dev");
        cmd.representative = Some("Camille Martin".into());
        cmd.email = Some("camille@nova.example".into());
        cmd.phone = Some("06 12 34 56 78".into());
        cmd.address = Some(crate::domain::Address {
            street: "1 rue de la Paix".into(),
            postal_code: "75002".into(),
            city: "Paris".into(),
            country: "FR".into(),
        });

        let Outcome::Applied(opportunity_id) = Executor::new(&mut store)
            .execute(&cmd, &human_ctx())
            .unwrap()
        else {
            panic!("expected Applied")
        };
        let opportunity = row::opportunity_by_id(store.connection(), opportunity_id)
            .unwrap()
            .unwrap();
        let party = crate::clients::client_by_id(store.connection(), opportunity.client_id)
            .unwrap()
            .unwrap();
        assert_eq!(party.address.as_ref().unwrap().city, "Paris");
        let contacts = crate::clients::list_contacts(store.connection(), party.id).unwrap();
        assert_eq!(contacts.len(), 1);
        assert_eq!(contacts[0].name, "Camille Martin");
        assert_eq!(contacts[0].email.as_deref(), Some("camille@nova.example"));
        assert_eq!(contacts[0].phone.as_deref(), Some("06 12 34 56 78"));
    }

    #[test]
    fn creating_a_prospect_with_an_existing_exact_name_attaches_without_duplicating() {
        let mut store = empty_store("attach-existing");
        Executor::new(&mut store)
            .execute(&new_prospect("Lumen Conseil"), &human_ctx())
            .unwrap();
        Executor::new(&mut store)
            .execute(&new_prospect("lumen conseil"), &human_ctx())
            .unwrap();

        let parties = crate::clients::list_clients(store.connection()).unwrap();
        assert_eq!(parties.len(), 1);
        assert_eq!(
            crate::prospection::list_opportunities(store.connection())
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn a_quote_makes_the_prospect_appear_in_the_client_list() {
        let mut store = empty_store("quote-reveals");
        let Outcome::Applied(opportunity_id) = Executor::new(&mut store)
            .execute(&new_prospect("Lumen Conseil"), &human_ctx())
            .unwrap()
        else {
            panic!("expected Applied")
        };
        let client_id = row::opportunity_by_id(store.connection(), opportunity_id)
            .unwrap()
            .unwrap()
            .client_id;

        Executor::new(&mut store)
            .execute(
                &crate::quotes::CreateQuote {
                    client_id,
                    opportunity_id: Some(opportunity_id),
                    lines: vec![crate::domain::QuoteLine {
                        description: "Prestation".into(),
                        kind: crate::domain::LineKind::Forfait {
                            amount: Money::from_cents(100_000),
                        },
                        vat_rate: crate::domain::VatRate::Standard,
                    }],
                    discount: None,
                    terms: None,
                    valid_until: date(2026, Month::December, 31),
                },
                &human_ctx(),
            )
            .unwrap();

        let listed = crate::clients::list_clients_with(
            store.connection(),
            crate::clients::ClientFilter::ActiveOnly,
        )
        .unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "Lumen Conseil");
    }

    #[test]
    fn an_empty_prospect_name_is_refused() {
        let mut store = empty_store("empty-name");
        let err = Executor::new(&mut store)
            .execute(&new_prospect("   "), &human_ctx())
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("nom du prospect")));
    }

    #[test]
    fn updating_a_prospect_party_is_allowed_until_a_quote_exists() {
        let mut store = empty_store("update-prospect");
        let Outcome::Applied(opportunity_id) = Executor::new(&mut store)
            .execute(&new_prospect("Lumen Conseil"), &human_ctx())
            .unwrap()
        else {
            panic!("expected Applied")
        };
        let opportunity = row::opportunity_by_id(store.connection(), opportunity_id)
            .unwrap()
            .unwrap();

        let Outcome::Applied(_) = Executor::new(&mut store)
            .execute(
                &UpdateProspect {
                    id: opportunity_id,
                    revision: opportunity.revision,
                    client_revision: 1,
                    name: "Refonte v2".into(),
                    amount: opportunity.amount,
                    probability: opportunity.probability,
                    next_action_at: opportunity.next_action_at,
                    source: opportunity.source.clone(),
                    prospect_name: "Lumen Conseil SASU".into(),
                    address: None,
                    representative: Some("Camille".into()),
                    email: Some("camille@lumen.example".into()),
                    phone: None,
                },
                &human_ctx(),
            )
            .unwrap()
        else {
            panic!("expected Applied")
        };

        let party = crate::clients::client_by_id(store.connection(), opportunity.client_id)
            .unwrap()
            .unwrap();
        assert_eq!(party.name, "Lumen Conseil SASU");
        let contacts = crate::clients::list_contacts(store.connection(), party.id).unwrap();
        assert_eq!(contacts[0].name, "Camille");

        Executor::new(&mut store)
            .execute(
                &crate::quotes::CreateQuote {
                    client_id: opportunity.client_id,
                    opportunity_id: Some(opportunity_id),
                    lines: vec![crate::domain::QuoteLine {
                        description: "Prestation".into(),
                        kind: crate::domain::LineKind::Forfait {
                            amount: Money::from_cents(100_000),
                        },
                        vat_rate: crate::domain::VatRate::Standard,
                    }],
                    discount: None,
                    terms: None,
                    valid_until: date(2026, Month::December, 31),
                },
                &human_ctx(),
            )
            .unwrap();

        let err = Executor::new(&mut store)
            .execute(
                &UpdateProspect {
                    id: opportunity_id,
                    revision: 2,
                    client_revision: 2,
                    name: "Refonte v3".into(),
                    amount: opportunity.amount,
                    probability: opportunity.probability,
                    next_action_at: opportunity.next_action_at,
                    source: None,
                    prospect_name: "Ne doit pas changer".into(),
                    address: None,
                    representative: None,
                    email: None,
                    phone: None,
                },
                &human_ctx(),
            )
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("déjà un client")));
    }
}
