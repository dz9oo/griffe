//! TVA antérieurement déduite à reverser (case 15).
//!
//! Un fait journalisé par période CA3 : il entre dans la brute, coupe la chaîne 27 → 22 dès la
//! saisie, sans snapshot des cases. Un agent propose, un humain confirme. Pas d'`Update` : changer
//! le montant, c'est rétracter puis reposer. Une déclaration déjà déposée verrouille le 15
//! (présent ou absent).

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::Date;

use crate::app::{AppError, Command};
use crate::domain::{Money, format_date, parse_date};
use crate::fiscal::{FiscalDeadlineKind, ca3_period_key};

use super::filings::filing_for;
use super::vat_carry::parse_after_period;
use super::vat_refund::vat_refund_for;
use super::{BoxCoverage, FormBox};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum VatReversalError {
    #[error("un montant à rendre est un nombre positif")]
    NotPositive,

    #[error(
        "une TVA trop déduite a déjà été enregistrée pour {0} : rétractez-la avant d'en poser \
         une autre"
    )]
    AlreadyRecorded(String),

    #[error("cette période précède le bilan d'ouverture : les chiffres ne sont pas dans le coffre")]
    PeriodNotInVault,

    #[error(
        "cette déclaration est déjà déposée : rétractez le dépôt avant de changer le montant rendu"
    )]
    DeclarationAlreadyFiled,

    #[error("aucune TVA trop déduite enregistrée pour cette période")]
    NotFound,

    #[error("période invalide {0:?} : attendu AAAA-MM")]
    InvalidPeriod(String),

    #[error("rendre autant ferait passer le versement demandé au-dessus du crédit restant ({max})")]
    RefundWouldExceed { max: Money },
}

impl From<VatReversalError> for AppError {
    fn from(e: VatReversalError) -> Self {
        Self::Domain(e.to_string())
    }
}

/// La TVA trop déduite enregistrée, telle qu'en base.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VatReversalRecord {
    pub period_key: String,
    pub amount: Money,
    #[serde(with = "crate::domain::serde_date::date")]
    pub recorded_on: Date,
}

/// Enregistre une TVA trop déduite à reverser sur une période CA3 (case 15).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordVatReversal {
    pub period_key: String,
    pub amount: Money,
    #[serde(with = "crate::domain::serde_date::date")]
    pub recorded_on: Date,
}

impl Command for RecordVatReversal {
    type Output = VatReversalRecord;
    const NAME: &'static str = "society.record_vat_reversal";

    fn requires_confirmation(&self) -> bool {
        true
    }

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let month = parse_after_period(&self.period_key)
            .map_err(|_| VatReversalError::InvalidPeriod(self.period_key.clone()))?;
        if self.amount <= Money::ZERO {
            return Err(VatReversalError::NotPositive.into());
        }
        let period_key = ca3_period_key(month);
        let (boxes, coverage) = super::ca3_boxes(conn, self.recorded_on, &period_key)?;
        if coverage == BoxCoverage::Incomplete {
            return Err(VatReversalError::PeriodNotInVault.into());
        }
        if filing_for(conn, FiscalDeadlineKind::Ca3, &period_key)?.is_some() {
            return Err(VatReversalError::DeclarationAlreadyFiled.into());
        }
        if vat_reversal_for(conn, &period_key)?.is_some() {
            return Err(VatReversalError::AlreadyRecorded(period_key).into());
        }
        if let Some(refund) = vat_refund_for(conn, &period_key)? {
            let credit = box_amount(&boxes, "25");
            let new_25 = credit - self.amount;
            if refund.amount > new_25 {
                let max = if new_25.is_negative() {
                    Money::ZERO
                } else {
                    new_25
                };
                return Err(VatReversalError::RefundWouldExceed { max }.into());
            }
        }
        conn.execute(
            "INSERT INTO vat_reversals (period_key, amount_cents, recorded_on)
             VALUES (?1, ?2, ?3)",
            params![
                period_key,
                self.amount.cents(),
                format_date(self.recorded_on)
            ],
        )?;
        Ok(VatReversalRecord {
            period_key,
            amount: self.amount,
            recorded_on: self.recorded_on,
        })
    }
}

/// Retire une TVA trop déduite. Refusé tant que la CA3 de la période est marquée déposée.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetractVatReversal {
    pub period_key: String,
}

impl Command for RetractVatReversal {
    type Output = ();
    const NAME: &'static str = "society.retract_vat_reversal";

