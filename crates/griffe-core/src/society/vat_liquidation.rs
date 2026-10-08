//! Liquidation de TVA d'un mois écoulé — écriture conservée, pas un recalcul à la lecture.
//!
//! L'OD est datée du dernier jour, pièce `CA3-AAAA-MM`, journal `OD`. Elle solde le
//! livre tel qu'il est : 445710 du mois, 445660 du mois (hors à-nouveaux),
//! 445670 jusqu'au dernier jour, et le reversement enregistré du mois. Elle ne
//! relit pas les factures à leur date d'émission.
//!
//! L'arrondi de la déductible est [`crate::accounting::round_to_euro`] (art. 1657 CGI) : 0,50 €
//! compte pour 1. L'écart, quand la ligne arrondie dépasse le livre, est débité
//! au 445660 et crédité au 758. Si le livre dépasse la ligne arrondie, rien
//! n'est posé et aucun compte de charge n'est inventé. Un 445670 créditeur
//! n'est pas liquidé non plus.
//!
//! La paire d'une prestation intracommunautaire (445200, 445662) est soldée au
//! centime quand les deux nets du mois sont opposés : débit 445200, crédit
//! 445662. Pas de 758, pas de 635, pas de 658 pour cet écart. Un mois sans ce
//! mouvement ne gagne pas ces lignes. Le préfixe `3310-CA3` du libellé est
//! celui des autres lignes de cette OD, pas un numéro de case.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::Date;

use crate::app::{AppError, Command};
use crate::company::company_profile;
use crate::domain::{Money, Month};
use crate::fiscal::FiscalDeadlineKind;
use crate::journal::{self, sync_vat_liquidation};
use crate::ledger::accounts;
use crate::ledger::{Account, Journal, LedgerEntry, LedgerLine};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum VatLiquidationError {
    #[error("période CA3 illisible ({0}) : attendu AAAA-MM")]
    InvalidPeriod(String),

    #[error("le mois n'est pas fini")]
    MonthNotFinished,

    #[error("exercice clos")]
    FiscalYearClosed,

    #[error("déclaration déjà déposée")]
    DeclarationAlreadyFiled,

    #[error("le profil de la société est absent")]
    ProfileMissing,

    #[error("la date de clôture d'exercice n'est pas configurée")]
    FiscalYearEndMissing,

    #[error(
        "la TVA déductible du livre ({book} centimes) dépasse la ligne arrondie ({rounded} centimes)"
    )]
    DeductibleExceedsRoundedEuro { book: i64, rounded: i64 },

    #[error("le 445670 est créditeur de {cents} centimes")]
    VatCreditCreditor { cents: i64 },
}

