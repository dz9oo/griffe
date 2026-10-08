//! Paie saisie : trois écritures conservées, une fois pour le mois.
//!
//! [`crate::ledger::payroll_form`] fige la forme (641, 645, 421, 431, puis les
//! règlements). Ce n'est pas un bulletin. [`crate::ledger::Ledger::build`]
//! ne l'appelle pas : il lit les écritures déjà posées. Le brut du profil ne
//! poste rien. Pas de prélèvement à la source, pas de bulletin, pas de fichier
//! DSN.
//!
//! Une correction extourne. Un second appel identique ne double pas. La lecture
//! n'insère rien.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::Date;

use crate::app::{AppError, Command};
use crate::domain::{Money, Month};
use crate::journal::{self, PayrollPieces};
use crate::ledger::{self, LedgerEntry};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PostedPayroll {
    pub period_key: String,
    #[serde(with = "crate::domain::serde_date::date")]
    pub on: Date,
    pub written: bool,
}

#[derive(Debug, Error, PartialEq, Eq)]
enum PayrollError {
    #[error("montant de paie négatif")]
    NegativeAmount,
    #[error("montant de paie au-delà du plafond")]
    AmountTooLarge,
    #[error("la retenue salariale dépasse le brut")]
    WithholdingExceedsGross,
}

impl From<PayrollError> for AppError {
    fn from(error: PayrollError) -> Self {
        Self::Domain(error.to_string())
    }
}

/// Pose la forme de paie au jour `on`, une fois pour ce mois.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordPayroll {
    #[serde(with = "crate::domain::serde_date::date")]
    pub on: Date,
    pub gross: Money,
    pub employee_withholding: Money,
    pub employer_contributions: Money,
}

impl Command for RecordPayroll {
    type Output = PostedPayroll;
    const NAME: &'static str = "society.record_payroll";

    fn requires_confirmation(&self) -> bool {
        true
    }

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let gross = accepted(self.gross)?;
        let withholding = accepted(self.employee_withholding)?;
        let employer = accepted(self.employer_contributions)?;
        if withholding.cents() > gross.cents() {
            return Err(PayrollError::WithholdingExceedsGross.into());
        }
        if withholding.checked_add(employer).is_none() {
            return Err(PayrollError::AmountTooLarge.into());
        }
        let month = Month::new(self.on.year(), u8::from(self.on.month()))
            .map_err(|_| AppError::Domain("mois de paie illisible".into()))?;
        let period_key = month.to_string();
        let pieces = pieces_from(ledger::payroll_form(self.on, gross, withholding, employer))?;
        let written = journal::sync_payroll(conn, &period_key, &pieces)?;
        Ok(PostedPayroll {
            period_key,
            on: self.on,
            written,
        })
    }
}

fn accepted(amount: Money) -> Result<Money, PayrollError> {
    if amount.is_negative() {
        return Err(PayrollError::NegativeAmount);
    }
    if amount.cents() > Money::MAX_INPUT.cents() {
        return Err(PayrollError::AmountTooLarge);
    }
    Ok(amount)
}

fn pieces_from(entries: Vec<LedgerEntry>) -> Result<PayrollPieces, AppError> {
    let mut accrual = None;
    let mut net = None;
    let mut urssaf = None;
    for entry in entries {
        match entry.piece_ref.as_str() {
            "PAIE" => accrual = Some(entry),
            "PAIE-NET" => net = Some(entry),
            "PAIE-URSSAF" => urssaf = Some(entry),
            other => {
                return Err(AppError::Domain(format!(
                    "pièce de paie inattendue : {other}"
                )));
            }
        }
    }
    Ok(PayrollPieces {
        accrual,
        net,
        urssaf,
    })
}

#[cfg(test)]
mod tests {
    use super::RecordPayroll;
    use crate::app::{Actor, ExecutionContext, Executor, Outcome};
    use crate::billing::{ImportBankTransactions, ParsedTransaction, SettleBankTransaction};
    use crate::company::SetCompanyProfile;
    use crate::domain::{
        Address, FiscalYear, FiscalYearEnd, Money, OpeningBalanceLine, Siren, VatRegime,
    };
    use crate::fiscal::{FiscalDeadlineKind, fiscal_calendar};
    use crate::ledger::build_ledger;
    use crate::opening_balance::RecordOpeningBalance;
    use crate::store::Store;
    use crate::store::testing::test_store;
    use rusqlite::OptionalExtension;
    use time::Date;
    use time::Month as TimeMonth;

