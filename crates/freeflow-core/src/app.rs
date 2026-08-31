//! Couche applicative : `Command`/`Query`, journal d'audit, idempotence, politique de
//! confirmation. C'est l'unique surface que consomment la CLI, le serveur MCP et la GUI —
//! aucune logique métier ne doit jamais être dupliquée dans un front.

mod actor;
mod audit;
mod command;
mod error;
mod executor;
mod pending;
mod queries;
pub(crate) mod revision;

pub use actor::Actor;
pub use audit::{AuditEntry, AuditOutcome, ChainStatus, verify_chain};
pub use command::Command;
pub use error::AppError;
pub use executor::{ExecutionContext, Executor, Outcome};
pub use pending::{PendingAction, PendingActionId, PendingActionStatus};
pub use queries::{list_pending_actions, pending_action_by_id, recent_audit_entries};

#[cfg(test)]
mod tests {
    use rusqlite::{Connection, params};
    use serde::{Deserialize, Serialize};

    use super::*;
    use crate::store::{Passphrase, Store};

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct CreateWidget {
        id: String,
        name: String,
        idempotency_key: Option<String>,
    }

    impl Command for CreateWidget {
        type Output = String;
        const NAME: &'static str = "test.create_widget";

        fn idempotency_key(&self) -> Option<&str> {
            self.idempotency_key.as_deref()
        }

        fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
            conn.execute(
                "INSERT INTO test_widgets (id, name) VALUES (?1, ?2)",
                params![self.id, self.name],
            )?;
            Ok(self.name.clone())
        }
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct SendCriticalEmail {
        to: String,
    }

    impl Command for SendCriticalEmail {
        type Output = ();
        const NAME: &'static str = "test.send_critical_email";

        fn requires_confirmation(&self) -> bool {
            true
        }

        fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
            conn.execute(
                "INSERT INTO test_widgets (id, name) VALUES (?1, 'sent')",
                params![format!("email-{}", self.to)],
            )?;
            Ok(())
        }
    }

    fn test_store(label: &str) -> Store {
        let dir = std::env::temp_dir().join(format!(
            "freeflow-app-test-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ));
        let store = Store::create(&dir.join("vault.db"), &Passphrase::from("s3cret")).unwrap();
        store
            .connection()
            .execute(
                "CREATE TABLE test_widgets (id TEXT PRIMARY KEY, name TEXT NOT NULL)",
                [],
            )
            .unwrap();
        store
    }

    fn widget_count(store: &Store) -> i64 {
        store
            .connection()
            .query_row("SELECT count(*) FROM test_widgets", [], |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn dry_run_writes_nothing() {
        let mut store = test_store("dry-run");
        let mut executor = Executor::new(&mut store);
        let cmd = CreateWidget {
            id: "w1".into(),
            name: "Widget 1".into(),
            idempotency_key: None,
        };
        let outcome = executor
            .execute(&cmd, &ExecutionContext::new(Actor::Human, true))
            .unwrap();
        assert_eq!(outcome, Outcome::DryRun);
        assert_eq!(widget_count(&store), 0);
    }

    #[test]
    fn applying_a_command_writes_and_logs_the_correct_actor() {
        let mut store = test_store("apply");
        let mut executor = Executor::new(&mut store);
        let cmd = CreateWidget {
            id: "w1".into(),
            name: "Widget 1".into(),
            idempotency_key: None,
        };
        let outcome = executor
            .execute(&cmd, &ExecutionContext::new(Actor::Human, false))
            .unwrap();
        assert_eq!(outcome, Outcome::Applied("Widget 1".to_string()));
        assert_eq!(widget_count(&store), 1);

        let actor_kind: String = store
            .connection()
            .query_row(
                "SELECT actor_kind FROM audit_log ORDER BY sequence DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(actor_kind, "human");
    }

    #[test]
    fn same_idempotency_key_applies_only_once() {
        let mut store = test_store("idempotency");
        let mut executor = Executor::new(&mut store);
        let cmd = CreateWidget {
            id: "w1".into(),
            name: "Widget 1".into(),
            idempotency_key: Some("req-42".into()),
        };
        let ctx = ExecutionContext::new(Actor::Human, false);

        executor.execute(&cmd, &ctx).unwrap();
        let second = executor.execute(&cmd, &ctx).unwrap();

        assert_eq!(second, Outcome::AlreadyApplied("Widget 1".to_string()));
        assert_eq!(widget_count(&store), 1);
    }

    #[test]
    fn agent_cannot_apply_a_command_requiring_confirmation() {
        let mut store = test_store("confirmation");
        let mut executor = Executor::new(&mut store);
        let cmd = SendCriticalEmail {
            to: "client@example.com".into(),
        };
        let ctx = ExecutionContext::new(
            Actor::Agent {
                session: "sess-1".into(),
            },
            false,
        );

        let outcome = executor.execute(&cmd, &ctx).unwrap();
        assert!(matches!(outcome, Outcome::PendingConfirmation(_)));
        assert_eq!(
            widget_count(&store),
            0,
            "aucune mutation métier tant que l'humain n'a pas confirmé"
        );

        let pending_count: i64 = store
            .connection()
            .query_row(
                "SELECT count(*) FROM pending_actions WHERE status = 'pending'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pending_count, 1);
    }

    #[test]
    fn human_bypasses_confirmation_for_the_same_command() {
        let mut store = test_store("human-bypass");
        let mut executor = Executor::new(&mut store);
        let cmd = SendCriticalEmail {
            to: "client@example.com".into(),
        };
        let outcome = executor
            .execute(&cmd, &ExecutionContext::new(Actor::Human, false))
            .unwrap();
        assert_eq!(outcome, Outcome::Applied(()));
        assert_eq!(widget_count(&store), 1);
    }

    #[test]
    fn confirming_a_pending_action_applies_it_exactly_once() {
        let mut store = test_store("confirm-loop");
        let cmd = SendCriticalEmail {
            to: "client@example.com".into(),
        };
        let propose_ctx = ExecutionContext::new(
            Actor::Agent {
                session: "sess-1".into(),
            },
            false,
        );

        // Un `Executor` frais par appel : il n'emprunte `store` que le temps de son usage,
        // ce qui permet d'inspecter la base entre chaque étape du scénario.
        let outcome = Executor::new(&mut store)
            .execute(&cmd, &propose_ctx)
            .unwrap();
        let Outcome::PendingConfirmation(id) = outcome else {
            panic!("expected PendingConfirmation, got {outcome:?}")
        };
        assert_eq!(widget_count(&store), 0);

        let confirmed = Executor::new(&mut store)
            .confirm::<SendCriticalEmail>(id)
            .unwrap();
        assert_eq!(confirmed, Outcome::Applied(()));
        assert_eq!(widget_count(&store), 1);

        let err = Executor::new(&mut store)
            .confirm::<SendCriticalEmail>(id)
            .unwrap_err();
        assert!(matches!(err, AppError::PendingActionAlreadyResolved(_)));
    }

    #[test]
    fn audit_chain_detects_tampering() {
        let mut store = test_store("chain-tamper");
        let mut executor = Executor::new(&mut store);
        for i in 0..3 {
            let cmd = CreateWidget {
                id: format!("w{i}"),
                name: format!("Widget {i}"),
                idempotency_key: None,
            };
            executor
                .execute(&cmd, &ExecutionContext::new(Actor::Human, false))
                .unwrap();
        }
        assert_eq!(
            verify_chain(store.connection()).unwrap(),
            ChainStatus::Intact
        );

        store
            .connection()
            .execute(
                "UPDATE audit_log SET command_json = 'tampered' WHERE sequence = 2",
                [],
            )
            .unwrap();
        assert_eq!(
            verify_chain(store.connection()).unwrap(),
            ChainStatus::BrokenAt(2)
        );
    }
}
