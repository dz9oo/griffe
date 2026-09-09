//! Le jour : mât, gestes, mois — trois lectures, aucune commande.
//!
//! Lot 49. Le cœur compose des faits typés ; c'est la fenêtre (et le rendu CLI) qui rédige le
//! français. `today` est un argument d'adaptateur, jamais lu ici.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use time::Date;

use crate::app::AppError;
use crate::billing::{aged_balance, list_bank_transactions, list_invoices};
use crate::clients::client_by_id;
use crate::company::company_profile;
use crate::domain::{
    ClientId, FollowUpKind, FollowUpSubject, InvoiceId, MissionId, Money, Month, format_date,
};
use crate::fiscal::{FiscalDeadline, FiscalDeadlineKind, fiscal_calendar};
use crate::follow_up::{CardStatus, FollowUpCard, follow_up_board, follow_up_queue};
use crate::forecast::{build_forecast_inputs, first_shortfall_month, forecast_12_months};
use crate::missions::{MissionFilter, list_missions_with};
use crate::opening_balance::opening_balance;
use crate::prospection::{list_interactions, list_open_opportunities, list_opportunities};
use crate::setup::{SetupStatus, SetupStep, setup_status};

/// Piste sous ce seuil (mois) : le fait passe sceau.
pub const RUNWAY_ALARM_MONTHS: u32 = 2;
/// Piste de tant de mois ou moins, pipeline vide : une note, pas encore l'alarme.
pub const RUNWAY_NOTE_MONTHS: u32 = 3;
/// Créance échue depuis au moins autant de jours : le fait « à encaisser » passe sceau.
pub const RECEIVABLE_STALE_DAYS: i64 = 30;
/// Une obligation d'État au-delà n'est plus un geste du jour — elle vit dans le mois.
pub const STATE_DUTY_HORIZON_DAYS: i64 = 30;
/// Trois à cinq gestes, pas vingt.
pub const MAX_GESTURES: usize = 5;
/// Liste courte du mât.
pub const MAX_RECEIVABLES: usize = 3;

// ---------------------------------------------------------------------------------------------
// Mât
// ---------------------------------------------------------------------------------------------

/// Faits du mât : banque, piste, à encaisser, signaux typés. Aucune phrase française.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Mast {
    pub bank: Money,
    /// `None` : pas de brûlage mesurable (charges nulles, trésorerie qui ne descend pas).
    pub runway_months: Option<u32>,
    pub receivables: Vec<Receivable>,
    pub open_conversations: u32,
    pub signals: Vec<MastSignal>,
}

impl Mast {
    /// Le filet du mât passe sceau.
    #[must_use]
    pub fn is_alarming(&self) -> bool {
        self.signals.iter().any(MastSignal::is_alarming)
    }
}

/// Une créance encore due, pour le fait « 6 200 € chez Atlas ».
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Receivable {
    pub party: String,
    pub amount: Money,
    pub days_overdue: i64,
    pub invoice_id: InvoiceId,
    pub client_id: ClientId,
}

/// Trou que la fenêtre rédige en une phrase italique, ou fait alarmant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MastSignal {
    /// Piste sous [`RUNWAY_ALARM_MONTHS`].
    RunwayAlarm { months: u32 },
    /// Piste de [`RUNWAY_NOTE_MONTHS`] mois ou moins, et rien derrière.
    RunwayUnbacked { months: u32 },
    /// Aucune opportunité ouverte.
    PipelineEmpty,
    /// Une seule conversation ouverte.
    PipelineThin { party: String, count: u32 },
    /// Créance échue depuis [`RECEIVABLE_STALE_DAYS`] jours ou plus.
    ReceivableStale { party: String, days: i64 },
}

impl MastSignal {
    #[must_use]
    pub const fn is_alarming(&self) -> bool {
        matches!(
            self,
            Self::RunwayAlarm { .. } | Self::ReceivableStale { .. }
        )
    }
}

/// # Errors
pub fn day_mast(conn: &Connection, today: Date) -> Result<Mast, AppError> {
    let bank = cash_in_bank(conn, today)?;
    let runway_months = runway_months(conn, today, bank)?;
    let receivables = receivables(conn, today)?;
    let conversations = list_open_opportunities(conn)?;
    let open_conversations = u32::try_from(conversations.len()).unwrap_or(u32::MAX);
    let mut signals = Vec::new();

    if let Some(months) = runway_months
        && months < RUNWAY_ALARM_MONTHS
    {
        signals.push(MastSignal::RunwayAlarm { months });
    }

    match open_conversations {
        0 => {
            signals.push(MastSignal::PipelineEmpty);
            if let Some(months) = runway_months
                && months <= RUNWAY_NOTE_MONTHS
            {
                signals.push(MastSignal::RunwayUnbacked { months });
            }
        }
        1 => {
            let opp = &conversations[0];
            let party = name_of_client(conn, opp.client_id, opp.name.clone())?;
            signals.push(MastSignal::PipelineThin { party, count: 1 });
        }
        _ => {}
    }

    if let Some(stale) = receivables
        .iter()
        .find(|r| r.days_overdue >= RECEIVABLE_STALE_DAYS)
    {
        signals.push(MastSignal::ReceivableStale {
            party: stale.party.clone(),
            days: stale.days_overdue,
        });
    }

    Ok(Mast {
        bank,
        runway_months,
        receivables,
        open_conversations,
        signals,
    })
}

