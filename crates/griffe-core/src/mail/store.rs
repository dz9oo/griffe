//! Lectures et écritures du courrier. Pas de réseau.

use rusqlite::{Connection, OptionalExtension, params};
use time::format_description::well_known::Rfc3339;
use time::{Date, Duration, OffsetDateTime, UtcOffset};
use uuid::Uuid;

use super::error::MailError;
use super::model::{
    HOURLY_CAP, MailPreset, MailProfile, OutboundMessage, OutboundStatus, OutboundView,
    ReadyLetter, SmtpEndpoint, SubmissionBatch, TlsMode,
};
use super::secret::MailSecret;
use crate::app::AppError;
use crate::domain::{FollowUpSubject, entered_cycle_key};
use crate::follow_up::{CardStatus, events_for, follow_up_queue};

pub(super) fn profile(conn: &Connection) -> Result<MailProfile, AppError> {
    let row = conn
        .query_row(
            "SELECT from_name, from_address, smtp_host, smtp_port, smtp_tls, smtp_username,
                    secret IS NOT NULL AND length(secret) > 0, preset, auto_send, signature
             FROM mail_account WHERE id = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, bool>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, Option<String>>(9)?,
                ))
            },
        )
        .optional()?;
    let Some((name, from, host, port, tls, username, has_secret, preset, auto_send, signature)) =
        row
    else {
        return Ok(MailProfile::default());
    };
    let tls = TlsMode::parse(&tls).unwrap_or(TlsMode::StartTls);
    let port = u16::try_from(port.unwrap_or(i64::from(tls.port()))).unwrap_or(tls.port());
    let from_address = from.unwrap_or_default();
    let host = host.unwrap_or_default();
    let username = username.unwrap_or_default();
    let ready = has_secret && !from_address.is_empty() && !host.is_empty() && !username.is_empty();
    Ok(MailProfile {
        from_name: name.unwrap_or_default(),
        from_address,
        host,
        port,
        tls,
        username,
        has_secret,
        preset: MailPreset::parse(&preset),
        auto_send: auto_send == 1,
        ready,
        signature: signature.unwrap_or_default(),
    })
}

pub(super) fn save_signature(
    conn: &Connection,
    signature: Option<&str>,
    now: &str,
) -> Result<(), AppError> {
    let updated = conn.execute(
        "UPDATE mail_account SET signature = ?1, updated_at = ?2 WHERE id = 1",
        params![signature, now],
    )?;
    if updated == 0 {
        conn.execute(
            "INSERT INTO mail_account (id, smtp_tls, preset, auto_send, signature, updated_at)
             VALUES (1, 'starttls', 'custom', 0, ?1, ?2)",
            params![signature, now],
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)] // insertion SQL mécanique, pas une API publique.
pub(super) fn save_account(
    conn: &Connection,
    from_name: Option<&str>,
    from_address: &str,
    host: &str,
    port: u16,
    tls: TlsMode,
    username: &str,
    preset: MailPreset,
    now: &str,
) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO mail_account (
            id, from_name, from_address, smtp_host, smtp_port, smtp_tls, smtp_username,
            preset, auto_send, updated_at
         ) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, ?8)
         ON CONFLICT(id) DO UPDATE SET
            from_name = excluded.from_name,
            from_address = excluded.from_address,
            smtp_host = excluded.smtp_host,
            smtp_port = excluded.smtp_port,
            smtp_tls = excluded.smtp_tls,
            smtp_username = excluded.smtp_username,
            preset = excluded.preset,
            updated_at = excluded.updated_at",
        params![
            from_name,
            from_address,
            host,
            i64::from(port),
            tls.as_str(),
            username,
            preset.as_str(),
            now
        ],
    )?;
    Ok(())
}

