//! La société : paysage, se payer, relevé, chapitres — lectures pures, aucune commande.
//!
//! Lot 51. Le cœur compose des faits typés ; c'est la fenêtre (et le rendu CLI) qui rédige le
//! français. `today` est un argument d'adaptateur, jamais lu ici.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use time::Date;

use crate::accounting::compute_result;
use crate::app::AppError;
use crate::billing::{aged_balance, list_bank_transactions, list_invoices};
use crate::clients::client_by_id;
use crate::closing::{ClosingStage, ClosingStepKey, StepStatus, closing_checklist};
use crate::company::company_profile;
use crate::day::cash_in_bank;
use crate::domain::{
    BankTransaction, BankTransactionId, FiscalYearEnd, Money, Month, Side, VatRegime,
};
use crate::fiscal::{
    DAS2_THRESHOLD, DIVIDEND_INCOME_TAX_BPS, FiscalDeadlineKind, IS_ACOMPTE_DISPENSATION,
    dividend_social_charges_bps, fiscal_calendar, next_cfe, next_is_acompte,
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
        next_duty: duties.into_iter().next(),
        closing,
        unmatched,
    })
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

/// # Errors
pub fn society_duties(conn: &Connection, today: Date) -> Result<Vec<Duty>, AppError> {
    let calendar = fiscal_calendar(conn, today)?;
    Ok(calendar
        .into_iter()
        .map(|d| Duty {
            kind: d.kind,
            due_on: d.due_on,
            amount: d.amount,
            deposit: DepositPlace::of(d.kind),
        })
        .collect())
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
/// # Errors
///
/// [`AppError::Domain`] si la démarche se fait ici (`ApprovalMeeting`), ou une erreur de lecture
/// du calendrier.
pub fn duty_briefing(
    conn: &Connection,
    kind: FiscalDeadlineKind,
    today: Date,
) -> Result<DutyBriefing, AppError> {
    let Some(meta) = briefing_meta(kind) else {
        return Err(AppError::Domain(
            "l'approbation des comptes se fait ici, pas sur un site d'État".into(),
        ));
    };
    let calendar = fiscal_calendar(conn, today)?;
    let found = calendar.iter().find(|d| d.kind == kind);
    let due_on = found.map_or_else(|| fallback_due(kind, today), |d| d.due_on);
    let cal_amount = found.and_then(|d| d.amount);
    let amount = amount_story(conn, kind, today, cal_amount)?;
    Ok(DutyBriefing {
        kind,
        due_on,
        deposit: DepositPlace::of(kind),
        amount,
        form: meta.form,
        url: meta.url,
        path: meta.path,
        expect: meta.expect.to_vec(),
    })
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
                        vat_regime: Some(VatRegime::RealNormalQuarterly),
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
        let briefing =
            duty_briefing(store.connection(), FiscalDeadlineKind::IsAcompte, today()).unwrap();
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
        let briefing =
            duty_briefing(store.connection(), FiscalDeadlineKind::IsAcompte, today()).unwrap();
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
        let briefing =
            duty_briefing(store.connection(), FiscalDeadlineKind::IsAcompte, today()).unwrap();
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
        let briefing = duty_briefing(store.connection(), FiscalDeadlineKind::Ca3, today()).unwrap();
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
        let briefing = duty_briefing(store.connection(), FiscalDeadlineKind::Cfe, today()).unwrap();
        assert_eq!(briefing.amount, AmountStory::External);
        assert!(briefing.expect.contains(&Expect::NoticeInSpace));
    }

    #[test]
    fn dsn_is_not_your_hands() {
        let mut store = test_store("dsn");
        set_profile(&mut store);
        let briefing = duty_briefing(store.connection(), FiscalDeadlineKind::Dsn, today()).unwrap();
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
        )
        .unwrap_err();
        assert!(err.to_string().contains("pas sur un site"), "{err}");
    }

    #[test]
    fn empty_vault_still_has_the_is_acompte_path() {
        let store = test_store("empty-briefing");
        let briefing =
            duty_briefing(store.connection(), FiscalDeadlineKind::IsAcompte, today()).unwrap();
        assert_eq!(
            briefing.amount,
            AmountStory::Unknown {
                reason: UnknownReason::NoProfile,
            }
        );
        assert_eq!(briefing.form, Some("2571"));
        assert_eq!(briefing.path, &["Déclarer", "Impôt sur les sociétés"][..]);
    }
}
