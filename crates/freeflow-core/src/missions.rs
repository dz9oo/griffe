//! Missions polymorphes (régie / forfait / récurrent), jalons, saisie de temps, échéancier de
//! facturation. Bâti sur la couche applicative (lot 2) ; `row` est `pub(crate)` car la
//! prospection (lot 3) y écrit aussi lorsqu'un gain d'opportunité crée une mission.

mod commands;
mod error;
mod queries;
mod schedule;

pub(crate) mod row;

pub use commands::{
    ArchiveMission, CloseMission, CreateMission, DeleteMission, DeleteTimeEntry, LogTime,
    ReopenMission, UnarchiveMission, UpdateMission, UpdateTimeEntry,
};
pub use error::MissionsError;
pub use queries::{
    MissionFilter, MissionReferences, MonthlyCapacity, effective_daily_rate, list_active_missions,
    list_missions, list_missions_with, list_time_entries, mission_by_id, mission_references,
    monthly_capacity, time_entry_by_id,
};
pub use schedule::{BillingSchedule, billing_schedule};

#[cfg(test)]
mod tests {
    use rusqlite::params;
    use time::{Date, Month as TimeMonth};

    use super::*;
    use crate::app::{Actor, AppError, ExecutionContext, Executor, Outcome};
    use crate::domain::{ClientId, Milestone, MissionId, MissionKind, Money, Month, TimeCategory};
    use crate::store::{Passphrase, Store};

    fn date(year: i32, month: TimeMonth, day: u8) -> Date {
        Date::from_calendar_date(year, month, day).unwrap()
    }

