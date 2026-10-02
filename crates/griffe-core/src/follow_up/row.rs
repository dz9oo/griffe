//! Correspondance SQL du journal de relances.

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::app::AppError;
use crate::domain::{
    self, FollowUpEvent, FollowUpEventId, FollowUpFact, FollowUpSubject, InteractionId, InvoiceId,
    OpportunityId, PhraseStep,
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

/// Une phrase de prospection, dans l'ordre du coffre.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProspectPhrase {
    pub key: String,
    pub position: i64,
    pub label: String,
    pub offset_days: i64,
    pub subject: String,
    pub body: String,
    pub revision: i64,
}

impl ProspectPhrase {
    #[must_use]
    pub fn step(&self) -> PhraseStep {
        PhraseStep {
            key: self.key.clone(),
            offset_days: self.offset_days,
            label: self.label.clone(),
            subject: self.subject.clone(),
            body: self.body.clone(),
        }
    }
}

pub(super) fn list_phrases(conn: &Connection) -> Result<Vec<ProspectPhrase>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, position, label, offset_days, subject, body, revision
           FROM prospect_phrases
          ORDER BY position ASC, id ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(ProspectPhrase {
            key: row.get(0)?,
            position: row.get(1)?,
            label: row.get(2)?,
            offset_days: row.get(3)?,
            subject: row.get(4)?,
            body: row.get(5)?,
            revision: row.get(6)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
}

pub(super) fn phrase_steps(conn: &Connection) -> Result<Vec<PhraseStep>, AppError> {
    Ok(list_phrases(conn)?
        .iter()
        .map(ProspectPhrase::step)
        .collect())
}

/// `false` si la révision ne correspond plus, ou si le moment n'existe pas.
pub(super) fn update_phrase(
    conn: &Connection,
    key: &str,
    label: &str,
    subject: &str,
    body: &str,
    revision: i64,
) -> Result<bool, AppError> {
    let updated = conn.execute(
        "UPDATE prospect_phrases
            SET label = ?1, subject = ?2, body = ?3, revision = revision + 1
          WHERE id = ?4 AND revision = ?5",
        params![label, subject, body, key, revision],
    )?;
    Ok(updated == 1)
}

/// Remplace la série vivante. L'appelant est déjà dans la transaction de l'exécuteur.
pub(super) fn replace_phrases(
    conn: &Connection,
    phrases: &[ProspectPhrase],
) -> Result<(), AppError> {
    conn.execute("DELETE FROM prospect_phrases", [])?;
    for phrase in phrases {
        conn.execute(
            "INSERT INTO prospect_phrases
                (id, position, label, offset_days, subject, body, revision)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                phrase.key,
                phrase.position,
                phrase.label,
                phrase.offset_days,
                phrase.subject,
                phrase.body,
                phrase.revision,
            ],
        )?;
    }
    Ok(())
}

pub(super) fn any_copied_series(conn: &Connection) -> Result<bool, AppError> {
    let count: i64 =
        conn.query_row("SELECT COUNT(*) FROM prospect_series", [], |row| row.get(0))?;
    Ok(count > 0)
}

pub(super) fn copied_steps(
    conn: &Connection,
    opportunity_id: OpportunityId,
    cycle_key: &str,
) -> Result<Option<Vec<PhraseStep>>, AppError> {
    let series_id: Option<String> = conn
        .query_row(
            "SELECT id FROM prospect_series
              WHERE opportunity_id = ?1 AND cycle_key = ?2",
            params![opportunity_id.to_string(), cycle_key],
            |row| row.get(0),
        )
        .optional()?;
    let Some(series_id) = series_id else {
        return Ok(None);
    };
    let mut stmt = conn.prepare(
        "SELECT phrase_key, label, offset_days, subject, body
           FROM prospect_series_steps
          WHERE series_id = ?1
          ORDER BY position ASC",
    )?;
    let rows = stmt.query_map(params![series_id], |row| {
        Ok(PhraseStep {
            key: row.get(0)?,
            label: row.get(1)?,
            offset_days: row.get(2)?,
            subject: row.get(3)?,
            body: row.get(4)?,
        })
    })?;
    let steps = rows.collect::<Result<Vec<_>, _>>()?;
    Ok(Some(steps))
}

