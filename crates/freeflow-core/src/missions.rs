//! Missions polymorphes (régie / forfait / récurrent), jalons, saisie de temps, échéancier de
//! facturation. Bâti sur la couche applicative (lot 2) ; `row` est `pub(crate)` car la
//! prospection (lot 3) y écrit aussi lorsqu'un gain d'opportunité crée une mission.

mod commands;
mod error;
mod queries;
mod schedule;

pub(crate) mod row;

pub use commands::{CreateMission, LogTime};
pub use error::MissionsError;
pub use queries::{MonthlyCapacity, effective_daily_rate, list_active_missions, monthly_capacity};
pub use schedule::{BillingSchedule, billing_schedule};

#[cfg(test)]
mod tests {
    use time::{Date, Month as TimeMonth};

    use super::*;
    use crate::app::{Actor, AppError, ExecutionContext, Executor, Outcome};
    use crate::domain::{ClientId, MissionKind, Money, Month, TimeCategory};
    use crate::store::Store;

    fn date(year: i32, month: TimeMonth, day: u8) -> Date {
        Date::from_calendar_date(year, month, day).unwrap()
    }

    fn test_store(label: &str) -> (Store, ClientId) {
        let dir = std::env::temp_dir().join(format!(
            "freeflow-missions-test-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ));
        let store = Store::open_with_passphrase(&dir.join("vault.db"), "s3cret").unwrap();
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

        // Aucune commande ne clôt encore une mission (hors périmètre v1) : on simule l'état
        // directement, pour verrouiller le comportement du filtre dès que ce jour arrivera.
        store
            .connection()
            .execute(
                "UPDATE missions SET ended_on = '2026-12-31' WHERE id = ?1",
                [mission_id.to_string()],
            )
            .unwrap();
        assert!(list_active_missions(store.connection()).unwrap().is_empty());
    }
}
