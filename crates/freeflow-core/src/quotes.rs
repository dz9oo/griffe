//! Devis : versions immuables, remise répartie sans perte de centime, acceptation → mission.
//! Bâti sur la couche applicative (lot 2) et réutilise `missions::row` (lot 4) pour la
//! persistance de la mission créée à l'acceptation.

mod commands;
mod error;
mod pricing;
mod queries;
mod row;

pub use commands::{AcceptQuote, CreateQuote, DeclineQuote, ReviseQuote, SendQuote};
pub use error::QuoteError;
pub use pricing::{derive_mission, priced_lines};
pub use queries::{QuoteReferences, list_quotes, quote_by_id, quote_references};

#[cfg(test)]
mod tests {
    use time::{Date, Month};

    use super::*;
    use crate::app::{Actor, AppError, ExecutionContext, Executor, Outcome};
    use crate::domain::{
        ClientId, Discount, LineKind, MissionKind, Money, QuoteId, QuoteLine, QuoteStatus, VatRate,
    };
    use crate::store::{Passphrase, Store};

    fn date(year: i32, month: Month, day: u8) -> Date {
        Date::from_calendar_date(year, month, day).unwrap()
    }

    fn test_store(label: &str) -> (Store, ClientId) {
        let dir = std::env::temp_dir().join(format!(
            "freeflow-quotes-test-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ));
        let store = Store::create(&dir.join("vault.db"), &Passphrase::from("s3cret")).unwrap();
        let client_id = row::seed_client(store.connection());
        (store, client_id)
    }

    fn human_ctx() -> ExecutionContext {
        ExecutionContext::new(Actor::Human, false)
    }

    fn forfait_lines_30_40_30(total_cents: i64) -> Vec<QuoteLine> {
        let amounts =
            Money::from_cents(total_cents).allocate_proportionally(&[3_000, 4_000, 3_000]);
        vec![
            QuoteLine {
                description: "Acompte".to_string(),
                kind: LineKind::Forfait { amount: amounts[0] },
                vat_rate: VatRate::Standard,
            },
            QuoteLine {
                description: "Mi-parcours".to_string(),
                kind: LineKind::Forfait { amount: amounts[1] },
                vat_rate: VatRate::Standard,
            },
            QuoteLine {
                description: "Solde".to_string(),
                kind: LineKind::Forfait { amount: amounts[2] },
                vat_rate: VatRate::Standard,
            },
        ]
    }

    fn create(
        store: &mut Store,
        client_id: ClientId,
        lines: Vec<QuoteLine>,
        discount: Option<Discount>,
    ) -> QuoteId {
        let cmd = CreateQuote {
            client_id,
            opportunity_id: None,
            lines,
            discount,
            terms: Some("Paiement à 30 jours".to_string()),
            valid_until: date(2026, Month::October, 31),
        };
        let Outcome::Applied(id) = Executor::new(store).execute(&cmd, &human_ctx()).unwrap() else {
            panic!("expected Applied")
        };
        id
    }

    #[test]
    fn a_quote_starts_as_draft_version_one_and_is_its_own_root() {
        let (mut store, client_id) = test_store("create");
        let id = create(
            &mut store,
            client_id,
            forfait_lines_30_40_30(4_500_000),
            None,
        );
        let quote = row::quote_by_id(store.connection(), id).unwrap().unwrap();
        assert_eq!(quote.version, 1);
        assert_eq!(quote.root_id, id);
        assert_eq!(quote.status, QuoteStatus::Draft);
    }

    #[test]
    fn revising_a_quote_creates_a_new_version_sharing_the_same_root() {
        let (mut store, client_id) = test_store("revise");
        let v1 = create(
            &mut store,
            client_id,
            forfait_lines_30_40_30(4_500_000),
            None,
        );

        let revise = ReviseQuote {
            root_id: v1,
            lines: forfait_lines_30_40_30(5_000_000),
            discount: None,
            terms: Some("Paiement à 30 jours, révisé".to_string()),
            valid_until: date(2026, Month::November, 30),
        };
        let Outcome::Applied(v2) = Executor::new(&mut store)
            .execute(&revise, &human_ctx())
            .unwrap()
        else {
            panic!("expected Applied")
        };

        let quote_v2 = row::quote_by_id(store.connection(), v2).unwrap().unwrap();
        assert_eq!(quote_v2.root_id, v1);
        assert_eq!(quote_v2.version, 2);

        // La version 1 originale n'a pas bougé.
        let quote_v1 = row::quote_by_id(store.connection(), v1).unwrap().unwrap();
        assert_eq!(quote_v1.version, 1);
    }

    #[test]
    fn a_quote_content_can_never_be_updated_or_deleted() {
        let (mut store, client_id) = test_store("immutability");
        let id = create(
            &mut store,
            client_id,
            forfait_lines_30_40_30(4_500_000),
            None,
        );

        let update_err = store
            .connection()
            .execute(
                "UPDATE quotes SET terms = 'modifié' WHERE id = ?1",
                [id.to_string()],
            )
            .unwrap_err();
        assert!(update_err.to_string().to_lowercase().contains("immuable"));

        let delete_err = store
            .connection()
            .execute("DELETE FROM quotes WHERE id = ?1", [id.to_string()])
            .unwrap_err();
        assert!(delete_err.to_string().to_lowercase().contains("supprim"));

        let line_update_err = store
            .connection()
            .execute(
                "UPDATE quote_lines SET description = 'modifié' WHERE quote_id = ?1",
                [id.to_string()],
            )
            .unwrap_err();
        assert!(
            line_update_err
                .to_string()
                .to_lowercase()
                .contains("immuable")
        );
    }

    #[test]
    fn sending_a_quote_is_allowed_to_change_its_status() {
        let (mut store, client_id) = test_store("send-status");
        let id = create(
            &mut store,
            client_id,
            forfait_lines_30_40_30(4_500_000),
            None,
        );
        Executor::new(&mut store)
            .execute(&SendQuote { quote_id: id }, &human_ctx())
            .unwrap();
        let quote = row::quote_by_id(store.connection(), id).unwrap().unwrap();
        assert_eq!(quote.status, QuoteStatus::Sent);
    }

    #[test]
    fn accepting_a_draft_quote_is_rejected() {
        let (mut store, client_id) = test_store("accept-draft");
        let id = create(
            &mut store,
            client_id,
            forfait_lines_30_40_30(4_500_000),
            None,
        );
        let err = Executor::new(&mut store)
            .execute(
                &AcceptQuote {
                    quote_id: id,
                    started_on: date(2026, Month::September, 1),
                },
                &human_ctx(),
            )
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("envoyé")));
    }

