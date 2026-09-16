//! Correspondance ligne SQL <-> types du domaine pour les missions et le temps.
//!
//! Visibilité `pub(crate)` pour `insert_mission`/`mission_by_id` : la prospection et les devis y
//! écrivent/lisent aussi (un gain d'opportunité ou un devis accepté crée une mission) — c'est
//! ici, pas dans `prospection`/`quotes`, que vit la persistance de `Mission`, puisque c'est ce
//! module qui en porte la responsabilité métier complète. Les fonctions ajoutées au lot 16 pour
//! `Update*`/`Close*`/`Archive*`/`Delete*` restent `pub(super)` : elles n'ont pas besoin d'être
//! visibles au-delà de `missions::{commands, queries}`, et le rester au minimum réduit la
//! surface qui pourrait contourner les commandes.

use rusqlite::{Connection, OptionalExtension, Row, params};
use time::format_description::well_known::Rfc3339;

use crate::app::AppError;
use crate::domain::{
    self, Milestone, Mission, MissionId, MissionKind, TimeCategory, TimeEntry, TimeEntryId,
};

fn conv_err(e: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
}

pub(crate) fn insert_mission(conn: &Connection, mission: &Mission) -> Result<(), AppError> {
    let kind_json = serde_json::to_string(&mission.kind)?;
    conn.execute(
        "INSERT INTO missions
            (id, client_id, quote_id, opportunity_id, name, kind, started_on, ended_on)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            mission.id.to_string(),
            mission.client_id.to_string(),
            mission.quote_id.map(|id| id.to_string()),
            mission.opportunity_id.map(|id| id.to_string()),
            mission.name,
            kind_json,
            domain::format_date(mission.started_on),
            mission.ended_on.map(domain::format_date),
        ],
    )?;
    insert_milestones(conn, mission.id, &mission.milestones)
}

fn insert_milestones(
    conn: &Connection,
    mission_id: MissionId,
    milestones: &[Milestone],
) -> Result<(), AppError> {
    for (position, milestone) in milestones.iter().enumerate() {
        conn.execute(
            "INSERT INTO milestones (mission_id, position, label, share_bps, due_on) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                mission_id.to_string(),
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
    let opportunity_id: Option<String> = row.get("opportunity_id")?;
    let started_on: String = row.get("started_on")?;
    let ended_on: Option<String> = row.get("ended_on")?;
    let id: String = row.get("id")?;
    let client_id: String = row.get("client_id")?;

    let archived_at: Option<String> = row.get("archived_at")?;
    let archived_at = archived_at
        .map(|s| time::OffsetDateTime::parse(&s, &Rfc3339))
        .transpose()
        .map_err(conv_err)?;

    Ok(Mission {
        id: id.parse().map_err(conv_err)?,
        client_id: client_id.parse().map_err(conv_err)?,
        quote_id: quote_id.map(|s| s.parse()).transpose().map_err(conv_err)?,
        opportunity_id: opportunity_id
            .map(|s| s.parse())
            .transpose()
            .map_err(conv_err)?,
        name: row.get("name")?,
        kind,
        milestones: Vec::new(),
        started_on: domain::parse_date(&started_on).map_err(conv_err)?,
        ended_on: ended_on
            .map(|s| domain::parse_date(&s))
            .transpose()
            .map_err(conv_err)?,
        revision: row.get("revision")?,
        archived_at,
    })
}

fn row_to_milestone(row: &Row) -> rusqlite::Result<Milestone> {
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
}

fn milestones_for_mission(conn: &Connection, id: MissionId) -> Result<Vec<Milestone>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT label, share_bps, due_on FROM milestones WHERE mission_id = ?1 ORDER BY position ASC",
    )?;
    let rows = stmt.query_map([id.to_string()], row_to_milestone)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
}

