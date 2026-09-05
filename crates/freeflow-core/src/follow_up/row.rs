//! Correspondance SQL du journal de relances.

use rusqlite::{Connection, OptionalExtension, Row, params};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::app::AppError;
use crate::domain::{
    self, FollowUpEvent, FollowUpEventId, FollowUpFact, FollowUpSubject, InteractionId, InvoiceId,
    OpportunityId,
};

fn conv_err(e: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
}

fn subject_kind(subject: FollowUpSubject) -> &'static str {
    match subject {
        FollowUpSubject::Opportunity(_) => "opportunity",
        FollowUpSubject::Invoice(_) => "invoice",
    }
}

fn subject_id(subject: FollowUpSubject) -> String {
    match subject {
        FollowUpSubject::Opportunity(id) => id.to_string(),
        FollowUpSubject::Invoice(id) => id.to_string(),
    }
}

fn parse_subject(kind: &str, id: &str) -> rusqlite::Result<FollowUpSubject> {
    match kind {
        "opportunity" => Ok(FollowUpSubject::Opportunity(
            id.parse::<OpportunityId>().map_err(conv_err)?,
        )),
        "invoice" => Ok(FollowUpSubject::Invoice(
            id.parse::<InvoiceId>().map_err(conv_err)?,
        )),
        other => Err(conv_err(UnknownKind(other.to_string()))),
    }
}

#[derive(Debug, thiserror::Error)]
#[error("sujet de relance inconnu : {0}")]
struct UnknownKind(String);

pub(super) fn insert_event(conn: &Connection, event: &FollowUpEvent) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO follow_up_events
            (id, subject_kind, subject_id, fact, at, until_on, rendered_subject, rendered_body,
             retracts, interaction_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            event.id.to_string(),
            subject_kind(event.subject),
            subject_id(event.subject),
            event.fact.as_str(),
            event.at.format(&Rfc3339)?,
            event.until.map(domain::format_date),
            event.rendered_subject,
            event.rendered_body,
            event.retracts.map(|id| id.to_string()),
            event.interaction_id.map(|id| id.to_string()),
        ],
    )?;
    Ok(())
}

pub(super) fn events_for(
    conn: &Connection,
    subject: FollowUpSubject,
) -> Result<Vec<FollowUpEvent>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT * FROM follow_up_events
          WHERE subject_kind = ?1 AND subject_id = ?2
          ORDER BY at ASC, id ASC",
    )?;
    let rows = stmt.query_map(
        params![subject_kind(subject), subject_id(subject)],
        row_to_event,
    )?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
}

fn row_to_event(row: &Row<'_>) -> rusqlite::Result<FollowUpEvent> {
    let kind: String = row.get("subject_kind")?;
    let sid: String = row.get("subject_id")?;
    let fact: String = row.get("fact")?;
    let at: String = row.get("at")?;
    let until_on: Option<String> = row.get("until_on")?;
    let retracts: Option<String> = row.get("retracts")?;
    let interaction_id: Option<String> = row.get("interaction_id")?;
    let id: String = row.get("id")?;
    Ok(FollowUpEvent {
        id: id.parse().map_err(conv_err)?,
        subject: parse_subject(&kind, &sid)?,
        fact: fact.parse::<FollowUpFact>().map_err(conv_err)?,
        at: OffsetDateTime::parse(&at, &Rfc3339).map_err(conv_err)?,
        until: until_on
            .map(|s| domain::parse_date(&s))
            .transpose()
            .map_err(conv_err)?,
        rendered_subject: row.get("rendered_subject")?,
        rendered_body: row.get("rendered_body")?,
        retracts: retracts
            .map(|s| s.parse::<FollowUpEventId>())
            .transpose()
            .map_err(conv_err)?,
        interaction_id: interaction_id
            .map(|s| s.parse::<InteractionId>())
            .transpose()
            .map_err(conv_err)?,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FollowUpSettings {
    pub sender_email: Option<String>,
    pub sender_name: Option<String>,
}

pub(super) fn settings(conn: &Connection) -> Result<FollowUpSettings, AppError> {
    conn.query_row(
        "SELECT sender_email, sender_name FROM follow_up_settings WHERE id = 1",
        [],
        |row| {
            Ok(FollowUpSettings {
                sender_email: row.get(0)?,
                sender_name: row.get(1)?,
            })
        },
    )
    .optional()?
    .map_or(
        Ok(FollowUpSettings {
            sender_email: None,
            sender_name: None,
        }),
        Ok,
    )
}

pub(super) fn upsert_settings(
    conn: &Connection,
    email: &str,
    name: Option<&str>,
) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO follow_up_settings (id, sender_email, sender_name, updated_at)
         VALUES (1, ?1, ?2, ?3)
         ON CONFLICT (id) DO UPDATE SET
            sender_email = excluded.sender_email,
            sender_name = excluded.sender_name,
            updated_at = excluded.updated_at",
        params![email, name, OffsetDateTime::now_utc().format(&Rfc3339)?],
    )?;
    Ok(())
}
