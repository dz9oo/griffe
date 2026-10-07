//! Récit des travaux d'un dossier. Une note markdown par fiche, à côté des lignes
//! d'estimation : le récit dit où on en est, l'estimation dit ce que chaque ligne vaut.

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::app::{AppError, Command};
use crate::clients::client_by_id;
use crate::domain::ClientId;

/// 64 Kio, octets UTF-8. Un octet de plus est refusé.
pub const MAX_BODY_BYTES: usize = 64 * 1024;

const ENTITY: &str = "récit";

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DossierWorkError {
    #[error("dossier introuvable")]
    UnknownDossier,
    #[error("Ce récit dépasse 64 Kio.")]
    TooLong,
}

impl From<DossierWorkError> for AppError {
    fn from(error: DossierWorkError) -> Self {
        Self::Domain(error.to_string())
    }
}

/// Texte enregistré et révision lue. Sans ligne, le corps est vide et la révision vaut 0.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DossierWork {
    pub body: String,
    pub revision: i64,
}

/// Remplace le récit du dossier.
///
/// Même texte et même révision : rien n'est réécrit. Une révision différente de celle
/// du coffre est refusée. Le corps est gardé tel quel, espaces compris.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveDossierWork {
    pub client: ClientId,
    pub body: String,
    pub revision: i64,
}

impl Command for SaveDossierWork {
    type Output = DossierWork;
    const NAME: &'static str = "people.save_work";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        if client_by_id(conn, self.client)?.is_none() {
            return Err(DossierWorkError::UnknownDossier.into());
        }
        if self.body.len() > MAX_BODY_BYTES {
            return Err(DossierWorkError::TooLong.into());
        }
        let current = dossier_work(conn, self.client)?;
        if self.revision != current.revision {
            return Err(AppError::Conflict {
                entity: ENTITY,
                id: self.client.to_string(),
            });
        }
        if self.body == current.body {
            return Ok(current);
        }
        let revision = current.revision + 1;
        let updated_at = OffsetDateTime::now_utc().format(&Rfc3339)?;
        conn.execute(
            "INSERT INTO dossier_work_notes (client_id, body, revision, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(client_id) DO UPDATE SET
               body = excluded.body,
               revision = excluded.revision,
               updated_at = excluded.updated_at",
            params![self.client.to_string(), self.body, revision, updated_at],
        )?;
        Ok(DossierWork {
            body: self.body.clone(),
            revision,
        })
    }
}

