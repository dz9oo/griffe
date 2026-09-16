//! Demande de versement d'un crédit de TVA (case 26 / 3519).
//!
//! Un fait journalisé par période CA3 : il coupe la chaîne 27 → 22 dès le choix, sans snapshot
//! des cases. Un agent propose, un humain confirme. Pas d'`Update` : changer le montant, c'est
//! rétracter puis redemander. Une déclaration déjà déposée verrouille le 26 (présent ou absent).

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::Date;

use crate::app::{AppError, Command};
use crate::domain::{Money, Month, format_date, parse_date};
use crate::fiscal::{
    CA12_REFUND_THRESHOLD, Ca3Periodicity, FiscalDeadlineKind, VAT_REFUND_IN_YEAR_THRESHOLD,
    ca3_period_key,
};

use super::filings::filing_for;
use super::vat_carry::parse_after_period;
use super::{BoxCoverage, FormBox};

fn below_threshold_msg(min: Money, calendar_year_end: bool) -> String {
    if calendar_year_end {
        format!("à la dernière déclaration de l'année, à partir de {min}")
    } else {
        format!("en cours d'année, le versement commence à {min}")
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum VatRefundError {
    #[error("{}", below_threshold_msg(*min, *calendar_year_end))]
    BelowThreshold { min: Money, calendar_year_end: bool },

    #[error("le versement dépasse le crédit de cette période ({max})")]
    ExceedsCredit { max: Money },

    #[error("le versement demandé doit être strictement positif")]
    NotPositive,

    #[error(
        "un versement de crédit de TVA a déjà été demandé pour {0} : rétractez-le avant d'en \
         demander un autre"
    )]
    AlreadyRequested(String),

    #[error("cette période n'a pas de crédit de TVA à verser")]
    NoCredit,

    #[error("cette période précède le bilan d'ouverture : les chiffres ne sont pas dans le coffre")]
    PeriodNotInVault,

    #[error(
        "cette déclaration est déjà déposée : rétractez le dépôt avant de changer le versement"
    )]
    DeclarationAlreadyFiled,

    #[error("aucune demande de versement de crédit de TVA pour cette période")]
    NotFound,

    #[error("période invalide {0:?} : attendu AAAA-MM")]
    InvalidPeriod(String),
}

impl From<VatRefundError> for AppError {
    fn from(e: VatRefundError) -> Self {
        Self::Domain(e.to_string())
    }
}

/// Le versement demandé, tel qu'en base.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VatRefundRecord {
    pub period_key: String,
    pub amount: Money,
    #[serde(with = "crate::domain::serde_date::date")]
    pub requested_on: Date,
}

/// Ce que la lettre (et les autres façades) peuvent proposer sur un crédit de période.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VatRefundStatus {
    BelowThreshold {
        credit: Money,
        min: Money,
        calendar_year_end: bool,
    },
    Offered {
        credit: Money,
        min: Money,
        calendar_year_end: bool,
    },
    Requested {
        amount: Money,
        remainder: Money,
    },
}

/// Demande le versement d'un crédit de TVA sur une période CA3 (case 26).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestVatRefund {
    pub period_key: String,
    pub amount: Money,
    #[serde(with = "crate::domain::serde_date::date")]
    pub requested_on: Date,
}

impl Command for RequestVatRefund {
    type Output = VatRefundRecord;
    const NAME: &'static str = "society.request_vat_refund";

