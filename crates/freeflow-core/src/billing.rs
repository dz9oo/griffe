//! Facturation & TVA — le lot le plus sensible : numérotation sans trou, immuabilité imposée
//! par le store (triggers SQL, pas seulement l'application), chaînage cryptographique, TVA
//! arrondie par taux. Les mentions légales obligatoires (SIREN, indemnité forfaitaire, etc.)
//! et le rendu PDF/Factur-X sont un sujet de rendu, pas d'invariant de données : ils relèvent
//! du lot dédié au rendu de facture, pas de celui-ci.

mod commands;
mod error;
mod import;
mod queries;
mod row;
mod totals;

pub use commands::{
    EmitInvoice, EmittedInvoice, ImportBankTransactions, IssueCreditNote, ReconcileTransaction,
    RecordPayment,
};
pub use error::BillingError;
pub use import::{
    ImportError, ParsedTransaction, parse_csv_bank_statement, parse_ofx_bank_statement,
};
pub use queries::{
    AgedInvoice, AgingBucket, ChainStatus, aged_balance, invoice_by_id, list_invoices, verify_chain,
};
pub use totals::{InvoiceTotals, VatBreakdownLine, compute_totals};

#[cfg(test)]
mod tests {
    use time::{Date, Month};

    use super::*;
    use crate::app::{Actor, AppError, ExecutionContext, Executor, Outcome};
    use crate::domain::{ClientId, InvoiceLine, Money, PaymentMethod, VatRate};
    use crate::store::{Passphrase, Store};

    fn date(year: i32, month: Month, day: u8) -> Date {
        Date::from_calendar_date(year, month, day).unwrap()
    }