/// Lit le récit d'un dossier. L'absence de ligne est un récit vide, révision 0.
///
/// # Errors
///
/// Retourne une erreur si la lecture SQL échoue.
pub fn dossier_work(conn: &Connection, client: ClientId) -> Result<DossierWork, AppError> {
    let found = conn
        .query_row(
            "SELECT body, revision FROM dossier_work_notes WHERE client_id = ?1",
            [client.to_string()],
            |row| {
                Ok(DossierWork {
                    body: row.get(0)?,
                    revision: row.get(1)?,
                })
            },
        )
        .optional()?;
    Ok(found.unwrap_or_else(|| DossierWork {
        body: String::new(),
        revision: 0,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Actor, ExecutionContext, Executor, Outcome};
    use crate::clients::CreateClient;
    use crate::store::testing::test_store;

    fn human() -> ExecutionContext {
        ExecutionContext::new(Actor::Human, false)
    }

    fn client(store: &mut crate::store::Store, name: &str) -> ClientId {
        match Executor::new(store)
            .execute(
                &CreateClient {
                    name: name.into(),
                    siren: None,
                    vat_number: None,
                    address: None,
                },
                &human(),
            )
            .unwrap()
        {
            Outcome::Applied(id) => id,
            other => panic!("création attendue, obtenu {other:?}"),
        }
    }

    fn save(
        store: &mut crate::store::Store,
        id: ClientId,
        body: &str,
        revision: i64,
    ) -> Result<DossierWork, AppError> {
        match Executor::new(store).execute(
            &SaveDossierWork {
                client: id,
                body: body.into(),
                revision,
            },
            &human(),
        )? {
            Outcome::Applied(note) => Ok(note),
            other => panic!("écriture attendue, obtenu {other:?}"),
        }
    }

    fn stamp(store: &crate::store::Store, id: ClientId) -> Option<String> {
        store
            .connection()
            .query_row(
                "SELECT updated_at FROM dossier_work_notes WHERE client_id = ?1",
                [id.to_string()],
                |row| row.get(0),
            )
            .optional()
            .unwrap()
    }

    #[test]
    fn saving_reads_back_and_a_fresh_revision_replaces_the_text() {
        let mut store = test_store("travaux-save");
        let id = client(&mut store, "Atelier des travaux");
        let empty = dossier_work(store.connection(), id).unwrap();
        assert_eq!(empty.body, "");
        assert_eq!(empty.revision, 0);

        let body = "# Le chantier\n\nUne ligne.\n";
        let saved = save(&mut store, id, body, 0).unwrap();
        assert_eq!(saved.revision, 1);
        assert_eq!(saved.body, body);
        let read = dossier_work(store.connection(), id).unwrap();
        assert_eq!(read, saved);

        let next = "Le chantier a avancé.\n";
        let revised = save(&mut store, id, next, 1).unwrap();
        assert_eq!(revised.revision, 2);
        assert_eq!(dossier_work(store.connection(), id).unwrap().body, next);
    }

    #[test]
    fn a_stale_revision_is_refused_and_the_stored_text_stays() {
        let mut store = test_store("travaux-stale");
        let id = client(&mut store, "Cave du nord");
        save(&mut store, id, "Le texte du coffre.\n", 0).unwrap();

        let err = save(&mut store, id, "Un texte périmé.\n", 0).unwrap_err();
        assert!(matches!(err, AppError::Conflict { .. }), "{err}");
        let read = dossier_work(store.connection(), id).unwrap();
        assert_eq!(read.body, "Le texte du coffre.\n");
        assert_eq!(read.revision, 1);
    }

    #[test]
    fn the_same_text_and_revision_do_not_write_again() {
        let mut store = test_store("travaux-idem");
        let id = client(&mut store, "Ferme du récit");
        let body = "Déjà écrit.\n";
        save(&mut store, id, body, 0).unwrap();
        let before = stamp(&store, id).unwrap();

        let again = save(&mut store, id, body, 1).unwrap();
        assert_eq!(again.revision, 1);
        assert_eq!(again.body, body);
        assert_eq!(stamp(&store, id).as_deref(), Some(before.as_str()));
        assert_eq!(dossier_work(store.connection(), id).unwrap().revision, 1);
    }

    #[test]
    fn sixty_four_kib_is_kept_and_one_extra_byte_is_refused() {
        let mut store = test_store("travaux-size");
        let id = client(&mut store, "Atelier du plafond");
        let exact = "a".repeat(MAX_BODY_BYTES);
        let saved = save(&mut store, id, &exact, 0).unwrap();
        assert_eq!(saved.revision, 1);
        assert_eq!(
            dossier_work(store.connection(), id).unwrap().body.len(),
            MAX_BODY_BYTES
        );

        let over = "b".repeat(MAX_BODY_BYTES + 1);
        let err = save(&mut store, id, &over, 1).unwrap_err();
        assert!(
            matches!(err, AppError::Domain(ref msg) if msg.contains("64 Kio")),
            "{err}"
        );
        let read = dossier_work(store.connection(), id).unwrap();
        assert_eq!(read.revision, 1);
        assert_eq!(read.body, exact);
    }

    #[test]
    fn an_unknown_client_is_refused() {
        let mut store = test_store("travaux-unknown");
        let err = save(&mut store, ClientId::new(), "Personne.\n", 0).unwrap_err();
        assert!(
            matches!(err, AppError::Domain(ref msg) if msg.contains("introuvable")),
            "{err}"
        );
    }
}
