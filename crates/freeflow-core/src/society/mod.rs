//! La société : paysage, se payer, relevé, chapitres — lectures, et le journal des dépôts
//! (lot 56).
//!
//! Lot 51. Le cœur compose des faits typés ; c'est la fenêtre (et le rendu CLI) qui rédige le
//! français. `today` est un argument d'adaptateur, jamais lu ici.

mod filings;
mod vat_carry;
mod vat_refund;
mod vat_reversal;

pub use filings::{
    DutyFiling, DutyFilingError, MarkCatchUpFiled, MarkDutyFiled, RetractDutyFiled, filing_for,
    list_filings,
};
pub use vat_carry::{
    DeleteVatCarryIn, RecordVatCarryIn, UpdateVatCarryIn, VatCarryInError, VatCarryInRecord,
    parse_after_period, vat_carry_in,
};
pub use vat_refund::{
    RequestVatRefund, RetractVatRefund, VatRefundError, VatRefundRecord, VatRefundStatus,
    vat_refund_for,
};
pub use vat_reversal::{
    RecordVatReversal, RetractVatReversal, VatReversalError, VatReversalRecord, vat_reversal_for,
};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use time::Date;

use crate::accounting::{compute_result, vat_due_for_period};
use crate::app::AppError;
use crate::billing::{aged_balance, list_bank_transactions, list_invoices};
use crate::clients::client_by_id;
use crate::closing::{ClosingStage, ClosingStepKey, StepStatus, closing_checklist};
use crate::company::company_profile;
use crate::day::cash_in_bank;
use crate::domain::{
    BankTransaction, BankTransactionId, Ca3FilingRule, FiscalYearEnd, Money, Month, Side, VatRate,
    VatRegime, add_months, sub_months,
};
use crate::fiscal::{
    Ca3Periodicity, DAS2_THRESHOLD, DIVIDEND_INCOME_TAX_BPS, FiscalDeadline, FiscalDeadlineKind,
    IS_ACOMPTE_DISPENSATION, VatFilingScheme, ca3_filings_in_range, ca3_period_key,
    dividend_social_charges_bps, fiscal_calendar, next_cfe, next_is_acompte,
    simplified_regime_applies_to,
};
use crate::fiscal_year::fiscal_year_ending_in;
use crate::forecast::{build_forecast_inputs, forecast_12_months};
use crate::opening_balance::opening_balance;
use crate::prospection::list_opportunities;

/// Nombre de mois de piste à garder après s'être payé.
pub const RUNWAY_KEPT_MONTHS: u32 = 3;
/// Mois courant « maigre » s'il porte moins de tant de noms.
pub const CONVERSATION_THIN: u32 = 3;
/// Charges sociales par défaut (dix-millièmes) quand le profil n'en pose pas — 57 %, le
/// rapport 2 800 € nets / ~4 400 € déboursés de la maquette.
pub const DEFAULT_SALARY_CHARGE_BPS: u32 = 5_700;
/// Mois futurs du paysage, au-delà d'aujourd'hui.
const LANDSCAPE_FUTURE_MONTHS: usize = 4;

// ---------------------------------------------------------------------------------------------
// Se payer
// ---------------------------------------------------------------------------------------------

/// Montant possible ce mois-ci sans casser la piste, et les deux portes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PayYourself {
    pub bank: Money,
    pub monthly_burn: Money,
    /// Ce qu'on peut sortir ce mois-ci et garder encore [`RUNWAY_KEPT_MONTHS`] mois de piste.
    pub possible: Money,
    pub runway_kept_months: u32,
    pub salary: SalaryDoor,
    pub dividend: DividendDoor,
    pub annual: AnnualPay,
    /// Coût société de 1 000 € de dividendes nets, aux taux du jour (PFU + prélèvements).
    pub dividend_cost_per_thousand: Money,
}

/// La porte salaire : toujours ouverte — `FreeFlow` n'est pas un logiciel de paie, il dit
/// seulement ce que la société débourse.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SalaryDoor {
    pub net: Money,
    pub company_cost: Money,
}