/// Banque du jour : 512 d'ouverture + mouvements du relevé jusqu'à `today`.
///
/// # Errors
pub fn cash_in_bank(conn: &Connection, today: Date) -> Result<Money, AppError> {
    let opening = opening_balance(conn)?;
    let opens_on = opening.as_ref().map(|o| o.balance.opens_on);
    let mut cents: i128 = 0;
    if let Some(record) = &opening {
        for line in &record.balance.lines {
            if line.account.starts_with("512") {
                cents += i128::from(line.signed().cents());
            }
        }
    }
    for tx in list_bank_transactions(conn)? {
        if tx.occurred_on > today {
            continue;
        }
        if opens_on.is_none_or(|d| tx.occurred_on >= d) {
            cents += i128::from(tx.amount_cents);
        }
    }
    let cents = i64::try_from(cents).unwrap_or(if cents.is_negative() {
        i64::MIN
    } else {
        i64::MAX
    });
    Ok(Money::from_cents(cents))
}

fn runway_months(conn: &Connection, today: Date, bank: Money) -> Result<Option<u32>, AppError> {
    let mut inputs = build_forecast_inputs(conn, today, bank)?;
    // La piste du mât est le cash en banque moins le brûlage connu. Les créances sont le fait
    // « à encaisser », le pipeline pondéré un signal à part : les compter ici masquerait une
    // piste courte derrière un Atlas pas encore payé.
    inputs.weighted_pipeline = Money::ZERO;
    inputs.unpaid_invoices.clear();
    inputs.pending_mission_revenue.clear();
    if inputs.monthly_known_expenses == Money::ZERO && bank.cents() >= 0 {
        return Ok(None);
    }
    let forecast = forecast_12_months(today, &inputs);
    let months = match first_shortfall_month(&forecast) {
        None => u32::try_from(forecast.len()).unwrap_or(12),
        Some(month) => {
            let first = forecast[0].month;
            let mut n = 0u32;
            let mut cursor = first;
            while cursor < month {
                n += 1;
                cursor = cursor.succ();
            }
            n
        }
    };
    Ok(Some(months))
}

fn receivables(conn: &Connection, today: Date) -> Result<Vec<Receivable>, AppError> {
    let mut aged = aged_balance(conn, today)?;
    aged.retain(|a| a.outstanding.cents() > 0);
    aged.sort_by(|a, b| {
        b.outstanding
            .cmp(&a.outstanding)
            .then(b.days_overdue.cmp(&a.days_overdue))
    });
    aged.truncate(MAX_RECEIVABLES);
    let mut out = Vec::with_capacity(aged.len());
    for a in aged {
        let party = name_of_client(conn, a.client_id, a.client_id.to_string())?;
        out.push(Receivable {
            party,
            amount: a.outstanding,
            days_overdue: a.days_overdue,
            invoice_id: a.invoice_id,
            client_id: a.client_id,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// Gestes
// ---------------------------------------------------------------------------------------------

/// Un verbe typé : la fenêtre écrit « Écrire à Camille », pas le cœur.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GestureVerb {
    Setup,
    Write,
    Remind,
    FileStatement,
    KnowVat,
    KnowDuty,
}

/// Source d'un geste — assez pour que chaque façade branche l'action (dossier, brouillon,
/// relevé, impôts) sans recalculer la file.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GestureSource {
    Setup {
        step: SetupStep,
    },
    FollowUp {
        subject: FollowUpSubject,
        follow_kind: FollowUpKind,
        status: CardStatus,
        title: String,
        party: String,
        contact_name: Option<String>,
        amount: Money,
        #[serde(with = "crate::domain::serde_date::date::option")]
        due_on: Option<Date>,
        days_until: Option<i64>,
        drafted: bool,
        block_reason: Option<String>,
    },
    BankStatement {
        unmatched: u32,
    },
    StateDuty {
        deadline: FiscalDeadlineKind,
        #[serde(with = "crate::domain::serde_date::date")]
        due_on: Date,
        amount: Option<Money>,
        period_key: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DayGesture {
    pub verb: GestureVerb,
    pub source: GestureSource,
}

/// File du jour : relances dues, relevé non lu, prochaine obligation d'État. 3 à 5.
///
/// # Errors
pub fn day_gestures(conn: &Connection, today: Date) -> Result<Vec<DayGesture>, AppError> {
    let setup = setup_status(conn)?;
    let queue = follow_up_queue(conn, today)?;
    let unmatched = unmatched_count(conn)?;
    let calendar = fiscal_calendar(conn, today)?;
    let duty = next_state_duty(conn, &calendar, today)?;

    let mut gestes = Vec::new();
    if !setup.is_done() {
        gestes.push(setup_gesture(&setup));
    }
    for card in &queue {
        if gestes.len() >= MAX_GESTURES {
            break;
        }
        gestes.push(follow_up_gesture(card));
    }
    if unmatched > 0 && gestes.len() < MAX_GESTURES {
        let bank = DayGesture {
            verb: GestureVerb::FileStatement,
            source: GestureSource::BankStatement { unmatched },
        };
        let insert_at = if gestes.first().is_some_and(|g| {
            matches!(
                g.verb,
                GestureVerb::Setup | GestureVerb::Write | GestureVerb::Remind
            )
        }) {
            1.min(gestes.len())
        } else {
            0
        };
        let insert_at = insert_at.min(gestes.len());
        gestes.insert(insert_at, bank);
        gestes.truncate(MAX_GESTURES);
    }
    if let Some(d) = duty
        && gestes.len() < MAX_GESTURES
    {
        gestes.push(state_duty_gesture(d));
    }
    Ok(gestes)
}

fn unmatched_count(conn: &Connection) -> Result<u32, AppError> {
    let n = list_bank_transactions(conn)?
        .into_iter()
        .filter(|t| !t.is_matched())
        .count();
    Ok(u32::try_from(n).unwrap_or(u32::MAX))
}

fn next_state_duty<'a>(
    conn: &Connection,
    calendar: &'a [FiscalDeadline],
    today: Date,
) -> Result<Option<&'a FiscalDeadline>, AppError> {
    let soon: Vec<&FiscalDeadline> = calendar
        .iter()
        .filter(|d| {
            (d.due_on - today).whole_days() <= STATE_DUTY_HORIZON_DAYS
                && d.kind != FiscalDeadlineKind::ApprovalMeeting
        })
        .collect();
    let mut open = Vec::new();
    for d in soon {
        if duty_is_filed(conn, d.kind, &d.period_key)? {
            continue;
        }
        open.push(d);
    }
    Ok(open
        .iter()
        .copied()
        .find(|d| is_vat_deadline(d.kind))
        .or_else(|| open.first().copied()))
}

fn duty_is_filed(
    conn: &Connection,
    kind: FiscalDeadlineKind,
    period_key: &str,
) -> Result<bool, AppError> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM duty_filings WHERE kind = ?1 AND period_key = ?2",
        rusqlite::params![kind.as_str(), period_key],
        |row| row.get(0),
    )?;
    Ok(n > 0)
}

