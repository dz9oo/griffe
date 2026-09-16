//! File et tableau de relances — lectures dérivées du journal.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use time::Date;

use crate::app::AppError;
use crate::billing::{aged_balance, invoice_by_id};
use crate::clients::{client_by_id, list_contacts};
use crate::domain::{
    CadenceStep, ClientId, FollowUpEvent, FollowUpKind, FollowUpSubject, Money, Opportunity,
    derive_cursor, parse_email, render_template,
};
use crate::prospection::{list_open_opportunities, opportunity_by_id};

use super::error::FollowUpError;
use super::row;

/// Une carte de la file : ce que les trois façades affichent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FollowUpCard {
    pub subject: FollowUpSubject,
    pub kind: FollowUpKind,
    pub title: String,
    pub party: String,
    pub contact_name: Option<String>,
    pub contact_email: Option<String>,
    pub amount: Money,
    pub step_index: usize,
    pub step_count: usize,
    pub step_key: Option<String>,
    pub step_label: Option<String>,
    #[serde(with = "crate::domain::serde_date::date::option")]
    pub due_on: Option<Date>,
    pub days_until: Option<i64>,
    pub status: CardStatus,
    pub block_reason: Option<String>,
    pub preview_subject: Option<String>,
    pub preview_body: Option<String>,
    pub history: Vec<HistoryItem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CardStatus {
    /// À faire aujourd'hui.
    Due,
    /// Date dépassée.
    Overdue,
    /// Brouillon ouvert, en attente de « envoyé ».
    Drafted,
    /// Plus tard.
    Later,
    /// Cadence finie, pas de prochaine date.
    Exhausted,
    /// Il manque l'expéditeur ou le destinataire.
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryItem {
    pub fact: String,
    pub label: String,
    #[serde(with = "crate::domain::serde_date::date")]
    pub on: Date,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

pub(super) struct LoadedSubject {
    pub title: String,
    pub party: String,
    pub amount: Money,
    pub outstanding: Option<Money>,
    pub recipient: Option<(String, String)>,
    pub events: Vec<FollowUpEvent>,
    pub cursor: crate::domain::FollowUpCursor,
    pub invoice_number: Option<String>,
    pub invoice_due_on: Option<Date>,
    pub today: Date,
}

/// # Errors
pub(super) fn load_subject(
    conn: &Connection,
    subject: FollowUpSubject,
    today: Date,
) -> Result<LoadedSubject, AppError> {
    match subject {
        FollowUpSubject::Opportunity(id) => {
            let opportunity =
                opportunity_by_id(conn, id)?.ok_or(FollowUpError::OpportunityInactive)?;
            if opportunity.stage.is_closed() || opportunity.archived_at.is_some() {
                return Err(FollowUpError::OpportunityInactive.into());
            }
            load_opportunity(conn, &opportunity, today)
        }
        FollowUpSubject::Invoice(id) => {
            let invoice = invoice_by_id(conn, id)?.ok_or(FollowUpError::InvoiceInactive)?;
            if invoice.credited_invoice_id.is_some() {
                return Err(FollowUpError::InvoiceInactive.into());
            }
            let aged = aged_balance(conn, today)?;
            let outstanding = aged
                .iter()
                .find(|a| a.invoice_id == id)
                .map(|a| a.outstanding)
                .ok_or(FollowUpError::InvoiceInactive)?;
            let client =
                client_by_id(conn, invoice.client_id)?.ok_or(FollowUpError::InvoiceInactive)?;
            let events = row::events_for(conn, subject)?;
            let cursor = derive_cursor(FollowUpKind::Invoice, &events, invoice.due_on, today);
            Ok(LoadedSubject {
                title: invoice.number.clone(),
                party: client.name,
                amount: outstanding,
                outstanding: Some(outstanding),
                recipient: first_recipient(conn, invoice.client_id)?,
                events,
                cursor,
                invoice_number: Some(invoice.number),
                invoice_due_on: Some(invoice.due_on),
                today,
            })
        }
    }
}

fn load_opportunity(
    conn: &Connection,
    opportunity: &Opportunity,
    today: Date,
) -> Result<LoadedSubject, AppError> {
    let client =
        client_by_id(conn, opportunity.client_id)?.ok_or(FollowUpError::OpportunityInactive)?;
    let subject = FollowUpSubject::Opportunity(opportunity.id);
    let events = row::events_for(conn, subject)?;
    let anchor = opportunity.next_action_at.unwrap_or(today);
    let cursor = derive_cursor(FollowUpKind::Prospect, &events, anchor, today);
    Ok(LoadedSubject {
        title: opportunity.name.clone(),
        party: client.name,
        amount: opportunity.amount,
        outstanding: None,
        recipient: first_recipient(conn, opportunity.client_id)?,
        events,
        cursor,
        invoice_number: None,
        invoice_due_on: None,
        today,
    })
}

/// # Errors
pub fn card_for(
    conn: &Connection,
    subject: FollowUpSubject,
    today: Date,
) -> Result<FollowUpCard, AppError> {
    let loaded = load_subject(conn, subject, today)?;
    Ok(card_from_loaded(conn, subject, &loaded))
}

fn card_from_loaded(
    conn: &Connection,
    subject: FollowUpSubject,
    loaded: &LoadedSubject,
) -> FollowUpCard {
    let settings = row::settings(conn).ok();
    let sender_missing = settings.as_ref().is_none_or(|s| s.sender_email.is_none());
    let recipient_missing = loaded.recipient.is_none();
    let (status, block_reason) = if sender_missing {
        (
            CardStatus::Blocked,
            Some("indiquez l'email avec lequel vous écrivez".to_string()),
        )
    } else if recipient_missing {
        (
            CardStatus::Blocked,
            Some("ajoutez un email sur la fiche du contact".to_string()),
        )
    } else if loaded.cursor.exhausted {
        (CardStatus::Exhausted, None)
    } else if loaded.cursor.drafted {
        (CardStatus::Drafted, None)
    } else {
        match loaded.cursor.days_until(loaded.today) {
            Some(d) if d < 0 => (CardStatus::Overdue, None),
            Some(0) => (CardStatus::Due, None),
            Some(_) => (CardStatus::Later, None),
            None => (CardStatus::Exhausted, None),
        }
    };

    let step = loaded.cursor.step;
    let (preview_subject, preview_body) = if let Some(step) = step {
        let last_draft = domain_last_draft(&loaded.events, loaded.cursor);
        if let Some((s, b)) = last_draft {
            (Some(s), Some(b))
        } else {
            let ctx = crate::domain::TemplateContext {
                sujet: loaded.title.clone(),
                prenom: loaded
                    .recipient
                    .as_ref()
                    .map_or_else(|| loaded.party.clone(), |(n, _)| n.clone()),
                ..crate::domain::TemplateContext::default()
            };
            (
                Some(render_template(step.subject, &ctx)),
                Some(render_template(step.body, &ctx)),
            )
        }
    } else {
        (None, None)
    };

    let history = crate::domain::active_events(&loaded.events)
        .into_iter()
        .map(|e| HistoryItem {
            fact: e.fact.as_str().to_string(),
            label: history_label(e.fact),
            on: e.at.date(),
            subject: e.rendered_subject.clone(),
            body: e.rendered_body.clone(),
        })
        .collect();

    FollowUpCard {
        subject,
        kind: subject.kind(),
        title: loaded.title.clone(),
        party: loaded.party.clone(),
        contact_name: loaded.recipient.as_ref().map(|(n, _)| n.clone()),
        contact_email: loaded.recipient.as_ref().map(|(_, e)| e.clone()),
        amount: loaded.amount,
        step_index: loaded.cursor.position,
        step_count: CadenceStep::cadence(subject.kind()).len(),
        step_key: step.map(|s| s.key.to_string()),
        step_label: step.map(|s| s.label.to_string()),
        due_on: loaded.cursor.due_on,
        days_until: loaded.cursor.days_until(loaded.today),
        status,
        block_reason,
        preview_subject,
        preview_body,
        history,
    }
}

fn domain_last_draft(
    events: &[FollowUpEvent],
    cursor: crate::domain::FollowUpCursor,
) -> Option<(String, String)> {
    if !cursor.drafted {
        return None;
    }
    crate::domain::active_events(events)
        .into_iter()
        .rev()
        .find(|e| e.fact == crate::domain::FollowUpFact::DraftPrepared)
        .and_then(|e| Some((e.rendered_subject.clone()?, e.rendered_body.clone()?)))
}

fn history_label(fact: crate::domain::FollowUpFact) -> String {
    match fact {
        crate::domain::FollowUpFact::DraftPrepared => "brouillon ouvert".to_string(),
        crate::domain::FollowUpFact::MarkedSent => "envoyé".to_string(),
        crate::domain::FollowUpFact::StepSkipped => "étape sautée".to_string(),
        crate::domain::FollowUpFact::Snoozed => "reporté".to_string(),
        crate::domain::FollowUpFact::DateSet => "date posée".to_string(),
        crate::domain::FollowUpFact::Retracted => "annulé".to_string(),
    }
}

/// File du jour : en retard, puis dû aujourd'hui, puis brouillon, puis bloqué.
/// « Plus tard » est renvoyé à part par [`follow_up_board`].
///
/// # Errors
pub fn follow_up_queue(conn: &Connection, today: Date) -> Result<Vec<FollowUpCard>, AppError> {
    let mut cards = all_cards(conn, today)?;
    cards.retain(|c| {
        matches!(
            c.status,
            CardStatus::Overdue | CardStatus::Due | CardStatus::Drafted | CardStatus::Blocked
        ) && !(c.status == CardStatus::Blocked && c.days_until.is_some_and(|d| d > 0))
    });
    cards.sort_by_key(|c| {
        let rank = match c.status {
            CardStatus::Overdue => 0,
            CardStatus::Due => 1,
            CardStatus::Drafted => 2,
            CardStatus::Blocked => 3,
            _ => 4,
        };
        (rank, c.days_until.unwrap_or(0), c.title.clone())
    });
    Ok(cards)
}

/// Vue d'ensemble : toutes les relances actives, y compris plus tard et cadence terminée.
///
/// # Errors
pub fn follow_up_board(conn: &Connection, today: Date) -> Result<Vec<FollowUpCard>, AppError> {
    let mut cards = all_cards(conn, today)?;
    cards.sort_by_key(|c| {
        let rank = match c.status {
            CardStatus::Overdue => 0,
            CardStatus::Due | CardStatus::Drafted => 1,
            CardStatus::Blocked => 2,
            CardStatus::Later => 3,
            CardStatus::Exhausted => 4,
        };
        (rank, c.days_until.unwrap_or(i64::MAX), c.title.clone())
    });
    Ok(cards)
}

fn all_cards(conn: &Connection, today: Date) -> Result<Vec<FollowUpCard>, AppError> {
    let mut cards = Vec::new();
    for opportunity in list_open_opportunities(conn)? {
        let subject = FollowUpSubject::Opportunity(opportunity.id);
        let loaded = load_opportunity(conn, &opportunity, today)?;
        cards.push(card_from_loaded(conn, subject, &loaded));
    }
    for aged in aged_balance(conn, today)? {
        if aged.days_overdue < 0 {
            continue;
        }
        let subject = FollowUpSubject::Invoice(aged.invoice_id);
        match load_subject(conn, subject, today) {
            Ok(loaded) => cards.push(card_from_loaded(conn, subject, &loaded)),
            Err(AppError::Domain(_)) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(cards)
}

/// Identité d'envoi configurée, éventuellement absente.
///
/// # Errors
pub fn follow_up_sender(conn: &Connection) -> Result<row::FollowUpSettings, AppError> {
    row::settings(conn)
}

/// # Errors
pub fn events_for(
    conn: &Connection,
    subject: FollowUpSubject,
) -> Result<Vec<FollowUpEvent>, AppError> {
    row::events_for(conn, subject)
}

/// Premier contact qui a un email, sinon rien.
pub(super) fn first_recipient(
    conn: &Connection,
    client_id: ClientId,
) -> Result<Option<(String, String)>, AppError> {
    let contacts = list_contacts(conn, client_id)?;
    Ok(contacts.into_iter().find_map(|c| {
        c.email
            .as_deref()
            .and_then(|e| parse_email(e).ok())
            .map(|email| (c.name, email))
    }))
}
