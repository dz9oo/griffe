//! Journal d'audit append-only, chaîné par hash : toute mutation appliquée (ou action en
//! attente créée) y laisse une trace horodatée. Une altération directe de la base — en
//! contournement de la couche applicative — casse la chaîne et devient détectable via
//! [`verify_chain`].

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use super::actor::Actor;
use super::error::AppError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuditOutcome {
    Applied,
    AlreadyApplied,
    PendingConfirmation,
    Confirmed,
}

impl AuditOutcome {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::AlreadyApplied => "already_applied",
            Self::PendingConfirmation => "pending_confirmation",
            Self::Confirmed => "confirmed",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub sequence: i64,
    pub occurred_at: String,
    pub actor_kind: String,
    pub actor_session: Option<String>,
    pub command_name: String,
    pub command_json: String,
    pub outcome: String,
    pub output_json: Option<String>,
    pub previous_hash: Option<String>,
    pub hash: String,
}

pub(super) fn append(
    conn: &Connection,
    actor: &Actor,
    command_name: &str,
    command_json: &str,
    outcome: AuditOutcome,
    output_json: Option<&str>,
) -> Result<AuditEntry, AppError> {
    let previous_hash: Option<String> = conn
        .query_row(
            "SELECT hash FROM audit_log ORDER BY sequence DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()?;

    let occurred_at = OffsetDateTime::now_utc().format(&Rfc3339)?;
    let actor_kind = actor.kind();
    let actor_session = actor.session();
    let outcome_str = outcome.as_str();

    let hash = compute_hash(
        previous_hash.as_deref(),
        actor_kind,
        actor_session,
        command_name,
        command_json,
        outcome_str,
        output_json,
    );

    conn.execute(
        "INSERT INTO audit_log
            (id, sequence, occurred_at, actor_kind, actor_session, command_name, command_json, outcome, output_json, previous_hash, hash)
         VALUES (?1, (SELECT COALESCE(MAX(sequence), 0) + 1 FROM audit_log), ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            uuid::Uuid::now_v7().to_string(),
            occurred_at,
            actor_kind,
            actor_session,
            command_name,
            command_json,
            outcome_str,
            output_json,
            previous_hash,
            hash,
        ],
    )?;

    let sequence: i64 =
        conn.query_row("SELECT MAX(sequence) FROM audit_log", [], |row| row.get(0))?;

    Ok(AuditEntry {
        sequence,
        occurred_at,
        actor_kind: actor_kind.to_string(),
        actor_session: actor_session.map(str::to_string),
        command_name: command_name.to_string(),
        command_json: command_json.to_string(),
        outcome: outcome_str.to_string(),
        output_json: output_json.map(str::to_string),
        previous_hash,
        hash,
    })
}

#[allow(clippy::too_many_arguments)] // fonction interne purement mécanique, pas une API publique.
fn compute_hash(
    previous_hash: Option<&str>,
    actor_kind: &str,
    actor_session: Option<&str>,
    command_name: &str,
    command_json: &str,
    outcome: &str,
    output_json: Option<&str>,
) -> String {
    // Chaque champ est suivi d'un octet séparateur `0x00` : sans lui, `("ab", "c")` et
    // `("a", "bc")` produiraient la même concaténation et donc le même hash.
    let mut hasher = Sha256::new();
    for field in [
        previous_hash.unwrap_or_default(),
        actor_kind,
        actor_session.unwrap_or_default(),
        command_name,
        command_json,
        outcome,
        output_json.unwrap_or_default(),
    ] {
        hasher.update(field.as_bytes());
        hasher.update([0u8]);
    }
    hex::encode(hasher.finalize())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainStatus {
    Intact,
    BrokenAt(i64),
}

/// Revérifie l'intégralité du journal d'audit en recalculant chaque hash à partir du contenu
/// stocké. Détecte toute altération directe de la base, en contournement de la couche
/// applicative.
///
/// # Errors
///
/// Retourne une erreur si la lecture du journal échoue.
pub fn verify_chain(conn: &Connection) -> Result<ChainStatus, AppError> {
    let mut stmt = conn.prepare(
        "SELECT sequence, actor_kind, actor_session, command_name, command_json, outcome, output_json, previous_hash, hash
         FROM audit_log ORDER BY sequence ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, Option<String>>(6)?,
            row.get::<_, Option<String>>(7)?,
            row.get::<_, String>(8)?,
        ))
    })?;

    let mut expected_previous: Option<String> = None;
    for row in rows {
        let (
            sequence,
            actor_kind,
            actor_session,
            command_name,
            command_json,
            outcome,
            output_json,
            previous_hash,
            hash,
        ) = row?;
        if previous_hash != expected_previous {
            return Ok(ChainStatus::BrokenAt(sequence));
        }
        let recomputed = compute_hash(
            previous_hash.as_deref(),
            &actor_kind,
            actor_session.as_deref(),
            &command_name,
            &command_json,
            &outcome,
            output_json.as_deref(),
        );
        if recomputed != hash {
            return Ok(ChainStatus::BrokenAt(sequence));
        }
        expected_previous = Some(hash);
    }
    Ok(ChainStatus::Intact)
}