/// Copie la série vivante pour ce cycle. Ne remplace pas une copie déjà là.
pub(super) fn copy_series(
    conn: &Connection,
    opportunity_id: OpportunityId,
    cycle_key: &str,
    phrases: &[ProspectPhrase],
) -> Result<(), AppError> {
    if copied_steps(conn, opportunity_id, cycle_key)?.is_some() {
        return Ok(());
    }
    let series_id = uuid::Uuid::now_v7().to_string();
    conn.execute(
        "INSERT INTO prospect_series (id, opportunity_id, cycle_key) VALUES (?1, ?2, ?3)",
        params![series_id, opportunity_id.to_string(), cycle_key],
    )?;
    for phrase in phrases {
        conn.execute(
            "INSERT INTO prospect_series_steps
                (series_id, position, phrase_key, label, offset_days, subject, body)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                series_id,
                phrase.position,
                phrase.key,
                phrase.label,
                phrase.offset_days,
                phrase.subject,
                phrase.body,
            ],
        )?;
    }
    Ok(())
}

/// La série de cette conversation : la copie de son cycle, sinon la série vivante.
///
/// Le genre, s'il y en a un, ne remplace que le sujet et le corps des moments
/// qu'il connaît encore. Une clé qui n'existe plus que sur la copie garde les
/// mots de cette copie. L'ordre, les noms et les écarts ne viennent pas du genre.
pub(super) fn steps_for_opportunity(
    conn: &Connection,
    opportunity_id: OpportunityId,
    events: &[FollowUpEvent],
) -> Result<Vec<PhraseStep>, AppError> {
    let cycle = domain::entered_cycle_key(events);
    let mut steps = if let Some(copied) = copied_steps(conn, opportunity_id, &cycle)?
        && !copied.is_empty()
    {
        copied
    } else {
        phrase_steps(conn)?
    };
    if let Some(genre) = genre_of_opportunity(conn, opportunity_id)? {
        let words = genre_words(conn, &genre.id)?;
        for step in &mut steps {
            if let Some(word) = words.iter().find(|word| word.key == step.key) {
                step.subject.clone_from(&word.subject);
                step.body.clone_from(&word.body);
            }
        }
    }
    Ok(steps)
}

/// Un genre de gens. Les mots seulement : pas de libellé, pas d'ordre, pas d'écart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProspectGenre {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct GenreWord {
    pub key: String,
    pub subject: String,
    pub body: String,
}

fn now_stamp() -> Result<String, AppError> {
    Ok(OffsetDateTime::now_utc().format(&Rfc3339)?)
}