impl From<VatLiquidationError> for AppError {
    fn from(error: VatLiquidationError) -> Self {
        Self::Domain(error.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PostedVatLiquidation {
    pub period_key: String,
    pub piece_ref: String,
    #[serde(with = "crate::domain::serde_date::date")]
    pub on: Date,
    pub written: bool,
}

/// Pose l'OD de liquidation du mois `period_key`. `on` est le jour du geste :
/// le mois doit être fini (son dernier jour, ou après).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiquidateCa3 {
    pub period_key: String,
    #[serde(with = "crate::domain::serde_date::date")]
    pub on: Date,
}

impl Command for LiquidateCa3 {
    type Output = PostedVatLiquidation;
    const NAME: &'static str = "society.liquidate_ca3";

    fn requires_confirmation(&self) -> bool {
        true
    }

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let month = super::parse_after_period(&self.period_key)
            .map_err(|_| VatLiquidationError::InvalidPeriod(self.period_key.clone()))?;
        let period_key = month.to_string();
        let last_day = month.last_day();
        if self.on < last_day {
            return Err(VatLiquidationError::MonthNotFinished.into());
        }
        if journal::date_in_recorded_exercise(conn, last_day)? {
            return Err(VatLiquidationError::FiscalYearClosed.into());
        }
        if super::filing_for(conn, FiscalDeadlineKind::Ca3, &period_key)?.is_some() {
            return Err(VatLiquidationError::DeclarationAlreadyFiled.into());
        }
        let desired = desired_entry(conn, month, &period_key, last_day)?;
        let written = sync_vat_liquidation(conn, &period_key, desired)?;
        Ok(PostedVatLiquidation {
            period_key: period_key.clone(),
            piece_ref: format!("CA3-{period_key}"),
            on: last_day,
            written,
        })
    }
}

struct BookedVat {
    deductible: Money,
    rounded: Money,
    collected: Money,
    previous: Money,
    reversal: Money,
    intracom: Money,
}

fn desired_entry(
    conn: &Connection,
    month: Month,
    period_key: &str,
    last_day: Date,
) -> Result<Option<LedgerEntry>, AppError> {
    let booked = booked_vat(conn, month, period_key)?;
    let gap = booked.rounded - booked.deductible;
    if gap.is_negative() {
        return Err(VatLiquidationError::DeductibleExceedsRoundedEuro {
            book: booked.deductible.cents(),
            rounded: booked.rounded.cents(),
        }
        .into());
    }
    if booked.previous.is_negative() {
        return Err(VatLiquidationError::VatCreditCreditor {
            cents: -booked.previous.cents(),
        }
        .into());
    }
    let lines = liquidation_lines(&booked, gap, french_month(month));
    if lines.is_empty() {
        return Ok(None);
    }
    let entry = LedgerEntry {
        journal: Journal::Misc,
        number: 0,
        date: last_day,
        piece_ref: format!("CA3-{period_key}"),
        piece_date: last_day,
        label: format!("3310-CA3 {}", french_month(month)),
        lines,
    };
    if !entry.is_balanced() {
        return Err(AppError::Domain(
            "écriture de liquidation de TVA déséquilibrée — refusée".into(),
        ));
    }
    Ok(Some(entry))
}

fn booked_vat(conn: &Connection, month: Month, period_key: &str) -> Result<BookedVat, AppError> {
    let profile = company_profile(conn)?.ok_or(VatLiquidationError::ProfileMissing)?;
    if profile.fiscal_year_end.is_none() {
        return Err(VatLiquidationError::FiscalYearEndMissing.into());
    }
    let book = crate::accounting::ca3_month(conn, month)?;
    let reversal = match super::vat_reversal_for(conn, period_key)? {
        Some(record) if record.amount.cents() > 0 => record.amount,
        _ => Money::ZERO,
    };
    Ok(BookedVat {
        deductible: book.deductible_book,
        rounded: book.deductible_rounded,
        collected: book.collected_signed,
        previous: book.credit_on_book,
        reversal,
        intracom: book.intracom,
    })
}

fn liquidation_lines(booked: &BookedVat, gap: Money, month_name: &str) -> Vec<LedgerLine> {
    let mut lines = Vec::new();
    let label = |suffix: &str| format!("3310-CA3 {month_name} - {suffix}");
    if booked.rounded.cents() > 0 {
        lines.push(ca3_line(
            accounts::VAT_DEDUCTIBLE,
            -booked.rounded,
            label("TVA Déductible ABS"),
        ));
    }
    if booked.previous.cents() > 0 {
        lines.push(ca3_line(
            accounts::VAT_CREDIT,
            -booked.previous,
            label("Report du crédit de TVA"),
        ));
    }
    if !booked.collected.is_zero() {
        lines.push(ca3_line(
            accounts::VAT_COLLECTED,
            -booked.collected,
            label("TVA collectée"),
        ));
    }
    let position = booked.previous + booked.rounded - booked.reversal + booked.collected;
    if position.cents() > 0 {
        lines.push(ca3_line(
            accounts::VAT_CREDIT,
            position,
            label("Crédit de TVA"),
        ));
    } else if position.cents() < 0 {
        lines.push(ca3_line(
            accounts::VAT_DUE,
            position,
            label("TVA à décaisser"),
        ));
    }
    if gap.cents() > 0 {
        let ecart = label("Écarts de TVA");
        lines.push(ca3_line(accounts::VAT_DEDUCTIBLE, gap, ecart.clone()));
        lines.push(ca3_line(accounts::SUNDRY_INCOME, -gap, ecart));
    }
    if booked.reversal.cents() > 0 {
        lines.push(ca3_line(
            accounts::VAT_DEDUCTIBLE,
            booked.reversal,
            label("TVA antérieurement déduite à reverser"),
        ));
    }
    if !booked.intracom.is_zero() {
        lines.push(ca3_line(
            accounts::VAT_INTRACOM_DUE,
            booked.intracom,
            label("TVA due intracommunautaire"),
        ));
        lines.push(ca3_line(
            accounts::VAT_INTRACOM_DEDUCTIBLE,
            -booked.intracom,
            label("TVA déductible intracommunautaire"),
        ));
    }
    lines
}

fn ca3_line(account: Account, amount: Money, label: String) -> LedgerLine {
    LedgerLine {
        account,
        aux: None,
        amount,
        line_label: Some(label),
    }
}

fn french_month(month: Month) -> &'static str {
    match time::Month::try_from(month.month()).expect("Month n'admet que 1..=12") {
        time::Month::January => "Janvier",
        time::Month::February => "Février",
        time::Month::March => "Mars",
        time::Month::April => "Avril",
        time::Month::May => "Mai",
        time::Month::June => "Juin",
        time::Month::July => "Juillet",
        time::Month::August => "Août",
        time::Month::September => "Septembre",
        time::Month::October => "Octobre",
        time::Month::November => "Novembre",
        time::Month::December => "Décembre",
    }
}

#[cfg(test)]
#[allow(clippy::too_many_lines)]
mod tests {
    use time::Month as TimeMonth;

    use super::*;
    use crate::app::{Actor, ExecutionContext, Executor, Outcome};
    use crate::billing::{
        EmitInvoice, ImportBankTransactions, RecordPayment, SettleBankTransaction,
    };
    use crate::clients::CreateClient;
    use crate::company::SetCompanyProfile;
    use crate::domain::{
        Address, ExpenseCategory, ExpensePaidBy, FiscalYear, FiscalYearEnd, InvoiceLine, Money,
        OpeningBalanceLine, PaymentMethod, Siren, VatRate, VatRegime,
    };
    use crate::expenses::RecordExpense;
    use crate::fiscal::FiscalDeadlineKind;
    use crate::ledger::build_ledger;
    use crate::opening_balance::RecordOpeningBalance;
    use crate::society::{MarkDutyFiled, RecordVatCarryIn, RecordVatReversal, duty_briefing};
    use crate::store::Store;
    use crate::store::testing::test_store;

    fn date(year: i32, month: TimeMonth, day: u8) -> Date {
        Date::from_calendar_date(year, month, day).unwrap()
    }

    fn human() -> ExecutionContext {
        ExecutionContext::new(Actor::Human, false)
    }

    fn applied<T: std::fmt::Debug>(outcome: Outcome<T>) -> T {
        match outcome {
            Outcome::Applied(value) => value,
            other => panic!("attendu Applied, reçu {other:?}"),
        }
    }

    fn cents(n: i64) -> Money {
        Money::from_cents(n)
    }