    fn requires_confirmation(&self) -> bool {
        true
    }

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let month = parse_after_period(&self.period_key)
            .map_err(|_| VatRefundError::InvalidPeriod(self.period_key.clone()))?;
        if self.amount <= Money::ZERO {
            return Err(VatRefundError::NotPositive.into());
        }
        let (boxes, coverage) = super::ca3_boxes(conn, self.requested_on, &self.period_key)?;
        if coverage == BoxCoverage::Incomplete {
            return Err(VatRefundError::PeriodNotInVault.into());
        }
        let credit = boxes
            .iter()
            .find(|b| b.case == "25")
            .and_then(|b| b.amount)
            .unwrap_or(Money::ZERO);
        if credit.is_zero() {
            return Err(VatRefundError::NoCredit.into());
        }
        if self.amount > credit {
            return Err(VatRefundError::ExceedsCredit { max: credit }.into());
        }
        let periodicity = super::ca3_periodicity(conn)?;
        let year_end = calendar_year_end_month(month, periodicity);
        let min = refund_min(year_end);
        if self.amount < min {
            return Err(VatRefundError::BelowThreshold {
                min,
                calendar_year_end: year_end,
            }
            .into());
        }
        if filing_for(conn, FiscalDeadlineKind::Ca3, &self.period_key)?.is_some() {
            return Err(VatRefundError::DeclarationAlreadyFiled.into());
        }
        let period_key = ca3_period_key(month);
        if vat_refund_for(conn, &period_key)?.is_some() {
            return Err(VatRefundError::AlreadyRequested(period_key).into());
        }
        conn.execute(
            "INSERT INTO vat_refunds (period_key, amount_cents, requested_on)
             VALUES (?1, ?2, ?3)",
            params![
                period_key,
                self.amount.cents(),
                format_date(self.requested_on)
            ],
        )?;
        Ok(VatRefundRecord {
            period_key,
            amount: self.amount,
            requested_on: self.requested_on,
        })
    }
}

/// Retire une demande de versement. Refusé tant que la CA3 de la période est marquée déposée.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetractVatRefund {
    pub period_key: String,
}

impl Command for RetractVatRefund {
    type Output = ();
    const NAME: &'static str = "society.retract_vat_refund";

    fn requires_confirmation(&self) -> bool {
        true
    }

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        if filing_for(conn, FiscalDeadlineKind::Ca3, &self.period_key)?.is_some() {
            return Err(VatRefundError::DeclarationAlreadyFiled.into());
        }
        let n = conn.execute(
            "DELETE FROM vat_refunds WHERE period_key = ?1",
            params![self.period_key],
        )?;
        if n == 0 {
            return Err(VatRefundError::NotFound.into());
        }
        Ok(())
    }
}