pub(super) fn save_secret(conn: &Connection, secret: &str, now: &str) -> Result<(), AppError> {
    let updated = conn.execute(
        "UPDATE mail_account SET secret = ?1, updated_at = ?2 WHERE id = 1",
        params![secret, now],
    )?;
    if updated == 0 {
        return Err(MailError::Incomplete.into());
    }
    Ok(())
}

pub(super) fn clear_secret(conn: &Connection, now: &str) -> Result<(), AppError> {
    conn.execute(
        "UPDATE mail_account SET secret = NULL, updated_at = ?1 WHERE id = 1",
        [now],
    )?;
    Ok(())
}

pub(super) fn set_auto(conn: &Connection, enabled: bool, now: &str) -> Result<(), AppError> {
    let updated = conn.execute(
        "UPDATE mail_account SET auto_send = ?1, updated_at = ?2 WHERE id = 1",
        params![i64::from(enabled), now],
    )?;
    if updated == 0 {
        conn.execute(
            "INSERT INTO mail_account (id, smtp_tls, preset, auto_send, updated_at)
             VALUES (1, 'starttls', 'custom', ?1, ?2)",
            params![i64::from(enabled), now],
        )?;
    }
    Ok(())
}

pub(super) fn load_secret(conn: &Connection) -> Result<Option<MailSecret>, AppError> {
    let value: Option<String> = conn
        .query_row("SELECT secret FROM mail_account WHERE id = 1", [], |row| {
            row.get(0)
        })
        .optional()?
        .flatten();
    Ok(value.and_then(|raw| MailSecret::new(raw).ok()))
}

pub(super) struct NewLetter {
    pub(super) kind: String,
    pub(super) anchor: Option<String>,
    pub(super) to_address: String,
    pub(super) subject: String,
    pub(super) body: String,
    pub(super) send_after: String,
    pub(super) session_token: Option<String>,
    pub(super) follow_subject: Option<String>,
    pub(super) follow_subject_id: Option<String>,
    pub(super) follow_cycle: Option<String>,
    pub(super) follow_step: Option<String>,
}

pub(super) fn insert_letter(
    conn: &Connection,
    letter: &NewLetter,
    now: &str,
) -> Result<String, AppError> {
    let current = profile(conn)?;
    if !current.ready {
        return Err(MailError::Incomplete.into());
    }
    let id = Uuid::now_v7().to_string();
    let message_id = format!("<{id}@griffe.local>");
    conn.execute(
        "INSERT INTO outbound_mail (
            id, kind, status, anchor, from_address, from_name, to_address, subject, body,
            message_id, send_after, session_token, follow_subject, follow_subject_id,
            follow_cycle, follow_step, created_at, updated_at
         ) VALUES (
            ?1, ?2, 'armed', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?16
         )",
        params![
            id,
            letter.kind,
            letter.anchor,
            current.from_address,
            optional_text(&current.from_name),
            letter.to_address,
            letter.subject,
            letter.body,
            message_id,
            letter.send_after,
            letter.session_token,
            letter.follow_subject,
            letter.follow_subject_id,
            letter.follow_cycle,
            letter.follow_step,
            now,
        ],
    )?;
    Ok(id)
}

pub(super) fn cancel(conn: &Connection, id: &str, now: &str) -> Result<(), AppError> {
    let updated = conn.execute(
        "UPDATE outbound_mail SET status = 'cancelled', updated_at = ?2
         WHERE id = ?1 AND status = 'armed'",
        params![id, now],
    )?;
    if updated == 0 {
        let status: Option<String> = conn
            .query_row(
                "SELECT status FROM outbound_mail WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .optional()?;
        if status.is_some_and(|value| value == "sending" || value == "sent") {
            return Err(MailError::NotArmed.into());
        }
        return Err(MailError::Missing.into());
    }
    Ok(())
}

pub(super) fn release_stale(
    conn: &Connection,
    session_token: &str,
    now: &str,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE outbound_mail
         SET status = 'cancelled', error = 'restée', updated_at = ?2
         WHERE status = 'armed' AND session_token IS NOT NULL AND session_token != ?1",
        params![session_token, now],
    )?;
    Ok(())
}