    fn profile(store: &mut Store) {
        applied(
            Executor::new(store)
                .execute(
                    &SetCompanyProfile {
                        name: "Argon Digital".into(),
                        legal_form: "SASU".into(),
                        siren: Siren::parse("552100554").unwrap(),
                        vat_number: None,
                        address: Address {
                            street: "12 rue de la Paix".into(),
                            postal_code: "75002".into(),
                            city: "Paris".into(),
                            country: "FR".into(),
                        },
                        share_capital: Some(cents(100_000)),
                        rcs_city: Some("Paris".into()),
                        iban: None,
                        fiscal_year_end: Some(FiscalYearEnd::new(9, 30).unwrap()),
                        vat_regime: Some(VatRegime::RealNormalMonthly),
                        director_monthly_gross: None,
                        director_charge_ratio_bps: None,
                        president_name: Some("Léa Martin".into()),
                        sole_shareholder_name: Some("Léa Martin".into()),
                        sole_shareholder_address: Some("12 rue de la Paix, 75002 Paris".into()),
                        share_count: Some(1000),
                    },
                    &human(),
                )
                .unwrap(),
        );
    }

    fn opening(store: &mut Store, opens_on: Date, lines: &[&str]) {
        let lines: Vec<OpeningBalanceLine> =
            lines.iter().map(|line| line.parse().unwrap()).collect();
        applied(
            Executor::new(store)
                .execute(
                    &RecordOpeningBalance {
                        opens_on,
                        source: Some("à-nouveaux de test".into()),
                        lines,
                        tax_losses: Money::ZERO,
                        prior_corporate_tax: None,
                        prior_vat_due: None,
                    },
                    &human(),
                )
                .unwrap(),
        );
    }

    /// Dépense payée par l'associé : le 445660 est daté du jour de la facture.
    /// `vat` centimes à 20 % exige un TTC de `vat × 6`.
    fn associate_vat(store: &mut Store, on: Date, vat: i64) {
        applied(
            Executor::new(store)
                .execute(
                    &RecordExpense {
                        label: format!("TVA {vat}"),
                        category: ExpenseCategory::Software,
                        amount: cents(vat * 6),
                        vat_rate: VatRate::Standard,
                        vat_deductible: cents(vat),
                        incurred_on: on,
                        receipt_hash: None,
                        receipt_filename: None,
                        bank_transaction_id: None,
                        supplier: None,
                        paid_by: ExpensePaidBy::Associate,

                        reverse_charge: false,
                    },
                    &human(),
                )
                .unwrap(),
        );
    }

    fn liquidate(
        store: &mut Store,
        period: &str,
        on: Date,
    ) -> Result<Outcome<PostedVatLiquidation>, AppError> {
        Executor::new(store).execute(
            &LiquidateCa3 {
                period_key: period.into(),
                on,
            },
            &human(),
        )
    }

    fn fy_ending_september(end_year: i32) -> FiscalYear {
        FiscalYear::new(
            date(end_year - 1, TimeMonth::October, 1),
            date(end_year, TimeMonth::September, 30),
        )
    }

    fn account_balance(store: &Store, exercise: FiscalYear, number: &str) -> i64 {
        let ledger = build_ledger(store.connection(), exercise).unwrap();
        ledger
            .entries
            .iter()
            .flat_map(|entry| entry.lines.iter())
            .filter(|line| line.account.number == number)
            .map(|line| line.amount.cents())
            .sum()
    }