pub(super) fn list_genres(conn: &Connection) -> Result<Vec<ProspectGenre>, AppError> {
    let mut stmt = conn
        .prepare("SELECT id, name FROM prospect_genres ORDER BY name COLLATE NOCASE ASC, id ASC")?;
    let rows = stmt.query_map([], |row| {
        Ok(ProspectGenre {
            id: row.get(0)?,
            name: row.get(1)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
}

pub(super) fn latest_genre(conn: &Connection) -> Result<Option<ProspectGenre>, AppError> {
    conn.query_row(
        "SELECT id, name FROM prospect_genres ORDER BY touched_at DESC, id DESC LIMIT 1",
        [],
        |row| {
            Ok(ProspectGenre {
                id: row.get(0)?,
                name: row.get(1)?,
            })
        },
    )
    .optional()
    .map_err(AppError::from)
}

pub(super) fn genre_by_id(conn: &Connection, id: &str) -> Result<Option<ProspectGenre>, AppError> {
    conn.query_row(
        "SELECT id, name FROM prospect_genres WHERE id = ?1",
        [id],
        |row| {
            Ok(ProspectGenre {
                id: row.get(0)?,
                name: row.get(1)?,
            })
        },
    )
    .optional()
    .map_err(AppError::from)
}

pub(super) fn genre_by_name(
    conn: &Connection,
    name: &str,
) -> Result<Option<ProspectGenre>, AppError> {
    conn.query_row(
        "SELECT id, name FROM prospect_genres WHERE name = ?1",
        [name],
        |row| {
            Ok(ProspectGenre {
                id: row.get(0)?,
                name: row.get(1)?,
            })
        },
    )
    .optional()
    .map_err(AppError::from)
}

pub(super) fn genre_of_opportunity(
    conn: &Connection,
    opportunity_id: OpportunityId,
) -> Result<Option<ProspectGenre>, AppError> {
    conn.query_row(
        "SELECT g.id, g.name
           FROM opportunities AS o
           JOIN prospect_genres AS g ON g.id = o.genre_id
          WHERE o.id = ?1",
        [opportunity_id.to_string()],
        |row| {
            Ok(ProspectGenre {
                id: row.get(0)?,
                name: row.get(1)?,
            })
        },
    )
    .optional()
    .map_err(AppError::from)
}

pub(super) fn genre_words(conn: &Connection, genre_id: &str) -> Result<Vec<GenreWord>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT phrase_key, subject, body
           FROM prospect_genre_words
          WHERE genre_id = ?1
          ORDER BY phrase_key ASC",
    )?;
    let rows = stmt.query_map([genre_id], |row| {
        Ok(GenreWord {
            key: row.get(0)?,
            subject: row.get(1)?,
            body: row.get(2)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
}

pub(super) fn touch_genre(conn: &Connection, genre_id: &str) -> Result<(), AppError> {
    conn.execute(
        "UPDATE prospect_genres SET touched_at = ?1 WHERE id = ?2",
        params![now_stamp()?, genre_id],
    )?;
    Ok(())
}

pub(super) fn insert_genre(conn: &Connection, name: &str) -> Result<ProspectGenre, AppError> {
    let genre = ProspectGenre {
        id: uuid::Uuid::now_v7().to_string(),
        name: name.to_string(),
    };
    conn.execute(
        "INSERT INTO prospect_genres (id, name, touched_at) VALUES (?1, ?2, ?3)",
        params![genre.id, genre.name, now_stamp()?],
    )?;
    Ok(genre)
}

pub(super) fn upsert_genre_word(
    conn: &Connection,
    genre_id: &str,
    key: &str,
    subject: &str,
    body: &str,
) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO prospect_genre_words (genre_id, phrase_key, subject, body)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (genre_id, phrase_key) DO UPDATE
            SET subject = excluded.subject, body = excluded.body",
        params![genre_id, key, subject, body],
    )?;
    Ok(())
}

/// Chaque genre vivant reçoit les mots par défaut des moments qu'il n'a pas,
/// et perd ceux que la série vivante ne porte plus.
pub(super) fn sync_genre_words(conn: &Connection) -> Result<(), AppError> {
    conn.execute(
        "DELETE FROM prospect_genre_words
          WHERE phrase_key NOT IN (SELECT id FROM prospect_phrases)",
        [],
    )?;
    conn.execute(
        "INSERT INTO prospect_genre_words (genre_id, phrase_key, subject, body)
         SELECT g.id, p.id, p.subject, p.body
           FROM prospect_genres AS g
           JOIN prospect_phrases AS p
          WHERE NOT EXISTS (
              SELECT 1 FROM prospect_genre_words AS w
               WHERE w.genre_id = g.id AND w.phrase_key = p.id
          )",
        [],
    )?;
    Ok(())
}

pub(super) fn copy_genre_words(
    conn: &Connection,
    genre_id: &str,
    source_id: Option<&str>,
) -> Result<(), AppError> {
    let phrases = list_phrases(conn)?;
    let source = source_id.map(|id| genre_words(conn, id)).transpose()?;
    for phrase in phrases {
        let (subject, body) = source
            .as_ref()
            .and_then(|words| words.iter().find(|word| word.key == phrase.key))
            .map_or((phrase.subject.as_str(), phrase.body.as_str()), |word| {
                (word.subject.as_str(), word.body.as_str())
            });
        upsert_genre_word(conn, genre_id, &phrase.key, subject, body)?;
    }
    Ok(())
}

pub(super) fn set_opportunity_genre(
    conn: &Connection,
    opportunity_id: OpportunityId,
    genre_id: Option<&str>,
) -> Result<bool, AppError> {
    let updated = conn.execute(
        "UPDATE opportunities SET genre_id = ?1 WHERE id = ?2",
        params![genre_id, opportunity_id.to_string()],
    )?;
    Ok(updated == 1)
}

pub(super) fn clear_genre_from_dossiers(conn: &Connection, genre_id: &str) -> Result<(), AppError> {
    conn.execute(
        "UPDATE opportunities SET genre_id = NULL WHERE genre_id = ?1",
        [genre_id],
    )?;
    Ok(())
}

pub(super) fn delete_genre(conn: &Connection, genre_id: &str) -> Result<(), AppError> {
    clear_genre_from_dossiers(conn, genre_id)?;
    conn.execute(
        "DELETE FROM prospect_genre_words WHERE genre_id = ?1",
        [genre_id],
    )?;
    conn.execute("DELETE FROM prospect_genres WHERE id = ?1", [genre_id])?;
    Ok(())
}