    fn test_store(label: &str) -> (Store, ClientId) {
        let dir = std::env::temp_dir().join(format!(
            "freeflow-billing-test-{label}-{}-{}",
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

    fn sample_lines() -> Vec<InvoiceLine> {
        vec![InvoiceLine {
            description: "Plateforme paiements — septembre".to_string(),
            quantity: 9.5,
            unit_price: Money::from_cents(65_000),
            vat_rate: VatRate::Standard,
        }]
    }

    fn emit(store: &mut Store, client_id: ClientId, issued_on: Date) -> EmittedInvoice {
        let cmd = EmitInvoice {
            client_id,
            mission_id: None,
            lines: sample_lines(),
            issued_on,
            payment_terms_days: 30,
        };
        let Outcome::Applied(emitted) = Executor::new(store).execute(&cmd, &human_ctx()).unwrap()
        else {
            panic!("expected Applied")
        };
        emitted
    }

    #[test]
    fn an_agent_cannot_emit_an_invoice_without_human_confirmation() {
        let (mut store, client_id) = test_store("agent-confirmation");
        let cmd = EmitInvoice {
            client_id,
            mission_id: None,
            lines: sample_lines(),
            issued_on: date(2026, Month::September, 1),
            payment_terms_days: 30,
        };
        let agent_ctx = ExecutionContext::new(
            Actor::Agent {
                session: "sess-1".into(),
            },
            false,
        );

        let outcome = Executor::new(&mut store).execute(&cmd, &agent_ctx).unwrap();
        assert!(matches!(outcome, Outcome::PendingConfirmation(_)));

        let count: i64 = store
            .connection()
            .query_row("SELECT count(*) FROM invoices", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            count, 0,
            "aucune facture ne doit être émise tant qu'un humain n'a pas confirmé"
        );
    }

    #[test]
    fn invoice_numbers_are_sequential_per_fiscal_year_starting_at_one() {
        let (mut store, client_id) = test_store("numbering-basic");
        let first = emit(&mut store, client_id, date(2026, Month::September, 1));
        let second = emit(&mut store, client_id, date(2026, Month::September, 15));
        let next_year = emit(&mut store, client_id, date(2027, Month::January, 5));

        assert_eq!(first.number, "FA-2026-0001");
        assert_eq!(second.number, "FA-2026-0002");
        assert_eq!(
            next_year.number, "FA-2027-0001",
            "l'exercice suivant repart à 1"
        );
    }

    #[test]
    fn emitting_with_no_lines_is_rejected() {
        let (mut store, client_id) = test_store("empty-invoice");
        let cmd = EmitInvoice {
            client_id,
            mission_id: None,
            lines: Vec::new(),
            issued_on: date(2026, Month::September, 1),
            payment_terms_days: 30,
        };
        let err = Executor::new(&mut store)
            .execute(&cmd, &human_ctx())
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("au moins une ligne")));
    }

    #[test]
    fn an_issued_invoice_can_never_be_updated_or_deleted() {
        let (mut store, client_id) = test_store("immutability");
        let emitted = emit(&mut store, client_id, date(2026, Month::September, 1));

        let update_err = store
            .connection()
            .execute(
                "UPDATE invoices SET number = 'FA-2026-9999' WHERE id = ?1",
                [emitted.id.to_string()],
            )
            .unwrap_err();
        assert!(update_err.to_string().to_lowercase().contains("immuable"));

        let delete_err = store
            .connection()
            .execute(
                "DELETE FROM invoices WHERE id = ?1",
                [emitted.id.to_string()],
            )
            .unwrap_err();
        assert!(delete_err.to_string().to_lowercase().contains("immuable"));

        let line_update_err = store
            .connection()
            .execute(
                "UPDATE invoice_lines SET quantity = 0 WHERE invoice_id = ?1",
                [emitted.id.to_string()],
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
    fn the_hash_chain_links_consecutive_invoices_and_verifies_intact() {
        let (mut store, client_id) = test_store("chain-intact");
        emit(&mut store, client_id, date(2026, Month::September, 1));
        emit(&mut store, client_id, date(2026, Month::September, 2));
        emit(&mut store, client_id, date(2026, Month::September, 3));

        assert_eq!(
            verify_chain(store.connection()).unwrap(),
            ChainStatus::Intact
        );
    }

    #[test]
    fn tampering_an_invoice_directly_in_the_database_breaks_the_chain() {
        let (mut store, client_id) = test_store("chain-tamper");
        emit(&mut store, client_id, date(2026, Month::September, 1));
        let second = emit(&mut store, client_id, date(2026, Month::September, 2));
        emit(&mut store, client_id, date(2026, Month::September, 3));

        assert_eq!(
            verify_chain(store.connection()).unwrap(),
            ChainStatus::Intact
        );

        // Simule un accès direct à la base contournant la couche applicative : le trigger
        // d'immuabilité est désactivé explicitement ici pour les besoins du test — c'est
        // exactement ce cas de figure (une altération hors du chemin normal) que le chaînage
        // cryptographique doit rester capable de détecter, indépendamment des triggers.
        store
            .connection()
            .execute_batch("DROP TRIGGER trg_invoice_lines_immutable_update;")
            .unwrap();
        store
            .connection()
            .execute(
                "UPDATE invoice_lines SET quantity = 999 WHERE invoice_id = ?1",
                [second.id.to_string()],
            )
            .unwrap();

        assert_eq!(
            verify_chain(store.connection()).unwrap(),
            ChainStatus::BrokenAt(second.id)
        );
    }

    #[test]
    fn a_credit_note_exactly_cancels_the_original_invoice() {
        let (mut store, client_id) = test_store("credit-note");
        let original = emit(&mut store, client_id, date(2026, Month::September, 1));

        let credit = IssueCreditNote {
            invoice_id: original.id,
            issued_on: date(2026, Month::September, 5),
        };
        let Outcome::Applied(credit_note) = Executor::new(&mut store)
            .execute(&credit, &human_ctx())
            .unwrap()
        else {
            panic!("expected Applied")
        };

        let original_invoice = row::invoice_by_id(store.connection(), original.id)
            .unwrap()
            .unwrap();
        let credit_invoice = row::invoice_by_id(store.connection(), credit_note.id)
            .unwrap()
            .unwrap();

        let original_totals = compute_totals(&original_invoice.lines);
        let credit_totals = compute_totals(&credit_invoice.lines);

        assert_eq!(
            original_totals.total_ttc + credit_totals.total_ttc,
            Money::ZERO,
            "la somme des deux factures doit retomber sur zéro"
        );
        assert_eq!(
            original_totals.total_vat + credit_totals.total_vat,
            Money::ZERO
        );
        assert_eq!(
            original_totals.subtotal_ht + credit_totals.subtotal_ht,
            Money::ZERO
        );
        assert_eq!(credit_invoice.credited_invoice_id, Some(original.id));
    }

    #[test]
    fn cannot_credit_the_same_invoice_twice() {
        let (mut store, client_id) = test_store("credit-note-twice");
        let original = emit(&mut store, client_id, date(2026, Month::September, 1));
        Executor::new(&mut store)
            .execute(
                &IssueCreditNote {
                    invoice_id: original.id,
                    issued_on: date(2026, Month::September, 5),
                },
                &human_ctx(),
            )
            .unwrap();

        let err = Executor::new(&mut store)
            .execute(
                &IssueCreditNote {
                    invoice_id: original.id,
                    issued_on: date(2026, Month::September, 6),
                },
                &human_ctx(),
            )
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("déjà un avoir")));
    }

    #[test]
    fn cannot_credit_a_credit_note() {
        let (mut store, client_id) = test_store("credit-note-of-credit-note");
        let original = emit(&mut store, client_id, date(2026, Month::September, 1));
        let Outcome::Applied(credit) = Executor::new(&mut store)
            .execute(
                &IssueCreditNote {
                    invoice_id: original.id,
                    issued_on: date(2026, Month::September, 5),
                },
                &human_ctx(),
            )
            .unwrap()
        else {
            panic!("expected Applied")
        };

        let err = Executor::new(&mut store)
            .execute(
                &IssueCreditNote {
                    invoice_id: credit.id,
                    issued_on: date(2026, Month::September, 6),
                },
                &human_ctx(),
            )
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("avoir sur un avoir")));
    }

    #[test]
    fn recording_a_payment_reduces_the_aged_balance() {
        let (mut store, client_id) = test_store("aged-balance");
        let invoice = emit(&mut store, client_id, date(2026, Month::August, 1)); // échéance 31 août
        let totals = compute_totals(&sample_lines());

        let aged_before =
            aged_balance(store.connection(), date(2026, Month::September, 30)).unwrap();
        assert_eq!(aged_before.len(), 1);
        assert_eq!(aged_before[0].outstanding, totals.total_ttc);
        assert_eq!(
            aged_before[0].bucket,
            AgingBucket::Due1To30,
            "30 j de retard tombe dans la tranche 1-30"
        );

        let pay = RecordPayment {
            invoice_id: invoice.id,
            amount: totals.total_ttc,
            received_on: date(2026, Month::September, 10),
            method: PaymentMethod::BankTransfer,
        };
        Executor::new(&mut store)
            .execute(&pay, &human_ctx())
            .unwrap();

        let aged_after =
            aged_balance(store.connection(), date(2026, Month::September, 30)).unwrap();
        assert!(
            aged_after.is_empty(),
            "une facture intégralement payée sort de la balance âgée"
        );
    }

    #[test]
    fn importing_the_same_csv_twice_does_not_duplicate_transactions() {
        let (mut store, _client_id) = test_store("import-dedup");
        let csv = "date;description;montant\n2026-08-30;Virement Lumen Bank;7800.00\n";
        let parsed = parse_csv_bank_statement(csv).unwrap();

        let first = Executor::new(&mut store)
            .execute(
                &ImportBankTransactions {
                    transactions: parsed.clone(),
                },
                &human_ctx(),
            )
            .unwrap();
        let second = Executor::new(&mut store)
            .execute(
                &ImportBankTransactions {
                    transactions: parsed,
                },
                &human_ctx(),
            )
            .unwrap();

        assert_eq!(first, Outcome::Applied(1));
        assert_eq!(
            second,
            Outcome::Applied(0),
            "le même relevé réimporté ne doit rien ajouter"
        );
    }

    #[test]
    fn reconciling_a_transaction_records_a_matching_payment() {
        let (mut store, client_id) = test_store("reconcile");
        let invoice = emit(&mut store, client_id, date(2026, Month::September, 1));
        let totals = compute_totals(&sample_lines());

        let csv = format!(
            "date;description;montant\n2026-09-05;Virement client;{:.2}\n",
            totals.total_ttc.euros()
        );
        let parsed = parse_csv_bank_statement(&csv).unwrap();
        Executor::new(&mut store)
            .execute(
                &ImportBankTransactions {
                    transactions: parsed,
                },
                &human_ctx(),
            )
            .unwrap();

        let tx_id: crate::domain::BankTransactionId = store
            .connection()
            .query_row("SELECT id FROM bank_transactions LIMIT 1", [], |row| {
                row.get::<_, String>(0)
            })
            .unwrap()
            .parse()
            .unwrap();

        Executor::new(&mut store)
            .execute(
                &ReconcileTransaction {
                    transaction_id: tx_id,
                    invoice_id: invoice.id,
                },
                &human_ctx(),
            )
            .unwrap();

        let aged = aged_balance(store.connection(), date(2026, Month::September, 30)).unwrap();
        assert!(
            aged.is_empty(),
            "le rapprochement doit avoir soldé la facture"
        );
    }

    #[test]
    fn list_invoices_returns_the_most_recently_emitted_invoice_first() {
        let (mut store, client_id) = test_store("list-invoices");
        let first = emit(&mut store, client_id, date(2026, Month::August, 1));
        let second = emit(&mut store, client_id, date(2026, Month::September, 1));

        let listed = list_invoices(store.connection()).unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].id, second.id);
        assert_eq!(listed[1].id, first.id);
    }
}