    fn live_lines(store: &Store, piece: &str) -> Vec<(String, i64, Option<String>)> {
        store
            .connection()
            .prepare(
                "SELECT l.account, l.amount_cents, l.ecriture_lib
                   FROM journal_lines l
                   JOIN journal_entries e ON e.id = l.entry_id
                  WHERE e.piece_ref = ?1
                    AND e.reversed_at IS NULL
                    AND e.reversal_of IS NULL
                  ORDER BY l.position",
            )
            .unwrap()
            .query_map([piece], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    fn journal_count(store: &Store) -> i64 {
        store
            .connection()
            .query_row("SELECT count(*) FROM journal_entries", [], |row| row.get(0))
            .unwrap()
    }

    fn live_liquidations(store: &Store) -> i64 {
        store
            .connection()
            .query_row(
                "SELECT count(*) FROM journal_entries
                  WHERE source_kind = 'vat_liquidation'
                    AND reversed_at IS NULL
                    AND reversal_of IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn seed_october(store: &mut Store) {
        profile(store);
        opening(
            store,
            date(2024, TimeMonth::October, 1),
            &[
                "445670:Crédit de TVA:D:50.00",
                "445660:TVA déductible sur ABS:C:1.20",
                "101000:Capital social:C:48.80",
            ],
        );
        // 1,50 + 25,50 + 0,80 + 13,00 = 40,80 €. L'à-nouveau de 1,20 € n'en fait pas partie.
        for (day, vat) in [(2, 150), (15, 2_550), (17, 80), (31, 1_300)] {
            associate_vat(store, date(2024, TimeMonth::October, day), vat);
        }
    }

    #[test]
    fn october_2024_posts_the_five_lines_once_and_keeps_the_opening_credit() {
        let mut store = test_store("ca3-oct-2024");
        seed_october(&mut store);
        let exercise = fy_ending_september(2025);
        let before = build_ledger(store.connection(), exercise).unwrap();
        assert!(
            before
                .entries
                .iter()
                .all(|entry| !entry.piece_ref.starts_with("CA3-")),
            "la lecture du livre n'insère pas la liquidation"
        );
        assert_eq!(live_liquidations(&store), 0);
        let rows_before_read = journal_count(&store);
        let _ = build_ledger(store.connection(), exercise).unwrap();
        assert_eq!(journal_count(&store), rows_before_read);

        let before = duty_briefing(
            store.connection(),
            FiscalDeadlineKind::Ca3,
            date(2024, TimeMonth::November, 19),
            Some("2024-10"),
        )
        .unwrap();
        let box_of = |case: &str| {
            before
                .boxes
                .iter()
                .find(|b| b.case == case)
                .and_then(|b| b.amount)
        };
        // Déductible 4 080, ligne arrondie 4 100. Report 5 000. Crédit 9 100.
        assert_eq!(box_of("20"), Some(cents(4_100)));
        assert_eq!(box_of("22"), Some(cents(5_000)));
        assert_eq!(box_of("25"), Some(cents(9_100)));
        assert_eq!(box_of("27"), Some(cents(9_100)));
        assert!(before.boxes.iter().all(|b| b.case != "08"));
        assert!(before.boxes.iter().all(|b| b.case != "19"));
        assert!(!before.vat_liquidated);
        assert!(before.vat_month_elapsed);

        applied(liquidate(&mut store, "2024-10", date(2024, TimeMonth::October, 31)).unwrap());
        let lines = live_lines(&store, "CA3-2024-10");
        assert_eq!(
            lines,
            vec![
                (
                    "445660".into(),
                    -4_100,
                    Some("3310-CA3 Octobre - TVA Déductible ABS".into())
                ),
                (
                    "445670".into(),
                    -5_000,
                    Some("3310-CA3 Octobre - Report du crédit de TVA".into())
                ),
                (
                    "445670".into(),
                    9_100,
                    Some("3310-CA3 Octobre - Crédit de TVA".into())
                ),
                (
                    "445660".into(),
                    20,
                    Some("3310-CA3 Octobre - Écarts de TVA".into())
                ),
                (
                    "758000".into(),
                    -20,
                    Some("3310-CA3 Octobre - Écarts de TVA".into())
                ),
            ]
        );
        assert_eq!(lines.len(), 5);
        assert!(
            lines
                .iter()
                .all(|(account, _, _)| account != "445200" && account != "445662")
        );
        let head: (String, String, String) = store
            .connection()
            .query_row(
                "SELECT journal, entry_date, piece_ref FROM journal_entries
                  WHERE piece_ref = 'CA3-2024-10' AND reversed_at IS NULL AND reversal_of IS NULL",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            head,
            ("OD".into(), "2024-10-31".into(), "CA3-2024-10".into())
        );
        assert_eq!(account_balance(&store, exercise, "445660"), -120);
        assert_eq!(account_balance(&store, exercise, "445670"), 9_100);
        assert_eq!(account_balance(&store, exercise, "445510"), 0);
        assert_eq!(account_balance(&store, exercise, "758000"), -20);
        assert_eq!(account_balance(&store, exercise, "635000"), 0);

        let rows = journal_count(&store);
        let again =
            applied(liquidate(&mut store, "2024-10", date(2024, TimeMonth::November, 19)).unwrap());
        assert!(!again.written);
        assert_eq!(journal_count(&store), rows);
        assert_eq!(live_liquidations(&store), 1);
        assert_eq!(account_balance(&store, exercise, "445670"), 9_100);
        let after = duty_briefing(
            store.connection(),
            FiscalDeadlineKind::Ca3,
            date(2024, TimeMonth::November, 19),
            Some("2024-10"),
        )
        .unwrap();
        assert!(after.vat_liquidated);
        assert!(after.vat_month_elapsed);
    }

    #[test]
    fn november_carries_the_october_credit_without_a_new_sundry_line() {
        let mut store = test_store("ca3-nov-2024");
        seed_october(&mut store);
        applied(liquidate(&mut store, "2024-10", date(2024, TimeMonth::October, 31)).unwrap());
        associate_vat(&mut store, date(2024, TimeMonth::November, 29), 1_000);

        applied(liquidate(&mut store, "2024-11", date(2024, TimeMonth::November, 30)).unwrap());
        assert_eq!(
            live_lines(&store, "CA3-2024-11"),
            vec![
                (
                    "445660".into(),
                    -1_000,
                    Some("3310-CA3 Novembre - TVA Déductible ABS".into())
                ),
                (
                    "445670".into(),
                    -9_100,
                    Some("3310-CA3 Novembre - Report du crédit de TVA".into())
                ),
                (
                    "445670".into(),
                    10_100,
                    Some("3310-CA3 Novembre - Crédit de TVA".into())
                ),
            ]
        );
        let exercise = fy_ending_september(2025);
        assert_eq!(account_balance(&store, exercise, "445670"), 10_100);
        assert_eq!(account_balance(&store, exercise, "758000"), -20);
        assert_eq!(account_balance(&store, exercise, "635000"), 0);
        assert_eq!(account_balance(&store, exercise, "445660"), -120);

        let rows = journal_count(&store);
        applied(liquidate(&mut store, "2024-11", date(2024, TimeMonth::December, 15)).unwrap());
        assert_eq!(journal_count(&store), rows);
        assert_eq!(account_balance(&store, exercise, "445670"), 10_100);
    }

    #[test]
    fn a_reversal_brings_the_september_credit_down() {
        let mut store = test_store("ca3-sep-2026");
        profile(&mut store);
        opening(
            &mut store,
            date(2025, TimeMonth::October, 1),
            &[
                "445670:Crédit de TVA:D:400.00",
                "101000:Capital social:C:400.00",
            ],
        );
        applied(
            Executor::new(&mut store)
                .execute(
                    &RecordVatCarryIn {
                        after_period: "2026-08".into(),
                        credit: cents(40_000),
                        source: Some("CA3 août".into()),
                    },
                    &human(),
                )
                .unwrap(),
        );
        applied(
            Executor::new(&mut store)
                .execute(
                    &RecordVatReversal {
                        period_key: "2026-09".into(),
                        amount: cents(1_000),
                        recorded_on: date(2026, TimeMonth::September, 15),
                    },
                    &human(),
                )
                .unwrap(),
        );
        // La correction laisse 445660 créditeur de 10,00 €.
        // Le règlement pose ce crédit dans le livre.
        applied(
            Executor::new(&mut store)
                .execute(
                    &ImportBankTransactions {
                        transactions: vec![crate::billing::ParsedTransaction {
                            occurred_on: date(2026, TimeMonth::September, 12),
                            amount_cents: 1_000,
                            description: "Crédit 445660 — correction déjà dans le livre".into(),
                            fitid: None,
                        }],
                    },
                    &human(),
                )
                .unwrap(),
        );
        let tx = crate::billing::list_bank_transactions(store.connection())
            .unwrap()
            .into_iter()
            .find(|tx| tx.amount_cents == 1_000)
            .unwrap()
            .id;
        applied(
            Executor::new(&mut store)
                .execute(
                    &SettleBankTransaction {
                        transaction_id: tx,
                        account: "445660".parse().unwrap(),
                        label: None,
                    },
                    &human(),
                )
                .unwrap(),
        );

        let exercise = fy_ending_september(2026);
        assert_eq!(account_balance(&store, exercise, "445670"), 40_000);
        assert_eq!(account_balance(&store, exercise, "445660"), -1_000);
        let before = duty_briefing(
            store.connection(),
            FiscalDeadlineKind::Ca3,
            date(2026, TimeMonth::September, 15),
            Some("2026-09"),
        )
        .unwrap();
        let box_of = |case: &str| {
            before
                .boxes
                .iter()
                .find(|b| b.case == case)
                .and_then(|b| b.amount)
        };
        assert_eq!(box_of("15"), Some(cents(1_000)));
        assert_eq!(box_of("22"), Some(cents(40_000)));
        assert_eq!(box_of("25"), Some(cents(39_000)));
        assert_eq!(box_of("27"), Some(cents(39_000)));

        applied(liquidate(&mut store, "2026-09", date(2026, TimeMonth::September, 30)).unwrap());
        assert_eq!(
            live_lines(&store, "CA3-2026-09"),
            vec![
                (
                    "445670".into(),
                    -40_000,
                    Some("3310-CA3 Septembre - Report du crédit de TVA".into())
                ),
                (
                    "445670".into(),
                    39_000,
                    Some("3310-CA3 Septembre - Crédit de TVA".into())
                ),
                (
                    "445660".into(),
                    1_000,
                    Some("3310-CA3 Septembre - TVA antérieurement déduite à reverser".into())
                ),
            ]
        );
        assert_eq!(live_lines(&store, "CA3-2026-09").len(), 3);
        assert!(
            live_lines(&store, "CA3-2026-09")
                .iter()
                .all(|(account, _, _)| account != "445200" && account != "445662")
        );
        assert_eq!(account_balance(&store, exercise, "445670"), 39_000);
        assert_eq!(account_balance(&store, exercise, "445660"), 0);
        assert_eq!(account_balance(&store, exercise, "445510"), 0);
        assert_eq!(account_balance(&store, exercise, "635000"), 0);
        assert_eq!(account_balance(&store, exercise, "758000"), 0);

        let after = duty_briefing(
            store.connection(),
            FiscalDeadlineKind::Ca3,
            date(2026, TimeMonth::September, 30),
            Some("2026-09"),
        )
        .unwrap();
        let after_box = |case: &str| {
            after
                .boxes
                .iter()
                .find(|b| b.case == case)
                .and_then(|b| b.amount)
        };
        assert_eq!(after_box("15"), Some(cents(1_000)));
        assert_eq!(after_box("22"), Some(cents(40_000)));
        assert_eq!(after_box("25"), Some(cents(39_000)));
        assert_eq!(after_box("27"), Some(cents(39_000)));
        let october = duty_briefing(
            store.connection(),
            FiscalDeadlineKind::Ca3,
            date(2026, TimeMonth::October, 8),
            Some("2026-10"),
        )
        .unwrap();
        let oct22 = october
            .boxes
            .iter()
            .find(|b| b.case == "22")
            .and_then(|b| b.amount);
        assert_eq!(oct22, Some(cents(39_000)));

        let rows = journal_count(&store);
        applied(liquidate(&mut store, "2026-09", date(2026, TimeMonth::October, 8)).unwrap());
        assert_eq!(journal_count(&store), rows);
        assert_eq!(live_liquidations(&store), 1);
        assert_eq!(account_balance(&store, exercise, "445670"), 39_000);
    }

    fn import_debit(store: &mut Store, on: Date, cents: i64) -> crate::domain::BankTransactionId {
        applied(
            Executor::new(store)
                .execute(
                    &ImportBankTransactions {
                        transactions: vec![crate::billing::ParsedTransaction {
                            occurred_on: on,
                            amount_cents: -cents,
                            description: format!("prestation {on}"),
                            fitid: None,
                        }],
                    },
                    &human(),
                )
                .unwrap(),
        );
        crate::billing::list_bank_transactions(store.connection())
            .unwrap()
            .into_iter()
            .find(|tx| tx.occurred_on == on && tx.amount_cents == -cents)
            .unwrap()
            .id
    }

    fn pay_reverse_charge(store: &mut Store, incurred: Date, statement: Date) -> String {
        let transaction_id = import_debit(store, statement, 10_000);
        let id = applied(
            Executor::new(store)
                .execute(
                    &RecordExpense {
                        label: "Prestation".into(),
                        category: ExpenseCategory::Software,
                        amount: cents(10_000),
                        vat_rate: VatRate::Standard,
                        vat_deductible: cents(0),
                        incurred_on: incurred,
                        receipt_hash: None,
                        receipt_filename: None,
                        bank_transaction_id: Some(transaction_id),
                        supplier: Some("Atelier".into()),
                        paid_by: ExpensePaidBy::Company,
                        reverse_charge: true,
                    },
                    &human(),
                )
                .unwrap(),
        );
        format!("DEP-{id}")
    }

    /// Un seul paiement autoliquidé. La liquidation solde la paire au centime.
    /// Pas de 758. Les autres mois de la suite gardent leurs lignes.
    #[test]
    fn reverse_charge_of_the_month_is_settled_to_the_cent_without_a_sundry_line() {
        let mut store = test_store("ca3-intracom-one");
        profile(&mut store);
        let exercise = fy_ending_september(2026);
        let piece = pay_reverse_charge(
            &mut store,
            date(2025, TimeMonth::November, 1),
            date(2025, TimeMonth::November, 6),
        );
        assert_eq!(
            live_lines(&store, &piece),
            vec![
                ("651000".into(), 10_000, None),
                ("445662".into(), 2_000, None),
                ("445200".into(), -2_000, None),
                ("512000".into(), -10_000, None),
            ]
        );
        let rows_before_read = journal_count(&store);
        let _ = build_ledger(store.connection(), exercise).unwrap();
        assert_eq!(journal_count(&store), rows_before_read);

        let before = duty_briefing(
            store.connection(),
            FiscalDeadlineKind::Ca3,
            date(2025, TimeMonth::December, 1),
            Some("2025-11"),
        )
        .unwrap();
        assert!(before.boxes.iter().all(|b| b.case != "08"));
        assert!(before.boxes.iter().all(|b| b.case != "20"));

        applied(liquidate(&mut store, "2025-11", date(2025, TimeMonth::November, 30)).unwrap());
        assert_eq!(account_balance(&store, exercise, "445200"), 0);
        assert_eq!(account_balance(&store, exercise, "445662"), 0);
        assert_eq!(account_balance(&store, exercise, "758000"), 0);
        assert_eq!(account_balance(&store, exercise, "635000"), 0);
        assert_eq!(account_balance(&store, exercise, "658000"), 0);
        let lines = live_lines(&store, "CA3-2025-11");
        assert_eq!(
            lines,
            vec![
                (
                    "445200".into(),
                    2_000,
                    Some("3310-CA3 Novembre - TVA due intracommunautaire".into())
                ),
                (
                    "445662".into(),
                    -2_000,
                    Some("3310-CA3 Novembre - TVA déductible intracommunautaire".into())
                ),
            ]
        );
        assert!(lines.iter().all(|(account, _, _)| account != "758000"));

        let rows = journal_count(&store);
        let again =
            applied(liquidate(&mut store, "2025-11", date(2025, TimeMonth::December, 19)).unwrap());
        assert!(!again.written);
        assert_eq!(journal_count(&store), rows);
        assert_eq!(account_balance(&store, exercise, "445200"), 0);
        assert_eq!(account_balance(&store, exercise, "445662"), 0);

        let mut october = test_store("ca3-oct-untouched");
        seed_october(&mut october);
        applied(liquidate(&mut october, "2024-10", date(2024, TimeMonth::October, 31)).unwrap());
        let october_lines = live_lines(&october, "CA3-2024-10");
        assert_eq!(october_lines.len(), 5);
        assert_eq!(october_lines[0].1, -4_100);
        assert_eq!(october_lines[3].1, 20);
        assert_eq!(october_lines[4].1, -20);
        assert!(
            october_lines
                .iter()
                .all(|(account, _, _)| account != "445200" && account != "445662")
        );

        let mut september = test_store("ca3-sep-untouched");
        seed_september_2026(&mut september);
        applied(
            liquidate(
                &mut september,
                "2026-09",
                date(2026, TimeMonth::September, 30),
            )
            .unwrap(),
        );
        let september_lines = live_lines(&september, "CA3-2026-09");
        assert_eq!(september_lines.len(), 3);
        assert_eq!(september_lines[0].1, -40_000);
        assert_eq!(september_lines[1].1, 39_000);
        assert_eq!(september_lines[2].1, 1_000);
        assert!(
            september_lines
                .iter()
                .all(|(account, _, _)| account != "445200" && account != "445662")
        );
    }

    fn seed_september_2026(store: &mut Store) {
        profile(store);
        opening(
            store,
            date(2025, TimeMonth::October, 1),
            &[
                "445670:Crédit de TVA:D:400.00",
                "101000:Capital social:C:400.00",
            ],
        );
        applied(
            Executor::new(store)
                .execute(
                    &RecordVatCarryIn {
                        after_period: "2026-08".into(),
                        credit: cents(40_000),
                        source: Some("CA3 août".into()),
                    },
                    &human(),
                )
                .unwrap(),
        );
        applied(
            Executor::new(store)
                .execute(
                    &RecordVatReversal {
                        period_key: "2026-09".into(),
                        amount: cents(1_000),
                        recorded_on: date(2026, TimeMonth::September, 15),
                    },
                    &human(),
                )
                .unwrap(),
        );
        applied(
            Executor::new(store)
                .execute(
                    &ImportBankTransactions {
                        transactions: vec![crate::billing::ParsedTransaction {
                            occurred_on: date(2026, TimeMonth::September, 12),
                            amount_cents: 1_000,
                            description: "Crédit 445660 — correction déjà dans le livre".into(),
                            fitid: None,
                        }],
                    },
                    &human(),
                )
                .unwrap(),
        );
        let tx = crate::billing::list_bank_transactions(store.connection())
            .unwrap()
            .into_iter()
            .find(|tx| tx.amount_cents == 1_000)
            .unwrap()
            .id;
        applied(
            Executor::new(store)
                .execute(
                    &SettleBankTransaction {
                        transaction_id: tx,
                        account: "445660".parse().unwrap(),
                        label: None,
                    },
                    &human(),
                )
                .unwrap(),
        );
    }

    /// Trois paiements de 2 000 centimes font 6 000. Après liquidation les
    /// deux comptes sont à zéro.
    #[test]
    fn three_reverse_charges_of_2000_are_6000_and_liquidate_to_zero() {
        assert_eq!(2_000 + 2_000 + 2_000, 6_000);

        let mut store = test_store("ca3-intracom-three");
        profile(&mut store);
        let exercise = fy_ending_september(2026);
        for day in 1..=3 {
            pay_reverse_charge(
                &mut store,
                date(2025, TimeMonth::November, day),
                date(2025, TimeMonth::November, day),
            );
        }
        let ledger = build_ledger(store.connection(), exercise).unwrap();
        let economic: i64 = ledger
            .entries
            .iter()
            .filter(|entry| entry.piece_ref.starts_with("DEP-"))
            .flat_map(|entry| entry.lines.iter())
            .filter(|line| line.account.number == "445662" && line.amount.cents() > 0)
            .map(|line| line.amount.cents())
            .sum();
        assert_eq!(economic, 6_000);

        applied(liquidate(&mut store, "2025-11", date(2025, TimeMonth::November, 30)).unwrap());
        assert_eq!(
            live_lines(&store, "CA3-2025-11"),
            vec![
                (
                    "445200".into(),
                    6_000,
                    Some("3310-CA3 Novembre - TVA due intracommunautaire".into())
                ),
                (
                    "445662".into(),
                    -6_000,
                    Some("3310-CA3 Novembre - TVA déductible intracommunautaire".into())
                ),
            ]
        );
        assert_eq!(account_balance(&store, exercise, "445200"), 0);
        assert_eq!(account_balance(&store, exercise, "445662"), 0);
        assert_eq!(account_balance(&store, exercise, "758000"), 0);
        assert_eq!(account_balance(&store, exercise, "635000"), 0);
        assert_eq!(account_balance(&store, exercise, "658000"), 0);
        let rows = journal_count(&store);
        applied(liquidate(&mut store, "2025-11", date(2025, TimeMonth::December, 2)).unwrap());
        assert_eq!(journal_count(&store), rows);
        assert_eq!(account_balance(&store, exercise, "445662"), 0);
        assert_eq!(account_balance(&store, exercise, "445200"), 0);
    }

    #[test]
    fn a_collected_vat_of_20000_debits_445710_and_credits_445510() {
        let mut store = test_store("ca3-due");
        profile(&mut store);
        let client = applied(
            Executor::new(&mut store)
                .execute(
                    &CreateClient {
                        name: "Client".into(),
                        siren: None,
                        vat_number: None,
                        address: None,
                    },
                    &human(),
                )
                .unwrap(),
        );
        let invoice = applied(
            Executor::new(&mut store)
                .execute(
                    &EmitInvoice {
                        client_id: client,
                        mission_id: None,
                        lines: vec![InvoiceLine {
                            description: "Prestation".into(),
                            quantity: 1.0,
                            unit_price: cents(100_000),
                            vat_rate: VatRate::Standard,
                        }],
                        issued_on: date(2026, TimeMonth::March, 2),
                        payment_terms_days: 30,
                    },
                    &human(),
                )
                .unwrap(),
        );
        applied(
            Executor::new(&mut store)
                .execute(
                    &RecordPayment {
                        invoice_id: invoice.id,
                        amount: cents(120_000),
                        received_on: date(2026, TimeMonth::March, 18),
                        method: PaymentMethod::BankTransfer,
                    },
                    &human(),
                )
                .unwrap(),
        );
        let exercise = fy_ending_september(2026);
        assert_eq!(account_balance(&store, exercise, "445710"), -20_000);
        assert_eq!(live_liquidations(&store), 0);

        applied(liquidate(&mut store, "2026-03", date(2026, TimeMonth::March, 31)).unwrap());
        assert_eq!(
            live_lines(&store, "CA3-2026-03"),
            vec![
                (
                    "445710".into(),
                    20_000,
                    Some("3310-CA3 Mars - TVA collectée".into())
                ),
                (
                    "445510".into(),
                    -20_000,
                    Some("3310-CA3 Mars - TVA à décaisser".into())
                ),
            ]
        );
        assert_eq!(account_balance(&store, exercise, "445710"), 0);
        assert_eq!(account_balance(&store, exercise, "445510"), -20_000);
        assert_eq!(account_balance(&store, exercise, "758000"), 0);
        assert_eq!(account_balance(&store, exercise, "445670"), 0);
    }

    #[test]
    fn recording_an_expense_does_not_post_a_liquidation() {
        let mut store = test_store("ca3-expense-alone");
        profile(&mut store);
        associate_vat(&mut store, date(2026, TimeMonth::January, 4), 1_980);
        let exercise = fy_ending_september(2026);
        let ledger = build_ledger(store.connection(), exercise).unwrap();
        assert!(
            ledger
                .entries
                .iter()
                .any(|entry| entry.piece_ref.starts_with("DEP-"))
        );
        assert!(
            ledger
                .entries
                .iter()
                .all(|entry| !entry.piece_ref.starts_with("CA3-"))
        );
        assert_eq!(live_liquidations(&store), 0);
    }

    #[test]
    fn an_unfinished_month_is_refused() {
        let mut store = test_store("ca3-not-finished");
        profile(&mut store);
        let err = liquidate(&mut store, "2024-10", date(2024, TimeMonth::October, 30)).unwrap_err();
        assert!(err.to_string().contains("pas fini"), "{err}");
        assert_eq!(live_liquidations(&store), 0);
    }

    #[test]
    fn a_filed_month_is_refused() {
        let mut store = test_store("ca3-filed");
        profile(&mut store);
        applied(
            Executor::new(&mut store)
                .execute(
                    &MarkDutyFiled {
                        kind: FiscalDeadlineKind::Ca3,
                        period_key: "2024-10".into(),
                        due_on: date(2024, TimeMonth::November, 19),
                        filed_on: date(2024, TimeMonth::November, 18),
                    },
                    &human(),
                )
                .unwrap(),
        );
        let err =
            liquidate(&mut store, "2024-10", date(2024, TimeMonth::November, 19)).unwrap_err();
        assert!(err.to_string().contains("déjà déposée"), "{err}");
        assert_eq!(live_liquidations(&store), 0);
    }

    #[test]
    fn a_closed_exercise_is_refused() {
        let mut store = test_store("ca3-closed");
        profile(&mut store);
        store
            .connection()
            .execute(
                "INSERT INTO fiscal_years
                    (id, starts_on, ends_on, revenue_ht_cents, expenses_cents,
                     director_remuneration_cents, result_before_tax_cents, corporate_tax_cents,
                     net_result_cents, retained_earnings_cents, created_at)
                 VALUES (?1, '2024-10-01', '2025-09-30', 0, 0, 0, 0, 0, 0, 0, '2026-01-01T00:00:00Z')",
                [crate::domain::FiscalYearId::new().to_string()],
            )
            .unwrap();
        let err =
            liquidate(&mut store, "2024-10", date(2024, TimeMonth::November, 19)).unwrap_err();
        assert!(err.to_string().contains("clos"), "{err}");
        assert_eq!(live_liquidations(&store), 0);
    }

    #[test]
    fn a_deductible_above_the_rounded_euro_is_refused() {
        // 80,49 €. round_to_euro (art. 1657 CGI) donne 80 €. Le livre dépasse
        // la ligne de 49 centimes. Aucune écriture, même pas une charge inventée.
        let mut store = test_store("ca3-gap-down");
        profile(&mut store);
        associate_vat(&mut store, date(2024, TimeMonth::October, 15), 8_049);
        let err = liquidate(&mut store, "2024-10", date(2024, TimeMonth::October, 31)).unwrap_err();
        assert!(err.to_string().contains("8049"), "{err}");
        assert!(err.to_string().contains("8000"), "{err}");
        assert_eq!(live_liquidations(&store), 0);
    }

    #[test]
    fn a_changed_deductible_before_filing_reverses_and_reposts() {
        let mut store = test_store("ca3-extourne");
        seed_october(&mut store);
        applied(liquidate(&mut store, "2024-10", date(2024, TimeMonth::October, 31)).unwrap());
        associate_vat(&mut store, date(2024, TimeMonth::October, 20), 1);
        applied(liquidate(&mut store, "2024-10", date(2024, TimeMonth::November, 2)).unwrap());

        let reversed: i64 = store
            .connection()
            .query_row(
                "SELECT count(*) FROM journal_entries
                  WHERE piece_ref = 'CA3-2024-10' AND reversed_at IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(reversed, 1);
        let extourne: i64 = store
            .connection()
            .query_row(
                "SELECT count(*) FROM journal_entries WHERE piece_ref = 'EXT-CA3-2024-10'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(extourne, 1);
        // 4 080 + 1 = 4 081. Arrondi 4 100. Écart 19. Le report ne bouge pas.
        assert_eq!(
            live_lines(&store, "CA3-2024-10"),
            vec![
                (
                    "445660".into(),
                    -4_100,
                    Some("3310-CA3 Octobre - TVA Déductible ABS".into())
                ),
                (
                    "445670".into(),
                    -5_000,
                    Some("3310-CA3 Octobre - Report du crédit de TVA".into())
                ),
                (
                    "445670".into(),
                    9_100,
                    Some("3310-CA3 Octobre - Crédit de TVA".into())
                ),
                (
                    "445660".into(),
                    19,
                    Some("3310-CA3 Octobre - Écarts de TVA".into())
                ),
                (
                    "758000".into(),
                    -19,
                    Some("3310-CA3 Octobre - Écarts de TVA".into())
                ),
            ]
        );
        let exercise = fy_ending_september(2025);
        assert_eq!(account_balance(&store, exercise, "445670"), 9_100);
        assert_eq!(live_liquidations(&store), 1);
    }

    #[test]
    fn a_creditor_vat_credit_is_refused() {
        // Un 445670 créditeur n'est pas reporté. Le solder
        // inventerait une ligne. Rien n'est posé.
        let mut store = test_store("ca3-credit-creditor");
        profile(&mut store);
        opening(
            &mut store,
            date(2024, TimeMonth::October, 1),
            &[
                "445670:Crédit de TVA:C:10.00",
                "101000:Capital social:D:10.00",
            ],
        );
        let err = liquidate(&mut store, "2024-10", date(2024, TimeMonth::October, 31)).unwrap_err();
        assert!(err.to_string().contains("créditeur"), "{err}");
        assert!(err.to_string().contains("1000"), "{err}");
        assert_eq!(live_liquidations(&store), 0);
    }
}