/// Le versement demandé sur cette période, s'il existe.
///
/// # Errors
///
/// Erreur de lecture SQLite, ou `requested_on` illisible.
pub fn vat_refund_for(
    conn: &Connection,
    period_key: &str,
) -> Result<Option<VatRefundRecord>, AppError> {
    let row: Option<(String, i64, String)> = conn
        .query_row(
            "SELECT period_key, amount_cents, requested_on
             FROM vat_refunds WHERE period_key = ?1",
            params![period_key],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((period_key, amount_cents, requested_on)) = row else {
        return Ok(None);
    };
    Ok(Some(VatRefundRecord {
        period_key,
        amount: Money::from_cents(amount_cents),
        requested_on: parse_date(&requested_on).map_err(|e| AppError::Domain(e.to_string()))?,
    }))
}

/// Dernière CA3 de l'année civile : décembre (mensuel) ou T4 (mois de début = 10, trimestriel).
#[must_use]
pub(super) fn calendar_year_end(period_key: &str, periodicity: Ca3Periodicity) -> bool {
    parse_after_period(period_key).is_ok_and(|month| calendar_year_end_month(month, periodicity))
}

const fn calendar_year_end_month(month: Month, periodicity: Ca3Periodicity) -> bool {
    match periodicity {
        Ca3Periodicity::Monthly => month.month() == 12,
        Ca3Periodicity::Quarterly => month.month() == 10,
    }
}

const fn refund_min(calendar_year_end: bool) -> Money {
    if calendar_year_end {
        CA12_REFUND_THRESHOLD
    } else {
        VAT_REFUND_IN_YEAR_THRESHOLD
    }
}

/// Statut du versement pour un briefing CA3 déjà chiffré.
///
/// # Errors
///
/// Lecture SQLite, ou périodicité illisible.
pub(super) fn status_for_boxes(
    conn: &Connection,
    period_key: &str,
    coverage: BoxCoverage,
    boxes: &[FormBox],
) -> Result<Option<VatRefundStatus>, AppError> {
    if coverage != BoxCoverage::Complete {
        return Ok(None);
    }
    let Some(credit) = boxes.iter().find(|b| b.case == "25").and_then(|b| b.amount) else {
        return Ok(None);
    };
    if let Some(requested) = vat_refund_for(conn, period_key)? {
        let remainder = boxes
            .iter()
            .find(|b| b.case == "27")
            .and_then(|b| b.amount)
            .unwrap_or(Money::ZERO);
        return Ok(Some(VatRefundStatus::Requested {
            amount: requested.amount,
            remainder,
        }));
    }
    let year_end = calendar_year_end(period_key, super::ca3_periodicity(conn)?);
    let min = refund_min(year_end);
    if credit < min {
        Ok(Some(VatRefundStatus::BelowThreshold {
            credit,
            min,
            calendar_year_end: year_end,
        }))
    } else {
        Ok(Some(VatRefundStatus::Offered {
            credit,
            min,
            calendar_year_end: year_end,
        }))
    }
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
        MarkDutyFiled, RecordVatCarryIn, RetractDutyFiled, duty_briefing, vat_carry_in,
    };
    use crate::store::{Passphrase, Store};

    fn date(year: i32, month: TimeMonth, day: u8) -> Date {
        Date::from_calendar_date(year, month, day).unwrap()
    }

    fn test_store(label: &str) -> Store {
        let dir = std::env::temp_dir().join(format!(
            "freeflow-vat-refund-test-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ));
        Store::create(&dir.join("vault.db"), &Passphrase::from("s3cret")).unwrap()
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

    fn seed_carry_324(store: &mut Store) {
        applied(
            Executor::new(store)
                .execute(
                    &RecordVatCarryIn {
                        after_period: "2026-08".into(),
                        credit: Money::from_cents(32_400),
                        source: Some("CA3 août".into()),
                    },
                    &human(),
                )
                .unwrap(),
        );
    }

    fn request(
        store: &mut Store,
        period: &str,
        cents: i64,
        on: Date,
    ) -> Result<Outcome<VatRefundRecord>, AppError> {
        Executor::new(store).execute(
            &RequestVatRefund {
                period_key: period.into(),
                amount: Money::from_cents(cents),
                requested_on: on,
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

    #[test]
    fn a_324_euro_credit_in_september_is_below_the_in_year_threshold() {
        let mut store = test_store("sept-below");
        set_monthly_profile(&mut store);
        seed_carry_324(&mut store);
        let on = date(2026, TimeMonth::September, 8);
        let err = request(&mut store, "2026-09", 32_400, on).unwrap_err();
        assert!(err.to_string().contains("760"), "{err}");
        assert!(
            vat_refund_for(store.connection(), "2026-09")
                .unwrap()
                .is_none()
        );

        let september = ca3(&store, on, "2026-09");
        assert_eq!(
            box_of(&september, "22").amount,
            Some(Money::from_cents(32_400))
        );
        assert_eq!(
            box_of(&september, "25").amount,
            Some(Money::from_cents(32_400))
        );
        assert_eq!(
            box_of(&september, "27").amount,
            Some(Money::from_cents(32_400))
        );
        assert!(!has_case(&september, "26"), "{:?}", september.boxes);
        assert!(!has_case(&september, "28"), "{:?}", september.boxes);
        assert_eq!(
            september.vat_refund,
            Some(VatRefundStatus::BelowThreshold {
                credit: Money::from_cents(32_400),
                min: VAT_REFUND_IN_YEAR_THRESHOLD,
                calendar_year_end: false,
            })
        );
    }

    #[test]
    fn the_same_324_euros_can_be_requested_on_december() {
        let mut store = test_store("dec-ok");
        set_monthly_profile(&mut store);
        seed_carry_324(&mut store);
        let on = date(2027, TimeMonth::January, 8);
        let rec = applied(request(&mut store, "2026-12", 32_400, on).unwrap());
        assert_eq!(rec.period_key, "2026-12");
        assert_eq!(rec.amount, Money::from_cents(32_400));

        let december = ca3(&store, on, "2026-12");
        assert_eq!(
            box_of(&december, "22").amount,
            Some(Money::from_cents(32_400))
        );
        assert_eq!(
            box_of(&december, "25").amount,
            Some(Money::from_cents(32_400))
        );
        assert_eq!(
            box_of(&december, "26").amount,
            Some(Money::from_cents(32_400))
        );
        assert!(!has_case(&december, "27"), "{:?}", december.boxes);
        assert!(!has_case(&december, "28"), "{:?}", december.boxes);
        assert_eq!(
            december.vat_refund,
            Some(VatRefundStatus::Requested {
                amount: Money::from_cents(32_400),
                remainder: Money::ZERO,
            })
        );

        let january = ca3(&store, date(2027, TimeMonth::February, 8), "2027-01");
        assert!(
            !has_case(&january, "22"),
            "le 26 de décembre a coupé la chaîne : {:?}",
            january.boxes
        );
        let carry = vat_carry_in(store.connection()).unwrap().unwrap();
        assert_eq!(carry.after_period, "2026-08");
        assert_eq!(carry.credit, Money::from_cents(32_400));
    }

    #[test]
    fn a_partial_refund_must_meet_the_760_floor_in_march() {
        let mut store = test_store("march-partial");
        set_monthly_profile(&mut store);
        applied(
            Executor::new(&mut store)
                .execute(
                    &RecordExpense {
                        label: "matériel".into(),
                        category: ExpenseCategory::Software,
                        amount: Money::from_cents(504_000),
                        vat_rate: VatRate::Standard,
                        vat_deductible: Money::from_cents(84_000),
                        incurred_on: date(2026, TimeMonth::March, 12),
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
        let on = date(2026, TimeMonth::April, 8);
        let too_low = request(&mut store, "2026-03", 20_000, on).unwrap_err();
        assert!(too_low.to_string().contains("760"), "{too_low}");

        let rec = applied(request(&mut store, "2026-03", 76_000, on).unwrap());
        assert_eq!(rec.amount, Money::from_cents(76_000));

        let march = ca3(&store, on, "2026-03");
        assert_eq!(box_of(&march, "25").amount, Some(Money::from_cents(84_000)));
        assert_eq!(box_of(&march, "26").amount, Some(Money::from_cents(76_000)));
        assert_eq!(box_of(&march, "27").amount, Some(Money::from_cents(8_000)));
        assert!(!has_case(&march, "22"), "{:?}", march.boxes);

        let april = ca3(&store, date(2026, TimeMonth::May, 8), "2026-04");
        assert_eq!(box_of(&april, "22").amount, Some(Money::from_cents(8_000)));
        assert_eq!(box_of(&april, "25").amount, Some(Money::from_cents(8_000)));
        assert_eq!(box_of(&april, "27").amount, Some(Money::from_cents(8_000)));
    }

    #[test]
    fn an_agent_only_deposits_a_pending_action() {
        let mut store = test_store("agent");
        set_monthly_profile(&mut store);
        seed_carry_324(&mut store);
        let outcome = Executor::new(&mut store)
            .execute(
                &RequestVatRefund {
                    period_key: "2026-12".into(),
                    amount: Money::from_cents(32_400),
                    requested_on: date(2027, TimeMonth::January, 8),
                },
                &agent(),
            )
            .unwrap();
        assert!(
            matches!(outcome, Outcome::PendingConfirmation(_)),
            "agent → pending : {outcome:?}"
        );
        assert!(
            vat_refund_for(store.connection(), "2026-12")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn retracting_a_filing_does_not_drop_the_refund() {
        let mut store = test_store("retract-filing");
        set_monthly_profile(&mut store);
        seed_carry_324(&mut store);
        let on = date(2027, TimeMonth::January, 8);
        applied(request(&mut store, "2026-12", 32_400, on).unwrap());
        let briefing = ca3(&store, on, "2026-12");
        applied(
            Executor::new(&mut store)
                .execute(
                    &MarkDutyFiled {
                        kind: FiscalDeadlineKind::Ca3,
                        period_key: "2026-12".into(),
                        due_on: briefing.due_on,
                        filed_on: on,
                    },
                    &human(),
                )
                .unwrap(),
        );
        let while_filed = Executor::new(&mut store)
            .execute(
                &RetractVatRefund {
                    period_key: "2026-12".into(),
                },
                &human(),
            )
            .unwrap_err();
        assert!(while_filed.to_string().contains("déposée"), "{while_filed}");

        applied(
            Executor::new(&mut store)
                .execute(
                    &RetractDutyFiled {
                        kind: FiscalDeadlineKind::Ca3,
                        period_key: "2026-12".into(),
                    },
                    &human(),
                )
                .unwrap(),
        );
        let still = vat_refund_for(store.connection(), "2026-12")
            .unwrap()
            .expect("le 26 survit au dépôt rétracté");
        assert_eq!(still.amount, Money::from_cents(32_400));

        applied(
            Executor::new(&mut store)
                .execute(
                    &RetractVatRefund {
                        period_key: "2026-12".into(),
                    },
                    &human(),
                )
                .unwrap(),
        );
        let restored = ca3(&store, on, "2026-12");
        assert_eq!(
            box_of(&restored, "27").amount,
            Some(Money::from_cents(32_400))
        );
        assert!(!has_case(&restored, "26"), "{:?}", restored.boxes);
    }

    #[test]
    fn a_refund_cannot_exceed_the_period_credit() {
        let mut store = test_store("exceeds");
        set_monthly_profile(&mut store);
        seed_carry_324(&mut store);
        let err = request(
            &mut store,
            "2026-12",
            32_401,
            date(2027, TimeMonth::January, 8),
        )
        .unwrap_err();
        assert!(err.to_string().contains("dépasse"), "{err}");
    }

    #[test]
    fn a_zero_refund_is_refused() {
        let mut store = test_store("zero");
        set_monthly_profile(&mut store);
        seed_carry_324(&mut store);
        let err = request(&mut store, "2026-12", 0, date(2027, TimeMonth::January, 8)).unwrap_err();
        assert!(err.to_string().contains("positif"), "{err}");
    }

    #[test]
    fn a_second_request_on_the_same_period_is_refused() {
        let mut store = test_store("second");
        set_monthly_profile(&mut store);
        seed_carry_324(&mut store);
        let on = date(2027, TimeMonth::January, 8);
        applied(request(&mut store, "2026-12", 32_400, on).unwrap());
        let err = request(&mut store, "2026-12", 32_400, on).unwrap_err();
        assert!(err.to_string().contains("déjà"), "{err}");
    }

    #[test]
    fn a_period_before_the_opening_balance_is_out_of_the_vault() {
        let mut store = test_store("before-opening");
        set_monthly_profile(&mut store);
        applied(
            Executor::new(&mut store)
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
        let err = request(
            &mut store,
            "2026-09",
            32_400,
            date(2026, TimeMonth::October, 8),
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("bilan d'ouverture") || err.to_string().contains("coffre"),
            "{err}"
        );
    }

    #[test]
    fn a_filed_declaration_locks_the_refund() {
        let mut store = test_store("filed-lock");
        set_monthly_profile(&mut store);
        seed_carry_324(&mut store);
        let on = date(2027, TimeMonth::January, 8);
        let briefing = ca3(&store, on, "2026-12");
        applied(
            Executor::new(&mut store)
                .execute(
                    &MarkDutyFiled {
                        kind: FiscalDeadlineKind::Ca3,
                        period_key: "2026-12".into(),
                        due_on: briefing.due_on,
                        filed_on: on,
                    },
                    &human(),
                )
                .unwrap(),
        );
        let err = request(&mut store, "2026-12", 32_400, on).unwrap_err();
        assert!(err.to_string().contains("déposée"), "{err}");
    }
}
