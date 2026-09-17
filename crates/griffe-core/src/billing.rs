//! Facturation & TVA — le lot le plus sensible : numérotation sans trou, immuabilité imposée
//! par le store (triggers SQL, pas seulement l'application), chaînage cryptographique, TVA
//! arrondie par taux. Les mentions légales obligatoires (SIREN, indemnité forfaitaire, etc.)
//! et le rendu PDF/Factur-X sont un sujet de rendu, pas d'invariant de données : ils relèvent
//! du lot dédié au rendu de facture, pas de celui-ci.

mod commands;
mod error;
pub(crate) mod import;
mod queries;
mod row;
mod totals;

pub use commands::{
    DeleteBankTransaction, EmitInvoice, EmittedInvoice, ImportBankTransactions,
    ImportIssuedInvoice, IssueCreditNote, ReconcileTransaction, RecordPayment,
    SettleBankTransaction, UnreconcileTransaction, UnsettleBankTransaction, VoidPayment,
};
pub use error::BillingError;
pub use import::{
    DetectedDialect, EXPECTED_FORMAT, ImportError, ParsedStatement, ParsedTransaction,
    StatementFormat, parse_bank_statement, parse_csv_bank_statement, parse_ofx_bank_statement,
};
pub use queries::{
    AgedInvoice, AgingBucket, ChainStatus, aged_balance, bank_transaction_by_id, invoice_by_id,
    invoice_by_number, list_bank_transactions, list_invoices, list_payments,
    new_transactions_among, paid_amount, payment_by_id, payments_for_invoice, unmatched_debits,
    verify_chain,
};
pub(crate) use row::{
    bank_transaction_for_expense, clear_transaction_match, mark_transaction_matched_expense,
};
pub use totals::{InvoiceTotals, VatBreakdownLine, compute_totals};

#[cfg(test)]
mod tests {
    use time::{Date, Month};