pub(super) fn freeze_sending(conn: &Connection, now: &str) -> Result<(), AppError> {
    conn.execute(
        "UPDATE outbound_mail
         SET status = 'uncertain',
             error = 'Griffe ne sait pas si elle est partie.',
             updated_at = ?1
         WHERE status = 'sending'",
        [now],
    )?;
    Ok(())
}

pub(super) fn claim(
    conn: &Connection,
    session_token: &str,
    now: &str,
    hour_ago: &str,
) -> Result<Vec<String>, AppError> {
    let sent: i64 = conn.query_row(
        "SELECT COUNT(*) FROM outbound_mail WHERE status = 'sent' AND updated_at >= ?1",
        [hour_ago],
        |row| row.get(0),
    )?;
    let room = HOURLY_CAP.saturating_sub(sent);
    if room <= 0 {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare(
        "SELECT id FROM outbound_mail
         WHERE status = 'armed' AND send_after <= ?1
           AND (session_token IS NULL OR session_token = ?2)
         ORDER BY send_after
         LIMIT ?3",
    )?;
    let ids: Vec<String> = stmt
        .query_map(params![now, session_token, room], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    let mut claimed = Vec::new();
    for id in ids {
        let updated = conn.execute(
            "UPDATE outbound_mail SET status = 'sending', updated_at = ?2
             WHERE id = ?1 AND status = 'armed'",
            params![id, now],
        )?;
        if updated == 1 {
            claimed.push(id);
        }
    }
    Ok(claimed)
}

pub(super) fn load_batch(
    conn: &Connection,
    ids: &[String],
) -> Result<Option<SubmissionBatch>, AppError> {
    let current = profile(conn)?;
    if !current.ready {
        return Err(MailError::Incomplete.into());
    }
    let Some(secret) = load_secret(conn)? else {
        return Err(MailError::Incomplete.into());
    };
    let mut letters = Vec::new();
    for id in ids {
        if let Some(letter) = load_ready(conn, id)? {
            letters.push(letter);
        }
    }
    if letters.is_empty() {
        return Ok(None);
    }
    Ok(Some(SubmissionBatch {
        endpoint: SmtpEndpoint {
            host: current.host,
            port: current.port,
            tls: current.tls,
            username: current.username,
        },
        secret,
        letters,
    }))
}

fn load_ready(conn: &Connection, id: &str) -> Result<Option<ReadyLetter>, AppError> {
    conn.query_row(
        "SELECT id, from_name, from_address, to_address, subject, body, message_id
         FROM outbound_mail WHERE id = ?1 AND status = 'sending'",
        [id],
        |row| {
            Ok(ReadyLetter {
                id: row.get(0)?,
                message: OutboundMessage {
                    from_name: row.get(1)?,
                    from_address: row.get(2)?,
                    to_address: row.get(3)?,
                    subject: row.get(4)?,
                    text: row.get(5)?,
                    message_id: row.get(6)?,
                },
            })
        },
    )
    .optional()
    .map_err(AppError::from)
}

pub(super) struct FollowLink {
    pub subject: FollowUpSubject,
    pub subject_line: String,
    pub body: String,
}

pub(super) fn follow_link(conn: &Connection, id: &str) -> Result<Option<FollowLink>, AppError> {
    let row: Option<(Option<String>, Option<String>, String, String)> = conn
        .query_row(
            "SELECT follow_subject, follow_subject_id, subject, body FROM outbound_mail WHERE id = ?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    let Some((kind, raw_id, subject_line, body)) = row else {
        return Err(MailError::Missing.into());
    };
    let Some(kind) = kind else {
        return Ok(None);
    };
    let Some(raw_id) = raw_id else {
        return Ok(None);
    };
    let subject = match kind.as_str() {
        "opportunity" => {
            FollowUpSubject::Opportunity(raw_id.parse().map_err(|_| MailError::Missing)?)
        }
        "invoice" => FollowUpSubject::Invoice(raw_id.parse().map_err(|_| MailError::Missing)?),
        _ => return Ok(None),
    };
    Ok(Some(FollowLink {
        subject,
        subject_line,
        body,
    }))
}

pub(super) fn mark_sent(
    conn: &Connection,
    id: &str,
    response: &str,
    now: &str,
) -> Result<(), AppError> {
    let updated = conn.execute(
        "UPDATE outbound_mail
         SET status = 'sent', smtp_response = ?2, error = NULL, updated_at = ?3
         WHERE id = ?1 AND status = 'sending'",
        params![id, response, now],
    )?;
    if updated == 0 {
        return Err(MailError::Missing.into());
    }
    Ok(())
}

pub(super) fn note(conn: &Connection, id: &str, message: &str) -> Result<(), AppError> {
    conn.execute(
        "UPDATE outbound_mail SET error = ?2 WHERE id = ?1",
        params![id, message],
    )?;
    Ok(())
}

pub(super) fn mark_failed(
    conn: &Connection,
    id: &str,
    message: &str,
    now: &str,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE outbound_mail
         SET status = 'failed', error = ?2, updated_at = ?3
         WHERE id = ?1 AND status IN ('sending', 'armed', 'uncertain', 'failed')",
        params![id, message, now],
    )?;
    Ok(())
}

pub(super) fn resolve(conn: &Connection, id: &str, sent: bool, now: &str) -> Result<(), AppError> {
    let status = if sent { "sent" } else { "failed" };
    let error = if sent {
        None
    } else {
        Some("Elle n'est pas partie.")
    };
    let updated = conn.execute(
        "UPDATE outbound_mail SET status = ?2, error = ?3, updated_at = ?4
         WHERE id = ?1 AND status = 'uncertain'",
        params![id, status, error, now],
    )?;
    if updated == 0 {
        return Err(MailError::NotWaiting.into());
    }
    Ok(())
}

struct StoredLetter {
    kind: String,
    anchor: Option<String>,
    to_address: String,
    session_token: Option<String>,
    subject: String,
    body: String,
    follow_subject: Option<String>,
    follow_subject_id: Option<String>,
    follow_cycle: Option<String>,
    follow_step: Option<String>,
    status: String,
}

pub(super) fn retry(
    conn: &Connection,
    id: &str,
    now: &str,
    send_after: &str,
    session_token: Option<&str>,
) -> Result<String, AppError> {
    let row = conn
        .query_row(
            "SELECT kind, anchor, to_address, session_token, subject, body,
                follow_subject, follow_subject_id, follow_cycle, follow_step, status
         FROM outbound_mail WHERE id = ?1",
            [id],
            |row| {
                Ok(StoredLetter {
                    kind: row.get(0)?,
                    anchor: row.get(1)?,
                    to_address: row.get(2)?,
                    session_token: row.get(3)?,
                    subject: row.get(4)?,
                    body: row.get(5)?,
                    follow_subject: row.get(6)?,
                    follow_subject_id: row.get(7)?,
                    follow_cycle: row.get(8)?,
                    follow_step: row.get(9)?,
                    status: row.get(10)?,
                })
            },
        )
        .optional()?;
    let Some(letter) = row else {
        return Err(MailError::Missing.into());
    };
    if letter.status != "failed" && letter.status != "uncertain" {
        return Err(MailError::NotWaiting.into());
    }
    conn.execute(
        "UPDATE outbound_mail SET status = 'failed', updated_at = ?2
         WHERE id = ?1 AND status = 'uncertain'",
        params![id, now],
    )?;
    let token = session_token
        .map(ToString::to_string)
        .or(letter.session_token);
    insert_letter(
        conn,
        &NewLetter {
            kind: letter.kind,
            anchor: letter.anchor,
            to_address: letter.to_address,
            subject: letter.subject,
            body: letter.body,
            send_after: send_after.to_string(),
            session_token: token,
            follow_subject: letter.follow_subject,
            follow_subject_id: letter.follow_subject_id,
            follow_cycle: letter.follow_cycle,
            follow_step: letter.follow_step,
        },
        now,
    )
}

pub(super) fn enroll(
    conn: &Connection,
    today: Date,
    now: &str,
    session_token: &str,
) -> Result<(), AppError> {
    let current = profile(conn)?;
    if !current.auto_send || !current.ready {
        return Ok(());
    }
    let cards = follow_up_queue(conn, today)?;
    for card in cards {
        if card.status != CardStatus::Due {
            continue;
        }
        let Some(to) = card.contact_email.filter(|value| !value.is_empty()) else {
            continue;
        };
        let Some(subject) = card
            .preview_subject
            .filter(|value| !value.trim().is_empty())
        else {
            continue;
        };
        let Some(body) = card.preview_body.filter(|value| !value.trim().is_empty()) else {
            continue;
        };
        let Some(step) = card.step_key else {
            continue;
        };
        let (follow_subject, follow_id) = match card.subject {
            FollowUpSubject::Opportunity(id) => ("opportunity", id.to_string()),
            FollowUpSubject::Invoice(id) => ("invoice", id.to_string()),
        };
        let events = events_for(conn, card.subject)?;
        let cycle = entered_cycle_key(&events);
        if follow_open(conn, follow_subject, &follow_id, &cycle, &step)? {
            continue;
        }
        match insert_letter(
            conn,
            &NewLetter {
                kind: "follow_up".to_string(),
                anchor: Some(format!("{follow_subject}:{follow_id}")),
                to_address: to,
                subject,
                body,
                send_after: now.to_string(),
                session_token: Some(session_token.to_string()),
                follow_subject: Some(follow_subject.to_string()),
                follow_subject_id: Some(follow_id),
                follow_cycle: Some(cycle),
                follow_step: Some(step),
            },
            now,
        ) {
            Ok(_) => {}
            Err(error) if duplicate_letter(&error) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn duplicate_letter(error: &AppError) -> bool {
    matches!(
        error,
        AppError::Sqlite(rusqlite::Error::SqliteFailure(code, _))
            if code.code == rusqlite::ErrorCode::ConstraintViolation
    )
}

fn follow_open(
    conn: &Connection,
    kind: &str,
    id: &str,
    cycle: &str,
    step: &str,
) -> Result<bool, AppError> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM outbound_mail
         WHERE follow_subject = ?1 AND follow_subject_id = ?2
           AND follow_cycle = ?3 AND follow_step = ?4
           AND status IN ('armed', 'sending', 'sent', 'uncertain')",
        params![kind, id, cycle, step],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

pub(super) fn for_anchor(
    conn: &Connection,
    anchor: &str,
    now: OffsetDateTime,
) -> Result<Option<OutboundView>, AppError> {
    let recent = stamp(now - Duration::minutes(2))?;
    let row = conn
        .query_row(
            "SELECT id, status, subject, to_address, error, send_after
             FROM outbound_mail
             WHERE anchor = ?1 AND (
               status IN ('armed', 'sending', 'failed', 'uncertain')
               OR (status = 'sent' AND updated_at >= ?2)
             )
             ORDER BY created_at DESC LIMIT 1",
            params![anchor, recent],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()?;
    let Some((id, status, subject, to_address, error, send_after)) = row else {
        return Ok(None);
    };
    let seconds_left = OffsetDateTime::parse(&send_after, &Rfc3339)
        .map_or(0, |due| (due - now).whole_seconds().clamp(0, 5));
    Ok(Some(OutboundView {
        id,
        status: OutboundStatus::parse(&status),
        subject,
        to_address,
        error,
        seconds_left,
    }))
}

pub(super) fn needs_tick(
    conn: &Connection,
    session_token: &str,
    now: OffsetDateTime,
    today: Date,
    release_stale: bool,
) -> Result<bool, AppError> {
    let now_stamp = stamp(now)?;
    let busy: bool = conn.query_row(
        "SELECT
            EXISTS(SELECT 1 FROM outbound_mail WHERE status = 'sending')
            OR EXISTS(
                SELECT 1 FROM outbound_mail
                WHERE status = 'armed' AND send_after <= ?1
                  AND (session_token IS NULL OR session_token = ?2)
            )
            OR (?3 AND EXISTS(
                SELECT 1 FROM outbound_mail
                WHERE status = 'armed'
                  AND session_token IS NOT NULL
                  AND session_token != ?2
            ))",
        params![now_stamp, session_token, release_stale],
        |row| row.get(0),
    )?;
    if busy {
        return Ok(true);
    }
    let account = profile(conn)?;
    if !account.auto_send || !account.ready {
        return Ok(false);
    }
    for card in follow_up_queue(conn, today)? {
        if card.status != CardStatus::Due {
            continue;
        }
        if card.contact_email.as_ref().is_none_or(String::is_empty) {
            continue;
        }
        if card
            .preview_subject
            .as_ref()
            .is_none_or(|value| value.trim().is_empty())
        {
            continue;
        }
        if card
            .preview_body
            .as_ref()
            .is_none_or(|value| value.trim().is_empty())
        {
            continue;
        }
        let Some(step) = card.step_key.as_deref() else {
            continue;
        };
        let (kind, id) = match card.subject {
            FollowUpSubject::Opportunity(id) => ("opportunity", id.to_string()),
            FollowUpSubject::Invoice(id) => ("invoice", id.to_string()),
        };
        let cycle = entered_cycle_key(&events_for(conn, card.subject)?);
        if !follow_open(conn, kind, &id, &cycle, step)? {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(super) fn hourly_pause(conn: &Connection, now: OffsetDateTime) -> Result<bool, AppError> {
    let ago = hour_ago(now)?;
    let paused: bool = conn.query_row(
        "SELECT
            (SELECT COUNT(*) FROM outbound_mail WHERE status = 'sent' AND updated_at >= ?1) >= ?2
            AND EXISTS (SELECT 1 FROM outbound_mail WHERE status = 'armed')",
        params![ago, HOURLY_CAP],
        |row| row.get(0),
    )?;
    Ok(paused)
}

pub(super) fn day_notes(conn: &Connection, today: Date) -> Result<Vec<String>, AppError> {
    let prefix = today.to_string();
    let mut stmt = conn.prepare(
        "SELECT status, subject, error FROM outbound_mail
         WHERE substr(updated_at, 1, 10) = ?1
           AND (
             status IN ('sent', 'failed', 'uncertain')
             OR (status = 'cancelled' AND error = 'restée')
           )
         ORDER BY updated_at DESC
         LIMIT 8",
    )?;
    let rows = stmt.query_map(params![prefix], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
        ))
    })?;
    let mut notes = Vec::new();
    for row in rows {
        let (status, subject, _) = row?;
        let line = match status.as_str() {
            "sent" => format!("« {subject} » est partie."),
            "failed" => format!("« {subject} » n'est pas partie."),
            "uncertain" => format!("« {subject} » : Griffe ne sait pas si elle est partie."),
            "cancelled" => "Une lettre est restée.".to_string(),
            _ => continue,
        };
        notes.push(line);
    }
    Ok(notes)
}

pub(super) fn stamp(now: OffsetDateTime) -> Result<String, AppError> {
    Ok(now.to_offset(UtcOffset::UTC).format(&Rfc3339)?)
}

pub(super) fn hour_ago(now: OffsetDateTime) -> Result<String, AppError> {
    stamp(now - Duration::hours(1))
}

fn optional_text(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}