fn is_vat_deadline(kind: FiscalDeadlineKind) -> bool {
    matches!(
        kind,
        FiscalDeadlineKind::Ca3 | FiscalDeadlineKind::VatInstalment | FiscalDeadlineKind::Ca12
    )
}

fn setup_gesture(setup: &SetupStatus) -> DayGesture {
    DayGesture {
        verb: GestureVerb::Setup,
        source: GestureSource::Setup {
            step: setup.next_step,
        },
    }
}

fn follow_up_gesture(card: &FollowUpCard) -> DayGesture {
    let verb = match card.kind {
        FollowUpKind::Prospect => GestureVerb::Write,
        FollowUpKind::Invoice => GestureVerb::Remind,
    };
    DayGesture {
        verb,
        source: GestureSource::FollowUp {
            subject: card.subject,
            follow_kind: card.kind,
            status: card.status,
            title: card.title.clone(),
            party: card.party.clone(),
            contact_name: card.contact_name.clone(),
            amount: card.amount,
            due_on: card.due_on,
            days_until: card.days_until,
            drafted: card.status == CardStatus::Drafted,
            block_reason: card.block_reason.clone(),
        },
    }
}

fn state_duty_gesture(deadline: &FiscalDeadline) -> DayGesture {
    let verb = if is_vat_deadline(deadline.kind) {
        GestureVerb::KnowVat
    } else {
        GestureVerb::KnowDuty
    };
    DayGesture {
        verb,
        source: GestureSource::StateDuty {
            deadline: deadline.kind,
            due_on: deadline.due_on,
            amount: deadline.amount,
            period_key: deadline.period_key.clone(),
        },
    }
}

// ---------------------------------------------------------------------------------------------
// Mois
// ---------------------------------------------------------------------------------------------

/// Marque sur la grille — le cœur dit la nature, la fenêtre pose le glyphe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DayMark {
    /// Relance, jalon, échéance de facture.
    Dot,
    /// Rencontre posée.
    Meet,
    /// Obligation d'État, fin d'exercice.
    Legal,
}

/// Nature d'un événement d'agenda. Libellé français : la fenêtre.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MonthEventKind {
    FollowUp,
    InvoiceDue,
    Meeting,
    Milestone,
    MissionEnd,
    StateDuty,
    YearEnd,
}

impl MonthEventKind {
    #[must_use]
    pub const fn mark(self) -> DayMark {
        match self {
            Self::Meeting => DayMark::Meet,
            Self::StateDuty | Self::YearEnd => DayMark::Legal,
            Self::FollowUp | Self::InvoiceDue | Self::Milestone | Self::MissionEnd => DayMark::Dot,
        }
    }
}