    #[test]
    fn accepting_a_forfait_quote_creates_a_mission_with_matching_milestones() {
        let (mut store, client_id) = test_store("accept-forfait");
        let id = create(
            &mut store,
            client_id,
            forfait_lines_30_40_30(4_500_000),
            None,
        );
        Executor::new(&mut store)
            .execute(&SendQuote { quote_id: id }, &human_ctx())
            .unwrap();

        let Outcome::Applied(mission_id) = Executor::new(&mut store)
            .execute(
                &AcceptQuote {
                    quote_id: id,
                    started_on: date(2026, Month::September, 1),
                },
                &human_ctx(),
            )
            .unwrap()
        else {
            panic!("expected Applied")
        };

        let mission = crate::missions::row::mission_by_id(store.connection(), mission_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            mission.kind,
            MissionKind::Forfait {
                budget: Money::from_cents(4_500_000)
            }
        );
        assert_eq!(mission.milestones.len(), 3);
        let shares: Vec<u32> = mission.milestones.iter().map(|m| m.share_bps).collect();
        assert_eq!(
            shares.iter().sum::<u32>(),
            10_000,
            "les parts des jalons doivent sommer à 100 %"
        );

        let quote = row::quote_by_id(store.connection(), id).unwrap().unwrap();
        assert_eq!(quote.status, QuoteStatus::Accepted);
    }