    fn date(year: i32, month: TimeMonth, day: u8) -> Date {
        Date::from_calendar_date(year, month, day).unwrap()
    }

    fn cents(n: i64) -> Money {
        Money::from_cents(n)
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

    fn profile(store: &mut Store, gross: Option<i64>, ratio_bps: Option<u32>) {
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
                        director_monthly_gross: gross.map(cents),
                        director_charge_ratio_bps: ratio_bps,
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

    fn record(
        store: &mut Store,
        on: Date,
        gross: i64,
        withholding: i64,
        employer: i64,
    ) -> PostedPayrollExpect {
        let posted = applied(
            Executor::new(store)
                .execute(
                    &RecordPayroll {
                        on,
                        gross: cents(gross),
                        employee_withholding: cents(withholding),
                        employer_contributions: cents(employer),
                    },
                    &human(),
                )
                .unwrap(),
        );
        PostedPayrollExpect(posted.written)
    }

    struct PostedPayrollExpect(bool);

    fn journal_count(store: &Store) -> i64 {
        store
            .connection()
            .query_row("SELECT count(*) FROM journal_entries", [], |row| row.get(0))
            .unwrap()
    }

    fn piece_head(store: &Store, piece: &str) -> Option<(String, String)> {
        store
            .connection()
            .query_row(
                "SELECT journal, entry_date FROM journal_entries
                  WHERE piece_ref = ?1
                    AND reversed_at IS NULL
                    AND reversal_of IS NULL",
                [piece],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .unwrap()
    }

    fn piece_lines(store: &Store, piece: &str) -> Vec<(String, i64)> {
        store
            .connection()
            .prepare(
                "SELECT l.account, l.amount_cents
                   FROM journal_lines l
                   JOIN journal_entries e ON e.id = l.entry_id
                  WHERE e.piece_ref = ?1
                    AND e.reversed_at IS NULL
                    AND e.reversal_of IS NULL
                  ORDER BY l.position",
            )
            .unwrap()
            .query_map([piece], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    fn piece_count(store: &Store, piece: &str) -> i64 {
        store
            .connection()
            .query_row(
                "SELECT count(*) FROM journal_entries WHERE piece_ref = ?1",
                [piece],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn dsn_amounts(store: &Store, today: Date) -> Vec<(Date, i64)> {
        fiscal_calendar(store.connection(), today)
            .unwrap()
            .into_iter()
            .filter(|deadline| deadline.kind == FiscalDeadlineKind::Dsn)
            .map(|deadline| (deadline.due_on, deadline.amount.map_or(0, Money::cents)))
            .collect()
    }

    fn account_cents(store: &Store, exercise: FiscalYear, number: &str) -> i64 {
        let ledger = build_ledger(store.connection(), exercise).unwrap();
        ledger
            .entries
            .iter()
            .flat_map(|entry| entry.lines.iter())
            .filter(|line| line.account.number == number)
            .map(|line| line.amount.cents())
            .sum()
    }

    fn has_account_prefix(store: &Store, exercise: FiscalYear, prefix: &str) -> bool {
        let ledger = build_ledger(store.connection(), exercise).unwrap();
        ledger
            .entries
            .iter()
            .flat_map(|entry| entry.lines.iter())
            .any(|line| line.account.number.starts_with(prefix))
    }

    fn settle(store: &mut Store, on: Date, amount_cents: i64, description: &str, account: &str) {
        applied(
            Executor::new(store)
                .execute(
                    &ImportBankTransactions {
                        transactions: vec![ParsedTransaction {
                            occurred_on: on,
                            amount_cents,
                            description: description.to_string(),
                            fitid: None,
                        }],
                    },
                    &human(),
                )
                .unwrap(),
        );
        let id = crate::billing::list_bank_transactions(store.connection())
            .unwrap()
            .into_iter()
            .find(|tx| tx.occurred_on == on && tx.description == description)
            .expect("mouvement importé")
            .id;
        applied(
            Executor::new(store)
                .execute(
                    &SettleBankTransaction {
                        transaction_id: id,
                        account: account.parse().unwrap(),
                        label: None,
                    },
                    &human(),
                )
                .unwrap(),
        );
    }

    /// Profil avec un brut de 300 000 centimes, aucune paie saisie : aucune
    /// pièce PAIE, aucune échéance DSN. Le ratio 8 000 donnerait 240 000
    /// centimes de cotisations estimées. Ce chiffre n'est pas une échéance.
    #[test]
    fn a_declared_gross_posts_neither_a_payroll_nor_a_dsn() {
        let mut store = test_store("paie-brut-seul");
        profile(&mut store, Some(300_000), Some(8_000));
        let today = date(2026, TimeMonth::March, 10);

        assert_eq!(piece_count(&store, "PAIE"), 0);
        assert_eq!(dsn_amounts(&store, today), Vec::<(Date, i64)>::new());
    }

    /// Forme écrite à la main : brut 100 000, retenue 20 000, patronales 40 000.
    /// Net 80 000. Le 431 reçoit 60 000. Un second appel ne double pas.
    /// Construire le livre ne réécrit pas les pièces.
    #[test]
    fn recording_the_form_posts_three_balanced_pieces_once() {
        let mut store = test_store("paie-forme");
        profile(&mut store, Some(300_000), Some(8_000));
        let on = date(2026, TimeMonth::January, 31);
        let written = record(&mut store, on, 100_000, 20_000, 40_000);
        assert!(written.0, "la première saisie écrit");

        assert_eq!(
            piece_head(&store, "PAIE"),
            Some(("OD".into(), "2026-01-31".into()))
        );
        assert_eq!(
            piece_lines(&store, "PAIE"),
            vec![
                ("641100".into(), 100_000),
                ("645000".into(), 40_000),
                ("421000".into(), -80_000),
                ("431000".into(), -60_000),
            ]
        );
        assert_eq!(
            piece_head(&store, "PAIE-NET"),
            Some(("BQ".into(), "2026-01-31".into()))
        );
        assert_eq!(
            piece_lines(&store, "PAIE-NET"),
            vec![("421000".into(), 80_000), ("512000".into(), -80_000)]
        );
        assert_eq!(
            piece_head(&store, "PAIE-URSSAF"),
            Some(("BQ".into(), "2026-01-31".into()))
        );
        assert_eq!(
            piece_lines(&store, "PAIE-URSSAF"),
            vec![("431000".into(), 60_000), ("512000".into(), -60_000)]
        );

        let stored = journal_count(&store);
        let again = record(&mut store, on, 100_000, 20_000, 40_000);
        assert!(!again.0, "un second appel identique n'écrit pas");
        assert_eq!(journal_count(&store), stored);
        assert_eq!(piece_count(&store, "PAIE"), 1);

        let exercise = FiscalYear::new(
            date(2025, TimeMonth::October, 1),
            date(2026, TimeMonth::September, 30),
        );
        let ledger = build_ledger(store.connection(), exercise).unwrap();
        assert_eq!(journal_count(&store), stored, "lire le livre n'insère rien");
        let _ = fiscal_calendar(store.connection(), on).unwrap();
        assert_eq!(journal_count(&store), stored, "le calendrier n'insère rien");
        assert_eq!(
            ledger
                .entries
                .iter()
                .filter(|entry| entry.piece_ref == "PAIE")
                .count(),
            1
        );
    }

    /// La paie de février est due le 15 mars. Le montant est le 431, 60 000,
    /// pas les 240 000 centimes du ratio du profil (300 000 × 8 000 / 10 000).
    #[test]
    fn the_deadline_after_the_entry_is_the_431_not_the_profile_ratio() {
        let mut store = test_store("paie-echeance");
        profile(&mut store, Some(300_000), Some(8_000));
        let on = date(2026, TimeMonth::February, 28);
        record(&mut store, on, 100_000, 20_000, 40_000);

        let today = date(2026, TimeMonth::March, 1);
        assert_eq!(
            dsn_amounts(&store, today),
            vec![(date(2026, TimeMonth::March, 15), 60_000)]
        );
    }

    /// À-nouveau 421 crédit 150 000 centimes. Un règlement de relevé du même
    /// montant sur 421000 le ramène à zéro. Aucune ligne 641, aucune ligne 645.
    #[test]
    fn settling_the_opening_421_on_the_statement_clears_it_without_a_641() {
        let mut store = test_store("paie-421");
        profile(&mut store, Some(300_000), Some(8_000));
        let opens_on = date(2024, TimeMonth::October, 1);
        let lines: Vec<OpeningBalanceLine> = [
            "421000:Personnel — rémunérations dues:C:1500.00",
            "512000:Banque:D:1500.00",
        ]
        .into_iter()
        .map(|line| line.parse().unwrap())
        .collect();
        applied(
            Executor::new(&mut store)
                .execute(
                    &RecordOpeningBalance {
                        opens_on,
                        source: Some("à-nouveau".into()),
                        lines,
                        tax_losses: Money::ZERO,
                        prior_corporate_tax: None,
                        prior_vat_due: None,
                    },
                    &human(),
                )
                .unwrap(),
        );
        settle(
            &mut store,
            date(2024, TimeMonth::November, 4),
            -150_000,
            "VIR REMUNERATION DUE",
            "421000",
        );

        let exercise = FiscalYear::new(opens_on, date(2025, TimeMonth::September, 30));
        assert_eq!(account_cents(&store, exercise, "421000"), 0);
        assert!(!has_account_prefix(&store, exercise, "641"));
        assert!(!has_account_prefix(&store, exercise, "645"));
    }

    /// Un débit de banque sur 431 n'est pas une cotisation : pas de 645, pas
    /// de 641, pas d'échéance. Le débit de 4 200 centimes reste au 431. Le
    /// brut du profil est absent.
    #[test]
    fn a_bank_debit_on_431_is_not_turned_into_contributions() {
        let mut store = test_store("paie-431");
        profile(&mut store, None, None);
        settle(
            &mut store,
            date(2025, TimeMonth::March, 3),
            -4_200,
            "Remboursement formation",
            "431000",
        );

        let exercise = FiscalYear::new(
            date(2024, TimeMonth::October, 1),
            date(2025, TimeMonth::September, 30),
        );
        assert_eq!(account_cents(&store, exercise, "431000"), 4_200);
        assert!(!has_account_prefix(&store, exercise, "641"));
        assert!(!has_account_prefix(&store, exercise, "645"));
        assert_eq!(piece_count(&store, "PAIE"), 0);
        assert_eq!(
            dsn_amounts(&store, date(2025, TimeMonth::March, 4)),
            Vec::<(Date, i64)>::new()
        );
    }

    /// Corriger le mois extourne. Brut 120 000, retenue 30 000, patronales
    /// 50 000 : net 90 000, 431 = 80 000. L'échéance de février porte 80 000.
    #[test]
    fn a_correction_reverses_and_posts_the_new_cents() {
        let mut store = test_store("paie-extourne");
        profile(&mut store, Some(300_000), Some(8_000));
        record(
            &mut store,
            date(2026, TimeMonth::January, 31),
            100_000,
            20_000,
            40_000,
        );
        let written = record(
            &mut store,
            date(2026, TimeMonth::January, 20),
            120_000,
            30_000,
            50_000,
        );
        assert!(written.0, "la correction écrit");

        assert_eq!(piece_count(&store, "EXT-PAIE"), 1);
        assert_eq!(piece_count(&store, "EXT-PAIE-NET"), 1);
        assert_eq!(piece_count(&store, "EXT-PAIE-URSSAF"), 1);
        assert_eq!(
            piece_head(&store, "PAIE"),
            Some(("OD".into(), "2026-01-20".into()))
        );
        assert_eq!(
            piece_lines(&store, "PAIE"),
            vec![
                ("641100".into(), 120_000),
                ("645000".into(), 50_000),
                ("421000".into(), -90_000),
                ("431000".into(), -80_000),
            ]
        );
        assert_eq!(
            dsn_amounts(&store, date(2026, TimeMonth::January, 31)),
            vec![(date(2026, TimeMonth::February, 15), 80_000)]
        );
    }
}