    fn test_store(label: &str) -> (Store, ClientId) {
        let dir = std::env::temp_dir().join(format!(
            "freeflow-missions-test-{label}-{}-{}",
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

    #[test]
    fn creating_a_regie_mission_and_logging_time_computes_the_effective_rate() {
        let (mut store, client_id) = test_store("regie-effective-rate");
        let create = CreateMission {
            client_id,
            quote_id: None,
            name: "Plateforme paiements".to_string(),
            kind: MissionKind::Regie {
                daily_rate: Money::from_cents(65_000),
            },
            milestones: Vec::new(),
            started_on: date(2026, TimeMonth::September, 1),
        };
        let Outcome::Applied(mission_id) = Executor::new(&mut store)
            .execute(&create, &human_ctx())
            .unwrap()
        else {
            panic!("expected Applied")
        };

        let log_billable = LogTime {
            mission_id,
            worked_on: date(2026, TimeMonth::September, 10),
            days: 9.5,
            category: TimeCategory::Billable,
            note: None,
        };
        Executor::new(&mut store)
            .execute(&log_billable, &human_ctx())
            .unwrap();

        // Le temps non facturable ne doit jamais entrer dans le calcul du TJM effectif.
        let log_admin = LogTime {
            mission_id,
            worked_on: date(2026, TimeMonth::September, 11),
            days: 2.0,
            category: TimeCategory::Admin,
            note: None,
        };
        Executor::new(&mut store)
            .execute(&log_admin, &human_ctx())
            .unwrap();

        let rate = effective_daily_rate(store.connection(), mission_id).unwrap();
        // Régie : TJM effectif == TJM contractuel par construction (CA = taux × jours facturés).
        assert_eq!(rate, Some(Money::from_cents(65_000)));
    }

    #[test]
    fn effective_daily_rate_reveals_scope_overrun_on_a_forfait_mission() {
        let (mut store, client_id) = test_store("forfait-effective-rate");
        let budget = Money::from_cents(4_500_000);
        let create = CreateMission {
            client_id,
            quote_id: None,
            name: "Refonte dashboard IoT".to_string(),
            kind: MissionKind::Forfait { budget },
            milestones: Vec::new(),
            started_on: date(2026, TimeMonth::September, 1),
        };
        let Outcome::Applied(mission_id) = Executor::new(&mut store)
            .execute(&create, &human_ctx())
            .unwrap()
        else {
            panic!("expected Applied")
        };

        let log = LogTime {
            mission_id,
            worked_on: date(2026, TimeMonth::September, 15),
            days: 38.5,
            category: TimeCategory::Billable,
            note: None,
        };
        Executor::new(&mut store)
            .execute(&log, &human_ctx())
            .unwrap();

        let rate = effective_daily_rate(store.connection(), mission_id)
            .unwrap()
            .unwrap();
        // 45 000 € ÷ 38,5 j : le forfait a coûté plus de jours que ce qu'un TJM confortable
        // aurait justifié — c'est précisément ce que ce calcul doit révéler.
        assert_eq!(rate, budget.divide_by_days(38.5));
        assert!(
            rate.cents() < Money::from_cents(150_000).cents(),
            "TJM effectif anormalement bas : dérapage détecté"
        );
    }

    #[test]
    fn effective_daily_rate_is_none_without_any_billable_day() {
        let (mut store, client_id) = test_store("no-billable-days");
        let create = CreateMission {
            client_id,
            quote_id: None,
            name: "Maintenance".to_string(),
            kind: MissionKind::Recurrent {
                monthly_amount: Money::from_cents(350_000),
            },
            milestones: Vec::new(),
            started_on: date(2026, TimeMonth::September, 1),
        };
        let Outcome::Applied(mission_id) = Executor::new(&mut store)
            .execute(&create, &human_ctx())
            .unwrap()
        else {
            panic!("expected Applied")
        };
        assert_eq!(
            effective_daily_rate(store.connection(), mission_id).unwrap(),
            None
        );
    }

    #[test]
    fn logging_time_on_an_unknown_mission_fails_cleanly() {
        let (mut store, _client_id) = test_store("log-time-unknown");
        let log = LogTime {
            mission_id: crate::domain::MissionId::new(),
            worked_on: date(2026, TimeMonth::September, 1),
            days: 1.0,
            category: TimeCategory::Billable,
            note: None,
        };
        let err = Executor::new(&mut store)
            .execute(&log, &human_ctx())
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("introuvable")));
    }

    #[test]
    fn monthly_capacity_counts_billable_days_across_every_mission() {
        let (mut store, client_id) = test_store("monthly-capacity");
        let create_a = CreateMission {
            client_id,
            quote_id: None,
            name: "Mission A".to_string(),
            kind: MissionKind::Regie {
                daily_rate: Money::from_cents(65_000),
            },
            milestones: Vec::new(),
            started_on: date(2026, TimeMonth::September, 1),
        };
        let create_b = CreateMission {
            name: "Mission B".to_string(),
            ..create_a.clone()
        };
        let Outcome::Applied(mission_a) = Executor::new(&mut store)
            .execute(&create_a, &human_ctx())
            .unwrap()
        else {
            panic!("expected Applied")
        };
        let Outcome::Applied(mission_b) = Executor::new(&mut store)
            .execute(&create_b, &human_ctx())
            .unwrap()
        else {
            panic!("expected Applied")
        };

        for (mission_id, day, amount) in [(mission_a, 2u8, 5.0), (mission_b, 3u8, 3.0)] {
            let log = LogTime {
                mission_id,
                worked_on: date(2026, TimeMonth::September, day),
                days: amount,
                category: TimeCategory::Billable,
                note: None,
            };
            Executor::new(&mut store)
                .execute(&log, &human_ctx())
                .unwrap();
        }
        // Une saisie non facturable, et une saisie hors du mois observé : ni l'une ni l'autre
        // ne doivent compter dans la capacité de septembre.
        Executor::new(&mut store)
            .execute(
                &LogTime {
                    mission_id: mission_a,
                    worked_on: date(2026, TimeMonth::September, 4),
                    days: 1.0,
                    category: TimeCategory::Admin,
                    note: None,
                },
                &human_ctx(),
            )
            .unwrap();
        Executor::new(&mut store)
            .execute(
                &LogTime {
                    mission_id: mission_a,
                    worked_on: date(2026, TimeMonth::October, 1),
                    days: 1.0,
                    category: TimeCategory::Billable,
                    note: None,
                },
                &human_ctx(),
            )
            .unwrap();

        let september = Month::new(2026, 9).unwrap();
        let capacity = monthly_capacity(store.connection(), september).unwrap();
        assert!((capacity.billable_days - 8.0).abs() < f64::EPSILON);
        assert_eq!(
            capacity.available_business_days,
            crate::domain::french_business_days_in(september.first_day(), september.last_day())
        );
        assert!(capacity.utilization_percent() > 0.0 && capacity.utilization_percent() < 100.0);
    }

    #[test]
    fn winning_an_opportunity_still_creates_a_mission_visible_to_this_module() {
        // Vérifie l'intégration avec le lot 3 : `WinOpportunity` écrit désormais via
        // `missions::row`, plus via une copie privée dans `prospection`.
        let (mut store, client_id) = test_store("cross-lot-integration");
        let opportunity = crate::prospection::CreateOpportunity {
            client_id,
            name: "Audit technique".to_string(),
            amount: Money::from_cents(1_500_000),
            probability: crate::domain::Probability::new(60).unwrap(),
            next_action_at: date(2026, TimeMonth::September, 2),
            source: None,
        };
        let Outcome::Applied(opportunity_id) = Executor::new(&mut store)
            .execute(&opportunity, &human_ctx())
            .unwrap()
        else {
            panic!("expected Applied")
        };
        let win = crate::prospection::WinOpportunity {
            opportunity_id,
            started_on: date(2026, TimeMonth::September, 3),
        };
        let Outcome::Applied(mission_id) = Executor::new(&mut store)
            .execute(&win, &human_ctx())
            .unwrap()
        else {
            panic!("expected Applied")
        };

        let mission = row::mission_by_id(store.connection(), mission_id)
            .unwrap()
            .unwrap();
        assert_eq!(mission.client_id, client_id);
        assert_eq!(
            mission.kind,
            MissionKind::Forfait {
                budget: Money::from_cents(1_500_000)
            }
        );
    }

    #[test]
    fn list_active_missions_excludes_missions_marked_ended() {
        let (mut store, client_id) = test_store("list-active");
        let create = CreateMission {
            client_id,
            quote_id: None,
            name: "Maintenance".to_string(),
            kind: MissionKind::Recurrent {
                monthly_amount: Money::from_cents(350_000),
            },
            milestones: Vec::new(),
            started_on: date(2026, TimeMonth::September, 1),
        };
        let Outcome::Applied(mission_id) = Executor::new(&mut store)
            .execute(&create, &human_ctx())
            .unwrap()
        else {
            panic!("expected Applied")
        };

        assert_eq!(list_active_missions(store.connection()).unwrap().len(), 1);

        Executor::new(&mut store)
            .execute(
                &CloseMission {
                    id: mission_id,
                    revision: 1,
                    ended_on: date(2026, TimeMonth::December, 31),
                },
                &human_ctx(),
            )
            .unwrap();
        assert!(list_active_missions(store.connection()).unwrap().is_empty());
    }

    fn create_mission(store: &mut Store, client_id: ClientId) -> MissionId {
        let create = CreateMission {
            client_id,
            quote_id: None,
            name: "Refonte dashboard IoT".to_string(),
            kind: MissionKind::Forfait {
                budget: Money::from_cents(4_500_000),
            },
            milestones: vec![
                Milestone {
                    label: "Acompte".to_string(),
                    share_bps: 3_000,
                    due_on: Some(date(2026, TimeMonth::September, 15)),
                },
                Milestone {
                    label: "Solde".to_string(),
                    share_bps: 7_000,
                    due_on: Some(date(2026, TimeMonth::November, 30)),
                },
            ],
            started_on: date(2026, TimeMonth::September, 1),
        };
        let Outcome::Applied(id) = Executor::new(store).execute(&create, &human_ctx()).unwrap()
        else {
            panic!("expected Applied")
        };
        id
    }

    #[test]
    fn updating_replaces_the_whole_milestone_collection_and_renumbers_positions() {
        let (mut store, client_id) = test_store("update-milestones");
        let id = create_mission(&mut store, client_id);
        let current = row::mission_by_id(store.connection(), id).unwrap().unwrap();

        let update = UpdateMission {
            id,
            revision: current.revision,
            name: current.name.clone(),
            kind: current.kind.clone(),
            milestones: vec![Milestone {
                label: "Livraison unique".to_string(),
                share_bps: 10_000,
                due_on: None,
            }],
            started_on: current.started_on,
            quote_id: current.quote_id,
        };
        let Outcome::Applied(new_revision) = Executor::new(&mut store)
            .execute(&update, &human_ctx())
            .unwrap()
        else {
            panic!("expected Applied")
        };
        assert_eq!(new_revision, 2);

        let updated = row::mission_by_id(store.connection(), id).unwrap().unwrap();
        assert_eq!(updated.milestones.len(), 1);
        assert_eq!(updated.milestones[0].label, "Livraison unique");
    }

    #[test]
    fn updating_cannot_repoint_the_quote() {
        let (mut store, client_id) = test_store("update-quote-immutable");
        let id = create_mission(&mut store, client_id);
        let current = row::mission_by_id(store.connection(), id).unwrap().unwrap();

        let update = UpdateMission {
            id,
            revision: current.revision,
            name: current.name.clone(),
            kind: current.kind.clone(),
            milestones: current.milestones.clone(),
            started_on: current.started_on,
            quote_id: Some(crate::domain::QuoteId::new()),
        };
        let err = Executor::new(&mut store)
            .execute(&update, &human_ctx())
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("devis")));
    }

    #[test]
    fn milestones_over_100_percent_are_refused() {
        let (mut store, client_id) = test_store("milestones-over-budget");
        let id = create_mission(&mut store, client_id);
        let current = row::mission_by_id(store.connection(), id).unwrap().unwrap();

        let update = UpdateMission {
            id,
            revision: current.revision,
            name: current.name.clone(),
            kind: current.kind.clone(),
            milestones: vec![
                Milestone {
                    label: "A".to_string(),
                    share_bps: 6_000,
                    due_on: None,
                },
                Milestone {
                    label: "B".to_string(),
                    share_bps: 6_000,
                    due_on: None,
                },
            ],
            started_on: current.started_on,
            quote_id: current.quote_id,
        };
        let err = Executor::new(&mut store)
            .execute(&update, &human_ctx())
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("100")));
    }

    #[test]
    fn closing_twice_is_refused() {
        let (mut store, client_id) = test_store("close-twice");
        let id = create_mission(&mut store, client_id);
        Executor::new(&mut store)
            .execute(
                &CloseMission {
                    id,
                    revision: 1,
                    ended_on: date(2026, TimeMonth::December, 31),
                },
                &human_ctx(),
            )
            .unwrap();
        let err = Executor::new(&mut store)
            .execute(
                &CloseMission {
                    id,
                    revision: 2,
                    ended_on: date(2026, TimeMonth::December, 31),
                },
                &human_ctx(),
            )
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("déjà clôturée")));
    }

    #[test]
    fn ending_before_starting_is_refused() {
        let (mut store, client_id) = test_store("ends-before-start");
        let id = create_mission(&mut store, client_id);
        let err = Executor::new(&mut store)
            .execute(
                &CloseMission {
                    id,
                    revision: 1,
                    ended_on: date(2026, TimeMonth::January, 1),
                },
                &human_ctx(),
            )
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(_)));
    }

    #[test]
    fn reopening_round_trips() {
        let (mut store, client_id) = test_store("reopen-round-trip");
        let id = create_mission(&mut store, client_id);
        Executor::new(&mut store)
            .execute(
                &CloseMission {
                    id,
                    revision: 1,
                    ended_on: date(2026, TimeMonth::December, 31),
                },
                &human_ctx(),
            )
            .unwrap();
        Executor::new(&mut store)
            .execute(&ReopenMission { id, revision: 2 }, &human_ctx())
            .unwrap();

        let mission = row::mission_by_id(store.connection(), id).unwrap().unwrap();
        assert_eq!(mission.ended_on, None);
        assert_eq!(mission.revision, 3);
    }

    #[test]
    fn a_closed_mission_and_an_archived_mission_are_two_different_things() {
        let (mut store, client_id) = test_store("close-vs-archive");
        let closed = create_mission(&mut store, client_id);
        Executor::new(&mut store)
            .execute(
                &CloseMission {
                    id: closed,
                    revision: 1,
                    ended_on: date(2026, TimeMonth::December, 31),
                },
                &human_ctx(),
            )
            .unwrap();
        // Close sans archiver : sort de la liste active, mais n'est pas archivée.
        let closed_mission = row::mission_by_id(store.connection(), closed)
            .unwrap()
            .unwrap();
        assert!(closed_mission.ended_on.is_some());
        assert!(closed_mission.archived_at.is_none());

        let archived = create_mission(&mut store, client_id);
        Executor::new(&mut store)
            .execute(
                &ArchiveMission {
                    id: archived,
                    revision: 1,
                },
                &human_ctx(),
            )
            .unwrap();
        // Archiver sans clore : sort aussi de la liste active, mais n'a pas de date de fin.
        let archived_mission = row::mission_by_id(store.connection(), archived)
            .unwrap()
            .unwrap();
        assert!(archived_mission.ended_on.is_none());
        assert!(archived_mission.archived_at.is_some());

        assert!(list_active_missions(store.connection()).unwrap().is_empty());
    }

    #[test]
    fn deleting_a_mission_with_time_entries_is_refused_and_names_them() {
        let (mut store, client_id) = test_store("delete-with-time-entries");
        let id = create_mission(&mut store, client_id);
        Executor::new(&mut store)
            .execute(
                &LogTime {
                    mission_id: id,
                    worked_on: date(2026, TimeMonth::September, 10),
                    days: 2.0,
                    category: TimeCategory::Billable,
                    note: None,
                },
                &human_ctx(),
            )
            .unwrap();

        let err = Executor::new(&mut store)
            .execute(&DeleteMission { id, revision: 1 }, &human_ctx())
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("saisie")));
        assert!(
            row::mission_by_id(store.connection(), id)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn deleting_a_mission_referenced_by_an_invoice_is_refused() {
        let (mut store, client_id) = test_store("delete-with-invoice");
        let id = create_mission(&mut store, client_id);
        store
            .connection()
            .execute(
                "INSERT INTO invoices (id, number, client_id, mission_id, status, issued_on, due_on, hash)
                 VALUES (?1, 'FA-2026-0001', ?2, ?3, 'issued', '2026-01-01', '2026-01-31', 'h')",
                params![
                    crate::domain::InvoiceId::new().to_string(),
                    client_id.to_string(),
                    id.to_string(),
                ],
            )
            .unwrap();

        let err = Executor::new(&mut store)
            .execute(&DeleteMission { id, revision: 1 }, &human_ctx())
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("facture")));
    }

    #[test]
    fn deleting_a_bare_mission_removes_its_milestones() {
        let (mut store, client_id) = test_store("delete-bare");
        let id = create_mission(&mut store, client_id);
        Executor::new(&mut store)
            .execute(&DeleteMission { id, revision: 1 }, &human_ctx())
            .unwrap();

        assert_eq!(row::mission_by_id(store.connection(), id).unwrap(), None);
        let milestone_count: i64 = store
            .connection()
            .query_row(
                "SELECT count(*) FROM milestones WHERE mission_id = ?1",
                [id.to_string()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(milestone_count, 0);
    }

    #[test]
    fn emptying_the_time_entries_then_deleting_succeeds() {
        let (mut store, client_id) = test_store("empty-then-delete");
        let id = create_mission(&mut store, client_id);
        let Outcome::Applied(entry_id) = Executor::new(&mut store)
            .execute(
                &LogTime {
                    mission_id: id,
                    worked_on: date(2026, TimeMonth::September, 10),
                    days: 2.0,
                    category: TimeCategory::Billable,
                    note: None,
                },
                &human_ctx(),
            )
            .unwrap()
        else {
            panic!("expected Applied")
        };

        Executor::new(&mut store)
            .execute(
                &DeleteTimeEntry {
                    id: entry_id,
                    revision: 1,
                },
                &human_ctx(),
            )
            .unwrap();
        Executor::new(&mut store)
            .execute(&DeleteMission { id, revision: 1 }, &human_ctx())
            .unwrap();
        assert_eq!(row::mission_by_id(store.connection(), id).unwrap(), None);
    }

    #[test]
    fn nan_zero_and_negative_days_are_refused() {
        let (mut store, client_id) = test_store("invalid-days");
        let id = create_mission(&mut store, client_id);
        for days in [f64::NAN, 0.0, -1.0] {
            let err = Executor::new(&mut store)
                .execute(
                    &LogTime {
                        mission_id: id,
                        worked_on: date(2026, TimeMonth::September, 10),
                        days,
                        category: TimeCategory::Billable,
                        note: None,
                    },
                    &human_ctx(),
                )
                .unwrap_err();
            assert!(matches!(err, AppError::Domain(msg) if msg.contains("jours")));
        }
    }

    #[test]
    fn time_entry_lifecycle_with_revisions() {
        let (mut store, client_id) = test_store("time-entry-lifecycle");
        let id = create_mission(&mut store, client_id);
        let Outcome::Applied(entry_id) = Executor::new(&mut store)
            .execute(
                &LogTime {
                    mission_id: id,
                    worked_on: date(2026, TimeMonth::September, 10),
                    days: 2.0,
                    category: TimeCategory::Billable,
                    note: None,
                },
                &human_ctx(),
            )
            .unwrap()
        else {
            panic!("expected Applied")
        };
        let entry = time_entry_by_id(store.connection(), entry_id)
            .unwrap()
            .unwrap();
        assert_eq!(entry.revision, 1);

        let Outcome::Applied(new_revision) = Executor::new(&mut store)
            .execute(
                &UpdateTimeEntry {
                    id: entry_id,
                    revision: 1,
                    worked_on: entry.worked_on,
                    days: 3.5,
                    category: TimeCategory::Admin,
                    note: Some("corrigé".to_string()),
                },
                &human_ctx(),
            )
            .unwrap()
        else {
            panic!("expected Applied")
        };
        assert_eq!(new_revision, 2);
        let updated = time_entry_by_id(store.connection(), entry_id)
            .unwrap()
            .unwrap();
        assert!((updated.days - 3.5).abs() < f64::EPSILON);
        assert_eq!(updated.category, TimeCategory::Admin);

        Executor::new(&mut store)
            .execute(
                &DeleteTimeEntry {
                    id: entry_id,
                    revision: 2,
                },
                &human_ctx(),
            )
            .unwrap();
        assert_eq!(
            time_entry_by_id(store.connection(), entry_id).unwrap(),
            None
        );
    }

    #[test]
    fn archiving_a_mission_still_blocks_deleting_its_client() {
        let (mut store, client_id) = test_store("archived-mission-blocks-client-delete");
        let id = create_mission(&mut store, client_id);
        Executor::new(&mut store)
            .execute(&ArchiveMission { id, revision: 1 }, &human_ctx())
            .unwrap();

        let refs = crate::clients::client_references(store.connection(), client_id).unwrap();
        assert_eq!(
            refs.missions, 1,
            "une mission archivée compte toujours comme référence"
        );
    }

    #[test]
    fn the_forecast_uses_the_real_milestone_due_dates_of_a_forfait_mission() {
        // Preuve du correctif du bug (a) : avant ce lot, `list_active_missions` chargeait les
        // missions sans leurs jalons, donc `forecast` calculait toujours une échéance implicite
        // à `today + 90 jours` pour un forfait, quels que soient ses jalons réels.
        let (mut store, client_id) = test_store("forecast-real-milestones");
        create_mission(&mut store, client_id);

        let today = date(2026, TimeMonth::August, 1);
        let inputs =
            crate::forecast::build_forecast_inputs(store.connection(), today, Money::from_cents(0))
                .unwrap();
        let due_on = inputs
            .pending_mission_revenue
            .iter()
            .map(|(due, _)| *due)
            .max()
            .unwrap();
        assert_eq!(
            due_on,
            date(2026, TimeMonth::November, 30),
            "l'échéance doit venir du vrai jalon « Solde », pas d'un repli à J+90"
        );
    }
}