    #[test]
    fn accepting_a_regie_quote_with_two_lines_is_rejected() {
        let (mut store, client_id) = test_store("accept-regie-mixed");
        let lines = vec![
            QuoteLine {
                description: "Dev senior".to_string(),
                kind: LineKind::Regie {
                    daily_rate: Money::from_cents(65_000),
                    days: 10.0,
                },
                vat_rate: VatRate::Standard,
            },
            QuoteLine {
                description: "Dev junior".to_string(),
                kind: LineKind::Regie {
                    daily_rate: Money::from_cents(45_000),
                    days: 10.0,
                },
                vat_rate: VatRate::Standard,
            },
        ];
        let id = create(&mut store, client_id, lines, None);
        Executor::new(&mut store)
            .execute(&SendQuote { quote_id: id }, &human_ctx())
            .unwrap();

        let err = Executor::new(&mut store)
            .execute(
                &AcceptQuote {
                    quote_id: id,
                    started_on: date(2026, Month::September, 1),
                },
                &human_ctx(),
            )
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("mission")));
    }

    #[test]
    fn discount_allocation_never_loses_or_creates_a_cent() {
        let lines = forfait_lines_30_40_30(456_789); // montant non rond, pour éprouver l'arrondi.
        let priced = priced_lines(&lines, Some(Discount::Percentage(1_500))); // 15 %
        let total_gross: Money = priced.iter().map(|(g, _)| *g).sum();
        let total_discount: Money = priced.iter().map(|(_, d)| *d).sum();
        assert_eq!(total_gross, Money::from_cents(456_789));
        assert_eq!(
            total_discount,
            total_gross.apply_rate_bps(1_500),
            "la remise totale doit correspondre exactement au taux appliqué au total"
        );
    }

    #[test]
    fn list_quotes_loads_every_version_with_its_lines() {
        let (mut store, client_id) = test_store("list");
        let v1 = create(
            &mut store,
            client_id,
            forfait_lines_30_40_30(4_500_000),
            None,
        );
        let revise = ReviseQuote {
            root_id: v1,
            lines: forfait_lines_30_40_30(5_000_000),
            discount: None,
            terms: None,
            valid_until: date(2026, Month::November, 30),
        };
        Executor::new(&mut store)
            .execute(&revise, &human_ctx())
            .unwrap();

        let all = list_quotes(store.connection()).unwrap();
        assert_eq!(all.len(), 2);
        assert!(
            all.iter().all(|q| q.lines.len() == 3),
            "chaque version doit porter ses lignes, chargées par lot"
        );
        assert!(all.iter().all(|q| q.root_id == v1));
    }

    #[test]
    fn quote_references_count_versions_and_the_mission_born_from_acceptance() {
        let (mut store, client_id) = test_store("references");
        let id = create(
            &mut store,
            client_id,
            forfait_lines_30_40_30(4_500_000),
            None,
        );
        let before = quote_references(store.connection(), id).unwrap();
        assert_eq!(
            before,
            QuoteReferences {
                missions: 0,
                versions: 1
            }
        );

        Executor::new(&mut store)
            .execute(&SendQuote { quote_id: id }, &human_ctx())
            .unwrap();
        Executor::new(&mut store)
            .execute(
                &AcceptQuote {
                    quote_id: id,
                    started_on: date(2026, Month::September, 1),
                },
                &human_ctx(),
            )
            .unwrap();

        let after = quote_references(store.connection(), id).unwrap();
        assert_eq!(after.missions, 1);
    }

    #[test]
    fn accepting_a_quote_carries_its_opportunity_lineage_onto_the_mission() {
        let (mut store, client_id) = test_store("lineage");
        let opportunity_id = crate::domain::OpportunityId::new();
        store
            .connection()
            .execute(
                "INSERT INTO opportunities
                    (id, client_id, name, stage, amount_cents, probability_percent, created_at)
                 VALUES (?1, ?2, 'Refonte', 'proposal', 4500000, 60, '2026-01-01T00:00:00Z')",
                [opportunity_id.to_string(), client_id.to_string()],
            )
            .unwrap();
        let cmd = CreateQuote {
            client_id,
            opportunity_id: Some(opportunity_id),
            lines: forfait_lines_30_40_30(4_500_000),
            discount: None,
            terms: None,
            valid_until: date(2026, Month::October, 31),
        };
        let Outcome::Applied(quote_id) = Executor::new(&mut store)
            .execute(&cmd, &human_ctx())
            .unwrap()
        else {
            panic!("expected Applied")
        };
        Executor::new(&mut store)
            .execute(&SendQuote { quote_id }, &human_ctx())
            .unwrap();
        let Outcome::Applied(mission_id) = Executor::new(&mut store)
            .execute(
                &AcceptQuote {
                    quote_id,
                    started_on: date(2026, Month::September, 1),
                },
                &human_ctx(),
            )
            .unwrap()
        else {
            panic!("expected Applied")
        };

        let mission = crate::missions::row::mission_by_id(store.connection(), mission_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            mission.opportunity_id,
            Some(opportunity_id),
            "la lignée opportunité → devis → mission doit être persistée"
        );
    }

    #[test]
    fn fixed_amount_discount_never_exceeds_the_gross_total() {
        let lines = forfait_lines_30_40_30(100_000);
        let priced = priced_lines(
            &lines,
            Some(Discount::FixedAmount(Money::from_cents(999_999))),
        );
        let total_discount: Money = priced.iter().map(|(_, d)| *d).sum();
        assert_eq!(
            total_discount,
            Money::from_cents(100_000),
            "la remise ne peut jamais dépasser le total HT"
        );
    }
}
