//! Export du **Fichier des Écritures Comptables** (FEC) d'un exercice — art. A. 47 A-1 du livre
//! des procédures fiscales (lot 28).
//!
//! Depuis le lot 31, ce module n'est plus que le **format** : les écritures viennent du grand
//! livre dérivé de [`crate::ledger`] (journaux `AN`, `VE`, `AC`, `BQ`, `OD`), qui porte aussi la
//! balance et le bilan. Le format suit le BOI-CF-IOR-60-40-20 : 18 colonnes dans l'ordre imposé,
//! séparateur `|`, dates en `AAAAMMJJ`, montants avec la virgule décimale et sans séparateur de
//! milliers, encodage UTF-8, nom de fichier `<SIREN>FEC<AAAAMMJJ>.txt` daté de la clôture. Les
//! colonnes de lettrage et de devise restent vides (euro seul ; le lettrage n'est pas modélisé).
//!
//! Limites assumées, dites dans le fichier lui-même par les libellés — voir le commentaire de
//! module de `crate::ledger`. Le FEC produit est un **export pour l'expert-comptable**, qui reste
//! maître des écritures définitives.

use rusqlite::Connection;
use time::Date;

use crate::app::AppError;
use crate::company::CompanyProfile;
use crate::domain::{Client, Expense, FiscalYear, Invoice, Money, OpeningBalance, Payment, Siren};
use crate::ledger::ledger_ending_in;
pub use crate::ledger::{
    Account, AuxAccount, Journal, Ledger, LedgerEntry as FecEntry, LedgerFacts,
    LedgerLine as FecLine, OpeningLines, accounts, charge_account, exercise_ending_in,
};

/// Séparateur de champs — l'un des deux admis (`|` ou tabulation).
pub const SEPARATOR: char = '|';

/// Les 18 colonnes, dans l'ordre imposé par l'art. A. 47 A-1 (comptabilité informatisée, régime
/// réel, colonnes `Debit`/`Credit`).
pub const HEADER: [&str; 18] = [
    "JournalCode",
    "JournalLib",
    "EcritureNum",
    "EcritureDate",
    "CompteNum",
    "CompteLib",
    "CompAuxNum",
    "CompAuxLib",
    "PieceRef",
    "PieceDate",
    "EcritureLib",
    "Debit",
    "Credit",
    "EcritureLet",
    "DateLet",
    "ValidDate",
    "Montantdevise",
    "Idevise",
];

/// Le FEC d'un exercice : identité, période, écritures triées et numérotées.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Fec {
    pub siren: Siren,
    pub exercise: FiscalYear,
    pub entries: Vec<FecEntry>,
}

/// Résumé sérialisable d'un export — ce que les façades rendent en `--json`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FecSummary {
    pub file_name: String,
    pub siren: Siren,
    pub starts_on: Date,
    pub ends_on: Date,
    pub entries: usize,
    pub lines: usize,
    pub total_debit_cents: i64,
    pub total_credit_cents: i64,
}

impl Fec {
    /// Le FEC d'un grand livre déjà dérivé.
    #[must_use]
    pub fn from_ledger(siren: Siren, ledger: Ledger) -> Self {
        Self {
            siren,
            exercise: ledger.exercise,
            entries: ledger.entries,
        }
    }

    /// Construit le FEC d'un exercice à partir des faits du domaine — fonction pure, testable
    /// sans base. Le bilan d'ouverture (lot 30) fournit les à-nouveaux si, et seulement si, il
    /// ouvre cet exercice ; sans exercice clos enregistré, l'IS et la rémunération du dirigeant
    /// sont recalculés depuis le profil (voir [`Ledger::build`]).
    #[must_use]
    pub fn build(
        profile: &CompanyProfile,
        exercise: FiscalYear,
        invoices: &[Invoice],
        clients: &[Client],
        payments: &[Payment],
        expenses: &[Expense],
        opening: Option<&OpeningBalance>,
    ) -> Self {
        let ledger = Ledger::build(LedgerFacts {
            profile,
            exercise,
            invoices,
            clients,
            payments,
            expenses,
            opening: opening
                .filter(|o| o.opens_on == exercise.start())
                .map(OpeningLines::from_opening_balance),
            snapshot: None,
            appropriations: &[],
        });
        Self::from_ledger(profile.siren, ledger)
    }