/// La porte dividende : fermée tant que le dernier exercice écoulé n'est pas clos et approuvé.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DividendDoor {
    Open { available: Money },
    Closed { reason: DividendClosed },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DividendClosed {
    YearNotClosed,
    NoProfit,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnnualPay {
    pub target: Option<Money>,
    pub paid: Money,
}

/// # Errors
pub fn pay_yourself(conn: &Connection, today: Date) -> Result<PayYourself, AppError> {
    let bank = cash_in_bank(conn, today)?;
    let burn = monthly_burn(conn, today, bank)?;
    let keep = mul_months(burn, RUNWAY_KEPT_MONTHS);
    let possible = if bank.cents() > keep.cents() {
        bank - keep
    } else {
        Money::ZERO
    };
    let profile = company_profile(conn)?;
    let charge_bps = profile
        .as_ref()
        .and_then(|p| p.director_charge_ratio_bps)
        .unwrap_or(DEFAULT_SALARY_CHARGE_BPS);
    let company_cost = possible + possible.apply_rate_bps(charge_bps);
    let dividend = dividend_door(
        conn,
        today,
        profile.as_ref().and_then(|p| p.fiscal_year_end),
    )?;
    let target = profile
        .as_ref()
        .and_then(|p| p.director_monthly_gross)
        .map(|g| mul_months(g, 12));
    let paid = paid_this_year(conn, today)?;
    Ok(PayYourself {
        bank,
        monthly_burn: burn,
        possible,
        runway_kept_months: RUNWAY_KEPT_MONTHS,
        salary: SalaryDoor {
            net: possible,
            company_cost,
        },
        dividend,
        annual: AnnualPay { target, paid },
        dividend_cost_per_thousand: dividend_cost(Money::from_cents(100_000), today),
    })
}

fn monthly_burn(conn: &Connection, today: Date, bank: Money) -> Result<Money, AppError> {
    let inputs = build_forecast_inputs(conn, today, bank)?;
    Ok(inputs.monthly_known_expenses)
}

fn mul_months(amount: Money, months: u32) -> Money {
    let cents = i128::from(amount.cents()).saturating_mul(i128::from(months));
    let cents = i64::try_from(cents).unwrap_or(if cents.is_negative() {
        i64::MIN
    } else {
        i64::MAX
    });
    Money::from_cents(cents)
}

fn dividend_cost(net: Money, today: Date) -> Money {
    let tax = net.apply_rate_bps(DIVIDEND_INCOME_TAX_BPS);
    let social = net.apply_rate_bps(dividend_social_charges_bps(today));
    net + tax + social
}

fn dividend_door(
    conn: &Connection,
    today: Date,
    year_end: Option<FiscalYearEnd>,
) -> Result<DividendDoor, AppError> {
    let Some(year_end) = year_end else {
        return Ok(DividendDoor::Closed {
            reason: DividendClosed::YearNotClosed,
        });
    };
    let current = year_end.containing(today);
    let last_ended = year_end.previous(current);
    let record = fiscal_year_ending_in(conn, last_ended.end().year())?;
    match record {
        Some(r) if r.is_approved() && r.net_result.cents() > 0 => Ok(DividendDoor::Open {
            available: r.net_result,
        }),
        Some(r) if r.is_approved() => Ok(DividendDoor::Closed {
            reason: DividendClosed::NoProfit,
        }),
        _ => Ok(DividendDoor::Closed {
            reason: DividendClosed::YearNotClosed,
        }),
    }
}

fn paid_this_year(conn: &Connection, today: Date) -> Result<Money, AppError> {
    let year = today.year();
    let mut cents: i128 = 0;
    for tx in list_bank_transactions(conn)? {
        if tx.occurred_on.year() != year {
            continue;
        }
        let Some(account) = tx.settlement_account.as_ref() else {
            continue;
        };
        let n = account.as_str();
        if n.starts_with("455") || n.starts_with("457") || n.starts_with("421") {
            cents += i128::from(tx.amount_cents.unsigned_abs());
        }
    }
    let cents = i64::try_from(cents).unwrap_or(i64::MAX);
    Ok(Money::from_cents(cents))
}

// ---------------------------------------------------------------------------------------------
// Paysage
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Landscape {
    pub from: Month,
    pub today: Month,
    #[serde(with = "crate::domain::serde_date::date::option")]
    pub year_end: Option<Date>,
    pub days_to_year_end: Option<i64>,
    pub bank: Money,
    pub expected: Option<ExpectedReceipt>,
    pub points: Vec<LandscapePoint>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LandscapePoint {
    pub month: Month,
    pub cash: Money,
    pub cash_if_received: Money,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExpectedReceipt {
    pub party: String,
    pub amount: Money,
    pub month: Month,
}

/// # Errors
pub fn society_landscape(conn: &Connection, today: Date) -> Result<Landscape, AppError> {
    let bank = cash_in_bank(conn, today)?;
    let profile = company_profile(conn)?;
    let year_end = profile.as_ref().and_then(|p| p.fiscal_year_end);
    let exercise_end = year_end.map(|e| e.containing(today).end());
    let days_to_year_end = exercise_end
        .filter(|d| *d >= today)
        .map(|d| (d - today).whole_days());
    let start = landscape_start(conn, today, year_end)?;
    let today_month = month_of(today);
    let mut points = Vec::new();
    let mut cursor = start;
    while cursor < today_month {
        let cash = cash_in_bank(conn, cursor.last_day())?;
        points.push(LandscapePoint {
            month: cursor,
            cash,
            cash_if_received: cash,
        });
        cursor = cursor.succ();
    }
    points.push(LandscapePoint {
        month: today_month,
        cash: bank,
        cash_if_received: bank,
    });

    let mut conservative = build_forecast_inputs(conn, today, bank)?;
    conservative.weighted_pipeline = Money::ZERO;
    conservative.unpaid_invoices.clear();
    conservative.pending_mission_revenue.clear();
    let mut received = build_forecast_inputs(conn, today, bank)?;
    received.weighted_pipeline = Money::ZERO;
    received.pending_mission_revenue.clear();
    let without_receipts = forecast_12_months(today, &conservative);
    let with_receipts = forecast_12_months(today, &received);
    for (c, r) in without_receipts
        .iter()
        .zip(&with_receipts)
        .skip(1)
        .take(LANDSCAPE_FUTURE_MONTHS)
    {
        points.push(LandscapePoint {
            month: c.month,
            cash: c.projected_cash,
            cash_if_received: r.projected_cash,
        });
    }

    let expected = expected_receipt(conn, today)?;
    Ok(Landscape {
        from: start,
        today: today_month,
        year_end: exercise_end,
        days_to_year_end,
        bank,
        expected,
        points,
    })
}

fn landscape_start(
    conn: &Connection,
    today: Date,
    year_end: Option<FiscalYearEnd>,
) -> Result<Month, AppError> {
    if let Some(end) = year_end {
        return Ok(month_of(end.containing(today).start()));
    }
    if let Some(opening) = opening_balance(conn)? {
        return Ok(month_of(opening.balance.opens_on));
    }
    Ok(month_of(today))
}

fn expected_receipt(conn: &Connection, today: Date) -> Result<Option<ExpectedReceipt>, AppError> {
    let aged = aged_balance(conn, today)?;
    let Some(first) = aged.into_iter().max_by_key(|a| a.outstanding) else {
        return Ok(None);
    };
    if first.outstanding.is_zero() {
        return Ok(None);
    }
    let party = client_by_id(conn, first.client_id)?
        .map_or_else(|| first.client_id.to_string(), |c| c.name);
    let due_on = today - time::Duration::days(first.days_overdue);
    Ok(Some(ExpectedReceipt {
        party,
        amount: first.outstanding,
        month: month_of(due_on),
    }))
}

fn month_of(date: Date) -> Month {
    Month::new(date.year(), u8::from(date.month())).expect("mois toujours valide pour une date")
}

// ---------------------------------------------------------------------------------------------
// Conversations de l'année
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct YearConversations {
    pub bars: Vec<ConversationBar>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConversationBar {
    pub month: Month,
    pub count: u32,
    pub now: bool,
    pub thin: bool,
}

/// # Errors
pub fn year_conversations(conn: &Connection, today: Date) -> Result<YearConversations, AppError> {
    let profile = company_profile(conn)?;
    let start = profile
        .as_ref()
        .and_then(|p| p.fiscal_year_end)
        .map_or_else(
            || {
                let mut m = month_of(today);
                for _ in 0..11 {
                    m = m.pred();
                }
                m
            },
            |end| month_of(end.containing(today).start()),
        );
    let today_month = month_of(today);
    let opportunities = list_opportunities(conn)?;
    let invoices = list_invoices(conn)?;
    let mut bars = Vec::with_capacity(12);
    let mut cursor = start;
    for _ in 0..12 {
        let mut names = std::collections::HashSet::new();
        for opp in &opportunities {
            if month_of(opp.created_at.date()) == cursor {
                names.insert(opp.client_id);
            }
        }
        for invoice in &invoices {
            if month_of(invoice.issued_on) == cursor {
                names.insert(invoice.client_id);
            }
        }
        if cursor == today_month {
            for opp in &opportunities {
                if !opp.stage.is_closed() && opp.archived_at.is_none() {
                    names.insert(opp.client_id);
                }
            }
        }
        let count = u32::try_from(names.len()).unwrap_or(u32::MAX);
        let now = cursor == today_month;
        bars.push(ConversationBar {
            month: cursor,
            count,
            now,
            thin: now && count < CONVERSATION_THIN,
        });
        cursor = cursor.succ();
    }
    Ok(YearConversations { bars })
}

// ---------------------------------------------------------------------------------------------
// Accueil
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IdentityShort {
    pub name: Option<String>,
    pub legal_form: Option<String>,
    pub capital: Option<Money>,
    pub year_end: Option<FiscalYearEnd>,
    pub days_to_year_end: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SocietyHome {
    pub identity: IdentityShort,
    pub landscape: Landscape,
    pub conversations: YearConversations,
    pub pay: PayYourself,
    pub next_duty: Option<Duty>,
    pub closing: ClosingCue,
    pub unmatched: u32,
    pub vat_position: Option<VatPosition>,
}

/// Crédit ou due de TVA que l'indépendant lit — le 27 / 28 dérivé, jamais l'ancre brute.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VatPosition {
    Credit { amount: Money },
    Due { amount: Money },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ClosingCue {
    pub days_left: Option<i64>,
    pub unmatched: u32,
    pub stage: ClosingStage,
}

/// # Errors
pub fn society_home(conn: &Connection, today: Date) -> Result<SocietyHome, AppError> {
    let profile = company_profile(conn)?;
    let year_end = profile.as_ref().and_then(|p| p.fiscal_year_end);
    let exercise_end = year_end.map(|e| e.containing(today).end());
    let days_to_year_end = exercise_end
        .filter(|d| *d >= today)
        .map(|d| (d - today).whole_days());
    let identity = IdentityShort {
        name: profile.as_ref().map(|p| p.name.clone()),
        legal_form: profile.as_ref().map(|p| p.legal_form.clone()),
        capital: profile.as_ref().and_then(|p| p.share_capital),
        year_end,
        days_to_year_end,
    };
    let landscape = society_landscape(conn, today)?;
    let conversations = year_conversations(conn, today)?;
    let pay = pay_yourself(conn, today)?;
    let duties = society_duties(conn, today)?;
    let unmatched = unmatched_count(conn)?;
    let closing = closing_cue(conn, today, days_to_year_end, unmatched)?;
    Ok(SocietyHome {
        identity,
        landscape,
        conversations,
        pay,
        next_duty: duties
            .into_iter()
            .find(|d| d.filed_on.is_none() && !d.catch_up),
        closing,
        unmatched,
        vat_position: vat_position(conn, today)?,
    })
}

/// Crédit (27) ou due (28) de la prochaine CA3 non déposée. `None` si néant, incomplet, ou pas de CA3.
///
/// # Errors
///
/// Lecture du calendrier, des cases, ou du coffre.
pub fn vat_position(conn: &Connection, today: Date) -> Result<Option<VatPosition>, AppError> {
    let periodicity = ca3_periodicity(conn)?;
    let profile = company_profile(conn)?;
    let day = profile.as_ref().map_or(Ca3FilingRule::EARLIEST_DAY, |p| {
        Ca3FilingRule::derive(&p.legal_form, &p.name, p.siren, &p.address.postal_code).day
    });
    let mut cursor = today;
    for _ in 0..24 {
        let filing = crate::fiscal::next_ca3_filing(cursor, periodicity, day);
        let period_key = ca3_period_key(filing.period_start);
        if filing_for(conn, FiscalDeadlineKind::Ca3, &period_key)?.is_some() {
            cursor = filing
                .due_on
                .checked_add(time::Duration::days(1))
                .unwrap_or(filing.due_on);
            continue;
        }
        let (boxes, coverage) = ca3_boxes(conn, today, &period_key)?;
        if coverage != BoxCoverage::Complete {
            return Ok(None);
        }
        if let Some(amount) = boxes
            .iter()
            .find(|b| b.case == "27")
            .and_then(|b| b.amount)
            .filter(|a| !a.is_zero())
        {
            return Ok(Some(VatPosition::Credit { amount }));
        }
        if let Some(amount) = boxes
            .iter()
            .find(|b| b.case == "28")
            .and_then(|b| b.amount)
            .filter(|a| !a.is_zero())
        {
            return Ok(Some(VatPosition::Due { amount }));
        }
        return Ok(None);
    }
    Ok(None)
}

fn unmatched_count(conn: &Connection) -> Result<u32, AppError> {
    let n = list_bank_transactions(conn)?
        .into_iter()
        .filter(|t| !t.is_matched())
        .count();
    Ok(u32::try_from(n).unwrap_or(u32::MAX))
}

fn closing_cue(
    conn: &Connection,
    today: Date,
    days_left: Option<i64>,
    unmatched: u32,
) -> Result<ClosingCue, AppError> {
    let period = company_profile(conn)?
        .as_ref()
        .and_then(|p| p.fiscal_year_end)
        .map_or_else(|| today.year(), |e| e.containing(today).end().year());
    let checklist = closing_checklist(conn, period, today)?;
    Ok(ClosingCue {
        days_left,
        unmatched,
        stage: checklist.stage,
    })
}

// ---------------------------------------------------------------------------------------------
// Impôts
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DepositPlace {
    ImpotsGouv,
    Post,
    Greffe,
    NetEntreprises,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Duty {
    pub kind: FiscalDeadlineKind,
    #[serde(with = "crate::domain::serde_date::date")]
    pub due_on: Date,
    pub amount: Option<Money>,
    pub deposit: DepositPlace,
    pub period_key: String,
    #[serde(with = "crate::domain::serde_date::date::option")]
    pub filed_on: Option<Date>,
    /// Schéma de TVA de l'exercice en cours — fait typé, pour que les adaptateurs disent
    /// « mois » ou « trimestre ».
    pub vat_scheme: VatFilingScheme,
    /// `due_on` est antérieure au premier jour d'usage de ce coffre : ce n'est pas un retard
    /// dans `FreeFlow`, c'est un rattrapage.
    pub catch_up: bool,
}

impl DepositPlace {
    #[must_use]
    pub const fn of(kind: FiscalDeadlineKind) -> Self {
        match kind {
            FiscalDeadlineKind::Cfe => Self::Post,
            FiscalDeadlineKind::AccountsFiling => Self::Greffe,
            FiscalDeadlineKind::Dsn => Self::NetEntreprises,
            FiscalDeadlineKind::ApprovalMeeting => Self::Internal,
            FiscalDeadlineKind::Ca3
            | FiscalDeadlineKind::VatInstalment
            | FiscalDeadlineKind::Ca12
            | FiscalDeadlineKind::IsAcompte
            | FiscalDeadlineKind::IsSolde
            | FiscalDeadlineKind::Liasse
            | FiscalDeadlineKind::Das2
            | FiscalDeadlineKind::Dividends2777 => Self::ImpotsGouv,
        }
    }
}

fn vat_scheme_for(conn: &Connection, today: Date) -> Result<VatFilingScheme, AppError> {
    let profile = company_profile(conn)?;
    let vat_regime = profile.as_ref().and_then(|p| p.vat_regime);
    let fye = profile
        .as_ref()
        .and_then(|p| p.fiscal_year_end)
        .unwrap_or(FiscalYearEnd::CALENDAR);
    Ok(VatFilingScheme::for_exercise(
        vat_regime,
        fye.current(today),
    ))
}

/// # Errors
pub fn society_duties(conn: &Connection, today: Date) -> Result<Vec<Duty>, AppError> {
    let calendar = fiscal_calendar(conn, today)?;
    let filings = list_filings(conn)?;
    let window_start = duty_window_start(conn, today)?;
    let window_end = add_months(today, 12);
    let vat_scheme = vat_scheme_for(conn, today)?;
    let started = crate::setup::vault_started_on(conn, today)?;
    let mut duties: Vec<Duty> = calendar
        .into_iter()
        .filter(|d| d.kind != FiscalDeadlineKind::ApprovalMeeting)
        .map(|d| {
            let filed_on = filings
                .iter()
                .find(|f| f.kind == d.kind && f.period_key == d.period_key)
                .map(|f| f.filed_on);
            duty_from_deadline(&d, filed_on, vat_scheme, started)
        })
        .collect();

    push_ca3_window(
        conn,
        today,
        window_start,
        window_end,
        &filings,
        vat_scheme,
        started,
        &mut duties,
    )?;
    push_is_acompte_window(
        window_start,
        window_end,
        &filings,
        vat_scheme,
        started,
        &mut duties,
    );

    for filing in &filings {
        if duties
            .iter()
            .any(|d| d.kind == filing.kind && d.period_key == filing.period_key)
        {
            continue;
        }
        if filing.due_on < window_start || filing.due_on > window_end {
            continue;
        }
        duties.push(Duty {
            kind: filing.kind,
            due_on: filing.due_on,
            amount: None,
            deposit: DepositPlace::of(filing.kind),
            period_key: filing.period_key.clone(),
            filed_on: Some(filing.filed_on),
            vat_scheme,
            catch_up: filing.due_on < started,
        });
    }

    duties.sort_by(|a, b| {
        a.due_on
            .cmp(&b.due_on)
            .then(a.kind.as_str().cmp(b.kind.as_str()))
    });
    Ok(duties)
}

fn duty_from_deadline(
    d: &FiscalDeadline,
    filed_on: Option<Date>,
    vat_scheme: VatFilingScheme,
    started: Date,
) -> Duty {
    Duty {
        kind: d.kind,
        due_on: d.due_on,
        amount: d.amount,
        deposit: DepositPlace::of(d.kind),
        period_key: d.period_key.clone(),
        filed_on,
        vat_scheme,
        catch_up: d.due_on < started,
    }
}

fn has_duty(duties: &[Duty], kind: FiscalDeadlineKind, period_key: &str) -> bool {
    duties
        .iter()
        .any(|d| d.kind == kind && d.period_key == period_key)
}

fn duty_window_start(conn: &Connection, today: Date) -> Result<Date, AppError> {
    let lookback = sub_months(today, 12);
    let opens_on = opening_balance(conn)?.map(|o| o.balance.opens_on);
    Ok(opens_on.map_or(lookback, |on| on.max(lookback)))
}

fn filed_on_for(
    filings: &[DutyFiling],
    kind: FiscalDeadlineKind,
    period_key: &str,
) -> Option<Date> {
    filings
        .iter()
        .find(|f| f.kind == kind && f.period_key == period_key)
        .map(|f| f.filed_on)
}

/// # Panics
///
/// Ne panique jamais : un début d'exercice a un mois valide.
#[allow(clippy::too_many_arguments)]
fn push_ca3_window(
    conn: &Connection,
    today: Date,
    window_start: Date,
    window_end: Date,
    filings: &[DutyFiling],
    vat_scheme: VatFilingScheme,
    started: Date,
    duties: &mut Vec<Duty>,
) -> Result<(), AppError> {
    let profile = company_profile(conn)?;
    let vat_regime = profile.as_ref().and_then(|p| p.vat_regime);
    let fye = profile
        .as_ref()
        .and_then(|p| p.fiscal_year_end)
        .unwrap_or(FiscalYearEnd::CALENDAR);
    let current = fye.current(today);
    let previous = fye.previous(current);
    let scheme = VatFilingScheme::for_exercise(vat_regime, current);
    let Some(periodicity) = scheme.ca3_periodicity() else {
        return Ok(());
    };
    let day = profile
        .as_ref()
        .map_or(crate::domain::Ca3FilingRule::EARLIEST_DAY, |p| {
            crate::domain::Ca3FilingRule::derive(
                &p.legal_form,
                &p.name,
                p.siren,
                &p.address.postal_code,
            )
            .day
        });
    let earliest = simplified_regime_applies_to(previous).then(|| {
        let start = current.start();
        Month::new(start.year(), u8::from(start.month()))
            .expect("un début d'exercice a un mois valide")
    });
    for filing in ca3_filings_in_range(window_start, window_end, periodicity, day, earliest) {
        let period_key = ca3_period_key(filing.period_start);
        if has_duty(duties, FiscalDeadlineKind::Ca3, &period_key) {
            continue;
        }
        let vat = vat_due_for_period(
            conn,
            filing.period_start.first_day(),
            filing.period_end.last_day(),
        )?;
        let filed_on = filed_on_for(filings, FiscalDeadlineKind::Ca3, &period_key);
        duties.push(Duty {
            kind: FiscalDeadlineKind::Ca3,
            due_on: filing.due_on,
            amount: Some(vat.due),
            deposit: DepositPlace::of(FiscalDeadlineKind::Ca3),
            period_key,
            filed_on,
            vat_scheme,
            catch_up: filing.due_on < started,
        });
    }
    Ok(())
}

fn push_is_acompte_window(
    window_start: Date,
    window_end: Date,
    filings: &[DutyFiling],
    vat_scheme: VatFilingScheme,
    started: Date,
    duties: &mut Vec<Duty>,
) {
    for due_on in is_acompte_dates_in_range(window_start, window_end) {
        let period_key = crate::domain::format_date(due_on);
        if has_duty(duties, FiscalDeadlineKind::IsAcompte, &period_key) {
            continue;
        }
        duties.push(Duty {
            kind: FiscalDeadlineKind::IsAcompte,
            due_on,
            amount: None,
            deposit: DepositPlace::of(FiscalDeadlineKind::IsAcompte),
            period_key: period_key.clone(),
            filed_on: filed_on_for(filings, FiscalDeadlineKind::IsAcompte, &period_key),
            vat_scheme,
            catch_up: due_on < started,
        });
    }
}

/// # Panics
///
/// Ne panique jamais : les mois 3, 6, 9 et 12 existent, et le 15 aussi.
fn is_acompte_dates_in_range(from: Date, to: Date) -> Vec<Date> {
    if to < from {
        return Vec::new();
    }
    let mut dates = Vec::new();
    let mut year = from.year();
    loop {
        for month in [3_u8, 6, 9, 12] {
            let due_on = Date::from_calendar_date(
                year,
                time::Month::try_from(month).expect("mois d'acompte valide"),
                15,
            )
            .expect("le 15 existe chaque mois");
            if due_on > to {
                return dates;
            }
            if due_on >= from {
                dates.push(due_on);
            }
        }
        year += 1;
        if year > to.year() + 1 {
            return dates;
        }
    }
}

const IMPOTS_URL: &str = "https://www.impots.gouv.fr/";
const INPI_URL: &str = "https://procedures.inpi.fr/";

/// Histoire du montant : faits, pas de phrase.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AmountStory {
    Due { amount: Money, basis: AmountBasis },
    Waiver { reason: WaiverReason },
    Unknown { reason: UnknownReason },
    External,
    NotYourHands { amount: Option<Money> },
    Declaration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AmountBasis {
    QuarterOfPriorIs,
    VatForPeriod,
    VatCredit,
    VatInstalment,
    Ca12Net,
    SnapshotIs,
    FeesBySupplier,
    DividendWithholding,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WaiverReason {
    PriorIsBelowThreshold,
    FirstExercise,
    VatInstalmentDispensation,
    Das2BelowThreshold,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnknownReason {
    NoProfile,
    PriorIsMissing,
    PriorVatMissing,
    /// La période déclarée précède le bilan d'ouverture : les chiffres ne sont pas dans le coffre.
    PeriodNotInVault,
}

/// Ce que l'écran demandera — la fenêtre en fait des phrases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Expect {
    NeedsProfessionalSpace,
    CheckPrefill,
    FileEvenIfZero,
    ModulateDown,
    PayElectronically,
    SeeEfiNotice,
    NoticeInSpace,
    PayrollExpertDoesIt,
    GuichetUnique,
    ConfidentialityOption,
}

/// Case d'un formulaire d'État : faits, pas de phrase.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FormBox {
    pub form: &'static str,
    pub case: &'static str,
    pub amount: Option<Money>,
    pub role: BoxRole,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BoxRole {
    Fill,
    LeaveEmpty,
    SiteComputes,
    Check,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BoxCoverage {
    Complete,
    Incomplete,
    NotApplicable,
}

/// Briefing d'une démarche hors de l'app. Aucune phrase française.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DutyBriefing {
    pub kind: FiscalDeadlineKind,
    #[serde(with = "crate::domain::serde_date::date")]
    pub due_on: Date,
    pub deposit: DepositPlace,
    pub amount: AmountStory,
    pub form: Option<&'static str>,
    pub url: &'static str,
    pub path: &'static [&'static str],
    pub expect: Vec<Expect>,
    pub period_key: String,
    #[serde(with = "crate::domain::serde_date::date::option")]
    pub filed_on: Option<Date>,
    pub boxes: Vec<FormBox>,
    pub coverage: BoxCoverage,
    pub vat_scheme: VatFilingScheme,
    pub catch_up: bool,
    pub vat_refund: Option<VatRefundStatus>,
    /// Montant de la case 15 s'il y a un fait, sinon `None`.
    #[serde(default)]
    pub vat_reversal: Option<Money>,
}

struct BriefingMeta {
    form: Option<&'static str>,
    url: &'static str,
    path: &'static [&'static str],
    expect: &'static [Expect],
}

fn briefing_meta(kind: FiscalDeadlineKind) -> Option<BriefingMeta> {
    const IMPOTS_PAY: &[Expect] = &[
        Expect::NeedsProfessionalSpace,
        Expect::CheckPrefill,
        Expect::PayElectronically,
    ];
    const IMPOTS_ZERO: &[Expect] = &[
        Expect::NeedsProfessionalSpace,
        Expect::CheckPrefill,
        Expect::FileEvenIfZero,
        Expect::PayElectronically,
    ];
    const IMPOTS_IS: &[Expect] = &[
        Expect::NeedsProfessionalSpace,
        Expect::CheckPrefill,
        Expect::ModulateDown,
        Expect::PayElectronically,
    ];
    const IMPOTS_LIASSE: &[Expect] = &[Expect::NeedsProfessionalSpace, Expect::SeeEfiNotice];
    const IMPOTS_CFE: &[Expect] = &[
        Expect::NeedsProfessionalSpace,
        Expect::NoticeInSpace,
        Expect::PayElectronically,
    ];
    const IMPOTS_DAS2: &[Expect] = &[Expect::NeedsProfessionalSpace, Expect::FileEvenIfZero];
    const GREFFE: &[Expect] = &[Expect::GuichetUnique, Expect::ConfidentialityOption];
    const DSN: &[Expect] = &[Expect::PayrollExpertDoesIt];
    match kind {
        FiscalDeadlineKind::ApprovalMeeting => None,
        FiscalDeadlineKind::Ca3 => Some(BriefingMeta {
            form: Some("CA3"),
            url: IMPOTS_URL,
            path: &["Déclarer", "TVA et taxes assimilées"],
            expect: IMPOTS_ZERO,
        }),
        FiscalDeadlineKind::VatInstalment => Some(BriefingMeta {
            form: Some("3514"),
            url: IMPOTS_URL,
            path: &["Déclarer", "TVA et taxes assimilées"],
            expect: IMPOTS_PAY,
        }),
        FiscalDeadlineKind::Ca12 => Some(BriefingMeta {
            form: Some("3517"),
            url: IMPOTS_URL,
            path: &["Déclarer", "TVA et taxes assimilées"],
            expect: IMPOTS_PAY,
        }),
        FiscalDeadlineKind::IsAcompte => Some(BriefingMeta {
            form: Some("2571"),
            url: IMPOTS_URL,
            path: &["Déclarer", "Impôt sur les sociétés"],
            expect: IMPOTS_IS,
        }),
        FiscalDeadlineKind::IsSolde => Some(BriefingMeta {
            form: Some("2572"),
            url: IMPOTS_URL,
            path: &["Déclarer", "Impôt sur les sociétés"],
            expect: IMPOTS_ZERO,
        }),
        FiscalDeadlineKind::Cfe => Some(BriefingMeta {
            form: None,
            url: IMPOTS_URL,
            path: &["Consulter", "Avis C.F.E"],
            expect: IMPOTS_CFE,
        }),
        FiscalDeadlineKind::Liasse => Some(BriefingMeta {
            form: Some("2065"),
            url: IMPOTS_URL,
            path: &[
                "Déclarer",
                "Impôt sur les sociétés",
                "régime simplifié (EFI)",
            ],
            expect: IMPOTS_LIASSE,
        }),
        FiscalDeadlineKind::AccountsFiling => Some(BriefingMeta {
            form: None,
            url: INPI_URL,
            path: &["Dépôt des comptes annuels"],
            expect: GREFFE,
        }),
        FiscalDeadlineKind::Das2 => Some(BriefingMeta {
            form: Some("DAS2"),
            url: IMPOTS_URL,
            path: &["Déclarer", "Honoraires (DAS2)"],
            expect: IMPOTS_DAS2,
        }),
        FiscalDeadlineKind::Dividends2777 => Some(BriefingMeta {
            form: Some("2777"),
            url: IMPOTS_URL,
            path: &["Déclarer", "Revenus de capitaux mobiliers"],
            expect: IMPOTS_PAY,
        }),
        FiscalDeadlineKind::Dsn => Some(BriefingMeta {
            form: None,
            url: IMPOTS_URL,
            path: &[],
            expect: DSN,
        }),
    }
}

fn fallback_due(kind: FiscalDeadlineKind, today: Date) -> Date {
    match kind {
        FiscalDeadlineKind::IsAcompte => next_is_acompte(today),
        FiscalDeadlineKind::Cfe => next_cfe(today),
        _ => today,
    }
}

/// Briefing d'une démarche à faire hors de `FreeFlow`.
///
/// `period_key` désigne une occurrence (`AAAA-MM` pour une CA3, `AAAA-MM-JJ` sinon). `None` vise
/// la prochaine occurrence non déposée dont `due_on >= today`, à défaut la prochaine du
/// calendrier.
///
/// # Errors
///
/// [`AppError::Domain`] si la démarche se fait ici (`ApprovalMeeting`), si la période demandée
/// n'existe pas, ou une erreur de lecture du calendrier.
pub fn duty_briefing(
    conn: &Connection,
    kind: FiscalDeadlineKind,
    today: Date,
    period_key: Option<&str>,
) -> Result<DutyBriefing, AppError> {
    let Some(meta) = briefing_meta(kind) else {
        return Err(AppError::Domain(
            "l'approbation des comptes se fait ici, pas sur un site d'État".into(),
        ));
    };
    let duties = society_duties(conn, today)?;
    let resolved = resolve_duty(&duties, kind, today, period_key)?;
    let due_on = resolved
        .as_ref()
        .map_or_else(|| fallback_due(kind, today), |d| d.due_on);
    let cal_amount = resolved.as_ref().and_then(|d| d.amount);
    let period_key = resolved.as_ref().map_or_else(
        || fallback_period_key(kind, due_on, today),
        |d| d.period_key.clone(),
    );
    let mut amount = amount_story(conn, kind, today, cal_amount)?;
    let (boxes, coverage) = form_guide(conn, kind, today, &amount, &period_key)?;
    if coverage == BoxCoverage::Incomplete
        && kind == FiscalDeadlineKind::Ca3
        && matches!(amount, AmountStory::Due { .. })
    {
        amount = AmountStory::Unknown {
            reason: UnknownReason::PeriodNotInVault,
        };
    }
    if kind == FiscalDeadlineKind::Ca3 && coverage == BoxCoverage::Complete {
        if let Some(due) = boxes.iter().find(|b| b.case == "28").and_then(|b| b.amount) {
            amount = AmountStory::Due {
                amount: due,
                basis: AmountBasis::VatForPeriod,
            };
        } else if let Some(credit) = boxes.iter().find(|b| b.case == "25").and_then(|b| b.amount) {
            amount = AmountStory::Due {
                amount: credit,
                basis: AmountBasis::VatCredit,
            };
        }
    }
    let vat_refund = if kind == FiscalDeadlineKind::Ca3 {
        vat_refund::status_for_boxes(conn, &period_key, coverage, &boxes)?
    } else {
        None
    };
    let vat_reversal = if kind == FiscalDeadlineKind::Ca3 {
        vat_reversal_for(conn, &period_key)?.map(|r| r.amount)
    } else {
        None
    };
    let filed_on =
        resolved
            .as_ref()
            .and_then(|d| d.filed_on)
            .or(filing_for(conn, kind, &period_key)?.map(|f| f.filed_on));
    let vat_scheme = resolved
        .as_ref()
        .map_or_else(|| vat_scheme_for(conn, today), |d| Ok(d.vat_scheme))?;
    Ok(DutyBriefing {
        kind,
        due_on,
        deposit: DepositPlace::of(kind),
        amount,
        form: meta.form,
        url: meta.url,
        path: meta.path,
        expect: meta.expect.to_vec(),
        period_key,
        filed_on,
        boxes,
        coverage,
        vat_scheme,
        catch_up: resolved.as_ref().is_some_and(|d| d.catch_up),
        vat_refund,
        vat_reversal,
    })
}

fn resolve_duty<'a>(
    duties: &'a [Duty],
    kind: FiscalDeadlineKind,
    today: Date,
    period_key: Option<&str>,
) -> Result<Option<&'a Duty>, AppError> {
    let of_kind: Vec<&Duty> = duties.iter().filter(|d| d.kind == kind).collect();
    if let Some(key) = period_key {
        return of_kind
            .into_iter()
            .find(|d| d.period_key == key)
            .map(Some)
            .ok_or_else(|| {
                AppError::Domain(format!(
                    "aucune échéance {} pour la période {key}",
                    kind.as_str()
                ))
            });
    }
    Ok(of_kind
        .iter()
        .copied()
        .find(|d| d.filed_on.is_none() && d.due_on >= today)
        .or_else(|| of_kind.iter().copied().find(|d| d.due_on >= today))
        .or_else(|| of_kind.first().copied()))
}

fn fallback_period_key(kind: FiscalDeadlineKind, due_on: Date, today: Date) -> String {
    if kind == FiscalDeadlineKind::Ca3 {
        let filing = crate::fiscal::next_ca3_filing(
            today,
            crate::fiscal::Ca3Periodicity::Monthly,
            crate::domain::Ca3FilingRule::EARLIEST_DAY,
        );
        return ca3_period_key(filing.period_start);
    }
    crate::domain::format_date(due_on)
}

fn amount_story(
    conn: &Connection,
    kind: FiscalDeadlineKind,
    today: Date,
    cal_amount: Option<Money>,
) -> Result<AmountStory, AppError> {
    Ok(match kind {
        FiscalDeadlineKind::ApprovalMeeting => {
            return Err(AppError::Domain(
                "l'approbation des comptes se fait ici, pas sur un site d'État".into(),
            ));
        }
        FiscalDeadlineKind::Cfe => AmountStory::External,
        FiscalDeadlineKind::Dsn => AmountStory::NotYourHands { amount: cal_amount },
        FiscalDeadlineKind::Liasse | FiscalDeadlineKind::AccountsFiling => AmountStory::Declaration,
        FiscalDeadlineKind::IsAcompte => is_acompte_story(conn, today)?,
        FiscalDeadlineKind::IsSolde => is_solde_story(conn, today, cal_amount)?,
        FiscalDeadlineKind::Ca3 => AmountStory::Due {
            amount: cal_amount.unwrap_or(Money::ZERO),
            basis: AmountBasis::VatForPeriod,
        },
        FiscalDeadlineKind::VatInstalment => match cal_amount {
            Some(amount) if !amount.is_zero() => AmountStory::Due {
                amount,
                basis: AmountBasis::VatInstalment,
            },
            Some(_) | None => AmountStory::Waiver {
                reason: WaiverReason::VatInstalmentDispensation,
            },
        },
        FiscalDeadlineKind::Ca12 => AmountStory::Due {
            amount: cal_amount.unwrap_or(Money::ZERO),
            basis: AmountBasis::Ca12Net,
        },
        FiscalDeadlineKind::Das2 => match cal_amount {
            Some(amount) if amount >= DAS2_THRESHOLD => AmountStory::Due {
                amount,
                basis: AmountBasis::FeesBySupplier,
            },
            Some(_) | None => AmountStory::Waiver {
                reason: WaiverReason::Das2BelowThreshold,
            },
        },
        FiscalDeadlineKind::Dividends2777 => AmountStory::Due {
            amount: cal_amount.unwrap_or(Money::ZERO),
            basis: AmountBasis::DividendWithholding,
        },
    })
}

fn form_guide(
    conn: &Connection,
    kind: FiscalDeadlineKind,
    today: Date,
    amount: &AmountStory,
    period_key: &str,
) -> Result<(Vec<FormBox>, BoxCoverage), AppError> {
    match kind {
        FiscalDeadlineKind::IsAcompte => Ok(is_acompte_boxes(amount)),
        FiscalDeadlineKind::Ca3 => ca3_boxes(conn, today, period_key),
        _ => Ok((Vec::new(), BoxCoverage::NotApplicable)),
    }
}

fn is_acompte_boxes(amount: &AmountStory) -> (Vec<FormBox>, BoxCoverage) {
    let (fill, coverage) = match amount {
        AmountStory::Due { amount, .. } => (Some(*amount), BoxCoverage::Complete),
        AmountStory::Unknown { .. } => (None, BoxCoverage::Incomplete),
        AmountStory::Waiver { .. }
        | AmountStory::External
        | AmountStory::NotYourHands { .. }
        | AmountStory::Declaration => (Some(Money::ZERO), BoxCoverage::Complete),
    };
    (
        vec![
            FormBox {
                form: "2571",
                case: "03",
                amount: fill,
                role: BoxRole::Fill,
            },
            FormBox {
                form: "2571",
                case: "10",
                amount: fill,
                role: BoxRole::SiteComputes,
            },
        ],
        coverage,
    )
}

/// # Errors
///
/// Lecture du profil.
pub(super) fn ca3_periodicity(conn: &Connection) -> Result<Ca3Periodicity, AppError> {
    let profile = company_profile(conn)?;
    Ok(
        Ca3Periodicity::from_regime(profile.as_ref().and_then(|p| p.vat_regime))
            .unwrap_or(Ca3Periodicity::Monthly),
    )
}

fn previous_ca3_period_key(period_key: &str, periodicity: Ca3Periodicity) -> Option<String> {
    let start = parse_year_month(period_key)?;
    let prev_start = match periodicity {
        Ca3Periodicity::Monthly => start.pred(),
        Ca3Periodicity::Quarterly => start.pred().pred().pred(),
    };
    Some(ca3_period_key(prev_start))
}

fn ca3_bounds_for(period_key: &str, periodicity: Ca3Periodicity) -> Result<(Date, Date), AppError> {
    let start = parse_year_month(period_key)
        .ok_or_else(|| AppError::Domain(format!("période CA3 invalide : {period_key}")))?;
    let end = match periodicity {
        Ca3Periodicity::Monthly => start,
        Ca3Periodicity::Quarterly => start.succ().succ(),
    };
    Ok((start.first_day(), end.last_day()))
}

/// Case 22 de `period_key` : crédit ancré après la période précédente, ou chaîne dérivée
/// jusqu'à ce point (ligne 27 de P−1).
fn ca3_prior_credit(
    conn: &Connection,
    period_key: &str,
    periodicity: Ca3Periodicity,
) -> Result<Money, AppError> {
    let carry = vat_carry_in(conn)?;
    let opens_on = opening_balance(conn)?.map(|o| o.balance.opens_on);
    let mut between: Vec<(Date, Date, String)> = Vec::new();
    let mut key = period_key.to_string();
    for _ in 0..24 {
        let Some(prev) = previous_ca3_period_key(&key, periodicity) else {
            break;
        };
        if carry.as_ref().is_some_and(|c| c.after_period == prev) {
            return fold_carried_credit(
                conn,
                carry.as_ref().map_or(Money::ZERO, |c| c.credit),
                &between,
            );
        }
        let (start, end) = ca3_bounds_for(&prev, periodicity)?;
        if opens_on.is_some_and(|o| start < o) {
            return fold_carried_credit(conn, Money::ZERO, &between);
        }
        between.push((start, end, prev.clone()));
        key = prev;
    }
    fold_carried_credit(conn, Money::ZERO, &between)
}

fn fold_carried_credit(
    conn: &Connection,
    mut credit: Money,
    between: &[(Date, Date, String)],
) -> Result<Money, AppError> {
    for (start, end, period_key) in between.iter().rev() {
        let vat = vat_due_for_period(conn, *start, *end)?;
        let reversal = vat_reversal_for(conn, period_key)?.map_or(Money::ZERO, |r| r.amount);
        let net = vat.due + reversal - credit;
        credit = if net.cents() >= 0 {
            Money::ZERO
        } else {
            let period_credit = -net;
            let refund = vat_refund_for(conn, period_key)?.map_or(Money::ZERO, |r| r.amount);
            period_credit - refund
        };
    }
    Ok(credit)
}

fn ca3_period_bounds(
    conn: &Connection,
    today: Date,
    period_key: &str,
) -> Result<(Date, Date), AppError> {
    let periodicity = ca3_periodicity(conn)?;
    if parse_year_month(period_key).is_some() {
        return ca3_bounds_for(period_key, periodicity);
    }
    let profile = company_profile(conn)?;
    let day = profile
        .as_ref()
        .map_or(crate::domain::Ca3FilingRule::EARLIEST_DAY, |p| {
            crate::domain::Ca3FilingRule::derive(
                &p.legal_form,
                &p.name,
                p.siren,
                &p.address.postal_code,
            )
            .day
        });
    let filing = crate::fiscal::next_ca3_filing(today, periodicity, day);
    Ok((
        filing.period_start.first_day(),
        filing.period_end.last_day(),
    ))
}

fn parse_year_month(key: &str) -> Option<Month> {
    let (year, month) = key.split_once('-')?;
    if month.len() != 2 {
        return None;
    }
    let year: i32 = year.parse().ok()?;
    let month: u8 = month.parse().ok()?;
    Month::new(year, month).ok()
}

/// # Errors
///
/// Période illisible, ou lecture des factures / dépenses / crédit repris.
pub(super) fn ca3_boxes(
    conn: &Connection,
    today: Date,
    period_key: &str,
) -> Result<(Vec<FormBox>, BoxCoverage), AppError> {
    let (period_start, period_end) = ca3_period_bounds(conn, today, period_key)?;
    let incomplete = opening_balance(conn)?.is_some_and(|o| period_start < o.balance.opens_on);
    if incomplete {
        return Ok((
            vec![
                ca3_box("02", None, BoxRole::Fill),
                ca3_box("08", None, BoxRole::Fill),
                ca3_box("20", None, BoxRole::Fill),
                ca3_box("28", None, BoxRole::Fill),
            ],
            BoxCoverage::Incomplete,
        ));
    }
    let vat = vat_due_for_period(conn, period_start, period_end)?;
    let periodicity = ca3_periodicity(conn)?;
    let prior_credit = ca3_prior_credit(conn, period_key, periodicity)?;
    let mut boxes = vec![ca3_box("02", Some(vat.taxable_ht), BoxRole::Fill)];
    let vat_20 = vat
        .collected_by_rate
        .iter()
        .find(|b| b.rate == VatRate::Standard)
        .map_or(Money::ZERO, |b| b.vat_amount);
    boxes.push(ca3_box("08", Some(vat_20), BoxRole::Fill));
    for (rate, case) in [(VatRate::Intermediate, "9B"), (VatRate::Reduced, "09")] {
        if let Some(line) = vat.collected_by_rate.iter().find(|b| b.rate == rate)
            && !line.vat_amount.is_zero()
        {
            boxes.push(ca3_box(case, Some(line.vat_amount), BoxRole::Fill));
        }
    }
    let reversal = vat_reversal_for(conn, period_key)?.map_or(Money::ZERO, |r| r.amount);
    if !reversal.is_zero() {
        boxes.push(ca3_box("15", Some(reversal), BoxRole::Fill));
    }
    if !vat.deductible_assets.is_zero() {
        boxes.push(ca3_box("19", Some(vat.deductible_assets), BoxRole::Fill));
    }
    boxes.push(ca3_box("20", Some(vat.deductible_other), BoxRole::Fill));
    if !prior_credit.is_zero() {
        boxes.push(ca3_box("22", Some(prior_credit), BoxRole::Fill));
    }
    let net = vat.due + reversal - prior_credit;
    if net.cents() >= 0 {
        boxes.push(ca3_box("28", Some(net), BoxRole::Fill));
    } else {
        let period_credit = -net;
        boxes.push(ca3_box("25", Some(period_credit), BoxRole::Fill));
        if let Some(refund) = vat_refund_for(conn, period_key)? {
            boxes.push(ca3_box("26", Some(refund.amount), BoxRole::Fill));
            let remainder = period_credit - refund.amount;
            if remainder.cents() > 0 {
                boxes.push(ca3_box("27", Some(remainder), BoxRole::Fill));
            }
        } else {
            boxes.push(ca3_box("27", Some(period_credit), BoxRole::Fill));
        }
    }
    Ok((boxes, BoxCoverage::Complete))
}

fn ca3_box(case: &'static str, amount: Option<Money>, role: BoxRole) -> FormBox {
    FormBox {
        form: "3310-CA3",
        case,
        amount,
        role,
    }
}

fn is_reference(conn: &Connection, today: Date) -> Result<(Option<Money>, bool, bool), AppError> {
    let profile = company_profile(conn)?;
    let has_profile = profile.is_some();
    let fye = profile
        .as_ref()
        .and_then(|p| p.fiscal_year_end)
        .unwrap_or(FiscalYearEnd::CALENDAR);
    let current = fye.current(today);
    let previous = fye.previous(current);
    let reprise = opening_balance(conn)?.filter(|o| previous.start() < o.balance.opens_on);
    let previous_result = match &reprise {
        Some(_) => None,
        None => profile
            .as_ref()
            .map(|p| compute_result(conn, previous, p))
            .transpose()?,
    };
    let reference_unknown = reprise
        .as_ref()
        .is_some_and(|o| o.prior_corporate_tax.is_none());
    let previous_is = match &reprise {
        Some(o) => o.prior_corporate_tax,
        None => previous_result.map(|r| r.corporate_tax),
    };
    Ok((previous_is, reference_unknown, has_profile))
}

fn is_acompte_story(conn: &Connection, today: Date) -> Result<AmountStory, AppError> {
    let (previous_is, reference_unknown, has_profile) = is_reference(conn, today)?;
    if !has_profile {
        return Ok(AmountStory::Unknown {
            reason: UnknownReason::NoProfile,
        });
    }
    if reference_unknown {
        return Ok(AmountStory::Unknown {
            reason: UnknownReason::PriorIsMissing,
        });
    }
    Ok(match previous_is {
        Some(is) if is < IS_ACOMPTE_DISPENSATION => AmountStory::Waiver {
            reason: WaiverReason::PriorIsBelowThreshold,
        },
        Some(is) => AmountStory::Due {
            amount: is
                .split_equally(4)
                .into_iter()
                .next()
                .unwrap_or(Money::ZERO),
            basis: AmountBasis::QuarterOfPriorIs,
        },
        None => AmountStory::Unknown {
            reason: UnknownReason::NoProfile,
        },
    })
}

fn is_solde_story(
    conn: &Connection,
    today: Date,
    cal_amount: Option<Money>,
) -> Result<AmountStory, AppError> {
    let (previous_is, reference_unknown, has_profile) = is_reference(conn, today)?;
    if !has_profile {
        return Ok(AmountStory::Unknown {
            reason: UnknownReason::NoProfile,
        });
    }
    if reference_unknown {
        return Ok(AmountStory::Unknown {
            reason: UnknownReason::PriorIsMissing,
        });
    }
    Ok(AmountStory::Due {
        amount: cal_amount.or(previous_is).unwrap_or(Money::ZERO),
        basis: AmountBasis::SnapshotIs,
    })
}

// ---------------------------------------------------------------------------------------------
// Clore
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BeatWhen {
    Done,
    Today,
    Later,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BeatKind {
    OpeningBalance {
        recorded: bool,
    },
    FileStatement {
        unmatched: u32,
    },
    Receipts {
        missing: bool,
    },
    CloseAccounts {
        #[serde(with = "crate::domain::serde_date::date")]
        ends_on: Date,
    },
    ThenApproveAndFile,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ClosingBeat {
    pub when: BeatWhen,
    #[serde(with = "crate::domain::serde_date::date::option")]
    pub on: Option<Date>,
    pub kind: BeatKind,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ClosingStory {
    pub days_left: Option<i64>,
    pub stage: ClosingStage,
    pub unmatched: u32,
    pub period: i32,
    pub beats: Vec<ClosingBeat>,
}

/// # Errors
pub fn closing_story(conn: &Connection, today: Date) -> Result<ClosingStory, AppError> {
    let profile = company_profile(conn)?;
    let year_end = profile.as_ref().and_then(|p| p.fiscal_year_end);
    let exercise = year_end.map(|e| e.containing(today));
    let period = exercise.map_or_else(|| today.year(), |e| e.end().year());
    let days_left = exercise
        .map(crate::domain::FiscalYear::end)
        .filter(|d| *d >= today)
        .map(|d| (d - today).whole_days());
    let checklist = closing_checklist(conn, period, today)?;
    let unmatched = unmatched_count(conn)?;
    let opening = checklist.step(ClosingStepKey::OpeningBalance);
    let expenses = checklist.step(ClosingStepKey::Expenses);
    let close = checklist.step(ClosingStepKey::Close);
    let ends_on = exercise.map_or_else(|| checklist.exercise.end(), crate::domain::FiscalYear::end);

    let mut beats = Vec::new();
    let opening_done =
        opening.is_some_and(|s| matches!(s.status, StepStatus::Done | StepStatus::Info));
    beats.push(ClosingBeat {
        when: if opening_done {
            BeatWhen::Done
        } else {
            BeatWhen::Today
        },
        on: None,
        kind: BeatKind::OpeningBalance {
            recorded: opening_done,
        },
    });
    let bank_when = if unmatched == 0 {
        BeatWhen::Done
    } else {
        BeatWhen::Today
    };
    beats.push(ClosingBeat {
        when: bank_when,
        on: None,
        kind: BeatKind::FileStatement { unmatched },
    });
    let receipts_missing = expenses.is_some_and(|s| s.status == StepStatus::Warning);
    beats.push(ClosingBeat {
        when: if receipts_missing {
            BeatWhen::Today
        } else if expenses.is_some_and(|s| matches!(s.status, StepStatus::Done | StepStatus::Info))
        {
            BeatWhen::Done
        } else {
            BeatWhen::Later
        },
        on: None,
        kind: BeatKind::Receipts {
            missing: receipts_missing,
        },
    });
    let close_status = close.map(|s| s.status);
    let (close_when, close_on) = match close_status {
        Some(StepStatus::Done | StepStatus::Info) => (BeatWhen::Done, None),
        Some(StepStatus::Later) | None if today <= ends_on => {
            (BeatWhen::Later, Some(ends_on.next_day().unwrap_or(ends_on)))
        }
        Some(StepStatus::Todo | StepStatus::Warning | StepStatus::Blocked) => {
            (BeatWhen::Today, None)
        }
        _ => (BeatWhen::Later, None),
    };
    beats.push(ClosingBeat {
        when: close_when,
        on: close_on,
        kind: BeatKind::CloseAccounts { ends_on },
    });
    beats.push(ClosingBeat {
        when: BeatWhen::Later,
        on: None,
        kind: BeatKind::ThenApproveAndFile,
    });
    Ok(ClosingStory {
        days_left,
        stage: checklist.stage,
        unmatched,
        period,
        beats,
    })
}

// ---------------------------------------------------------------------------------------------
// Relevé
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StatementReading {
    Expense,
    Debt { account: String, label: String },
    SelfPay,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatementMove {
    pub id: BankTransactionId,
    #[serde(with = "crate::domain::serde_date::date")]
    pub occurred_on: Date,
    pub amount: Money,
    pub description: String,
    pub suggested: StatementReading,
}

/// # Errors
pub fn statement_moves(conn: &Connection, today: Date) -> Result<Vec<StatementMove>, AppError> {
    let opening = opening_balance(conn)?;
    let lines = opening
        .as_ref()
        .map_or(&[][..], |r| r.balance.lines.as_slice());
    let mut moves: Vec<StatementMove> = list_bank_transactions(conn)?
        .into_iter()
        .filter(|t| !t.is_matched() && t.occurred_on <= today)
        .map(|t| StatementMove {
            suggested: suggested_reading(&t, lines),
            id: t.id,
            occurred_on: t.occurred_on,
            amount: Money::from_cents(t.amount_cents),
            description: t.description,
        })
        .collect();
    moves.sort_by(|a, b| b.occurred_on.cmp(&a.occurred_on).then(b.id.cmp(&a.id)));
    Ok(moves)
}

fn suggested_reading(
    tx: &BankTransaction,
    lines: &[crate::domain::OpeningBalanceLine],
) -> StatementReading {
    let abs = Money::from_cents(i64::try_from(tx.amount_cents.unsigned_abs()).unwrap_or(i64::MAX));
    for line in lines {
        if line.account.class() != 4 || line.amount != abs {
            continue;
        }
        let matches_side = (tx.is_debit() && line.side == Side::Credit)
            || (!tx.is_debit() && line.side == Side::Debit);
        if matches_side {
            return StatementReading::Debt {
                account: line.account.to_string(),
                label: line.label.clone(),
            };
        }
    }
    StatementReading::Expense
}

// ---------------------------------------------------------------------------------------------
// Identité
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IdentityCard {
    pub name: Option<String>,
    pub legal_form: Option<String>,
    pub capital: Option<Money>,
    pub street: Option<String>,
    pub postal_code: Option<String>,
    pub city: Option<String>,
    pub president_name: Option<String>,
    pub sole_shareholder_name: Option<String>,
    pub siren: Option<String>,
    pub year_end: Option<FiscalYearEnd>,
    pub vat_regime: Option<VatRegime>,
}

/// # Errors
pub fn society_identity(conn: &Connection) -> Result<IdentityCard, AppError> {
    let Some(p) = company_profile(conn)? else {
        return Ok(IdentityCard {
            name: None,
            legal_form: None,
            capital: None,
            street: None,
            postal_code: None,
            city: None,
            president_name: None,
            sole_shareholder_name: None,
            siren: None,
            year_end: None,
            vat_regime: None,
        });
    };
    Ok(IdentityCard {
        name: Some(p.name),
        legal_form: Some(p.legal_form),
        capital: p.share_capital,
        street: Some(p.address.street).filter(|s| !s.is_empty()),
        postal_code: Some(p.address.postal_code).filter(|s| !s.is_empty()),
        city: Some(p.address.city).filter(|s| !s.is_empty()),
        president_name: p.president_name,
        sole_shareholder_name: p.sole_shareholder_name,
        siren: Some(p.siren.to_string()),
        year_end: p.fiscal_year_end,
        vat_regime: p.vat_regime,
    })
}

#[cfg(test)]
#[allow(clippy::too_many_lines)]
mod tests {
    use time::Month as TimeMonth;

    use super::*;
    use crate::app::{Actor, ExecutionContext, Executor, Outcome};
    use crate::billing::{EmitInvoice, ImportBankTransactions, ParsedTransaction};
    use crate::clients::CreateClient;
    use crate::company::SetCompanyProfile;
    use crate::domain::{
        Address, ExpenseCategory, InvoiceLine, OpeningBalanceLine, Probability, Siren, VatRate,
        VatRegime,
    };
    use crate::expenses::RecordExpense;
    use crate::fiscal::FiscalDeadlineKind;
    use crate::opening_balance::RecordOpeningBalance;
    use crate::prospection::CreateOpportunity;
    use crate::store::{Passphrase, Store};

    fn date(year: i32, month: TimeMonth, day: u8) -> Date {
        Date::from_calendar_date(year, month, day).unwrap()
    }

    fn today() -> Date {
        date(2026, TimeMonth::September, 5)
    }

    fn test_store(label: &str) -> Store {
        let dir = std::env::temp_dir().join(format!(
            "freeflow-society-test-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ));
        Store::create(&dir.join("vault.db"), &Passphrase::from("s3cret")).unwrap()
    }

    fn human() -> ExecutionContext {
        ExecutionContext::new(Actor::Human, false)
    }

    fn applied<T: std::fmt::Debug>(outcome: Outcome<T>) -> T {
        match outcome {
            Outcome::Applied(v) => v,
            other => panic!("expected Applied, got {other:?}"),
        }
    }

    fn set_profile(store: &mut Store) {
        set_profile_with_regime(store, VatRegime::RealNormalQuarterly);
    }

    fn set_monthly_profile(store: &mut Store) {
        set_profile_with_regime(store, VatRegime::RealNormalMonthly);
    }

    fn set_vault_started_on(store: &Store, on: Date) {
        store
            .connection()
            .execute(
                "INSERT INTO setup (id, declared_new_company, updated_at, created_on)
                 VALUES (1, 0, '2026-01-01T00:00:00Z', ?1)
                 ON CONFLICT (id) DO UPDATE SET created_on = excluded.created_on",
                rusqlite::params![crate::domain::format_date(on)],
            )
            .unwrap();
    }

    fn set_profile_with_regime(store: &mut Store, vat_regime: VatRegime) {
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
                        vat_regime: Some(vat_regime),
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

    fn set_opening(store: &mut Store, lines: &[&str]) {
        set_opening_with_tax(store, lines, None, None);
    }

    fn set_opening_with_tax(
        store: &mut Store,
        lines: &[&str],
        prior_corporate_tax: Option<Money>,
        prior_vat_due: Option<Money>,
    ) {
        applied(
            Executor::new(store)
                .execute(
                    &RecordOpeningBalance {
                        opens_on: date(2025, TimeMonth::October, 1),
                        source: Some("cabinet".into()),
                        lines: lines
                            .iter()
                            .map(|l| l.parse::<OpeningBalanceLine>().unwrap())
                            .collect(),
                        tax_losses: Money::ZERO,
                        prior_corporate_tax,
                        prior_vat_due,
                    },
                    &human(),
                )
                .unwrap(),
        );
    }

    fn burn(store: &mut Store, monthly_cents: i64) {
        for month in [TimeMonth::June, TimeMonth::July, TimeMonth::August] {
            applied(
                Executor::new(store)
                    .execute(
                        &RecordExpense {
                            label: "loyer".into(),
                            category: ExpenseCategory::Software,
                            amount: Money::from_cents(monthly_cents),
                            vat_rate: VatRate::Zero,
                            vat_deductible: Money::ZERO,
                            incurred_on: date(2026, month, 15),
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
        }
    }

    #[test]
    fn twenty_eight_hundred_this_month_keep_three_months_of_runway() {
        // Banque 12 400,00 €. Brûlage 3 200,00 € / mois (trois loyers identiques).
        // Trois mois de piste = 9 600,00 €. Possible = 12 400 − 9 600 = 2 800,00 €.
        // Coût société au taux par défaut 57 % : 2 800 + 1 596 = 4 396,00 €.
        // 1 000 € de dividendes aux taux 2026 : 1 000 + 128 + 186 = 1 314,00 €.
        let mut store = test_store("pay-2800");
        set_profile(&mut store);
        set_opening(
            &mut store,
            &["101000:Capital:C:12400.00", "512000:Banque:D:12400.00"],
        );
        burn(&mut store, 320_000);
        let pay = pay_yourself(store.connection(), today()).unwrap();
        assert_eq!(pay.bank, Money::from_cents(1_240_000));
        assert_eq!(pay.monthly_burn, Money::from_cents(320_000));
        assert_eq!(pay.possible, Money::from_cents(280_000));
        assert_eq!(pay.runway_kept_months, 3);
        assert_eq!(pay.salary.net, Money::from_cents(280_000));
        assert_eq!(pay.salary.company_cost, Money::from_cents(439_600));
        assert_eq!(
            pay.dividend,
            DividendDoor::Closed {
                reason: DividendClosed::YearNotClosed
            }
        );
        assert_eq!(pay.dividend_cost_per_thousand, Money::from_cents(131_400));
        assert_eq!(pay.annual.target, Some(Money::from_cents(3_600_000)));
        assert_eq!(pay.annual.paid, Money::ZERO);
    }

    #[test]
    fn an_empty_vault_has_nothing_to_pay_and_a_closed_dividend() {
        let store = test_store("empty");
        let pay = pay_yourself(store.connection(), today()).unwrap();
        assert_eq!(pay.possible, Money::ZERO);
        assert_eq!(
            pay.dividend,
            DividendDoor::Closed {
                reason: DividendClosed::YearNotClosed
            }
        );
    }

    #[test]
    fn leroy_is_a_settled_debt_not_a_charge_of_the_year() {
        let mut store = test_store("leroy");
        set_profile(&mut store);
        set_opening(
            &mut store,
            &[
                "101000:Capital:C:1000.00",
                "401000:Cabinet Leroy:C:1200.00",
                "512000:Banque:D:2200.00",
            ],
        );
        applied(
            Executor::new(&mut store)
                .execute(
                    &ImportBankTransactions {
                        transactions: vec![ParsedTransaction {
                            occurred_on: date(2026, TimeMonth::September, 4),
                            amount_cents: -120_000,
                            description: "Cabinet Leroy".into(),
                            fitid: None,
                        }],
                    },
                    &human(),
                )
                .unwrap(),
        );
        let moves = statement_moves(store.connection(), today()).unwrap();
        assert_eq!(moves.len(), 1);
        assert_eq!(moves[0].amount, Money::from_cents(-120_000));
        assert_eq!(
            moves[0].suggested,
            StatementReading::Debt {
                account: "401000".into(),
                label: "Cabinet Leroy".into(),
            }
        );
    }

    #[test]
    fn the_landscape_marks_today_and_the_dotted_line_if_atlas_pays() {
        let mut store = test_store("landscape");
        set_profile(&mut store);
        set_opening(
            &mut store,
            &["101000:Capital:C:15000.00", "512000:Banque:D:15000.00"],
        );
        burn(&mut store, 321_075);
        let atlas = applied(
            Executor::new(&mut store)
                .execute(
                    &CreateClient {
                        name: "Atlas Digital".into(),
                        siren: None,
                        vat_number: None,
                        address: None,
                    },
                    &human(),
                )
                .unwrap(),
        );
        applied(
            Executor::new(&mut store)
                .execute(
                    &EmitInvoice {
                        client_id: atlas,
                        mission_id: None,
                        lines: vec![InvoiceLine {
                            description: "régie août".into(),
                            quantity: 1.0,
                            unit_price: Money::from_cents(620_000),
                            vat_rate: VatRate::Zero,
                        }],
                        issued_on: date(2026, TimeMonth::August, 7),
                        payment_terms_days: 30,
                    },
                    &human(),
                )
                .unwrap(),
        );
        let landscape = society_landscape(store.connection(), today()).unwrap();
        assert_eq!(landscape.bank, Money::from_cents(1_500_000));
        assert_eq!(landscape.today, Month::new(2026, 9).unwrap());
        assert_eq!(landscape.from, Month::new(2025, 10).unwrap());
        assert_eq!(landscape.days_to_year_end, Some(25));
        let expected = landscape.expected.expect("Atlas");
        assert_eq!(expected.party, "Atlas Digital");
        assert_eq!(expected.amount, Money::from_cents(620_000));
        let today_pt = landscape
            .points
            .iter()
            .find(|p| p.month == landscape.today)
            .expect("point d'aujourd'hui");
        assert_eq!(today_pt.cash, today_pt.cash_if_received);
        let after = landscape
            .points
            .iter()
            .find(|p| p.month > landscape.today)
            .expect("un mois après");
        assert!(
            after.cash_if_received.cents() > after.cash.cents(),
            "si Atlas paie, la ligne pointillée est au-dessus : {after:?}"
        );
    }

    #[test]
    fn september_is_thin_when_only_two_names_are_open() {
        let mut store = test_store("bars");
        set_profile(&mut store);
        let atelier = applied(
            Executor::new(&mut store)
                .execute(
                    &CreateClient {
                        name: "Atelier Nord".into(),
                        siren: None,
                        vat_number: None,
                        address: None,
                    },
                    &human(),
                )
                .unwrap(),
        );
        let hume = applied(
            Executor::new(&mut store)
                .execute(
                    &CreateClient {
                        name: "Hume".into(),
                        siren: None,
                        vat_number: None,
                        address: None,
                    },
                    &human(),
                )
                .unwrap(),
        );
        applied(
            Executor::new(&mut store)
                .execute(
                    &CreateOpportunity {
                        client_id: atelier,
                        name: "le devis Atelier Nord".into(),
                        amount: Money::from_cents(840_000),
                        probability: Probability::new(40).unwrap(),
                        next_action_at: today(),
                        source: None,
                    },
                    &human(),
                )
                .unwrap(),
        );
        applied(
            Executor::new(&mut store)
                .execute(
                    &CreateOpportunity {
                        client_id: hume,
                        name: "le devis Hume".into(),
                        amount: Money::from_cents(400_000),
                        probability: Probability::new(20).unwrap(),
                        next_action_at: today(),
                        source: None,
                    },
                    &human(),
                )
                .unwrap(),
        );
        let year = year_conversations(store.connection(), today()).unwrap();
        assert_eq!(year.bars.len(), 12);
        assert_eq!(year.bars[0].month, Month::new(2025, 10).unwrap());
        let sept = year.bars.iter().find(|b| b.now).expect("septembre");
        assert_eq!(sept.month, Month::new(2026, 9).unwrap());
        assert_eq!(sept.count, 2);
        assert!(sept.thin);
    }

    #[test]
    fn the_home_carries_identity_landscape_and_chapter_cues() {
        let mut store = test_store("home");
        set_profile(&mut store);
        let home = society_home(store.connection(), today()).unwrap();
        assert_eq!(home.identity.name.as_deref(), Some("Lumen Conseil"));
        assert_eq!(home.identity.days_to_year_end, Some(25));
        assert_eq!(
            home.pay.dividend,
            DividendDoor::Closed {
                reason: DividendClosed::YearNotClosed
            }
        );
        assert_eq!(home.landscape.days_to_year_end, Some(25));
    }

    #[test]
    fn is_acompte_briefing_is_a_quarter_of_prior_is() {
        let mut store = test_store("is-acompte-quarter");
        set_profile(&mut store);
        set_opening_with_tax(
            &mut store,
            &["101000:Capital:C:1000.00", "512000:Banque:D:1000.00"],
            Some(Money::from_cents(400_000)),
            None,
        );
        let briefing = duty_briefing(
            store.connection(),
            FiscalDeadlineKind::IsAcompte,
            today(),
            None,
        )
        .unwrap();
        assert_eq!(briefing.kind, FiscalDeadlineKind::IsAcompte);
        assert_eq!(briefing.due_on, date(2026, TimeMonth::September, 15));
        assert_eq!(
            briefing.amount,
            AmountStory::Due {
                amount: Money::from_cents(100_000),
                basis: AmountBasis::QuarterOfPriorIs,
            }
        );
        assert_eq!(briefing.form, Some("2571"));
        assert_eq!(briefing.path, &["Déclarer", "Impôt sur les sociétés"][..]);
        assert!(briefing.expect.contains(&Expect::CheckPrefill));
        assert!(briefing.expect.contains(&Expect::ModulateDown));
        assert!(briefing.url.contains("impots.gouv.fr"));
    }

    #[test]
    fn is_acompte_below_three_thousand_is_a_waiver() {
        let mut store = test_store("is-acompte-waiver");
        set_profile(&mut store);
        set_opening_with_tax(
            &mut store,
            &["101000:Capital:C:1000.00", "512000:Banque:D:1000.00"],
            Some(Money::from_cents(200_000)),
            None,
        );
        let briefing = duty_briefing(
            store.connection(),
            FiscalDeadlineKind::IsAcompte,
            today(),
            None,
        )
        .unwrap();
        assert_eq!(
            briefing.amount,
            AmountStory::Waiver {
                reason: WaiverReason::PriorIsBelowThreshold,
            }
        );
    }

    #[test]
    fn is_acompte_without_prior_is_is_unknown_not_zero() {
        let mut store = test_store("is-acompte-unknown");
        set_profile(&mut store);
        set_opening(
            &mut store,
            &["101000:Capital:C:1000.00", "512000:Banque:D:1000.00"],
        );
        let briefing = duty_briefing(
            store.connection(),
            FiscalDeadlineKind::IsAcompte,
            today(),
            None,
        )
        .unwrap();
        assert_eq!(
            briefing.amount,
            AmountStory::Unknown {
                reason: UnknownReason::PriorIsMissing,
            }
        );
    }

    #[test]
    fn ca3_at_zero_is_still_due_and_must_be_filed() {
        let mut store = test_store("ca3-zero");
        set_profile(&mut store);
        let briefing =
            duty_briefing(store.connection(), FiscalDeadlineKind::Ca3, today(), None).unwrap();
        assert_eq!(
            briefing.amount,
            AmountStory::Due {
                amount: Money::ZERO,
                basis: AmountBasis::VatForPeriod,
            }
        );
        assert_eq!(briefing.form, Some("CA3"));
        assert!(briefing.expect.contains(&Expect::FileEvenIfZero));
    }

    #[test]
    fn cfe_amount_is_on_the_notice_not_here() {
        let mut store = test_store("cfe");
        set_profile(&mut store);
        let briefing =
            duty_briefing(store.connection(), FiscalDeadlineKind::Cfe, today(), None).unwrap();
        assert_eq!(briefing.amount, AmountStory::External);
        assert!(briefing.expect.contains(&Expect::NoticeInSpace));
    }

    #[test]
    fn dsn_is_not_your_hands() {
        let mut store = test_store("dsn");
        set_profile(&mut store);
        let briefing =
            duty_briefing(store.connection(), FiscalDeadlineKind::Dsn, today(), None).unwrap();
        assert!(matches!(briefing.amount, AmountStory::NotYourHands { .. }));
        assert!(briefing.expect.contains(&Expect::PayrollExpertDoesIt));
        assert_eq!(briefing.form, None);
    }

    #[test]
    fn approval_meeting_has_no_external_briefing() {
        let store = test_store("approval");
        let err = duty_briefing(
            store.connection(),
            FiscalDeadlineKind::ApprovalMeeting,
            today(),
            None,
        )
        .unwrap_err();
        assert!(err.to_string().contains("pas sur un site"), "{err}");
    }

    #[test]
    fn empty_vault_still_has_the_is_acompte_path() {
        let store = test_store("empty-briefing");
        let briefing = duty_briefing(
            store.connection(),
            FiscalDeadlineKind::IsAcompte,
            today(),
            None,
        )
        .unwrap();
        assert_eq!(
            briefing.amount,
            AmountStory::Unknown {
                reason: UnknownReason::NoProfile,
            }
        );
        assert_eq!(briefing.form, Some("2571"));
        assert_eq!(briefing.path, &["Déclarer", "Impôt sur les sociétés"][..]);
        assert_eq!(briefing.boxes.len(), 2);
        assert_eq!(briefing.boxes[0].case, "03");
        assert_eq!(briefing.boxes[0].amount, None);
        assert_eq!(briefing.coverage, BoxCoverage::Incomplete);
    }

    #[test]
    fn is_acompte_at_zero_names_boxes_03_and_10() {
        let mut store = test_store("is-boxes-zero");
        set_profile(&mut store);
        set_opening_with_tax(
            &mut store,
            &["101000:Capital:C:1000.00", "512000:Banque:D:1000.00"],
            Some(Money::from_cents(200_000)),
            None,
        );
        let briefing = duty_briefing(
            store.connection(),
            FiscalDeadlineKind::IsAcompte,
            today(),
            None,
        )
        .unwrap();
        assert_eq!(
            briefing.amount,
            AmountStory::Waiver {
                reason: WaiverReason::PriorIsBelowThreshold,
            }
        );
        assert_eq!(briefing.boxes[0].case, "03");
        assert_eq!(briefing.boxes[0].amount, Some(Money::ZERO));
        assert_eq!(briefing.boxes[0].role, BoxRole::Fill);
        assert_eq!(briefing.boxes[1].case, "10");
        assert_eq!(briefing.boxes[1].role, BoxRole::SiteComputes);
        assert_eq!(briefing.coverage, BoxCoverage::Complete);
        assert!(briefing.filed_on.is_none());
    }

    #[test]
    fn marking_a_duty_filed_is_visible_on_the_briefing_and_retractable() {
        let mut store = test_store("duty-filed");
        set_profile(&mut store);
        set_opening_with_tax(
            &mut store,
            &["101000:Capital:C:1000.00", "512000:Banque:D:1000.00"],
            Some(Money::from_cents(200_000)),
            None,
        );
        let briefing = duty_briefing(
            store.connection(),
            FiscalDeadlineKind::IsAcompte,
            today(),
            None,
        )
        .unwrap();
        applied(
            Executor::new(&mut store)
                .execute(
                    &MarkDutyFiled {
                        kind: FiscalDeadlineKind::IsAcompte,
                        period_key: briefing.period_key.clone(),
                        due_on: briefing.due_on,
                        filed_on: today(),
                    },
                    &human(),
                )
                .unwrap(),
        );
        let after = duty_briefing(
            store.connection(),
            FiscalDeadlineKind::IsAcompte,
            today(),
            Some(briefing.period_key.as_str()),
        )
        .unwrap();
        assert_eq!(after.filed_on, Some(today()));
        let duties = society_duties(store.connection(), today()).unwrap();
        let row = duties
            .iter()
            .find(|d| {
                d.kind == FiscalDeadlineKind::IsAcompte && d.period_key == briefing.period_key
            })
            .expect("acompte");
        assert_eq!(row.filed_on, Some(today()));

        applied(
            Executor::new(&mut store)
                .execute(
                    &RetractDutyFiled {
                        kind: FiscalDeadlineKind::IsAcompte,
                        period_key: briefing.period_key,
                    },
                    &human(),
                )
                .unwrap(),
        );
        let retracted = duty_briefing(
            store.connection(),
            FiscalDeadlineKind::IsAcompte,
            today(),
            None,
        )
        .unwrap();
        assert!(retracted.filed_on.is_none());
    }

    #[test]
    fn a_filed_acompte_stays_visible_after_its_due_date() {
        let mut store = test_store("duty-filed-past");
        set_profile(&mut store);
        let due = date(2026, TimeMonth::September, 15);
        applied(
            Executor::new(&mut store)
                .execute(
                    &MarkDutyFiled {
                        kind: FiscalDeadlineKind::IsAcompte,
                        period_key: crate::domain::format_date(due),
                        due_on: due,
                        filed_on: today(),
                    },
                    &human(),
                )
                .unwrap(),
        );
        let later = date(2026, TimeMonth::September, 16);
        let duties = society_duties(store.connection(), later).unwrap();
        let row = duties
            .iter()
            .find(|d| d.kind == FiscalDeadlineKind::IsAcompte && d.filed_on.is_some())
            .expect("déposé reste");
        assert_eq!(row.due_on, due);
        assert_eq!(row.filed_on, Some(today()));
    }

    #[test]
    fn ca3_boxes_fill_from_invoices_and_expenses() {
        let mut store = test_store("ca3-boxes");
        set_profile(&mut store);
        let client_id = applied(
            Executor::new(&mut store)
                .execute(
                    &CreateClient {
                        name: "Atelier Nord".into(),
                        siren: None,
                        vat_number: None,
                        address: None,
                    },
                    &human(),
                )
                .unwrap(),
        );
        applied(
            Executor::new(&mut store)
                .execute(
                    &EmitInvoice {
                        client_id,
                        mission_id: None,
                        lines: vec![InvoiceLine {
                            description: "prestation".into(),
                            quantity: 1.0,
                            unit_price: Money::from_cents(100_000),
                            vat_rate: VatRate::Standard,
                        }],
                        issued_on: date(2026, TimeMonth::August, 10),
                        payment_terms_days: 30,
                    },
                    &human(),
                )
                .unwrap(),
        );
        applied(
            Executor::new(&mut store)
                .execute(
                    &RecordExpense {
                        label: "logiciel".into(),
                        category: ExpenseCategory::Software,
                        amount: Money::from_cents(12_000),
                        vat_rate: VatRate::Standard,
                        vat_deductible: Money::from_cents(2_000),
                        incurred_on: date(2026, TimeMonth::August, 12),
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
        let briefing =
            duty_briefing(store.connection(), FiscalDeadlineKind::Ca3, today(), None).unwrap();
        assert_eq!(briefing.coverage, BoxCoverage::Complete);
        let box_of = |case: &str| {
            briefing
                .boxes
                .iter()
                .find(|b| b.case == case)
                .unwrap_or_else(|| panic!("case {case}"))
        };
        assert_eq!(box_of("02").amount, Some(Money::from_cents(100_000)));
        assert_eq!(box_of("08").amount, Some(Money::from_cents(20_000)));
        assert_eq!(box_of("20").amount, Some(Money::from_cents(2_000)));
        assert_eq!(box_of("28").amount, Some(Money::from_cents(18_000)));
    }

    #[test]
    fn ca3_before_opening_balance_has_boxes_without_amounts() {
        let mut store = test_store("ca3-incomplete");
        set_profile(&mut store);
        set_opening(
            &mut store,
            &["101000:Capital:C:1000.00", "512000:Banque:D:1000.00"],
        );
        let early = date(2025, TimeMonth::September, 5);
        let briefing =
            duty_briefing(store.connection(), FiscalDeadlineKind::Ca3, early, None).unwrap();
        assert_eq!(briefing.coverage, BoxCoverage::Incomplete);
        assert_eq!(
            briefing.amount,
            AmountStory::Unknown {
                reason: UnknownReason::PeriodNotInVault,
            }
        );
        assert!(briefing.boxes.iter().all(|b| b.amount.is_none()));
        assert!(briefing.boxes.iter().any(|b| b.case == "08"));
    }

    #[test]
    fn a_vat_carry_in_seeds_the_next_ca3_credit_box() {
        let mut store = test_store("vat-carry-seed");
        set_monthly_profile(&mut store);
        applied(
            Executor::new(&mut store)
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
        let today = date(2026, TimeMonth::September, 8);
        let september = duty_briefing(
            store.connection(),
            FiscalDeadlineKind::Ca3,
            today,
            Some("2026-09"),
        )
        .unwrap();
        assert_eq!(september.coverage, BoxCoverage::Complete);
        let sept_22 = september
            .boxes
            .iter()
            .find(|x| x.case == "22")
            .expect("case 22");
        let sept_25 = september
            .boxes
            .iter()
            .find(|x| x.case == "25")
            .expect("case 25");
        let sept_27 = september
            .boxes
            .iter()
            .find(|x| x.case == "27")
            .expect("case 27");
        assert_eq!(sept_22.amount, Some(Money::from_cents(32_400)));
        assert_eq!(sept_25.amount, Some(Money::from_cents(32_400)));
        assert_eq!(
            sept_27.amount,
            Some(Money::from_cents(32_400)),
            "sans fait en septembre, le crédit se reporte"
        );
        assert!(
            september.boxes.iter().all(|b| b.case != "26"),
            "pas de 26 : {:?}",
            september.boxes
        );
        assert!(
            september.boxes.iter().all(|b| b.case != "28"),
            "rien à reverser : {:?}",
            september.boxes
        );

        let august = duty_briefing(
            store.connection(),
            FiscalDeadlineKind::Ca3,
            today,
            Some("2026-08"),
        )
        .unwrap();
        assert!(
            august
                .boxes
                .iter()
                .all(|b| b.case != "25" || b.amount != Some(Money::from_cents(32_400))),
            "août est la période déjà déposée, pas celle qui reçoit le crédit : {:?}",
            august.boxes
        );
    }

    #[test]
    fn a_vat_carry_in_chains_into_the_following_period() {
        let mut store = test_store("vat-carry-chain");
        set_monthly_profile(&mut store);
        applied(
            Executor::new(&mut store)
                .execute(
                    &RecordVatCarryIn {
                        after_period: "2026-08".into(),
                        credit: Money::from_cents(32_400),
                        source: None,
                    },
                    &human(),
                )
                .unwrap(),
        );
        applied(
            Executor::new(&mut store)
                .execute(
                    &RecordExpense {
                        label: "hébergement".into(),
                        category: ExpenseCategory::Software,
                        amount: Money::from_cents(12_000),
                        vat_rate: VatRate::Standard,
                        vat_deductible: Money::from_cents(2_000),
                        incurred_on: date(2026, TimeMonth::September, 12),
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
        let october = duty_briefing(
            store.connection(),
            FiscalDeadlineKind::Ca3,
            date(2026, TimeMonth::November, 5),
            Some("2026-10"),
        )
        .unwrap();
        let box_of = |case: &str| {
            october
                .boxes
                .iter()
                .find(|b| b.case == case)
                .unwrap_or_else(|| panic!("case {case}"))
        };
        assert_eq!(
            box_of("22").amount,
            Some(Money::from_cents(34_400)),
            "324 € repris + 20 € de septembre : {:?}",
            october.boxes
        );
        assert_eq!(box_of("25").amount, Some(Money::from_cents(34_400)));
        assert_eq!(box_of("27").amount, Some(Money::from_cents(34_400)));
    }

    #[test]
    fn a_monthly_ca3_filing_does_not_mark_the_overdue_one() {
        let mut store = test_store("ca3-monthly-two");
        set_monthly_profile(&mut store);
        let today = date(2026, TimeMonth::September, 8);
        let duties = society_duties(store.connection(), today).unwrap();
        let ca3: Vec<_> = duties
            .iter()
            .filter(|d| d.kind == FiscalDeadlineKind::Ca3)
            .collect();
        assert!(
            ca3.len() >= 2,
            "août en retard et septembre à venir : {ca3:?}"
        );
        let august = ca3
            .iter()
            .find(|d| d.due_on.month() == TimeMonth::August)
            .expect("août");
        let september = ca3
            .iter()
            .find(|d| d.due_on.month() == TimeMonth::September)
            .expect("septembre");
        assert_ne!(august.period_key, september.period_key);
        assert!(august.due_on < today && august.filed_on.is_none());
        assert!(september.due_on >= today && september.filed_on.is_none());
        let august_key = august.period_key.clone();
        let september_key = september.period_key.clone();
        let august_due = august.due_on;

        let next = duty_briefing(store.connection(), FiscalDeadlineKind::Ca3, today, None).unwrap();
        assert_eq!(next.period_key, september_key);

        applied(
            Executor::new(&mut store)
                .execute(
                    &MarkDutyFiled {
                        kind: FiscalDeadlineKind::Ca3,
                        period_key: next.period_key.clone(),
                        due_on: next.due_on,
                        filed_on: today,
                    },
                    &human(),
                )
                .unwrap(),
        );

        let after = society_duties(store.connection(), today).unwrap();
        let august = after.iter().find(|d| d.period_key == august_key).unwrap();
        let september = after
            .iter()
            .find(|d| d.period_key == september_key)
            .unwrap();
        assert!(
            august.filed_on.is_none(),
            "août n'a pas été déposé : {august:?}"
        );
        assert_eq!(september.filed_on, Some(today));

        let overdue = duty_briefing(
            store.connection(),
            FiscalDeadlineKind::Ca3,
            today,
            Some(august_key.as_str()),
        )
        .unwrap();
        assert_eq!(overdue.due_on, august_due);
        assert!(overdue.filed_on.is_none());
        assert_eq!(overdue.period_key, august_key);
    }

    #[test]
    fn briefing_without_period_does_not_reuse_a_filed_is_acompte() {
        let mut store = test_store("is-acompte-next");
        set_profile(&mut store);
        let due = date(2026, TimeMonth::September, 15);
        applied(
            Executor::new(&mut store)
                .execute(
                    &MarkDutyFiled {
                        kind: FiscalDeadlineKind::IsAcompte,
                        period_key: crate::domain::format_date(due),
                        due_on: due,
                        filed_on: today(),
                    },
                    &human(),
                )
                .unwrap(),
        );
        let later = date(2026, TimeMonth::September, 16);
        let filed = duty_briefing(
            store.connection(),
            FiscalDeadlineKind::IsAcompte,
            later,
            Some("2026-09-15"),
        )
        .unwrap();
        assert_eq!(filed.filed_on, Some(today()));
        assert_eq!(filed.due_on, due);

        let next = duty_briefing(
            store.connection(),
            FiscalDeadlineKind::IsAcompte,
            later,
            None,
        )
        .unwrap();
        assert!(
            next.filed_on.is_none(),
            "le 15 décembre n'est pas déposé : {next:?}"
        );
        assert!(next.due_on > due);
    }

    #[test]
    fn next_duty_skips_what_is_already_filed() {
        let mut store = test_store("next-duty-open");
        set_monthly_profile(&mut store);
        let today = date(2026, TimeMonth::September, 8);
        let duties = society_duties(store.connection(), today).unwrap();
        let first_ca3 = duties
            .iter()
            .find(|d| d.kind == FiscalDeadlineKind::Ca3)
            .unwrap();
        let first_key = first_ca3.period_key.clone();
        applied(
            Executor::new(&mut store)
                .execute(
                    &MarkDutyFiled {
                        kind: first_ca3.kind,
                        period_key: first_key.clone(),
                        due_on: first_ca3.due_on,
                        filed_on: today,
                    },
                    &human(),
                )
                .unwrap(),
        );
        let home = society_home(store.connection(), today).unwrap();
        let next = home.next_duty.expect("encore une échéance ouverte");
        assert!(next.filed_on.is_none());
        assert_ne!(next.period_key, first_key);
    }

    #[test]
    fn an_unfiled_ca3_older_than_two_months_stays_on_the_list() {
        let mut store = test_store("ca3-old-overdue");
        set_monthly_profile(&mut store);
        let today = date(2026, TimeMonth::November, 10);
        let duties = society_duties(store.connection(), today).unwrap();
        assert!(
            duties.iter().any(|d| {
                d.kind == FiscalDeadlineKind::Ca3
                    && d.due_on.month() == TimeMonth::August
                    && d.filed_on.is_none()
            }),
            "août non déposé ne disparaît pas : {duties:?}"
        );
    }

    #[test]
    fn marking_an_overdue_ca3_clears_it_from_unfiled() {
        let mut store = test_store("ca3-overdue-filed");
        set_monthly_profile(&mut store);
        set_vault_started_on(&store, date(2026, TimeMonth::January, 1));
        let today = date(2026, TimeMonth::September, 8);
        let duties = society_duties(store.connection(), today).unwrap();
        let august = duties
            .iter()
            .find(|d| d.kind == FiscalDeadlineKind::Ca3 && d.due_on.month() == TimeMonth::August)
            .expect("août");
        assert!(august.filed_on.is_none());
        assert!(
            !august.catch_up,
            "coffre ouvert en janvier : août est un retard"
        );
        let key = august.period_key.clone();
        let due = august.due_on;
        applied(
            Executor::new(&mut store)
                .execute(
                    &MarkDutyFiled {
                        kind: FiscalDeadlineKind::Ca3,
                        period_key: key.clone(),
                        due_on: due,
                        filed_on: today,
                    },
                    &human(),
                )
                .unwrap(),
        );
        let after = society_duties(store.connection(), today).unwrap();
        let august = after
            .iter()
            .find(|d| d.period_key == key)
            .expect("août toujours listé");
        assert_eq!(august.filed_on, Some(today));
        assert!(
            after
                .iter()
                .filter(|d| d.kind == FiscalDeadlineKind::Ca3
                    && d.filed_on.is_none()
                    && d.due_on < today
                    && !d.catch_up)
                .all(|d| d.period_key != key),
            "août n'est plus un retard : {after:?}"
        );
    }

    #[test]
    fn duties_before_vault_start_are_catch_up_not_overdue() {
        let mut store = test_store("ca3-catch-up");
        set_monthly_profile(&mut store);
        set_vault_started_on(&store, date(2026, TimeMonth::September, 8));
        let today = date(2026, TimeMonth::September, 8);
        let duties = society_duties(store.connection(), today).unwrap();
        let august = duties
            .iter()
            .find(|d| d.kind == FiscalDeadlineKind::Ca3 && d.due_on.month() == TimeMonth::August)
            .expect("août");
        assert!(august.catch_up);
        assert!(august.filed_on.is_none());
        let home = society_home(store.connection(), today).unwrap();
        assert!(
            home.next_duty
                .as_ref()
                .is_none_or(|d| !d.catch_up && d.due_on >= today),
            "le prochain geste ignore le rattrapage : {:?}",
            home.next_duty
        );
    }

    #[test]
    fn catch_up_at_due_clears_pre_vault_duties() {
        let mut store = test_store("catch-up-bulk");
        set_monthly_profile(&mut store);
        let started = date(2026, TimeMonth::September, 8);
        set_vault_started_on(&store, started);
        let n = applied(
            Executor::new(&mut store)
                .execute(&MarkCatchUpFiled { before: started }, &human())
                .unwrap(),
        );
        assert!(n > 0, "au moins août");
        let duties = society_duties(store.connection(), started).unwrap();
        assert!(
            duties
                .iter()
                .filter(|d| d.catch_up)
                .all(|d| d.filed_on.is_some()),
            "plus rien à rattraper : {duties:?}"
        );
    }
}
