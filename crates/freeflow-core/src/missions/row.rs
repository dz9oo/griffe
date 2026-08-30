//! Correspondance ligne SQL <-> types du domaine pour les missions et le temps.
//!
//! Visibilité `pub(crate)` : la prospection (lot 3) y écrit aussi, puisqu'un gain d'opportunité
//! crée une mission — c'est ici, pas dans `prospection`, que vit la persistance de `Mission`,
//! puisque c'est ce module qui en porte la responsabilité métier complète.

use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::app::AppError;
use crate::domain::{self, Milestone, Mission, MissionId, MissionKind, TimeCategory, TimeEntry};

fn conv_err(e: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
}

pub(crate) fn insert_mission(conn: &Connection, mission: &Mission) -> Result<(), AppError> {
    let kind_json = serde_json::to_string(&mission.kind)?;
    conn.execute(
        "INSERT INTO missions (id, client_id, quote_id, name, kind, started_on, ended_on)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            mission.id.to_string(),
            mission.client_id.to_string(),
            mission.quote_id.map(|id| id.to_string()),
            mission.name,
            kind_json,
            domain::format_date(mission.started_on),
            mission.ended_on.map(domain::format_date),
        ],
    )?;
    for (position, milestone) in mission.milestones.iter().enumerate() {
        conn.execute(
            "INSERT INTO milestones (mission_id, position, label, share_bps, due_on) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                mission.id.to_string(),
                i64::try_from(position).unwrap_or(i64::MAX),
                milestone.label,
                milestone.share_bps,
                milestone.due_on.map(domain::format_date),
            ],
        )?;
    }
    Ok(())
}

fn row_to_mission_without_milestones(row: &Row) -> rusqlite::Result<Mission> {
    let kind_json: String = row.get("kind")?;
    let kind: MissionKind = serde_json::from_str(&kind_json).map_err(conv_err)?;

    let quote_id: Option<String> = row.get("quote_id")?;
    let started_on: String = row.get("started_on")?;
    let ended_on: Option<String> = row.get("ended_on")?;
    let id: String = row.get("id")?;
    let client_id: String = row.get("client_id")?;

    Ok(Mission {
        id: id.parse().map_err(conv_err)?,
        client_id: client_id.parse().map_err(conv_err)?,
        quote_id: quote_id.map(|s| s.parse()).transpose().map_err(conv_err)?,
        name: row.get("name")?,
        kind,
        milestones: Vec::new(),
        started_on: domain::parse_date(&started_on).map_err(conv_err)?,
        ended_on: ended_on
            .map(|s| domain::parse_date(&s))
            .transpose()
            .map_err(conv_err)?,
    })
}

fn milestones_for_mission(conn: &Connection, id: MissionId) -> Result<Vec<Milestone>, AppError> {
    let mut stmt = conn.prepare("SELECT label, share_bps, due_on FROM milestones WHERE mission_id = ?1 ORDER BY position ASC")?;
    let rows = stmt.query_map([id.to_string()], |row| {
        let due_on: Option<String> = row.get("due_on")?;
        let share_bps: i64 = row.get("share_bps")?;
        Ok(Milestone {
            label: row.get("label")?,
            share_bps: u32::try_from(share_bps).map_err(conv_err)?,
            due_on: due_on
                .map(|s| domain::parse_date(&s))
                .transpose()
                .map_err(conv_err)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
}

/// Sans les jalons (inutiles pour un simple listing) — contrairement à [`mission_by_id`], qui
/// les charge pour les besoins qui en dépendent réellement (échéancier de facturation).
pub(crate) fn all_missions(conn: &Connection) -> Result<Vec<Mission>, AppError> {
    let mut stmt = conn.prepare("SELECT * FROM missions ORDER BY started_on DESC")?;
    let rows = stmt.query_map([], row_to_mission_without_milestones)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
}

pub(crate) fn mission_by_id(conn: &Connection, id: MissionId) -> Result<Option<Mission>, AppError> {
    let Some(mut mission) = conn
        .query_row(
            "SELECT * FROM missions WHERE id = ?1",
            [id.to_string()],
            row_to_mission_without_milestones,
        )
        .optional()?
    else {
        return Ok(None);
    };
    mission.milestones = milestones_for_mission(conn, id)?;
    Ok(Some(mission))
}

pub(crate) fn insert_time_entry(conn: &Connection, entry: &TimeEntry) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO time_entries (id, mission_id, worked_on, days, category, note) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            entry.id.to_string(),
            entry.mission_id.to_string(),
            domain::format_date(entry.worked_on),
            entry.days,
            entry.category.as_str(),
            entry.note,
        ],
    )?;
    Ok(())
}

fn row_to_time_entry(row: &Row) -> rusqlite::Result<TimeEntry> {
    let id: String = row.get("id")?;
    let mission_id: String = row.get("mission_id")?;
    let worked_on: String = row.get("worked_on")?;
    let category: String = row.get("category")?;

    Ok(TimeEntry {
        id: id.parse().map_err(conv_err)?,
        mission_id: mission_id.parse().map_err(conv_err)?,
        worked_on: domain::parse_date(&worked_on).map_err(conv_err)?,
        days: row.get("days")?,
        category: category.parse::<TimeCategory>().map_err(conv_err)?,
        note: row.get("note")?,
    })
}

pub(crate) fn time_entries_for_mission(
    conn: &Connection,
    mission_id: MissionId,
) -> Result<Vec<TimeEntry>, AppError> {
    let mut stmt = conn.prepare("SELECT * FROM time_entries WHERE mission_id = ?1")?;
    let rows = stmt.query_map([mission_id.to_string()], row_to_time_entry)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
}

/// Toutes les saisies de temps, tous clients confondus — utilisé pour la capacité mensuelle
/// globale (taux d'occupation de l'activité, pas d'une mission en particulier).
pub(crate) fn all_time_entries(conn: &Connection) -> Result<Vec<TimeEntry>, AppError> {
    let mut stmt = conn.prepare("SELECT * FROM time_entries")?;
    let rows = stmt.query_map([], row_to_time_entry)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
}

#[cfg(test)]
pub(crate) fn mission_count_for_client(
    conn: &Connection,
    client_id: crate::domain::ClientId,
) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT count(*) FROM missions WHERE client_id = ?1",
        [client_id.to_string()],
        |row| row.get(0),
    )
}

#[cfg(test)]
pub(crate) fn mission_id_for_client(
    conn: &Connection,
    client_id: crate::domain::ClientId,
) -> rusqlite::Result<MissionId> {
    let id: String = conn.query_row(
        "SELECT id FROM missions WHERE client_id = ?1",
        [client_id.to_string()],
        |row| row.get(0),
    )?;
    id.parse().map_err(conv_err)
}

#[cfg(test)]
pub(crate) fn mission_kind_for_client(
    conn: &Connection,
    client_id: crate::domain::ClientId,
) -> rusqlite::Result<MissionKind> {
    let json: String = conn.query_row(
        "SELECT kind FROM missions WHERE client_id = ?1",
        [client_id.to_string()],
        |row| row.get(0),
    )?;
    serde_json::from_str(&json).map_err(conv_err)
}