    /// `<SIREN>FEC<AAAAMMJJ>.txt`, daté de la clôture de l'exercice — le nom imposé.
    #[must_use]
    pub fn file_name(&self) -> String {
        format!("{}FEC{}.txt", self.siren, compact_date(self.exercise.end()))
    }

    #[must_use]
    pub fn total_debit(&self) -> Money {
        self.entries.iter().map(FecEntry::total_debit).sum()
    }

    #[must_use]
    pub fn total_credit(&self) -> Money {
        self.entries
            .iter()
            .flat_map(|e| e.lines.iter())
            .filter(|l| l.amount.is_negative())
            .map(|l| -l.amount)
            .sum()
    }

    #[must_use]
    pub fn summary(&self) -> FecSummary {
        FecSummary {
            file_name: self.file_name(),
            siren: self.siren,
            starts_on: self.exercise.start(),
            ends_on: self.exercise.end(),
            entries: self.entries.len(),
            lines: self.entries.iter().map(|e| e.lines.len()).sum(),
            total_debit_cents: self.total_debit().cents(),
            total_credit_cents: self.total_credit().cents(),
        }
    }

    /// Le contenu du fichier : en-tête puis une ligne par ligne d'écriture, `\n` en fin de chaque
    /// enregistrement, UTF-8.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        push_record(&mut out, &HEADER);
        for entry in &self.entries {
            let date = compact_date(entry.date);
            let piece_date = compact_date(entry.piece_date);
            for l in &entry.lines {
                let (debit, credit) = if l.amount.is_negative() {
                    (Money::ZERO, -l.amount)
                } else {
                    (l.amount, Money::ZERO)
                };
                let (aux_num, aux_lib) = l
                    .aux
                    .as_ref()
                    .map_or(("", ""), |a| (a.number.as_str(), a.label.as_str()));
                let fields: [&str; 18] = [
                    entry.journal.code(),
                    entry.journal.label(),
                    &entry.number.to_string(),
                    &date,
                    l.account.number.as_ref(),
                    l.account.label.as_ref(),
                    aux_num,
                    aux_lib,
                    &entry.piece_ref,
                    &piece_date,
                    &entry.label,
                    &decimal_comma(debit),
                    &decimal_comma(credit),
                    "",
                    "",
                    &date,
                    "",
                    "",
                ];
                push_record(&mut out, &fields);
            }
        }
        out
    }
}

/// `AAAAMMJJ`.
fn compact_date(date: Date) -> String {
    format!(
        "{:04}{:02}{:02}",
        date.year(),
        u8::from(date.month()),
        date.day()
    )
}

/// Montant positif avec la virgule décimale (`1234,56`), sans séparateur de milliers.
fn decimal_comma(amount: Money) -> String {
    amount.to_decimal_string().replace('.', ",")
}

/// Un champ ne doit contenir ni le séparateur ni une fin de ligne : ils deviennent des espaces.
fn sanitize(field: &str) -> String {
    field
        .chars()
        .map(|c| match c {
            SEPARATOR | '\t' | '\r' | '\n' => ' ',
            other => other,
        })
        .collect::<String>()
        .trim()
        .to_string()
}

fn push_record(out: &mut String, fields: &[&str; 18]) {
    for (i, field) in fields.iter().enumerate() {
        if i > 0 {
            out.push(SEPARATOR);
        }
        out.push_str(&sanitize(field));
    }
    out.push('\n');
}