    fn requires_confirmation(&self) -> bool {
        true
    }

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        if filing_for(conn, FiscalDeadlineKind::Ca3, &self.period_key)?.is_some() {
            return Err(VatReversalError::DeclarationAlreadyFiled.into());
        }
        let n = conn.execute(
            "DELETE FROM vat_reversals WHERE period_key = ?1",
            params![self.period_key],
        )?;
        if n == 0 {
            return Err(VatReversalError::NotFound.into());
        }
        Ok(())
    }
}

/// La TVA trop déduite enregistrée sur cette période, s'il en existe une.
///
/// # Errors
///
/// Erreur de lecture SQLite, ou `recorded_on` illisible.
pub fn vat_reversal_for(
    conn: &Connection,
    period_key: &str,
) -> Result<Option<VatReversalRecord>, AppError> {
    let row: Option<(String, i64, String)> = conn
        .query_row(
            "SELECT period_key, amount_cents, recorded_on
             FROM vat_reversals WHERE period_key = ?1",
            params![period_key],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((period_key, amount_cents, recorded_on)) = row else {
        return Ok(None);
    };
    Ok(Some(VatReversalRecord {
        period_key,
        amount: Money::from_cents(amount_cents),
        recorded_on: parse_date(&recorded_on).map_err(|e| AppError::Domain(e.to_string()))?,
    }))
}

fn box_amount(boxes: &[FormBox], case: &str) -> Money {
    boxes
        .iter()
        .find(|b| b.case == case)
        .and_then(|b| b.amount)
        .unwrap_or(Money::ZERO)
}

#[cfg(test)]
#[allow(clippy::too_many_lines)]
mod tests {
    use time::Month as TimeMonth;

    use super::*;
    use crate::app::{Actor, ExecutionContext, Executor, Outcome};
    use crate::company::SetCompanyProfile;
    use crate::domain::{
        Address, ExpenseCategory, FiscalYearEnd, OpeningBalanceLine, Siren, VatRate, VatRegime,
    };
    use crate::expenses::RecordExpense;
    use crate::opening_balance::RecordOpeningBalance;
    use crate::society::{
        MarkDutyFiled, RecordVatCarryIn, RequestVatRefund, RetractDutyFiled, duty_briefing,
        vat_carry_in, vat_position,
    };
    use crate::society::{VatPosition, society_home};
    use crate::store::Store;
    use crate::store::testing::test_store;

    fn date(year: i32, month: TimeMonth, day: u8) -> Date {
        Date::from_calendar_date(year, month, day).unwrap()
    }

    fn human() -> ExecutionContext {
        ExecutionContext::new(Actor::Human, false)
    }

    fn agent() -> ExecutionContext {
        ExecutionContext::new(
            Actor::Agent {
                session: "mcp-test".into(),
            },
            false,
        )
    }

    fn applied<T: std::fmt::Debug>(outcome: Outcome<T>) -> T {
        match outcome {
            Outcome::Applied(v) => v,
            other => panic!("expected Applied, got {other:?}"),
        }
    }

    fn set_monthly_profile(store: &mut Store) {
        applied(
            Executor::new(store)
                .execute(
                    &SetCompanyProfile {
                        name: "Lumen Conseil".into(),
                        legal_form: "SASU".into(),
                        siren: Siren::parse("552100554").unwrap(),
                        vat_number: None,
                        address: Address {
                            street: "18 rue des Ateliers".into(),
                            postal_code: "69003".into(),
                            city: "Lyon".into(),
                            country: "FR".into(),
                        },
                        share_capital: Some(Money::from_cents(100_000)),
                        rcs_city: Some("Lyon".into()),
                        iban: None,
                        fiscal_year_end: Some(FiscalYearEnd::new(9, 30).unwrap()),
                        vat_regime: Some(VatRegime::RealNormalMonthly),
                        director_monthly_gross: Some(Money::from_cents(300_000)),
                        director_charge_ratio_bps: None,
                        president_name: Some("Nicolas Lumen".into()),
                        sole_shareholder_name: Some("Nicolas Lumen".into()),
                        sole_shareholder_address: Some("18 rue des Ateliers, 69003 Lyon".into()),
                        share_count: Some(1000),
                    },
                    &human(),
                )
                .unwrap(),
        );
    }

    fn seed_carry(store: &mut Store, after_period: &str, cents: i64) {
        applied(
            Executor::new(store)
                .execute(
                    &RecordVatCarryIn {
                        after_period: after_period.into(),
                        credit: Money::from_cents(cents),
                        source: Some("CA3".into()),
                    },
                    &human(),
                )
                .unwrap(),
        );
    }

    fn seed_carry_324(store: &mut Store) {
        seed_carry(store, "2026-08", 32_400);
    }

    fn record(
        store: &mut Store,
        period: &str,
        cents: i64,
        on: Date,
    ) -> Result<Outcome<VatReversalRecord>, AppError> {
        Executor::new(store).execute(
            &RecordVatReversal {
                period_key: period.into(),
                amount: Money::from_cents(cents),
                recorded_on: on,
            },
            &human(),
        )
    }

    fn ca3(store: &Store, today: Date, period: &str) -> crate::society::DutyBriefing {
        duty_briefing(
            store.connection(),
            FiscalDeadlineKind::Ca3,
            today,
            Some(period),
        )
        .unwrap()
    }

    fn box_of<'a>(
        briefing: &'a crate::society::DutyBriefing,
        case: &str,
    ) -> &'a crate::society::FormBox {
        briefing
            .boxes
            .iter()
            .find(|b| b.case == case)
            .unwrap_or_else(|| panic!("case {case} : {:?}", briefing.boxes))
    }

    fn has_case(briefing: &crate::society::DutyBriefing, case: &str) -> bool {
        briefing.boxes.iter().any(|b| b.case == case)
    }

    fn cents(n: i64) -> Money {
        Money::from_cents(n)
    }

    #[test]
    fn september_five_euros_drops_the_credit_to_319() {
        let mut store = test_store("sept-5");
        set_monthly_profile(&mut store);
        seed_carry_324(&mut store);
        let on = date(2026, TimeMonth::September, 15);

        let september = ca3(&store, on, "2026-09");
        assert_eq!(box_of(&september, "22").amount, Some(cents(32_400)));
        assert_eq!(box_of(&september, "25").amount, Some(cents(32_400)));
        assert_eq!(box_of(&september, "27").amount, Some(cents(32_400)));
        assert!(!has_case(&september, "15"), "{:?}", september.boxes);
        assert!(!has_case(&september, "26"), "{:?}", september.boxes);
        assert!(!has_case(&september, "28"), "{:?}", september.boxes);
        assert!(september.vat_reversal.is_none());

        applied(record(&mut store, "2026-09", 500, on).unwrap());

        let september = ca3(&store, on, "2026-09");
        assert_eq!(box_of(&september, "15").amount, Some(cents(500)));
        assert_eq!(box_of(&september, "22").amount, Some(cents(32_400)));
        assert_eq!(box_of(&september, "25").amount, Some(cents(31_900)));
        assert_eq!(box_of(&september, "27").amount, Some(cents(31_900)));
        assert!(!has_case(&september, "26"), "{:?}", september.boxes);
        assert!(!has_case(&september, "28"), "{:?}", september.boxes);
        assert_eq!(september.vat_reversal, Some(cents(500)));

        let carry = vat_carry_in(store.connection()).unwrap().unwrap();
        assert_eq!(carry.after_period, "2026-08");
        assert_eq!(carry.credit, cents(32_400));

        let october = ca3(&store, date(2026, TimeMonth::October, 8), "2026-10");
        assert_eq!(box_of(&october, "22").amount, Some(cents(31_900)));
        assert_eq!(box_of(&october, "25").amount, Some(cents(31_900)));
        assert_eq!(box_of(&october, "27").amount, Some(cents(31_900)));
        assert!(!has_case(&october, "15"), "{:?}", october.boxes);

        let pos = vat_position(store.connection(), date(2026, TimeMonth::October, 8)).unwrap();
        assert_eq!(
            pos,
            Some(VatPosition::Credit {
                amount: cents(31_900)
            })
        );
        let home = society_home(store.connection(), date(2026, TimeMonth::October, 8)).unwrap();
        assert_eq!(home.vat_position, pos);
    }

    #[test]
    fn december_2025_four_hundred_sixty_four_euros() {
        let mut store = test_store("dec-464");
        set_monthly_profile(&mut store);
        applied(
            Executor::new(&mut store)
                .execute(
                    &RecordOpeningBalance {
                        opens_on: date(2025, TimeMonth::October, 1),
                        source: Some("cabinet".into()),
                        lines: ["101000:Capital:C:1000.00", "512000:Banque:D:1000.00"]
                            .iter()
                            .map(|l| l.parse::<OpeningBalanceLine>().unwrap())
                            .collect(),
                        tax_losses: Money::ZERO,
                        prior_corporate_tax: None,
                        prior_vat_due: None,
                    },
                    &human(),
                )
                .unwrap(),
        );
        seed_carry(&mut store, "2025-11", 64_300);
        applied(
            Executor::new(&mut store)
                .execute(
                    &RecordExpense {
                        label: "logiciel".into(),
                        category: ExpenseCategory::Software,
                        amount: Money::from_cents(14_400),
                        vat_rate: VatRate::Standard,
                        vat_deductible: Money::from_cents(2_400),
                        incurred_on: date(2025, TimeMonth::December, 12),
                        receipt_hash: None,
                        receipt_filename: None,
                        bank_transaction_id: None,
                        supplier: None,
                        paid_by: crate::domain::ExpensePaidBy::Company,
                    },
                    &human(),
                )
                .unwrap(),
        );
        let on = date(2026, TimeMonth::January, 8);
        let december = ca3(&store, on, "2025-12");
        assert_eq!(box_of(&december, "22").amount, Some(cents(64_300)));
        assert_eq!(box_of(&december, "20").amount, Some(cents(2_400)));
        assert_eq!(box_of(&december, "25").amount, Some(cents(66_700)));
        assert_eq!(box_of(&december, "27").amount, Some(cents(66_700)));

        applied(record(&mut store, "2025-12", 46_400, on).unwrap());
        let december = ca3(&store, on, "2025-12");
        assert_eq!(box_of(&december, "15").amount, Some(cents(46_400)));
        assert_eq!(box_of(&december, "22").amount, Some(cents(64_300)));
        assert_eq!(box_of(&december, "25").amount, Some(cents(20_300)));
        assert_eq!(box_of(&december, "27").amount, Some(cents(20_300)));
        assert!(!has_case(&december, "26"), "{:?}", december.boxes);
    }

    #[test]
    fn two_reversals_on_two_periods_coexist() {
        let mut store = test_store("two-periods");
        set_monthly_profile(&mut store);
        seed_carry_324(&mut store);
        applied(
            record(
                &mut store,
                "2026-09",
                500,
                date(2026, TimeMonth::September, 15),
            )
            .unwrap(),
        );
        applied(
            record(
                &mut store,
                "2026-10",
                1_000,
                date(2026, TimeMonth::October, 8),
            )
            .unwrap(),
        );
        assert_eq!(
            vat_reversal_for(store.connection(), "2026-09")
                .unwrap()
                .unwrap()
                .amount,
            cents(500)
        );
        assert_eq!(
            vat_reversal_for(store.connection(), "2026-10")
                .unwrap()
                .unwrap()
                .amount,
            cents(1_000)
        );
        let october = ca3(&store, date(2026, TimeMonth::October, 8), "2026-10");
        assert_eq!(box_of(&october, "22").amount, Some(cents(31_900)));
        assert_eq!(box_of(&october, "15").amount, Some(cents(1_000)));
        assert_eq!(box_of(&october, "27").amount, Some(cents(30_900)));
    }

    #[test]
    fn an_agent_only_deposits_a_pending_action() {
        let mut store = test_store("agent");
        set_monthly_profile(&mut store);
        seed_carry_324(&mut store);
        let outcome = Executor::new(&mut store)
            .execute(
                &RecordVatReversal {
                    period_key: "2026-09".into(),
                    amount: cents(500),
                    recorded_on: date(2026, TimeMonth::September, 15),
                },
                &agent(),
            )
            .unwrap();
        assert!(
            matches!(outcome, Outcome::PendingConfirmation(_)),
            "agent → pending : {outcome:?}"
        );
        assert!(
            vat_reversal_for(store.connection(), "2026-09")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn retracting_a_filing_does_not_drop_the_reversal() {
        let mut store = test_store("retract-filing");
        set_monthly_profile(&mut store);
        seed_carry_324(&mut store);
        let on = date(2026, TimeMonth::September, 15);
        applied(record(&mut store, "2026-09", 500, on).unwrap());
        let briefing = ca3(&store, on, "2026-09");
        applied(
            Executor::new(&mut store)
                .execute(
                    &MarkDutyFiled {
                        kind: FiscalDeadlineKind::Ca3,
                        period_key: "2026-09".into(),
                        due_on: briefing.due_on,
                        filed_on: on,
                    },
                    &human(),
                )
                .unwrap(),
        );
        let while_filed = Executor::new(&mut store)
            .execute(
                &RetractVatReversal {
                    period_key: "2026-09".into(),
                },
                &human(),
            )
            .unwrap_err();
        assert!(while_filed.to_string().contains("déposée"), "{while_filed}");
        let again = record(&mut store, "2026-09", 500, on).unwrap_err();
        assert!(again.to_string().contains("déposée"), "{again}");

        applied(
            Executor::new(&mut store)
                .execute(
                    &RetractDutyFiled {
                        kind: FiscalDeadlineKind::Ca3,
                        period_key: "2026-09".into(),
                    },
                    &human(),
                )
                .unwrap(),
        );
        let still = vat_reversal_for(store.connection(), "2026-09")
            .unwrap()
            .expect("le 15 survit au dépôt rétracté");
        assert_eq!(still.amount, cents(500));

        applied(
            Executor::new(&mut store)
                .execute(
                    &RetractVatReversal {
                        period_key: "2026-09".into(),
                    },
                    &human(),
                )
                .unwrap(),
        );
        let restored = ca3(&store, on, "2026-09");
        assert_eq!(box_of(&restored, "27").amount, Some(cents(32_400)));
        assert!(!has_case(&restored, "15"), "{:?}", restored.boxes);
    }

    #[test]
    fn a_reversal_larger_than_credit_creates_a_due() {
        let mut store = test_store("due");
        set_monthly_profile(&mut store);
        seed_carry_324(&mut store);
        let on = date(2026, TimeMonth::September, 15);
        applied(record(&mut store, "2026-09", 40_000, on).unwrap());
        let september = ca3(&store, on, "2026-09");
        assert_eq!(box_of(&september, "15").amount, Some(cents(40_000)));
        assert_eq!(box_of(&september, "28").amount, Some(cents(7_600)));
        assert!(!has_case(&september, "25"), "{:?}", september.boxes);
        assert!(!has_case(&september, "27"), "{:?}", september.boxes);
        let pos = vat_position(store.connection(), date(2026, TimeMonth::October, 8)).unwrap();
        assert_eq!(
            pos,
            Some(VatPosition::Due {
                amount: cents(7_600)
            })
        );
    }

    #[test]
    fn a_reversal_that_would_break_an_existing_refund_is_refused() {
        let mut store = test_store("refund-lock");
        set_monthly_profile(&mut store);
        seed_carry_324(&mut store);
        let on = date(2027, TimeMonth::January, 8);
        applied(
            Executor::new(&mut store)
                .execute(
                    &RequestVatRefund {
                        period_key: "2026-12".into(),
                        amount: cents(32_400),
                        requested_on: on,
                    },
                    &human(),
                )
                .unwrap(),
        );
        let err = record(&mut store, "2026-12", 500, on).unwrap_err();
        assert!(
            err.to_string().contains("versement") || err.to_string().contains("crédit"),
            "{err}"
        );
        assert!(
            vat_reversal_for(store.connection(), "2026-12")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn other_refusals() {
        let mut store = test_store("refusals");
        set_monthly_profile(&mut store);
        seed_carry_324(&mut store);
        let on = date(2026, TimeMonth::September, 15);
        let zero = record(&mut store, "2026-09", 0, on).unwrap_err();
        assert!(zero.to_string().contains("positif"), "{zero}");

        applied(record(&mut store, "2026-09", 500, on).unwrap());
        let second = record(&mut store, "2026-09", 500, on).unwrap_err();
        assert!(second.to_string().contains("déjà"), "{second}");

        let missing = Executor::new(&mut store)
            .execute(
                &RetractVatReversal {
                    period_key: "2026-10".into(),
                },
                &human(),
            )
            .unwrap_err();
        assert!(missing.to_string().contains("aucune"), "{missing}");

        let mut before = test_store("before-opening");
        set_monthly_profile(&mut before);
        applied(
            Executor::new(&mut before)
                .execute(
                    &RecordOpeningBalance {
                        opens_on: date(2026, TimeMonth::October, 1),
                        source: Some("cabinet".into()),
                        lines: ["101000:Capital:C:1000.00", "512000:Banque:D:1000.00"]
                            .iter()
                            .map(|l| l.parse::<OpeningBalanceLine>().unwrap())
                            .collect(),
                        tax_losses: Money::ZERO,
                        prior_corporate_tax: None,
                        prior_vat_due: None,
                    },
                    &human(),
                )
                .unwrap(),
        );
        let err = record(
            &mut before,
            "2026-09",
            500,
            date(2026, TimeMonth::October, 8),
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("bilan d'ouverture") || err.to_string().contains("coffre"),
            "{err}"
        );
    }
}
