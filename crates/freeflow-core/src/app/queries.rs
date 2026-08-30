//! Requêtes en lecture seule sur les actions en attente — nécessaires pour qu'un appelant
//! (CLI, GUI) découvre à l'exécution *quel* type de commande confirmer, avant de pouvoir
//! appeler [`super::Executor::confirm`] (générique, donc à typer explicitement) — et sur le
//! journal d'audit, pour le rail persistant de la GUI (lot 9).

use rusqlite::{Connection, OptionalExtension, Row, params};

use super::audit::AuditEntry;
use super::error::AppError;
use super::pending::{PendingAction, PendingActionId, PendingActionStatus};

fn conv_err(e: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
}

fn row_to_pending_action(row: &Row) -> rusqlite::Result<PendingAction> {
    let id: String = row.get("id")?;
    let status: String = row.get("status")?;
    Ok(PendingAction {
        id: id.parse().map_err(conv_err)?,
        actor_session: row.get("actor_session")?,
        command_name: row.get("command_name")?,
        command_json: row.get("command_json")?,
        status: match status.as_str() {
            "pending" => PendingActionStatus::Pending,
            "confirmed" => PendingActionStatus::Confirmed,
            "rejected" => PendingActionStatus::Rejected,
            other => return Err(conv_err(UnknownPendingStatus(other.to_string()))),
        },
    })
}

#[derive(Debug, thiserror::Error)]
#[error("statut d'action en attente inconnu : {0}")]
struct UnknownPendingStatus(String);

/// # Errors
pub fn pending_action_by_id(
    conn: &Connection,
    id: PendingActionId,
) -> Result<Option<PendingAction>, AppError> {
    conn.query_row(
        "SELECT * FROM pending_actions WHERE id = ?1",
        [id.to_string()],
        row_to_pending_action,
    )
    .optional()
    .map_err(AppError::from)
}

/// Actions encore en attente de confirmation, les plus anciennes d'abord.
///
/// # Errors
pub fn list_pending_actions(conn: &Connection) -> Result<Vec<PendingAction>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT * FROM pending_actions WHERE status = 'pending' ORDER BY created_at ASC",
    )?;
    let rows = stmt.query_map([], row_to_pending_action)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
}

fn row_to_audit_entry(row: &Row) -> rusqlite::Result<AuditEntry> {
    Ok(AuditEntry {
        sequence: row.get("sequence")?,
        occurred_at: row.get("occurred_at")?,
        actor_kind: row.get("actor_kind")?,
        actor_session: row.get("actor_session")?,
        command_name: row.get("command_name")?,
        command_json: row.get("command_json")?,
        outcome: row.get("outcome")?,
        output_json: row.get("output_json")?,
        previous_hash: row.get("previous_hash")?,
        hash: row.get("hash")?,
    })
}

/// Entrées du journal d'audit dont le numéro de séquence dépasse `since_sequence`, les plus
/// récentes d'abord, bornées à `limit` — c'est ce qu'interroge le rail d'audit de la GUI (lot 9)
/// à chaque battement de `PRAGMA data_version`.
///
/// # Errors
pub fn recent_audit_entries(
    conn: &Connection,
    since_sequence: i64,
    limit: i64,
) -> Result<Vec<AuditEntry>, AppError> {
    let mut stmt = conn
        .prepare("SELECT * FROM audit_log WHERE sequence > ?1 ORDER BY sequence DESC LIMIT ?2")?;
    let rows = stmt.query_map(params![since_sequence, limit], row_to_audit_entry)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Actor, Command, ExecutionContext, Executor, Outcome};
    use crate::store::{Passphrase, Store};
    use rusqlite::Connection as RusqliteConnection;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct NeedsConfirmation;

    impl Command for NeedsConfirmation {
        type Output = ();
        const NAME: &'static str = "test.needs_confirmation";
        fn requires_confirmation(&self) -> bool {
            true
        }
        fn apply(&self, _conn: &RusqliteConnection) -> Result<Self::Output, AppError> {
            Ok(())
        }
    }

    fn test_store(label: &str) -> Store {
        let dir = std::env::temp_dir().join(format!(
            "freeflow-app-queries-test-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ));
        Store::create(&dir.join("vault.db"), &Passphrase::from("s3cret")).unwrap()
    }

    #[test]
    fn a_pending_action_is_listed_and_fetchable_by_id() {
        let mut store = test_store("list-and-fetch");
        let ctx = ExecutionContext::new(
            Actor::Agent {
                session: "sess-1".into(),
            },
            false,
        );
        let outcome = Executor::new(&mut store)
            .execute(&NeedsConfirmation, &ctx)
            .unwrap();
        let Outcome::PendingConfirmation(id) = outcome else {
            panic!("expected PendingConfirmation")
        };

        let fetched = pending_action_by_id(store.connection(), id)
            .unwrap()
            .unwrap();
        assert_eq!(fetched.command_name, "test.needs_confirmation");
        assert_eq!(fetched.status, PendingActionStatus::Pending);

        let listed = list_pending_actions(store.connection()).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, id);
    }

    #[test]
    fn recent_audit_entries_are_newest_first_and_respect_the_since_and_limit_bounds() {
        let mut store = test_store("recent-audit");
        let ctx = ExecutionContext::new(Actor::Human, false);
        for _ in 0..3 {
            Executor::new(&mut store)
                .execute(&NeedsConfirmation, &ctx)
                .unwrap();
        }

        let all = recent_audit_entries(store.connection(), 0, 10).unwrap();
        assert_eq!(all.len(), 3);
        assert!(
            all[0].sequence > all[1].sequence && all[1].sequence > all[2].sequence,
            "les entrées les plus récentes doivent arriver en premier"
        );

        let since_first = recent_audit_entries(store.connection(), all[2].sequence, 10).unwrap();
        assert_eq!(
            since_first.len(),
            2,
            "since_sequence exclut les entrées déjà vues par l'appelant"
        );

        let capped = recent_audit_entries(store.connection(), 0, 1).unwrap();
        assert_eq!(capped.len(), 1);
        assert_eq!(capped[0].sequence, all[0].sequence);
    }
}