/// Toutes les missions correspondant à `where_clause` (déjà bindée, sans placeholder
/// utilisateur), avec leurs jalons — chargés en **deux requêtes au total**, pas une par mission
/// (N+1) : un `SELECT * FROM missions ...` puis un `SELECT ... FROM milestones ORDER BY
/// mission_id, position` regroupé ici en Rust.
pub(super) fn missions_matching(
    conn: &Connection,
    sql: &str,
    sql_params: impl rusqlite::Params,
) -> Result<Vec<Mission>, AppError> {
    let mut stmt = conn.prepare(sql)?;
    let mut missions = stmt
        .query_map(sql_params, row_to_mission_without_milestones)?
        .collect::<Result<Vec<_>, _>>()?;

    let mut milestones_stmt = conn.prepare(
        "SELECT mission_id, label, share_bps, due_on FROM milestones ORDER BY mission_id, position ASC",
    )?;
    let all_milestones: Vec<(String, Milestone)> = milestones_stmt
        .query_map([], |row| {
            let mission_id: String = row.get("mission_id")?;
            Ok((mission_id, row_to_milestone(row)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    for mission in &mut missions {
        let id_str = mission.id.to_string();
        mission.milestones = all_milestones
            .iter()
            .filter(|(mid, _)| *mid == id_str)
            .map(|(_, m)| m.clone())
            .collect();
    }
    Ok(missions)
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

/// Remplace toute la collection de jalons d'une mission — `UpdateMission` réécrit toujours l'état
/// complet (voir le commentaire de module de `commands.rs`) ; les positions sont renumérotées
/// depuis 0, dans l'ordre fourni.
pub(super) fn replace_milestones(
    conn: &Connection,
    mission_id: MissionId,
    milestones: &[Milestone],
) -> Result<(), AppError> {
    conn.execute(
        "DELETE FROM milestones WHERE mission_id = ?1",
        [mission_id.to_string()],
    )?;
    insert_milestones(conn, mission_id, milestones)
}

pub(super) fn update_mission_scalars(
    conn: &Connection,
    id: MissionId,
    name: &str,
    kind: &MissionKind,
    started_on: time::Date,
    new_revision: i64,
    expected_revision: i64,
) -> Result<(), AppError> {
    let kind_json = serde_json::to_string(kind)?;
    conn.execute(
        "UPDATE missions SET name = ?1, kind = ?2, started_on = ?3, revision = ?4
          WHERE id = ?5 AND revision = ?6",
        params![
            name,
            kind_json,
            domain::format_date(started_on),
            new_revision,
            id.to_string(),
            expected_revision,
        ],
    )?;
    Ok(())
}

pub(super) fn set_mission_ended(
    conn: &Connection,
    id: MissionId,
    ended_on: Option<time::Date>,
    new_revision: i64,
    expected_revision: i64,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE missions SET ended_on = ?1, revision = ?2 WHERE id = ?3 AND revision = ?4",
        params![
            ended_on.map(domain::format_date),
            new_revision,
            id.to_string(),
            expected_revision,
        ],
    )?;
    Ok(())
}

pub(super) fn set_mission_archived(
    conn: &Connection,
    id: MissionId,
    archived_at: Option<&str>,
    new_revision: i64,
    expected_revision: i64,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE missions SET archived_at = ?1, revision = ?2 WHERE id = ?3 AND revision = ?4",
        params![archived_at, new_revision, id.to_string(), expected_revision,],
    )?;
    Ok(())
}

pub(super) fn delete_mission(
    conn: &Connection,
    id: MissionId,
    expected_revision: i64,
) -> Result<(), AppError> {
    conn.execute(
        "DELETE FROM milestones WHERE mission_id = ?1",
        [id.to_string()],
    )?;
    conn.execute(
        "DELETE FROM missions WHERE id = ?1 AND revision = ?2",
        params![id.to_string(), expected_revision],
    )?;
    Ok(())
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
        revision: row.get("revision")?,
    })
}

pub(super) fn time_entry_by_id(
    conn: &Connection,
    id: TimeEntryId,
) -> Result<Option<TimeEntry>, AppError> {
    conn.query_row(
        "SELECT * FROM time_entries WHERE id = ?1",
        [id.to_string()],
        row_to_time_entry,
    )
    .optional()
    .map_err(AppError::from)
}

pub(super) fn time_entries_for_mission(
    conn: &Connection,
    mission_id: MissionId,
) -> Result<Vec<TimeEntry>, AppError> {
    let mut stmt =
        conn.prepare("SELECT * FROM time_entries WHERE mission_id = ?1 ORDER BY worked_on ASC")?;
    let rows = stmt.query_map([mission_id.to_string()], row_to_time_entry)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
}

/// Toutes les saisies de temps, tous clients confondus — utilisé pour la capacité mensuelle
/// globale (taux d'occupation de l'activité, pas d'une mission en particulier).
pub(super) fn all_time_entries(conn: &Connection) -> Result<Vec<TimeEntry>, AppError> {
    let mut stmt = conn.prepare("SELECT * FROM time_entries")?;
    let rows = stmt.query_map([], row_to_time_entry)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
}

#[allow(clippy::too_many_arguments)] // fonction interne purement mécanique, pas une API publique.
pub(super) fn update_time_entry(
    conn: &Connection,
    id: TimeEntryId,
    worked_on: time::Date,
    days: f64,
    category: TimeCategory,
    note: Option<&str>,
    new_revision: i64,
    expected_revision: i64,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE time_entries SET worked_on = ?1, days = ?2, category = ?3, note = ?4, revision = ?5
          WHERE id = ?6 AND revision = ?7",
        params![
            domain::format_date(worked_on),
            days,
            category.as_str(),
            note,
            new_revision,
            id.to_string(),
            expected_revision,
        ],
    )?;
    Ok(())
}

pub(super) fn delete_time_entry(
    conn: &Connection,
    id: TimeEntryId,
    expected_revision: i64,
) -> Result<(), AppError> {
    conn.execute(
        "DELETE FROM time_entries WHERE id = ?1 AND revision = ?2",
        params![id.to_string(), expected_revision],
    )?;
    Ok(())
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