/// Construit le FEC de l'exercice clos en `period` depuis la base — la requête que les trois
/// façades partagent, par-dessus le grand livre dérivé ([`crate::ledger::ledger_ending_in`]) :
/// à-nouveaux chaînés d'un exercice clos sur l'autre, opérations de clôture comprises.
///
/// # Errors
///
/// `AppError::Domain` sans profil d'entreprise (le SIREN nomme le fichier) ; erreur de lecture
/// SQLite sinon.
pub fn build_fec(conn: &Connection, period: i32) -> Result<Fec, AppError> {
    let (profile, ledger) = ledger_ending_in(conn, period)?;
    Ok(Fec::from_ledger(profile.siren, ledger))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Actor, ExecutionContext, Executor, Outcome};
    use crate::billing::{EmitInvoice, IssueCreditNote, RecordPayment, VoidPayment};
    use crate::company::SetCompanyProfile;
    use crate::domain::{
        Address, ClientId, InvoiceId, InvoiceLine, InvoiceStatus, PaymentId, VatRate,
    };
    use crate::domain::{ExpenseCategory, FiscalYearEnd, PaymentMethod};
    use crate::expenses::RecordExpense;
    use crate::store::{Passphrase, Store};
    use proptest::prelude::*;
    use time::Month as TimeMonth;
    use time::OffsetDateTime;

    fn applied<T>(outcome: Outcome<T>) -> T {
        match outcome {
            Outcome::Applied(value) => value,
            other => panic!(
                "attendu Applied, obtenu {}",
                match other {
                    Outcome::DryRun => "DryRun",
                    Outcome::AlreadyApplied(_) => "AlreadyApplied",
                    Outcome::PendingConfirmation(_) => "PendingConfirmation",
                    Outcome::Applied(_) => unreachable!(),
                }
            ),
        }
    }

    fn date(year: i32, month: TimeMonth, day: u8) -> Date {
        Date::from_calendar_date(year, month, day).unwrap()
    }

    fn profile() -> CompanyProfile {
        CompanyProfile {
            name: "Argon Digital".to_string(),
            legal_form: "SASU".to_string(),
            siren: Siren::parse("552100554").unwrap(),
            vat_number: None,
            address: Address {
                street: "12 rue de la Paix".to_string(),
                postal_code: "75002".to_string(),
                city: "Paris".to_string(),
                country: "FR".to_string(),
            },
            share_capital: None,
            rcs_city: None,
            iban: None,
            fiscal_year_end: Some(FiscalYearEnd::CALENDAR),
            vat_regime: None,
            director_monthly_gross: None,
            director_charge_ratio_bps: None,
        }
    }

    fn client(id: ClientId, name: &str) -> Client {
        Client {
            id,
            name: name.to_string(),
            siren: None,
            vat_number: None,
            address: None,
            created_at: OffsetDateTime::UNIX_EPOCH,
            revision: 1,
            archived_at: None,
        }
    }

    fn invoice(
        id: InvoiceId,
        number: &str,
        client_id: ClientId,
        lines: Vec<InvoiceLine>,
        issued_on: Date,
        credited: Option<InvoiceId>,
    ) -> Invoice {
        Invoice {
            id,
            number: number.to_string(),
            client_id,
            mission_id: None,
            lines,
            status: InvoiceStatus::Issued,
            issued_on,
            due_on: issued_on,
            previous_hash: None,
            hash: String::new(),
            credited_invoice_id: credited,
        }
    }

    fn standard_line(description: &str, quantity: f64, unit_cents: i64) -> InvoiceLine {
        InvoiceLine {
            description: description.to_string(),
            quantity,
            unit_price: Money::from_cents(unit_cents),
            vat_rate: VatRate::Standard,
        }
    }

    fn expense(
        label: &str,
        category: ExpenseCategory,
        ttc: i64,
        deductible: i64,
        on: Date,
    ) -> Expense {
        Expense {
            id: crate::domain::ExpenseId::new(),
            label: label.to_string(),
            category,
            amount: Money::from_cents(ttc),
            vat_rate: VatRate::Standard,
            vat_deductible: Money::from_cents(deductible),
            incurred_on: on,
            receipt_hash: None,
            receipt_filename: None,
            created_at: OffsetDateTime::UNIX_EPOCH,
            revision: 1,
        }
    }

    fn payment(invoice_id: InvoiceId, cents: i64, on: Date, voided_on: Option<Date>) -> Payment {
        Payment {
            id: PaymentId::new(),
            invoice_id,
            amount: Money::from_cents(cents),
            received_on: on,
            method: PaymentMethod::BankTransfer,
            bank_transaction_id: None,
            voided_at: voided_on.map(|d| d.midnight().assume_utc()),
        }
    }

    #[test]
    fn the_opening_balance_becomes_an_an_entry_on_the_first_day_of_its_exercise_only() {
        let opening = OpeningBalance {
            opens_on: date(2026, TimeMonth::January, 1),
            source: Some("bilan au 31/12/2025, cabinet X".to_string()),
            lines: vec![
                "101000:Capital social:C:1000.00".parse().unwrap(),
                "110000:Report à nouveau:C:500.00".parse().unwrap(),
                "512000:Banque:D:1500.00".parse().unwrap(),
            ],
        };
        let fec = Fec::build(
            &profile(),
            FiscalYear::calendar(2026),
            &[],
            &[],
            &[],
            &[],
            Some(&opening),
        );
        assert_eq!(fec.entries.len(), 1);
        let an = &fec.entries[0];
        assert_eq!(an.journal, Journal::Opening);
        assert_eq!(an.number, 1);
        assert_eq!(an.date, date(2026, TimeMonth::January, 1));
        assert!(an.is_balanced());
        assert_eq!(an.lines.len(), 3);
        assert_eq!(an.lines[0].account.number, "101000");
        assert_eq!(an.lines[0].account.label, "Capital social");
        assert_eq!(an.lines[0].amount, Money::from_cents(-100_000));
        assert_eq!(an.lines[2].amount, Money::from_cents(150_000));
        let rendered = fec.render();
        let first = rendered.lines().nth(1).unwrap();
        assert!(first.starts_with("AN|À-nouveaux|1|20260101|101000|Capital social|||AN|20260101|À-nouveaux — bilan au 31/12/2025, cabinet X|0,00|1000,00|"), "{first}");
        assert_eq!(fec.total_debit(), Money::from_cents(150_000));
        assert_eq!(fec.total_credit(), Money::from_cents(150_000));

        // L'exercice suivant s'ouvre sur la clôture du précédent, pas sur ce bilan : aucun AN.
        let next = Fec::build(
            &profile(),
            FiscalYear::calendar(2027),
            &[],
            &[],
            &[],
            &[],
            Some(&opening),
        );
        assert!(next.entries.is_empty());
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn the_rendered_file_is_the_dgfip_layout_with_balanced_entries() {
        let client_id: ClientId = "01900000-0000-7000-8000-00000000abcd".parse().unwrap();
        let inv_id = InvoiceId::new();
        let cn_id = InvoiceId::new();
        let invoices = vec![
            invoice(
                inv_id,
                "FA-2026-0001",
                client_id,
                vec![standard_line("Conseil", 2.0, 100_000)],
                date(2026, TimeMonth::March, 10),
                None,
            ),
            invoice(
                cn_id,
                "FA-2026-0002",
                client_id,
                vec![standard_line("Conseil", -2.0, 100_000)],
                date(2026, TimeMonth::April, 2),
                Some(inv_id),
            ),
        ];
        let clients = vec![client(client_id, "Acme | Cie")];
        let payments = vec![payment(
            inv_id,
            240_000,
            date(2026, TimeMonth::March, 31),
            None,
        )];
        let expenses = vec![expense(
            "Licence IDE",
            ExpenseCategory::Software,
            12_000,
            2_000,
            date(2026, TimeMonth::March, 12),
        )];

        let fec = Fec::build(
            &profile(),
            FiscalYear::calendar(2026),
            &invoices,
            &clients,
            &payments,
            &expenses,
            None,
        );
        assert_eq!(fec.file_name(), "552100554FEC20261231.txt");
        assert!(fec.entries.iter().all(FecEntry::is_balanced));
        assert_eq!(fec.total_debit(), fec.total_credit());

        let rendered = fec.render();
        let mut lines = rendered.lines();
        assert_eq!(lines.next().unwrap(), HEADER.join("|"));
        let body: Vec<&str> = lines.collect();
        // Facture : 3 lignes ; dépense : 3 ; règlement : 2 ; avoir : 3.
        assert_eq!(body.len(), 11, "{rendered}");
        assert!(
            body.iter().all(|l| l.split('|').count() == 18),
            "{rendered}"
        );
        assert_eq!(
            body[0],
            "VE|Ventes|1|20260310|411000|Clients|C01900000|Acme   Cie|FA-2026-0001|20260310|Facture FA-2026-0001 — Acme   Cie|2400,00|0,00|||20260310||",
            "{rendered}"
        );
        assert_eq!(
            body[1],
            "VE|Ventes|1|20260310|706000|Prestations de services|||FA-2026-0001|20260310|Facture FA-2026-0001 — Acme   Cie|0,00|2000,00|||20260310||"
        );
        assert_eq!(
            body[2],
            "VE|Ventes|1|20260310|445710|TVA collectée|||FA-2026-0001|20260310|Facture FA-2026-0001 — Acme   Cie|0,00|400,00|||20260310||"
        );
        // Dépense du 12 mars, journal AC : charge HT 100 €, TVA 20 €, banque 120 €.
        assert!(
            body[3].starts_with("AC|Achats|1|20260312|651000|"),
            "{}",
            body[3]
        );
        assert!(
            body[3].ends_with("|Licence IDE|100,00|0,00|||20260312||"),
            "{}",
            body[3]
        );
        assert!(body[4].contains("|445660|"), "{}", body[4]);
        assert!(body[4].ends_with("|20,00|0,00|||20260312||"), "{}", body[4]);
        assert!(body[5].contains("|512000|Banque|||DEP-"), "{}", body[5]);
        assert!(
            body[5].ends_with("|0,00|120,00|||20260312||"),
            "{}",
            body[5]
        );
        // Règlement du 31 mars, journal BQ.
        assert_eq!(
            body[6],
            "BQ|Banque|1|20260331|512000|Banque|||FA-2026-0001|20260331|Règlement FA-2026-0001 (virement)|2400,00|0,00|||20260331||"
        );
        assert_eq!(
            body[7],
            "BQ|Banque|1|20260331|411000|Clients|C01900000|Acme   Cie|FA-2026-0001|20260331|Règlement FA-2026-0001 (virement)|0,00|2400,00|||20260331||"
        );
        // Avoir : les côtés s'inversent, le numéro VE continue.
        assert_eq!(
            body[8],
            "VE|Ventes|2|20260402|411000|Clients|C01900000|Acme   Cie|FA-2026-0002|20260402|Avoir FA-2026-0002 sur FA-2026-0001 — Acme   Cie|0,00|2400,00|||20260402||"
        );
        assert!(
            body[9].ends_with("|2000,00|0,00|||20260402||"),
            "{}",
            body[9]
        );
        assert!(
            body[10].ends_with("|400,00|0,00|||20260402||"),
            "{}",
            body[10]
        );
    }

    #[test]
    fn only_facts_dated_in_the_exercise_are_exported_and_a_void_is_its_own_entry() {
        let client_id = ClientId::new();
        let inv_id = InvoiceId::new();
        let invoices = vec![invoice(
            inv_id,
            "FA-2025-0001",
            client_id,
            vec![standard_line("Conseil", 1.0, 100_000)],
            date(2025, TimeMonth::December, 20),
            None,
        )];
        // Encaissé fin 2025, annulé en 2026 : l'extourne seule figure au FEC 2026.
        let payments = vec![payment(
            inv_id,
            120_000,
            date(2025, TimeMonth::December, 30),
            Some(date(2026, TimeMonth::January, 15)),
        )];
        let expenses = vec![expense(
            "Hors exercice",
            ExpenseCategory::Other,
            5_000,
            0,
            date(2027, TimeMonth::January, 1),
        )];
        let fec = Fec::build(
            &profile(),
            FiscalYear::calendar(2026),
            &invoices,
            &[client(client_id, "Acme")],
            &payments,
            &expenses,
            None,
        );
        assert_eq!(fec.entries.len(), 1, "{:?}", fec.entries);
        let void = &fec.entries[0];
        assert_eq!(void.journal, Journal::Bank);
        assert_eq!(void.date, date(2026, TimeMonth::January, 15));
        assert!(
            void.label
                .starts_with("Annulation du règlement FA-2025-0001 du 2025-12-30")
        );
        assert_eq!(void.lines[0].account, accounts::CLIENTS);
        assert_eq!(void.lines[0].amount, Money::from_cents(120_000));
        assert_eq!(void.lines[1].account, accounts::BANK);
        assert_eq!(void.lines[1].amount, Money::from_cents(-120_000));

        let fec_2025 = Fec::build(
            &profile(),
            FiscalYear::calendar(2025),
            &invoices,
            &[client(client_id, "Acme")],
            &payments,
            &expenses,
            None,
        );
        // Facture et règlement, puis l'IS de clôture sur le bénéfice de 1 000 € (150 €) en OD
        // (lot 31).
        let kinds: Vec<_> = fec_2025.entries.iter().map(|e| e.journal).collect();
        assert_eq!(kinds, vec![Journal::Sales, Journal::Bank, Journal::Misc]);
        assert_eq!(fec_2025.entries[2].piece_ref, "OD-IS");
        assert_eq!(
            fec_2025.entries[2].lines[0].amount,
            Money::from_cents(15_000)
        );
        assert!(
            fec_2025.entries[1].label.starts_with("Règlement"),
            "l'encaissement lui-même reste au FEC 2025"
        );
    }

    #[test]
    fn every_expense_category_maps_to_a_class_6_account() {
        for category in [
            ExpenseCategory::Software,
            ExpenseCategory::Equipment,
            ExpenseCategory::Travel,
            ExpenseCategory::Meals,
            ExpenseCategory::Office,
            ExpenseCategory::Professional,
            ExpenseCategory::Other,
        ] {
            assert!(charge_account(category).number.starts_with('6'));
        }
    }

    #[test]
    fn a_non_deductible_vat_share_stays_in_the_charge() {
        // Restaurant 120 € TTC, TVA déductible plafonnée à 8 € : charge 112 €, TVA 8 €.
        let fec = Fec::build(
            &profile(),
            FiscalYear::calendar(2026),
            &[],
            &[],
            &[],
            &[expense(
                "Déjeuner client",
                ExpenseCategory::Meals,
                12_000,
                800,
                date(2026, TimeMonth::May, 5),
            )],
            None,
        );
        let entry = &fec.entries[0];
        assert_eq!(entry.lines[0].account, accounts::MEALS);
        assert_eq!(entry.lines[0].amount, Money::from_cents(11_200));
        assert_eq!(entry.lines[1].amount, Money::from_cents(800));
        assert!(entry.is_balanced());
    }

    fn line_strategy() -> impl Strategy<Value = InvoiceLine> {
        (
            -50.0f64..50.0,
            -5_000_000i64..5_000_000,
            prop::sample::select(VatRate::ALL.to_vec()),
        )
            .prop_map(|(quantity, unit, vat_rate)| InvoiceLine {
                description: "x".to_string(),
                quantity,
                unit_price: Money::from_cents(unit),
                vat_rate,
            })
    }

    fn expense_strategy() -> impl Strategy<Value = Expense> {
        (0i64..5_000_000, 0u8..=100, 1u8..=28).prop_map(|(ttc, share, day)| {
            let deductible = ttc * i64::from(share) / 100;
            expense(
                "dép.",
                ExpenseCategory::Other,
                ttc,
                deductible,
                date(2026, TimeMonth::June, day),
            )
        })
    }

    proptest! {
        #[test]
        fn every_entry_balances_and_the_file_balances_whatever_the_facts(
            lines in prop::collection::vec(line_strategy(), 1..6),
            expenses in prop::collection::vec(expense_strategy(), 0..4),
            paid in 0i64..1_000_000,
        ) {
            let client_id = ClientId::new();
            let inv_id = InvoiceId::new();
            let invoices = vec![invoice(
                inv_id, "FA-2026-0001", client_id, lines, date(2026, TimeMonth::June, 1), None,
            )];
            let payments = vec![payment(inv_id, paid, date(2026, TimeMonth::June, 15), None)];
            let fec = Fec::build(
                &profile(),
                FiscalYear::calendar(2026),
                &invoices,
                &[client(client_id, "Acme")],
                &payments,
                &expenses,
                None,
            );
            prop_assert!(fec.entries.iter().all(FecEntry::is_balanced));
            prop_assert_eq!(fec.total_debit(), fec.total_credit());
            let rendered = fec.render();
            for record in rendered.lines() {
                prop_assert_eq!(record.split('|').count(), 18);
            }
            // Numérotation continue par journal, dans l'ordre du fichier.
            for journal in [Journal::Sales, Journal::Purchases, Journal::Bank] {
                let numbers: Vec<u32> = fec
                    .entries
                    .iter()
                    .filter(|e| e.journal == journal)
                    .map(|e| e.number)
                    .collect();
                let expected: Vec<u32> = (1..=u32::try_from(numbers.len()).unwrap()).collect();
                prop_assert_eq!(numbers, expected);
            }
            for pair in fec.entries.windows(2) {
                prop_assert!(pair[0].date <= pair[1].date);
            }
        }
    }

    // --- Sur base : la requête partagée par les façades. ---

    fn fresh_store(tag: &str) -> (Store, ClientId) {
        let dir = std::env::temp_dir().join(format!(
            "freeflow-fec-{tag}-{}-{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ));
        let store = Store::create(&dir.join("vault.db"), &Passphrase::from("s3cret")).unwrap();
        let client_id = ClientId::new();
        store
            .connection()
            .execute(
                "INSERT INTO clients (id, name, created_at) \
                 VALUES (?1, 'Acme', '2025-01-01T00:00:00Z')",
                [client_id.to_string()],
            )
            .unwrap();
        (store, client_id)
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn build_fec_reads_invoices_credit_notes_payments_voids_and_expenses() {
        let (mut store, client_id) = fresh_store("db");
        let human = ExecutionContext::new(Actor::Human, false);
        let p = profile();
        Executor::new(&mut store)
            .execute(
                &SetCompanyProfile {
                    name: p.name.clone(),
                    legal_form: p.legal_form.clone(),
                    siren: p.siren,
                    vat_number: None,
                    address: p.address.clone(),
                    share_capital: None,
                    rcs_city: None,
                    iban: None,
                    fiscal_year_end: Some(FiscalYearEnd::new(6, 30).unwrap()),
                    vat_regime: None,
                    director_monthly_gross: None,
                    director_charge_ratio_bps: None,
                },
                &human,
            )
            .unwrap();
        let emitted = applied(
            Executor::new(&mut store)
                .execute(
                    &EmitInvoice {
                        client_id,
                        mission_id: None,
                        lines: vec![standard_line("Conseil", 3.0, 50_000)],
                        issued_on: date(2025, TimeMonth::September, 1),
                        payment_terms_days: 30,
                    },
                    &human,
                )
                .unwrap(),
        );
        let paid = applied(
            Executor::new(&mut store)
                .execute(
                    &RecordPayment {
                        invoice_id: emitted.id,
                        amount: Money::from_cents(180_000),
                        received_on: date(2025, TimeMonth::October, 1),
                        method: PaymentMethod::Card,
                    },
                    &human,
                )
                .unwrap(),
        );
        Executor::new(&mut store)
            .execute(
                &VoidPayment {
                    payment_id: paid,
                    reason: Some("doublon".to_string()),
                },
                &human,
            )
            .unwrap();
        Executor::new(&mut store)
            .execute(
                &IssueCreditNote {
                    invoice_id: emitted.id,
                    issued_on: date(2025, TimeMonth::November, 3),
                },
                &human,
            )
            .unwrap();
        Executor::new(&mut store)
            .execute(
                &RecordExpense {
                    label: "Train".to_string(),
                    category: ExpenseCategory::Travel,
                    amount: Money::from_cents(11_000),
                    vat_rate: VatRate::Intermediate,
                    vat_deductible: Money::from_cents(1_000),
                    incurred_on: date(2026, TimeMonth::February, 2),
                    receipt_hash: None,
                    receipt_filename: Some("billet.pdf".to_string()),
                },
                &human,
            )
            .unwrap();
        // Hors exercice (2026-27).
        Executor::new(&mut store)
            .execute(
                &RecordExpense {
                    label: "Après clôture".to_string(),
                    category: ExpenseCategory::Other,
                    amount: Money::from_cents(1_000),
                    vat_rate: VatRate::Zero,
                    vat_deductible: Money::ZERO,
                    incurred_on: date(2026, TimeMonth::July, 1),
                    receipt_hash: None,
                    receipt_filename: None,
                },
                &human,
            )
            .unwrap();

        // Exercice décalé clos le 30 juin 2026 : 2025-07-01 → 2026-06-30, dérivé du profil
        // (aucun exercice clos enregistré).
        let fec = build_fec(store.connection(), 2026).unwrap();
        assert_eq!(fec.exercise.start(), date(2025, TimeMonth::July, 1));
        assert_eq!(fec.exercise.end(), date(2026, TimeMonth::June, 30));
        assert_eq!(fec.file_name(), "552100554FEC20260630.txt");
        let labels: Vec<&str> = fec.entries.iter().map(|e| e.label.as_str()).collect();
        assert_eq!(labels.len(), 4, "{labels:?}");
        assert!(
            labels[0].starts_with("Facture FA-2025-0001 — Acme"),
            "{labels:?}"
        );
        assert!(
            labels[1].starts_with("Règlement FA-2025-0001 (carte)"),
            "{labels:?}"
        );
        assert!(
            labels[2].starts_with("Avoir FA-2025-0002 sur FA-2025-0001"),
            "{labels:?}"
        );
        assert_eq!(labels[3], "Train");
        assert_eq!(fec.entries[3].piece_ref, "billet.pdf");
        assert_eq!(fec.entries[3].lines[0].account, accounts::TRAVEL);
        assert_eq!(fec.entries[3].lines[0].amount, Money::from_cents(10_000));
        assert!(fec.entries.iter().all(FecEntry::is_balanced));
        assert_eq!(fec.total_debit(), fec.total_credit());
        // Numérotation par journal : VE 1-2, BQ 1, AC 1.
        let numbers: Vec<(Journal, u32)> =
            fec.entries.iter().map(|e| (e.journal, e.number)).collect();
        assert_eq!(
            numbers,
            vec![
                (Journal::Sales, 1),
                (Journal::Bank, 1),
                (Journal::Sales, 2),
                (Journal::Purchases, 1),
            ]
        );

        // L'annulation est horodatée au moment de la commande (aujourd'hui) : elle relève de
        // l'exercice suivant, avec la dépense d'après clôture.
        let next = build_fec(store.connection(), 2027).unwrap();
        assert_eq!(next.exercise.start(), date(2026, TimeMonth::July, 1));
        let labels: Vec<&str> = next.entries.iter().map(|e| e.label.as_str()).collect();
        assert_eq!(labels.len(), 2, "{labels:?}");
        assert_eq!(labels[0], "Après clôture");
        assert!(
            labels[1].starts_with("Annulation du règlement FA-2025-0001 du 2025-10-01"),
            "{labels:?}"
        );
        assert_eq!(next.entries[1].lines[0].amount, Money::from_cents(180_000));
        assert!(next.entries[1].is_balanced());
    }

    #[test]
    fn build_fec_requires_a_company_profile() {
        let (store, _) = fresh_store("no-profile");
        let err = build_fec(store.connection(), 2026).unwrap_err();
        assert!(matches!(err, AppError::Domain(_)), "{err}");
    }
}