    use super::*;
    use crate::app::{Actor, AppError, ExecutionContext, Executor, Outcome};
    use crate::domain::{ClientId, InvoiceLine, InvoiceOrigin, Money, PaymentMethod, VatRate};
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
    fn an_invoice_cancelled_by_a_credit_note_leaves_the_aged_balance() {
        // Lot 35 : trouvé par le scénario de preuve — une facture annulée restait « non
        // encaissée » en entier dans la balance âgée (donc au tableau de bord, dans le
        // prévisionnel et dans le parcours de clôture).
        let (mut store, client_id) = test_store("credit-note-aged");
        let original = emit(&mut store, client_id, date(2026, Month::September, 1));
        let kept = emit(&mut store, client_id, date(2026, Month::September, 2));
        let before = aged_balance(store.connection(), date(2026, Month::October, 15)).unwrap();
        assert_eq!(before.len(), 2);

        Executor::new(&mut store)
            .execute(
                &IssueCreditNote {
                    invoice_id: original.id,
                    issued_on: date(2026, Month::September, 5),
                },
                &human_ctx(),
            )
            .unwrap();

        let aged = aged_balance(store.connection(), date(2026, Month::October, 15)).unwrap();
        assert_eq!(aged.len(), 1, "{aged:?}");
        assert_eq!(aged[0].invoice_id, kept.id);
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

    fn record_full_payment(
        store: &mut Store,
        invoice_id: crate::domain::InvoiceId,
    ) -> crate::domain::PaymentId {
        let totals = compute_totals(&sample_lines());
        let Outcome::Applied(payment_id) = Executor::new(store)
            .execute(
                &RecordPayment {
                    invoice_id,
                    amount: totals.total_ttc,
                    received_on: date(2026, Month::September, 10),
                    method: PaymentMethod::BankTransfer,
                },
                &human_ctx(),
            )
            .unwrap()
        else {
            panic!("expected Applied")
        };
        payment_id
    }

    /// Importe une transaction créditrice du montant TTC de la facture d'échantillon et renvoie
    /// son id — via la query publique, plus de SQL manuel (comblé au lot 22).
    fn import_matching_transaction(store: &mut Store) -> crate::domain::BankTransactionId {
        let totals = compute_totals(&sample_lines());
        let csv = format!(
            "date;description;montant\n2026-09-05;Virement client;{:.2}\n",
            totals.total_ttc.euros()
        );
        let parsed = parse_csv_bank_statement(&csv).unwrap();
        Executor::new(store)
            .execute(
                &ImportBankTransactions {
                    transactions: parsed,
                },
                &human_ctx(),
            )
            .unwrap();
        list_bank_transactions(store.connection()).unwrap()[0].id
    }

    #[test]
    fn voiding_a_payment_restores_the_invoice_to_the_aged_balance() {
        let (mut store, client_id) = test_store("void-payment");
        let invoice = emit(&mut store, client_id, date(2026, Month::September, 1));
        let payment_id = record_full_payment(&mut store, invoice.id);
        assert!(
            aged_balance(store.connection(), date(2026, Month::October, 15))
                .unwrap()
                .is_empty()
        );

        Executor::new(&mut store)
            .execute(
                &VoidPayment {
                    payment_id,
                    reason: Some("saisi en double".to_string()),
                },
                &human_ctx(),
            )
            .unwrap();

        let aged = aged_balance(store.connection(), date(2026, Month::October, 15)).unwrap();
        assert_eq!(
            aged.len(),
            1,
            "un encaissement annulé ne solde plus la facture"
        );

        // Contre-écriture, pas suppression : le paiement reste dans l'historique, marqué annulé.
        let listed = list_payments(store.connection()).unwrap();
        assert_eq!(listed.len(), 1);
        assert!(listed[0].is_voided());
        assert_eq!(
            paid_amount(store.connection(), invoice.id).unwrap(),
            Money::ZERO
        );
    }

    #[test]
    fn voiding_a_payment_twice_is_rejected() {
        let (mut store, client_id) = test_store("void-twice");
        let invoice = emit(&mut store, client_id, date(2026, Month::September, 1));
        let payment_id = record_full_payment(&mut store, invoice.id);
        let void = VoidPayment {
            payment_id,
            reason: None,
        };
        Executor::new(&mut store)
            .execute(&void, &human_ctx())
            .unwrap();
        let err = Executor::new(&mut store)
            .execute(&void, &human_ctx())
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("déjà annulé")));
    }

    #[test]
    fn an_agent_voiding_a_payment_only_files_a_pending_action() {
        let (mut store, client_id) = test_store("void-agent");
        let invoice = emit(&mut store, client_id, date(2026, Month::September, 1));
        let payment_id = record_full_payment(&mut store, invoice.id);
        let outcome = Executor::new(&mut store)
            .execute(
                &VoidPayment {
                    payment_id,
                    reason: None,
                },
                &ExecutionContext::new(
                    Actor::Agent {
                        session: "sess-1".into(),
                    },
                    false,
                ),
            )
            .unwrap();
        assert!(matches!(outcome, Outcome::PendingConfirmation(_)));
        assert!(
            !list_payments(store.connection()).unwrap()[0].is_voided(),
            "rien ne doit être annulé tant qu'un humain n'a pas confirmé"
        );
    }

    #[test]
    fn reconciling_an_already_reconciled_transaction_is_rejected() {
        let (mut store, client_id) = test_store("reconcile-twice");
        let invoice = emit(&mut store, client_id, date(2026, Month::September, 1));
        let tx_id = import_matching_transaction(&mut store);
        let reconcile = ReconcileTransaction {
            transaction_id: tx_id,
            invoice_id: invoice.id,
        };
        Executor::new(&mut store)
            .execute(&reconcile, &human_ctx())
            .unwrap();

        // Avant le lot 22, ce second rapprochement créait silencieusement un doublon
        // d'encaissement pour la même ligne de relevé.
        let err = Executor::new(&mut store)
            .execute(&reconcile, &human_ctx())
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("déjà rapprochée")));
        assert_eq!(
            list_payments(store.connection()).unwrap().len(),
            1,
            "pas de second encaissement créé"
        );
    }

    #[test]
    fn unreconciling_frees_the_transaction_and_voids_the_payment_it_created() {
        let (mut store, client_id) = test_store("unreconcile");
        let invoice = emit(&mut store, client_id, date(2026, Month::September, 1));
        let tx_id = import_matching_transaction(&mut store);
        Executor::new(&mut store)
            .execute(
                &ReconcileTransaction {
                    transaction_id: tx_id,
                    invoice_id: invoice.id,
                },
                &human_ctx(),
            )
            .unwrap();

        let Outcome::Applied(voided) = Executor::new(&mut store)
            .execute(
                &UnreconcileTransaction {
                    transaction_id: tx_id,
                },
                &human_ctx(),
            )
            .unwrap()
        else {
            panic!("expected Applied")
        };
        assert!(
            voided.is_some(),
            "l'encaissement issu du rapprochement doit être retrouvé par sa lignée et annulé"
        );

        let tx = &list_bank_transactions(store.connection()).unwrap()[0];
        assert_eq!(tx.matched_invoice_id, None, "la transaction est libérée");
        assert!(list_payments(store.connection()).unwrap()[0].is_voided());
        assert_eq!(
            aged_balance(store.connection(), date(2026, Month::October, 15))
                .unwrap()
                .len(),
            1,
            "la facture redevient impayée"
        );

        // Et la transaction libérée peut être rapprochée à nouveau (vers la bonne facture).
        Executor::new(&mut store)
            .execute(
                &ReconcileTransaction {
                    transaction_id: tx_id,
                    invoice_id: invoice.id,
                },
                &human_ctx(),
            )
            .unwrap();
        assert!(
            aged_balance(store.connection(), date(2026, Month::October, 15))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn unreconciling_an_unmatched_transaction_is_rejected() {
        let (mut store, _client_id) = test_store("unreconcile-unmatched");
        let tx_id = import_matching_transaction(&mut store);
        let err = Executor::new(&mut store)
            .execute(
                &UnreconcileTransaction {
                    transaction_id: tx_id,
                },
                &human_ctx(),
            )
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("pas rapprochée")));
    }

    #[test]
    fn voiding_a_reconciled_payment_also_frees_its_transaction() {
        let (mut store, client_id) = test_store("void-reconciled");
        let invoice = emit(&mut store, client_id, date(2026, Month::September, 1));
        let tx_id = import_matching_transaction(&mut store);
        let Outcome::Applied(payment_id) = Executor::new(&mut store)
            .execute(
                &ReconcileTransaction {
                    transaction_id: tx_id,
                    invoice_id: invoice.id,
                },
                &human_ctx(),
            )
            .unwrap()
        else {
            panic!("expected Applied")
        };

        Executor::new(&mut store)
            .execute(
                &VoidPayment {
                    payment_id,
                    reason: None,
                },
                &human_ctx(),
            )
            .unwrap();
        let tx = &list_bank_transactions(store.connection()).unwrap()[0];
        assert_eq!(
            tx.matched_invoice_id, None,
            "laisser la transaction « rapprochée » vers une facture sans encaissement mentirait"
        );
    }

    #[test]
    fn unreconciling_a_pre_0013_reconciliation_frees_the_transaction_without_guessing() {
        let (mut store, client_id) = test_store("unreconcile-legacy");
        let invoice = emit(&mut store, client_id, date(2026, Month::September, 1));
        let tx_id = import_matching_transaction(&mut store);
        Executor::new(&mut store)
            .execute(
                &ReconcileTransaction {
                    transaction_id: tx_id,
                    invoice_id: invoice.id,
                },
                &human_ctx(),
            )
            .unwrap();
        // Simule un rapprochement antérieur à la migration 0013 : la lignée n'existe pas.
        store
            .connection()
            .execute("UPDATE payments SET bank_transaction_id = NULL", [])
            .unwrap();

        let Outcome::Applied(voided) = Executor::new(&mut store)
            .execute(
                &UnreconcileTransaction {
                    transaction_id: tx_id,
                },
                &human_ctx(),
            )
            .unwrap()
        else {
            panic!("expected Applied")
        };
        assert_eq!(
            voided, None,
            "sans lignée, aucun paiement n'est annulé par heuristique — jamais deviner"
        );
        let tx = &list_bank_transactions(store.connection()).unwrap()[0];
        assert_eq!(tx.matched_invoice_id, None);
        assert!(
            !list_payments(store.connection()).unwrap()[0].is_voided(),
            "le paiement orphelin reste intact, à annuler explicitement via VoidPayment"
        );
    }

    /// Lot 37 : un mouvement du relevé peut régler un compte de bilan — dette reprise (401),
    /// solde d'IS (444), crédit de TVA remboursé (445670) — sans charge ni produit. Une
    /// transaction déjà rapprochée ne se règle pas ; un compte hors plan exige un libellé ;
    /// défaire un règlement rend la transaction « à rapprocher ».
    #[test]
    #[allow(clippy::too_many_lines)]
    fn a_statement_line_can_settle_a_balance_sheet_account_and_be_unsettled() {
        let (mut store, _client) = test_store("settle");
        let Outcome::Applied(2) = Executor::new(&mut store)
            .execute(
                &ImportBankTransactions {
                    transactions: vec![
                        ParsedTransaction {
                            occurred_on: date(2026, Month::October, 10),
                            amount_cents: -60_000,
                            description: "VIR CABINET COMPTA".to_string(),
                            fitid: None,
                        },
                        ParsedTransaction {
                            occurred_on: date(2026, Month::November, 2),
                            amount_cents: 21_000,
                            description: "REMBOURSEMENT CREDIT TVA".to_string(),
                            fitid: None,
                        },
                    ],
                },
                &human_ctx(),
            )
            .unwrap()
        else {
            panic!("expected two imported transactions")
        };
        let transactions = list_bank_transactions(store.connection()).unwrap();
        let debit = transactions.iter().find(|t| t.is_debit()).unwrap().id;
        let credit = transactions.iter().find(|t| !t.is_debit()).unwrap().id;

        // Un agent propose, un humain confirme.
        let agent = ExecutionContext::new(
            Actor::Agent {
                session: "s".into(),
            },
            false,
        );
        assert!(matches!(
            Executor::new(&mut store)
                .execute(
                    &SettleBankTransaction {
                        transaction_id: debit,
                        account: "401000".parse().unwrap(),
                        label: None,
                    },
                    &agent
                )
                .unwrap(),
            Outcome::PendingConfirmation(_)
        ));

        // Compte hors plan sans libellé : refus explicite.
        let err = Executor::new(&mut store)
            .execute(
                &SettleBankTransaction {
                    transaction_id: debit,
                    account: "467100".parse().unwrap(),
                    label: None,
                },
                &human_ctx(),
            )
            .unwrap_err();
        assert!(err.to_string().contains("libellé"), "{err}");

        Executor::new(&mut store)
            .execute(
                &SettleBankTransaction {
                    transaction_id: debit,
                    account: "401000".parse().unwrap(),
                    label: Some("ignoré : le plan fixe prime".into()),
                },
                &human_ctx(),
            )
            .unwrap();
        Executor::new(&mut store)
            .execute(
                &SettleBankTransaction {
                    transaction_id: credit,
                    account: "445670".parse().unwrap(),
                    label: None,
                },
                &human_ctx(),
            )
            .unwrap();
        let settled = bank_transaction_by_id(store.connection(), debit)
            .unwrap()
            .unwrap();
        assert!(settled.is_matched() && settled.is_settled());
        assert_eq!(settled.settlement_account.unwrap().as_str(), "401000");
        assert_eq!(settled.settlement_label.as_deref(), Some("Fournisseurs"));
        assert!(unmatched_debits(store.connection()).unwrap().is_empty());

        // Déjà réglée : ni un second règlement, ni un rapprochement de facture.
        let err = Executor::new(&mut store)
            .execute(
                &SettleBankTransaction {
                    transaction_id: debit,
                    account: "444000".parse().unwrap(),
                    label: None,
                },
                &human_ctx(),
            )
            .unwrap_err();
        assert!(err.to_string().contains("déjà rapprochée"), "{err}");

        Executor::new(&mut store)
            .execute(
                &UnsettleBankTransaction {
                    transaction_id: debit,
                },
                &human_ctx(),
            )
            .unwrap();
        let freed = bank_transaction_by_id(store.connection(), debit)
            .unwrap()
            .unwrap();
        assert!(!freed.is_matched());
        assert_eq!(freed.settlement_label, None);
        let err = Executor::new(&mut store)
            .execute(
                &UnsettleBankTransaction {
                    transaction_id: debit,
                },
                &human_ctx(),
            )
            .unwrap_err();
        assert!(err.to_string().contains("n'est pas un règlement"), "{err}");
    }

    /// Lot 38 : l'identifiant de banque dédoublonne un ré-import même sous un autre format
    /// (libellés différents) ; sans identifiant, le triplet date/montant/libellé fait foi, comme
    /// avant ; une transaction non rapprochée se supprime, une rapprochée non.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn re_importing_the_same_statement_under_another_format_creates_no_duplicate_with_fitid() {
        let (mut store, _client) = test_store("fitid-dedup");
        let import = |store: &mut Store, bytes: &[u8]| -> u32 {
            let parsed = parse_bank_statement(bytes, None).unwrap();
            let Outcome::Applied(n) = Executor::new(store)
                .execute(
                    &ImportBankTransactions {
                        transactions: parsed.transactions,
                    },
                    &human_ctx(),
                )
                .unwrap()
            else {
                panic!("expected Applied")
            };
            n
        };
        // Un CSV qui porte les mêmes identifiants que l'OFX, avec d'autres libellés.
        let csv = b"Transaction ID;Date;Libell\xe9;Montant\n\
                    20260115-0001;15/01/2026;FRAIS;-12,50\n\
                    20260120-0002;20/01/2026;CABINET;-600,00\n\
                    20260128-0003;28/01/2026;LUMEN;1200,00\n";
        assert_eq!(import(&mut store, csv), 3);
        assert_eq!(
            import(&mut store, include_bytes!("billing/fixtures/ofx1.ofx")),
            0
        );
        assert_eq!(
            import(&mut store, include_bytes!("billing/fixtures/ofx2.ofx")),
            0
        );
        assert_eq!(import(&mut store, csv), 0);
        // Sans identifiant, seul le triplet exact dédoublonne.
        let plain =
            b"date;description;montant\n2026-01-15;FRAIS;-12.50\n2026-01-15;FRAIS BIS;-12.50\n";
        assert_eq!(import(&mut store, plain), 1);
        assert_eq!(import(&mut store, plain), 0);
        let all = list_bank_transactions(store.connection()).unwrap();
        assert_eq!(all.len(), 4);
        assert_eq!(
            all.iter().filter(|t| t.fitid.is_some()).count(),
            3,
            "les trois identifiants sont conservés"
        );
        let fresh = new_transactions_among(
            store.connection(),
            &parse_bank_statement(include_bytes!("billing/fixtures/ofx1.ofx"), None)
                .unwrap()
                .transactions,
        )
        .unwrap();
        assert_eq!(fresh, vec![false, false, false]);

        // Suppression : refusée sur une transaction rapprochée, faite sinon.
        let victim = all
            .iter()
            .find(|t| t.description == "FRAIS BIS")
            .unwrap()
            .id;
        Executor::new(&mut store)
            .execute(
                &SettleBankTransaction {
                    transaction_id: victim,
                    account: "401000".parse().unwrap(),
                    label: None,
                },
                &human_ctx(),
            )
            .unwrap();
        let err = Executor::new(&mut store)
            .execute(
                &DeleteBankTransaction {
                    transaction_id: victim,
                },
                &human_ctx(),
            )
            .unwrap_err();
        assert!(err.to_string().contains("rapprochée"), "{err}");
        Executor::new(&mut store)
            .execute(
                &UnsettleBankTransaction {
                    transaction_id: victim,
                },
                &human_ctx(),
            )
            .unwrap();
        let agent = ExecutionContext::new(
            Actor::Agent {
                session: "s".into(),
            },
            false,
        );
        assert!(matches!(
            Executor::new(&mut store)
                .execute(
                    &DeleteBankTransaction {
                        transaction_id: victim
                    },
                    &agent
                )
                .unwrap(),
            Outcome::PendingConfirmation(_)
        ));
        Executor::new(&mut store)
            .execute(
                &DeleteBankTransaction {
                    transaction_id: victim,
                },
                &human_ctx(),
            )
            .unwrap();
        assert_eq!(list_bank_transactions(store.connection()).unwrap().len(), 3);
    }

    fn camille_line() -> InvoiceLine {
        InvoiceLine {
            description: "Mission Camille".into(),
            quantity: 1.0,
            unit_price: Money::from_cents(500_000),
            vat_rate: VatRate::Standard,
        }
    }

    fn import_cmd(client_id: ClientId) -> ImportIssuedInvoice {
        ImportIssuedInvoice {
            number: "FAC-2026-0042".into(),
            client_id,
            mission_id: None,
            lines: vec![camille_line()],
            issued_on: date(2026, Month::September, 16),
            payment_terms_days: 30,
            credited_invoice_id: None,
        }
    }

    #[test]
    fn importing_a_tiime_invoice_keeps_its_number_and_does_not_allocate_fa() {
        let (mut store, client_id) = test_store("import-tiime");
        let Outcome::Applied(emitted) = Executor::new(&mut store)
            .execute(&import_cmd(client_id), &human_ctx())
            .unwrap()
        else {
            panic!("expected Applied")
        };
        assert_eq!(emitted.number, "FAC-2026-0042");
        let got = invoice_by_id(store.connection(), emitted.id)
            .unwrap()
            .unwrap();
        assert_eq!(got.origin, InvoiceOrigin::Imported);
        assert_eq!(got.due_on, date(2026, Month::October, 16));
        let totals = compute_totals(&got.lines);
        // à la main : 5 000,00 HT × 20 % = 1 000,00 TVA ; TTC 6 000,00
        assert_eq!(totals.subtotal_ht, Money::from_cents(500_000));
        assert_eq!(totals.total_vat, Money::from_cents(100_000));
        assert_eq!(totals.total_ttc, Money::from_cents(600_000));
        let seq: i64 = store
            .connection()
            .query_row("SELECT COUNT(*) FROM invoice_sequences", [], |r| r.get(0))
            .unwrap();
        assert_eq!(seq, 0, "l'import ne doit pas toucher invoice_sequences");
        let fa: i64 = store
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM invoices WHERE number LIKE 'FA-%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(fa, 0);
    }

    #[test]
    fn reimporting_the_same_number_is_a_clear_error() {
        let (mut store, client_id) = test_store("import-dup");
        Executor::new(&mut store)
            .execute(&import_cmd(client_id), &human_ctx())
            .unwrap();
        let err = Executor::new(&mut store)
            .execute(&import_cmd(client_id), &human_ctx())
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("FAC-2026-0042")));
        let count: i64 = store
            .connection()
            .query_row("SELECT COUNT(*) FROM invoices", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn an_agent_importing_only_deposits_a_pending_action() {
        let (mut store, client_id) = test_store("import-agent");
        let agent = ExecutionContext::new(
            Actor::Agent {
                session: "s".into(),
            },
            false,
        );
        let outcome = Executor::new(&mut store)
            .execute(&import_cmd(client_id), &agent)
            .unwrap();
        assert!(matches!(outcome, Outcome::PendingConfirmation(_)));
        let count: i64 = store
            .connection()
            .query_row("SELECT COUNT(*) FROM invoices", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn emit_invoice_still_allocates_fa_on_a_fresh_vault() {
        let (mut store, client_id) = test_store("emit-untouched");
        let emitted = emit(&mut store, client_id, date(2026, Month::September, 1));
        assert_eq!(emitted.number, "FA-2026-0001");
    }

    #[test]
    fn mixing_import_then_emit_is_a_documented_footgun_not_a_sql_abort() {
        let (mut store, client_id) = test_store("mix-footgun");
        Executor::new(&mut store)
            .execute(&import_cmd(client_id), &human_ctx())
            .unwrap();
        let emitted = emit(&mut store, client_id, date(2026, Month::September, 20));
        assert_eq!(emitted.number, "FA-2026-0001");
    }

    #[test]
    fn imported_invoice_reconciles_like_any_other() {
        let (mut store, client_id) = test_store("import-recon");
        let Outcome::Applied(inv) = Executor::new(&mut store)
            .execute(&import_cmd(client_id), &human_ctx())
            .unwrap()
        else {
            panic!("applied")
        };
        let tx = ImportBankTransactions {
            transactions: vec![ParsedTransaction {
                occurred_on: date(2026, Month::September, 20),
                amount_cents: 600_000,
                description: "CAMILLE".into(),
                fitid: None,
            }],
        };
        Executor::new(&mut store)
            .execute(&tx, &human_ctx())
            .unwrap();
        let bank_id = list_bank_transactions(store.connection()).unwrap()[0].id;
        Executor::new(&mut store)
            .execute(
                &ReconcileTransaction {
                    transaction_id: bank_id,
                    invoice_id: inv.id,
                },
                &human_ctx(),
            )
            .unwrap();
        let aged = aged_balance(store.connection(), date(2026, Month::September, 21)).unwrap();
        assert!(aged.iter().all(|a| a.invoice_id != inv.id));
    }

    #[test]
    fn issue_credit_note_on_an_imported_invoice_is_refused() {
        let (mut store, client_id) = test_store("no-fa-credit");
        let Outcome::Applied(inv) = Executor::new(&mut store)
            .execute(&import_cmd(client_id), &human_ctx())
            .unwrap()
        else {
            panic!("applied")
        };
        let err = Executor::new(&mut store)
            .execute(
                &IssueCreditNote {
                    invoice_id: inv.id,
                    issued_on: date(2026, Month::September, 17),
                },
                &human_ctx(),
            )
            .unwrap_err();
        assert!(
            matches!(err, AppError::Domain(msg) if msg.contains("née ailleurs") || msg.contains("import"))
        );
        let seq: i64 = store
            .connection()
            .query_row("SELECT COUNT(*) FROM invoice_sequences", [], |r| r.get(0))
            .unwrap();
        assert_eq!(seq, 0);
    }

    #[test]
    fn importing_a_credit_note_uses_the_foreign_number() {
        let (mut store, client_id) = test_store("import-avoir");
        let Outcome::Applied(inv) = Executor::new(&mut store)
            .execute(&import_cmd(client_id), &human_ctx())
            .unwrap()
        else {
            panic!("applied")
        };
        let avoir = ImportIssuedInvoice {
            number: "AV-2026-0003".into(),
            client_id,
            mission_id: None,
            lines: vec![InvoiceLine {
                description: "Mission Camille".into(),
                quantity: -1.0,
                unit_price: Money::from_cents(500_000),
                vat_rate: VatRate::Standard,
            }],
            issued_on: date(2026, Month::September, 17),
            payment_terms_days: 0,
            credited_invoice_id: Some(inv.id),
        };
        let Outcome::Applied(cn) = Executor::new(&mut store)
            .execute(&avoir, &human_ctx())
            .unwrap()
        else {
            panic!("applied")
        };
        assert_eq!(cn.number, "AV-2026-0003");
        let got = invoice_by_id(store.connection(), cn.id).unwrap().unwrap();
        assert_eq!(got.credited_invoice_id, Some(inv.id));
        assert_eq!(got.origin, InvoiceOrigin::Imported);
        let totals = compute_totals(&got.lines);
        assert_eq!(totals.total_ttc, Money::from_cents(-600_000));
        let seq: i64 = store
            .connection()
            .query_row("SELECT COUNT(*) FROM invoice_sequences", [], |r| r.get(0))
            .unwrap();
        assert_eq!(seq, 0);
    }

    #[test]
    fn importing_a_credit_note_with_positive_lines_is_refused() {
        let (mut store, client_id) = test_store("import-avoir-positif");
        let Outcome::Applied(inv) = Executor::new(&mut store)
            .execute(&import_cmd(client_id), &human_ctx())
            .unwrap()
        else {
            panic!("applied")
        };
        let before = aged_balance(store.connection(), date(2026, Month::September, 21)).unwrap();
        assert_eq!(before.len(), 1);
        // à la main : 5 000,00 HT × 20 % = 6 000,00 TTC encore dus
        assert_eq!(before[0].outstanding, Money::from_cents(600_000));

        let err = Executor::new(&mut store)
            .execute(
                &ImportIssuedInvoice {
                    number: "AV-2026-0003".into(),
                    client_id,
                    mission_id: None,
                    lines: vec![camille_line()],
                    issued_on: date(2026, Month::September, 17),
                    payment_terms_days: 0,
                    credited_invoice_id: Some(inv.id),
                },
                &human_ctx(),
            )
            .unwrap_err();
        assert!(
            matches!(err, AppError::Domain(ref msg) if msg.contains("négatif")),
            "{err}"
        );
        let count: i64 = store
            .connection()
            .query_row("SELECT COUNT(*) FROM invoices", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1, "l'avoir positif ne doit pas être collé");
        let aged = aged_balance(store.connection(), date(2026, Month::September, 21)).unwrap();
        assert_eq!(aged.len(), 1);
        assert_eq!(aged[0].invoice_id, inv.id);
        assert_eq!(aged[0].outstanding, Money::from_cents(600_000));
    }

    #[test]
    fn importing_a_credit_note_for_another_client_is_refused() {
        let (mut store, client_id) = test_store("import-avoir-autre-client");
        let Outcome::Applied(inv) = Executor::new(&mut store)
            .execute(&import_cmd(client_id), &human_ctx())
            .unwrap()
        else {
            panic!("applied")
        };
        let other = ClientId::new();
        store
            .connection()
            .execute(
                "INSERT INTO clients (id, name, created_at) VALUES (?1, 'Autre', '2026-01-01T00:00:00Z')",
                [other.to_string()],
            )
            .unwrap();
        let err = Executor::new(&mut store)
            .execute(
                &ImportIssuedInvoice {
                    number: "AV-2026-0003".into(),
                    client_id: other,
                    mission_id: None,
                    lines: vec![InvoiceLine {
                        description: "Mission Camille".into(),
                        quantity: -1.0,
                        unit_price: Money::from_cents(500_000),
                        vat_rate: VatRate::Standard,
                    }],
                    issued_on: date(2026, Month::September, 17),
                    payment_terms_days: 0,
                    credited_invoice_id: Some(inv.id),
                },
                &human_ctx(),
            )
            .unwrap_err();
        assert!(
            matches!(err, AppError::Domain(ref msg) if msg.contains("même client")),
            "{err}"
        );
        let count: i64 = store
            .connection()
            .query_row("SELECT COUNT(*) FROM invoices", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }
}