/// Cible d'un clic d'agenda — assez pour que la fenêtre branche sans deviner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MonthTarget {
    FollowUp { subject: FollowUpSubject },
    Mission { id: MissionId },
    Invoice { id: InvoiceId },
    Taxes,
    Closing,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MonthEvent {
    #[serde(with = "crate::domain::serde_date::date")]
    pub on: Date,
    pub kind: MonthEventKind,
    pub deadline: Option<FiscalDeadlineKind>,
    pub period_key: Option<String>,
    pub party: Option<String>,
    pub title: String,
    pub amount: Option<Money>,
    pub target: MonthTarget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackMilestone {
    pub label: String,
    #[serde(with = "crate::domain::serde_date::date")]
    pub due_on: Date,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionTrack {
    pub mission_id: MissionId,
    pub name: String,
    pub client_name: String,
    #[serde(with = "crate::domain::serde_date::date")]
    pub started_on: Date,
    #[serde(with = "crate::domain::serde_date::date::option")]
    pub ended_on: Option<Date>,
    pub whole_month: bool,
    pub milestones: Vec<TrackMilestone>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MonthView {
    pub month: Month,
    pub events: Vec<MonthEvent>,
    pub tracks: Vec<MissionTrack>,
    /// Dernière fin de mission du mois, s'il ne reste plus rien après sur le papier.
    #[serde(with = "crate::domain::serde_date::date::option")]
    pub empty_after: Option<Date>,
    pub next_month_empty: bool,
}

/// Événements et bandes de missions d'un mois civil.
///
/// # Errors
pub fn day_month(conn: &Connection, month: Month, today: Date) -> Result<MonthView, AppError> {
    let mut events = Vec::new();
    collect_follow_up_events(conn, month, today, &mut events)?;
    collect_invoice_due_events(conn, month, today, &mut events)?;
    collect_meeting_events(conn, month, today, &mut events)?;
    collect_mission_events(conn, month, &mut events)?;
    collect_fiscal_events(conn, month, today, &mut events)?;
    collect_year_end(conn, month, &mut events)?;

    events.sort_by(|a, b| a.on.cmp(&b.on).then(a.title.cmp(&b.title)));

    let tracks = mission_tracks(conn, month)?;
    let (empty_after, next_month_empty) = month_gap(conn, month, &tracks)?;

    Ok(MonthView {
        month,
        events,
        tracks,
        empty_after,
        next_month_empty,
    })
}

fn collect_follow_up_events(
    conn: &Connection,
    month: Month,
    today: Date,
    events: &mut Vec<MonthEvent>,
) -> Result<(), AppError> {
    for card in follow_up_board(conn, today)? {
        let Some(on) = card.due_on else { continue };
        if month_of(on) != month {
            continue;
        }
        events.push(MonthEvent {
            on,
            kind: MonthEventKind::FollowUp,
            deadline: None,
            period_key: None,
            party: Some(card.party.clone()),
            title: card.title.clone(),
            amount: Some(card.amount),
            target: MonthTarget::FollowUp {
                subject: card.subject,
            },
        });
    }
    Ok(())
}

fn collect_invoice_due_events(
    conn: &Connection,
    month: Month,
    today: Date,
    events: &mut Vec<MonthEvent>,
) -> Result<(), AppError> {
    let invoices = list_invoices(conn)?;
    for aged in aged_balance(conn, today)? {
        if aged.outstanding.cents() <= 0 {
            continue;
        }
        let Some(invoice) = invoices.iter().find(|i| i.id == aged.invoice_id) else {
            continue;
        };
        if month_of(invoice.due_on) != month {
            continue;
        }
        if events.iter().any(|e| {
            e.on == invoice.due_on
                && matches!(
                    e.target,
                    MonthTarget::FollowUp {
                        subject: FollowUpSubject::Invoice(id)
                    } if id == invoice.id
                )
        }) {
            continue;
        }
        let party = name_of_client(conn, aged.client_id, aged.client_id.to_string())?;
        events.push(MonthEvent {
            on: invoice.due_on,
            kind: MonthEventKind::InvoiceDue,
            deadline: None,
            period_key: None,
            party: Some(party),
            title: invoice.number.clone(),
            amount: Some(aged.outstanding),
            target: MonthTarget::Invoice { id: invoice.id },
        });
    }
    Ok(())
}

fn collect_meeting_events(
    conn: &Connection,
    month: Month,
    today: Date,
    events: &mut Vec<MonthEvent>,
) -> Result<(), AppError> {
    use crate::domain::InteractionKind;
    for opportunity in list_opportunities(conn)? {
        for interaction in list_interactions(conn, opportunity.id)? {
            if interaction.kind != InteractionKind::Meeting {
                continue;
            }
            let on = interaction.occurred_at.date();
            if on < today || month_of(on) != month {
                continue;
            }
            let party = name_of_client(conn, opportunity.client_id, opportunity.name.clone())?;
            events.push(MonthEvent {
                on,
                kind: MonthEventKind::Meeting,
                deadline: None,
                period_key: None,
                party: Some(party),
                title: interaction.note.clone(),
                amount: None,
                target: MonthTarget::FollowUp {
                    subject: FollowUpSubject::Opportunity(opportunity.id),
                },
            });
        }
    }
    Ok(())
}

fn collect_mission_events(
    conn: &Connection,
    month: Month,
    events: &mut Vec<MonthEvent>,
) -> Result<(), AppError> {
    let missions = list_missions_with(
        conn,
        MissionFilter {
            include_ended: true,
            include_archived: false,
        },
    )?;
    for mission in missions {
        let party = name_of_client(conn, mission.client_id, mission.name.clone())?;
        for milestone in &mission.milestones {
            let Some(on) = milestone.due_on else { continue };
            if month_of(on) != month {
                continue;
            }
            events.push(MonthEvent {
                on,
                kind: MonthEventKind::Milestone,
                deadline: None,
                period_key: None,
                party: Some(party.clone()),
                title: milestone.label.clone(),
                amount: None,
                target: MonthTarget::Mission { id: mission.id },
            });
        }
        if let Some(on) = mission.ended_on
            && month_of(on) == month
        {
            events.push(MonthEvent {
                on,
                kind: MonthEventKind::MissionEnd,
                deadline: None,
                period_key: None,
                party: Some(party),
                title: mission.name.clone(),
                amount: None,
                target: MonthTarget::Mission { id: mission.id },
            });
        }
    }
    Ok(())
}

fn collect_fiscal_events(
    conn: &Connection,
    month: Month,
    today: Date,
    events: &mut Vec<MonthEvent>,
) -> Result<(), AppError> {
    for deadline in fiscal_calendar(conn, today)? {
        if month_of(deadline.due_on) != month {
            continue;
        }
        events.push(MonthEvent {
            on: deadline.due_on,
            kind: MonthEventKind::StateDuty,
            deadline: Some(deadline.kind),
            period_key: Some(deadline.period_key.clone()),
            party: None,
            title: deadline.kind.as_str().to_string(),
            amount: deadline.amount,
            target: MonthTarget::Taxes,
        });
    }
    Ok(())
}

fn collect_year_end(
    conn: &Connection,
    month: Month,
    events: &mut Vec<MonthEvent>,
) -> Result<(), AppError> {
    let Some(profile) = company_profile(conn)? else {
        return Ok(());
    };
    let Some(fye) = profile.fiscal_year_end else {
        return Ok(());
    };
    let on = fye.end_in_year(month.year());
    if month_of(on) != month {
        return Ok(());
    }
    events.push(MonthEvent {
        on,
        kind: MonthEventKind::YearEnd,
        deadline: None,
        period_key: None,
        party: None,
        title: format_date(on),
        amount: None,
        target: MonthTarget::Closing,
    });
    Ok(())
}

fn mission_tracks(conn: &Connection, month: Month) -> Result<Vec<MissionTrack>, AppError> {
    let missions = list_missions_with(
        conn,
        MissionFilter {
            include_ended: true,
            include_archived: false,
        },
    )?;
    let start = month.first_day();
    let end = month.last_day();
    let mut tracks = Vec::new();
    for mission in missions {
        if !overlaps(mission.started_on, mission.ended_on, start, end) {
            continue;
        }
        let client_name = name_of_client(conn, mission.client_id, mission.name.clone())?;
        let whole_month = mission.started_on <= start && mission.ended_on.is_none_or(|d| d >= end);
        let milestones = mission
            .milestones
            .iter()
            .filter_map(|m| {
                let due_on = m.due_on?;
                if month_of(due_on) == month {
                    Some(TrackMilestone {
                        label: m.label.clone(),
                        due_on,
                    })
                } else {
                    None
                }
            })
            .collect();
        tracks.push(MissionTrack {
            mission_id: mission.id,
            name: mission.name,
            client_name,
            started_on: mission.started_on,
            ended_on: mission.ended_on,
            whole_month,
            milestones,
        });
    }
    tracks.sort_by(|a, b| a.client_name.cmp(&b.client_name).then(a.name.cmp(&b.name)));
    Ok(tracks)
}

fn month_gap(
    conn: &Connection,
    month: Month,
    tracks: &[MissionTrack],
) -> Result<(Option<Date>, bool), AppError> {
    let next = month.succ();
    let next_tracks = mission_tracks(conn, next)?;
    let next_month_empty = next_tracks.is_empty();
    let last_end = tracks.iter().filter_map(|t| t.ended_on).max();
    let continues = tracks
        .iter()
        .any(|t| t.ended_on.is_none_or(|d| d >= month.last_day()));
    let empty_after = if continues {
        None
    } else {
        last_end.filter(|d| *d < month.last_day())
    };
    Ok((empty_after, next_month_empty))
}

fn overlaps(started_on: Date, ended_on: Option<Date>, from: Date, until: Date) -> bool {
    started_on <= until && ended_on.unwrap_or(until) >= from
}

/// # Panics
///
/// Ne panique jamais : `date.month()` est toujours dans `1..=12`.
fn month_of(date: Date) -> Month {
    Month::new(date.year(), u8::from(date.month()))
        .expect("mois toujours valide pour une date réelle")
}

/// # Errors
fn name_of_client(conn: &Connection, id: ClientId, fallback: String) -> Result<String, AppError> {
    Ok(client_by_id(conn, id)?.map_or(fallback, |c| c.name))
}

#[cfg(test)]
mod tests {
    use time::Month as TimeMonth;

    use super::*;
    use crate::app::{Actor, ExecutionContext, Executor, Outcome};
    use crate::billing::{
        EmitInvoice, ImportBankTransactions, ParsedTransaction, unmatched_debits,
    };
    use crate::clients::{CreateClient, CreateContact};
    use crate::company::SetCompanyProfile;
    use crate::domain::{
        Address, ExpenseCategory, FiscalYearEnd, InteractionKind, InvoiceLine, Milestone,
        MissionKind, OpeningBalanceLine, Probability, Siren, VatRate, VatRegime,
    };
    use crate::expenses::RecordExpense;
    use crate::follow_up::SetFollowUpSender;
    use crate::missions::{CloseMission, CreateMission};
    use crate::opening_balance::RecordOpeningBalance;
    use crate::prospection::{CreateOpportunity, LogInteraction};
    use crate::store::{Passphrase, Store};

    fn date(year: i32, month: TimeMonth, day: u8) -> Date {
        Date::from_calendar_date(year, month, day).unwrap()
    }

    fn today() -> Date {
        date(2026, TimeMonth::September, 5)
    }

    fn test_store(label: &str) -> Store {
        let dir = std::env::temp_dir().join(format!(
            "freeflow-day-test-{label}-{}-{}",
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
                            street: "12 rue de la Paix".into(),
                            postal_code: "75002".into(),
                            city: "Paris".into(),
                            country: "FR".into(),
                        },
                        share_capital: Some(Money::from_cents(100_000)),
                        rcs_city: Some("Paris".into()),
                        iban: None,
                        fiscal_year_end: Some(FiscalYearEnd::new(9, 30).unwrap()),
                        vat_regime: Some(VatRegime::RealNormalMonthly),
                        director_monthly_gross: None,
                        director_charge_ratio_bps: None,
                        president_name: Some("Nicolas".into()),
                        sole_shareholder_name: Some("Nicolas".into()),
                        sole_shareholder_address: Some("12 rue de la Paix, 75002 Paris".into()),
                        share_count: Some(1000),
                    },
                    &human(),
                )
                .unwrap(),
        );
    }

    fn set_opening(store: &mut Store, bank_cents: i64) {
        let capital = format!(
            "101000:Capital:C:{}.{:02}",
            bank_cents / 100,
            bank_cents % 100
        );
        let bank = format!(
            "512000:Banque:D:{}.{:02}",
            bank_cents / 100,
            bank_cents % 100
        );
        applied(
            Executor::new(store)
                .execute(
                    &RecordOpeningBalance {
                        opens_on: date(2025, TimeMonth::October, 1),
                        source: Some("cabinet".into()),
                        lines: vec![
                            capital.parse::<OpeningBalanceLine>().unwrap(),
                            bank.parse::<OpeningBalanceLine>().unwrap(),
                        ],
                        tax_losses: Money::ZERO,
                        prior_corporate_tax: None,
                        prior_vat_due: None,
                    },
                    &human(),
                )
                .unwrap(),
        );
    }

    fn burn_three_months(store: &mut Store) {
        for (month, day) in [
            (TimeMonth::June, 15),
            (TimeMonth::July, 15),
            (TimeMonth::August, 15),
        ] {
            applied(
                Executor::new(store)
                    .execute(
                        &RecordExpense {
                            label: "loyer".into(),
                            category: ExpenseCategory::Software,
                            amount: Money::from_cents(321_075),
                            vat_rate: VatRate::Zero,
                            vat_deductible: Money::ZERO,
                            incurred_on: date(2026, month, day),
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

    fn import_txs(store: &mut Store, txs: &[ParsedTransaction]) {
        applied(
            Executor::new(store)
                .execute(
                    &ImportBankTransactions {
                        transactions: txs.to_vec(),
                    },
                    &human(),
                )
                .unwrap(),
        );
    }

    fn create_client(store: &mut Store, name: &str) -> crate::domain::ClientId {
        applied(
            Executor::new(store)
                .execute(
                    &CreateClient {
                        name: name.into(),
                        siren: None,
                        vat_number: None,
                        address: None,
                    },
                    &human(),
                )
                .unwrap(),
        )
    }

    fn add_contact(store: &mut Store, client_id: crate::domain::ClientId, name: &str, email: &str) {
        applied(
            Executor::new(store)
                .execute(
                    &CreateContact {
                        client_id,
                        name: name.into(),
                        email: Some(email.into()),
                        phone: None,
                        role: None,
                    },
                    &human(),
                )
                .unwrap(),
        );
    }

    #[allow(clippy::too_many_lines)]
    fn seed_letter(store: &mut Store) -> crate::domain::ClientId {
        set_profile(store);
        set_opening(store, 1_500_000);
        burn_three_months(store);
        import_txs(
            store,
            &[
                ParsedTransaction {
                    occurred_on: date(2026, TimeMonth::September, 1),
                    amount_cents: -80_000,
                    description: "VIR CABINET".into(),
                    fitid: None,
                },
                ParsedTransaction {
                    occurred_on: date(2026, TimeMonth::September, 2),
                    amount_cents: -120_000,
                    description: "CB LOGICIEL".into(),
                    fitid: None,
                },
                ParsedTransaction {
                    occurred_on: date(2026, TimeMonth::September, 3),
                    amount_cents: -15_700,
                    description: "FRAIS BANCAIRES".into(),
                    fitid: None,
                },
            ],
        );

        let atlas = create_client(store, "Atlas Digital");
        applied(
            Executor::new(store)
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
        applied(
            Executor::new(store)
                .execute(
                    &CreateMission {
                        client_id: atlas,
                        quote_id: None,
                        name: "régie Atlas".into(),
                        kind: MissionKind::Regie {
                            daily_rate: Money::from_cents(65_000),
                        },
                        milestones: vec![],
                        started_on: date(2026, TimeMonth::September, 1),
                    },
                    &human(),
                )
                .unwrap(),
        );

        let atelier = create_client(store, "Atelier Nord");
        add_contact(store, atelier, "Camille Rivière", "camille@atelier.test");
        applied(
            Executor::new(store)
                .execute(
                    &SetFollowUpSender {
                        email: "nicolas@lumen.test".into(),
                        name: Some("Nicolas".into()),
                    },
                    &human(),
                )
                .unwrap(),
        );
        applied(
            Executor::new(store)
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

        let hume = create_client(store, "Hume");
        let hume_mission = applied(
            Executor::new(store)
                .execute(
                    &CreateMission {
                        client_id: hume,
                        quote_id: None,
                        name: "forfait Hume".into(),
                        kind: MissionKind::Forfait {
                            budget: Money::from_cents(800_000),
                        },
                        milestones: vec![Milestone {
                            label: "maquettes".into(),
                            share_bps: 4_000,
                            due_on: Some(date(2026, TimeMonth::September, 22)),
                        }],
                        started_on: date(2026, TimeMonth::August, 1),
                    },
                    &human(),
                )
                .unwrap(),
        );
        let revision = crate::missions::mission_by_id(store.connection(), hume_mission)
            .unwrap()
            .unwrap()
            .revision;
        applied(
            Executor::new(store)
                .execute(
                    &CloseMission {
                        id: hume_mission,
                        revision,
                        ended_on: date(2026, TimeMonth::September, 23),
                    },
                    &human(),
                )
                .unwrap(),
        );

        let oid = match list_open_opportunities(store.connection())
            .unwrap()
            .into_iter()
            .find(|o| o.name.contains("Atelier"))
        {
            Some(o) => o.id,
            None => panic!("Camille"),
        };
        let meeting = date(2026, TimeMonth::September, 10);
        applied(
            Executor::new(store)
                .execute(
                    &LogInteraction {
                        opportunity_id: oid,
                        kind: InteractionKind::Meeting,
                        note: "un café, poser le besoin".into(),
                        occurred_at: Some(
                            meeting
                                .with_hms(10, 0, 0)
                                .unwrap()
                                .assume_offset(time::UtcOffset::from_hms(2, 0, 0).unwrap()),
                        ),
                    },
                    &human(),
                )
                .unwrap(),
        );
        atlas
    }

    #[test]
    fn an_empty_vault_has_zero_bank_no_runway_and_an_empty_pipeline_signal() {
        let store = test_store("empty");
        let mast = day_mast(store.connection(), today()).unwrap();
        assert_eq!(mast.bank, Money::ZERO);
        assert_eq!(mast.runway_months, None);
        assert!(mast.receivables.is_empty());
        assert_eq!(mast.open_conversations, 0);
        assert!(
            mast.signals
                .iter()
                .any(|s| matches!(s, MastSignal::PipelineEmpty)),
            "{:?}",
            mast.signals
        );
        assert!(!mast.is_alarming());
    }

    #[test]
    fn the_mast_reads_bank_runway_atlas_and_a_thin_pipeline() {
        // Banque = 15 000,00 € repris − 800 − 1 200 − 157 = 12 843,00 €.
        // Brûlage = 3 210,75 € / mois (trois loyers identiques en juin, juillet, août).
        // Piste : sept. 9 632,25 → oct. 6 421,50 → nov. 3 210,75 → déc. 0 → janv. −3 210,75
        // → 4 mois. Une seule conversation (Camille). 6 200 € chez Atlas, pas encore stale.
        let mut store = test_store("mast");
        seed_letter(&mut store);
        let mast = day_mast(store.connection(), today()).unwrap();
        assert_eq!(mast.bank, Money::from_cents(1_284_300));
        assert_eq!(mast.runway_months, Some(4));
        assert_eq!(mast.receivables.len(), 1);
        assert_eq!(mast.receivables[0].party, "Atlas Digital");
        assert_eq!(mast.receivables[0].amount, Money::from_cents(620_000));
        assert_eq!(mast.open_conversations, 1);
        assert!(
            mast.signals.iter().any(|s| matches!(
                s,
                MastSignal::PipelineThin { count: 1, party } if party == "Atelier Nord"
            )),
            "{:?}",
            mast.signals
        );
        assert!(
            !mast
                .signals
                .iter()
                .any(|s| matches!(s, MastSignal::RunwayAlarm { .. }))
        );
        assert!(!mast.is_alarming());
    }

    #[test]
    fn runway_under_two_months_is_an_alarm() {
        // Banque 1 500 €, brûlage 2 000 € / mois (trois loyers) → le mois en cours finit
        // déjà négatif → 0 mois, alarme.
        let mut store = test_store("alarm");
        set_opening(&mut store, 150_000);
        for month in [TimeMonth::June, TimeMonth::July, TimeMonth::August] {
            applied(
                Executor::new(&mut store)
                    .execute(
                        &RecordExpense {
                            label: "loyer".into(),
                            category: ExpenseCategory::Software,
                            amount: Money::from_cents(200_000),
                            vat_rate: VatRate::Zero,
                            vat_deductible: Money::ZERO,
                            incurred_on: date(2026, month, 10),
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
        // Banque 1 500 €, brûlage 2 000 € / mois → mois 0 déjà négatif → 0 mois.
        let mast = day_mast(store.connection(), today()).unwrap();
        assert_eq!(mast.bank, Money::from_cents(150_000));
        assert_eq!(mast.runway_months, Some(0));
        assert!(
            mast.signals
                .iter()
                .any(|s| matches!(s, MastSignal::RunwayAlarm { months: 0 })),
            "{:?}",
            mast.signals
        );
        assert!(mast.is_alarming());
    }

    #[test]
    fn gestures_fuse_the_follow_up_the_statement_and_the_next_vat() {
        let mut store = test_store("gestures");
        seed_letter(&mut store);
        let gestes = day_gestures(store.connection(), today()).unwrap();
        assert!(
            (3..=5).contains(&gestes.len()),
            "3 à 5 gestes, pas {} : {gestes:?}",
            gestes.len()
        );
        assert_eq!(gestes[0].verb, GestureVerb::Write);
        assert!(
            gestes.iter().any(|g| g.verb == GestureVerb::FileStatement),
            "{gestes:?}"
        );
        let bank = gestes
            .iter()
            .find(|g| g.verb == GestureVerb::FileStatement)
            .unwrap();
        match &bank.source {
            GestureSource::BankStatement { unmatched } => assert_eq!(*unmatched, 3),
            other => panic!("{other:?}"),
        }
        assert!(
            gestes.iter().any(|g| g.verb == GestureVerb::KnowVat),
            "TVA dans l'horizon : {gestes:?}"
        );
        assert!(
            !gestes.iter().any(|g| g.verb == GestureVerb::Setup),
            "profil, bilan et relevé sont en place"
        );
    }

    #[test]
    fn an_empty_vault_starts_with_the_setup_gesture() {
        let store = test_store("setup-geste");
        let gestes = day_gestures(store.connection(), today()).unwrap();
        assert_eq!(gestes[0].verb, GestureVerb::Setup);
    }

    #[test]
    fn the_month_lists_meetings_milestones_year_end_and_mission_tracks() {
        let mut store = test_store("month");
        seed_letter(&mut store);
        let month = Month::new(2026, 9).unwrap();
        let view = day_month(store.connection(), month, today()).unwrap();
        assert_eq!(view.month, month);

        assert!(
            view.events
                .iter()
                .any(|e| e.kind == MonthEventKind::Meeting && e.on.day() == 10),
            "rencontre le 10 : {:?}",
            view.events
        );
        assert!(
            view.events.iter().any(|e| {
                e.kind == MonthEventKind::Milestone && e.title == "maquettes" && e.on.day() == 22
            }),
            "{:?}",
            view.events
        );
        assert!(
            view.events
                .iter()
                .any(|e| e.kind == MonthEventKind::MissionEnd && e.on.day() == 23),
            "{:?}",
            view.events
        );
        assert!(
            view.events
                .iter()
                .any(|e| e.kind == MonthEventKind::YearEnd && e.on.day() == 30),
            "{:?}",
            view.events
        );
        assert!(
            view.events
                .iter()
                .any(|e| e.kind == MonthEventKind::StateDuty),
            "échéance d'État dans le mois : {:?}",
            view.events
        );

        assert_eq!(view.tracks.len(), 2);
        let atlas = view
            .tracks
            .iter()
            .find(|t| t.client_name == "Atlas Digital")
            .unwrap();
        assert!(atlas.whole_month, "{atlas:?}");
        let hume = view
            .tracks
            .iter()
            .find(|t| t.client_name == "Hume")
            .unwrap();
        assert_eq!(hume.ended_on, Some(date(2026, TimeMonth::September, 23)));
        assert_eq!(hume.milestones.len(), 1);
        assert_eq!(hume.milestones[0].due_on.day(), 22);

        // Atlas (régie sans fin) couvre tout le mois et déborde sur octobre : pas de trou.
        assert_eq!(view.empty_after, None);
        assert!(
            !view.next_month_empty,
            "la régie Atlas n'a pas de fin : octobre n'est pas vide"
        );
    }

    #[test]
    fn a_month_whose_last_mission_ends_leaves_a_gap_when_nothing_follows() {
        let mut store = test_store("gap");
        let hume = create_client(&mut store, "Hume");
        let id = applied(
            Executor::new(&mut store)
                .execute(
                    &CreateMission {
                        client_id: hume,
                        quote_id: None,
                        name: "forfait".into(),
                        kind: MissionKind::Forfait {
                            budget: Money::from_cents(100_000),
                        },
                        milestones: vec![],
                        started_on: date(2026, TimeMonth::September, 1),
                    },
                    &human(),
                )
                .unwrap(),
        );
        let revision = crate::missions::mission_by_id(store.connection(), id)
            .unwrap()
            .unwrap()
            .revision;
        applied(
            Executor::new(&mut store)
                .execute(
                    &CloseMission {
                        id,
                        revision,
                        ended_on: date(2026, TimeMonth::September, 23),
                    },
                    &human(),
                )
                .unwrap(),
        );
        let view = day_month(store.connection(), Month::new(2026, 9).unwrap(), today()).unwrap();
        assert_eq!(view.empty_after, Some(date(2026, TimeMonth::September, 23)));
        assert!(view.next_month_empty);
    }

    #[test]
    fn unmatched_debits_helper_still_counts_only_debits() {
        // Filet : day_gestures compte tous les mouvements non lus (crédits compris), pas
        // seulement les débits — un virement client sans facture est aussi « à ranger ».
        let mut store = test_store("unmatched");
        import_txs(
            &mut store,
            &[
                ParsedTransaction {
                    occurred_on: today(),
                    amount_cents: 50_000,
                    description: "VIR CLIENT".into(),
                    fitid: None,
                },
                ParsedTransaction {
                    occurred_on: today(),
                    amount_cents: -20_000,
                    description: "CB".into(),
                    fitid: None,
                },
            ],
        );
        assert_eq!(unmatched_debits(store.connection()).unwrap().len(), 1);
        let gestes = day_gestures(store.connection(), today()).unwrap();
        let bank = gestes
            .iter()
            .find(|g| g.verb == GestureVerb::FileStatement)
            .expect("relevé");
        match bank.source {
            GestureSource::BankStatement { unmatched } => assert_eq!(unmatched, 2),
            _ => panic!(),
        }
    }
}
