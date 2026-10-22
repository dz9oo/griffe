//! Miroir de la boîte. Second fichier `SQLCipher`, même clé que le coffre,
//! ouvert seulement quand le coffre l'est. Le livre n'y entre pas.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params};
use uuid::Uuid;

use crate::app::AppError;
use crate::store::{StoreError, VaultKey, tighten_file, unlock};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS mailbox (
    name TEXT PRIMARY KEY,
    uidvalidity INTEGER NOT NULL,
    last_uid INTEGER NOT NULL,
    modseq INTEGER
);
CREATE TABLE IF NOT EXISTS message (
    id TEXT PRIMARY KEY,
    mailbox TEXT NOT NULL,
    uid INTEGER NOT NULL,
    uidvalidity INTEGER NOT NULL,
    message_id TEXT,
    from_name TEXT,
    from_address TEXT,
    subject TEXT NOT NULL,
    sent_on TEXT,
    text TEXT NOT NULL,
    html TEXT,
    seen INTEGER NOT NULL CHECK (seen IN (0, 1)),
    notice TEXT CHECK (notice IN ('certain', 'probable')),
    notice_address TEXT,
    folder_role TEXT NOT NULL CHECK (folder_role IN ('inbox', 'sent', 'other')),
    choice_of TEXT CHECK (choice_of IN ('fiche', 'dossier')),
    choice_ids TEXT,
    body_state TEXT NOT NULL DEFAULT 'ready',
    UNIQUE (mailbox, uidvalidity, uid)
);
CREATE TABLE IF NOT EXISTS releve (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    at TEXT,
    error TEXT,
    applied_generation INTEGER NOT NULL DEFAULT 0
);
";

const MIRROR_V2: &str = "
ALTER TABLE message ADD COLUMN body_state TEXT NOT NULL DEFAULT 'ready';
ALTER TABLE mailbox ADD COLUMN modseq INTEGER;
CREATE TABLE IF NOT EXISTS releve (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    at TEXT,
    error TEXT
);
";

const MIRROR_V3: &str = "
ALTER TABLE releve ADD COLUMN applied_generation INTEGER NOT NULL DEFAULT 0;
";

const MIRROR_VERSION: i64 = 3;

/// Où est le miroir d'un coffre. `vault.db` donne `vault.db.mailbox`.
#[must_use]
pub fn mailbox_file(vault: &Path) -> PathBuf {
    let mut name = vault.as_os_str().to_owned();
    name.push(".mailbox");
    PathBuf::from(name)
}

/// Curseur de relève d'un dossier du serveur.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailboxCursor {
    pub name: String,
    pub uidvalidity: i64,
    pub last_uid: i64,
    /// `HIGHESTMODSEQ` déjà vu. Absent : le serveur n'annonce pas CONDSTORE.
    pub modseq: Option<i64>,
}

/// Ligne du miroir, sans qu'on la ressorte du coffre comptable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirrorMessage {
    pub id: String,
    pub mailbox: String,
    pub uid: i64,
    pub message_id: Option<String>,
    pub from_name: Option<String>,
    pub from_address: Option<String>,
    pub subject: String,
    pub sent_on: Option<String>,
    pub seen: bool,
    pub notice: Option<String>,
    pub notice_address: Option<String>,
    pub folder_role: String,
    pub choice_of: Option<String>,
    pub choice_ids: Option<String>,
}

/// Message à déposer. Le texte est déjà lu.
pub struct MirrorInsert {
    pub mailbox: String,
    pub uid: i64,
    pub uidvalidity: i64,
    pub message_id: Option<String>,
    pub from_name: Option<String>,
    pub from_address: Option<String>,
    pub subject: String,
    pub sent_on: Option<String>,
    pub text: String,
    pub html: Option<String>,
    pub seen: bool,
    pub notice: Option<String>,
    pub notice_address: Option<String>,
    pub folder_role: String,
    pub choice_of: Option<String>,
    pub choice_ids: Option<String>,
    /// `pending`, `ready`, `too_large` ou `unreadable`.
    pub body_state: String,
}

/// Fichier miroir ouvert.
pub struct Mailbox {
    conn: Connection,
}

impl Mailbox {
    /// Ouvre ou crée le miroir avec la clé du coffre.
    ///
    /// # Errors
    ///
    /// La clé ne lit pas le fichier, ou le schéma ne s'écrit pas.
    pub fn open(path: &Path, key: &VaultKey) -> Result<Self, StoreError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        unlock(&conn, key)?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version > MIRROR_VERSION {
            return Err(StoreError::SchemaTooNew);
        }
        if version == 0 {
            conn.execute_batch(SCHEMA)?;
        } else if version < 2 {
            conn.execute_batch(MIRROR_V2)?;
        }
        if version != 0 && version < 3 {
            conn.execute_batch(MIRROR_V3)?;
        }
        if version < MIRROR_VERSION {
            conn.pragma_update(None, "user_version", MIRROR_VERSION)?;
        }
        tighten_file(path);
        Ok(Self { conn })
    }

    /// # Errors
    ///
    /// Lecture impossible.
    pub fn cursors(&self) -> Result<Vec<MailboxCursor>, AppError> {
        let mut stmt = self
            .conn
            .prepare("SELECT name, uidvalidity, last_uid, modseq FROM mailbox ORDER BY name")?;
        let rows = stmt.query_map([], |row| {
            Ok(MailboxCursor {
                name: row.get(0)?,
                uidvalidity: row.get(1)?,
                last_uid: row.get(2)?,
                modseq: row.get(3)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
    }

    /// Remet les curseurs au début. La liste vide relit tout le compte.
    ///
    /// # Errors
    ///
    /// Écriture impossible.
    pub fn rewind_cursors(&self) -> Result<(), AppError> {
        self.conn
            .execute("UPDATE mailbox SET last_uid = 0, modseq = NULL", [])?;
        Ok(())
    }

    /// Génération d'adresses déjà appliquée au miroir. Zéro : aucune.
    ///
    /// # Errors
    ///
    /// Lecture impossible.
    pub fn applied_generation(&self) -> Result<i64, AppError> {
        self.conn
            .query_row(
                "SELECT applied_generation FROM releve WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map(|value| value.unwrap_or(0))
            .map_err(AppError::from)
    }

    /// Note la génération d'adresses que ce miroir a déjà appliquée.
    ///
    /// # Errors
    ///
    /// Écriture impossible.
    pub fn set_applied_generation(&self, generation: i64) -> Result<(), AppError> {
        self.conn.execute(
            "INSERT INTO releve (id, applied_generation) VALUES (1, ?1)
             ON CONFLICT(id) DO UPDATE SET applied_generation = excluded.applied_generation",
            [generation],
        )?;
        Ok(())
    }

    /// Retire les lignes dont l'UID n'est pas tenu. Renvoie les `message_id` effacés.
    ///
    /// # Errors
    ///
    /// Écriture impossible.
    pub fn retain_uids(
        &self,
        name: &str,
        uidvalidity: i64,
        keep: &[i64],
    ) -> Result<Vec<String>, AppError> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "CREATE TEMP TABLE IF NOT EXISTS griffe_keep (uid INTEGER PRIMARY KEY)",
            [],
        )?;
        tx.execute("DELETE FROM griffe_keep", [])?;
        {
            let mut insert = tx.prepare("INSERT INTO griffe_keep (uid) VALUES (?1)")?;
            for uid in keep {
                insert.execute([uid])?;
            }
        }
        let ids = {
            let mut stmt = tx.prepare(
                "SELECT message_id FROM message
                 WHERE mailbox = ?1 AND uidvalidity = ?2
                   AND message_id IS NOT NULL
                   AND uid NOT IN (SELECT uid FROM griffe_keep)",
            )?;
            let rows = stmt.query_map(params![name, uidvalidity], |row| row.get(0))?;
            rows.collect::<Result<Vec<String>, _>>()?
        };
        tx.execute(
            "DELETE FROM message
             WHERE mailbox = ?1 AND uidvalidity = ?2
               AND uid NOT IN (SELECT uid FROM griffe_keep)",
            params![name, uidvalidity],
        )?;
        tx.commit()?;
        Ok(ids)
    }

    /// Oublie les lignes d'un dossier dont les UID ne valent plus.
    ///
    /// # Errors
    ///
    /// Écriture impossible.
    pub fn clear_mailbox(&self, name: &str) -> Result<(), AppError> {
        self.conn
            .execute("DELETE FROM message WHERE mailbox = ?1", [name])?;
        self.conn
            .execute("DELETE FROM mailbox WHERE name = ?1", [name])?;
        Ok(())
    }

    /// # Errors
    ///
    /// Lecture impossible.
    pub fn has_uid(&self, mailbox: &str, uidvalidity: i64, uid: i64) -> Result<bool, AppError> {
        let found: i64 = self.conn.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM message
                WHERE mailbox = ?1 AND uidvalidity = ?2 AND uid = ?3
            )",
            params![mailbox, uidvalidity, uid],
            |row| row.get(0),
        )?;
        Ok(found == 1)
    }

    /// # Errors
    ///
    /// Écriture impossible.
    pub fn insert(&self, message: &MirrorInsert) -> Result<(), AppError> {
        let id = Uuid::now_v7().to_string();
        self.conn.execute(
            "INSERT INTO message (
                id, mailbox, uid, uidvalidity, message_id, from_name, from_address,
                subject, sent_on, text, html, seen, notice, notice_address,
                folder_role, choice_of, choice_ids, body_state
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18
             )",
            params![
                id,
                message.mailbox,
                message.uid,
                message.uidvalidity,
                message.message_id,
                message.from_name,
                message.from_address,
                message.subject,
                message.sent_on,
                message.text,
                message.html,
                i64::from(message.seen),
                message.notice,
                message.notice_address,
                message.folder_role,
                message.choice_of,
                message.choice_ids,
                message.body_state,
            ],
        )?;
        Ok(())
    }

    /// Avance le curseur. `last_uid` ne recule pas, sauf après un oubli du dossier.
    ///
    /// # Errors
    ///
    /// Écriture impossible.
    pub fn remember(
        &self,
        name: &str,
        uidvalidity: i64,
        last_uid: i64,
        modseq: Option<i64>,
    ) -> Result<(), AppError> {
        self.conn.execute(
            "INSERT INTO mailbox (name, uidvalidity, last_uid, modseq) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(name) DO UPDATE SET
                uidvalidity = excluded.uidvalidity,
                last_uid = MAX(mailbox.last_uid, excluded.last_uid),
                modseq = COALESCE(excluded.modseq, mailbox.modseq)",
            params![name, uidvalidity, last_uid, modseq],
        )?;
        Ok(())
    }

    /// Nombre de lettres du miroir.
    ///
    /// # Errors
    ///
    /// Lecture impossible.
    pub fn count(&self) -> Result<i64, AppError> {
        self.conn
            .query_row("SELECT COUNT(*) FROM message", [], |row| row.get(0))
            .map_err(AppError::from)
    }

    /// Identifiants des dernières lignes déposées. Un seul écrivain : ce sont
    /// les `limit` insertions les plus récentes.
    ///
    /// # Errors
    ///
    /// Lecture impossible.
    pub fn recent_ids(&self, limit: u32) -> Result<Vec<String>, AppError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM message ORDER BY rowid DESC LIMIT ?1")?;
        let rows = stmt.query_map([i64::from(limit)], |row| row.get(0))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
    }

    /// # Errors
    ///
    /// Lecture impossible.
    pub fn list(&self) -> Result<Vec<MirrorMessage>, AppError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, mailbox, uid, message_id, from_name, from_address, subject, sent_on,
                    seen, notice, notice_address, folder_role, choice_of, choice_ids
             FROM message
             ORDER BY sent_on DESC, uid DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            let seen: i64 = row.get(8)?;
            Ok(MirrorMessage {
                id: row.get(0)?,
                mailbox: row.get(1)?,
                uid: row.get(2)?,
                message_id: row.get(3)?,
                from_name: row.get(4)?,
                from_address: row.get(5)?,
                subject: row.get(6)?,
                sent_on: row.get(7)?,
                seen: seen == 1,
                notice: row.get(9)?,
                notice_address: row.get(10)?,
                folder_role: row.get(11)?,
                choice_of: row.get(12)?,
                choice_ids: row.get(13)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
    }

    /// Texte d'un message, pour les tests et la page. Absent s'il n'est pas là.
    ///
    /// # Errors
    ///
    /// Lecture impossible.
    pub fn text_of(&self, id: &str) -> Result<Option<String>, AppError> {
        self.conn
            .query_row("SELECT text FROM message WHERE id = ?1", [id], |row| {
                row.get(0)
            })
            .optional()
            .map_err(AppError::from)
    }

    /// Message complet, corps compris. Absent s'il n'est pas là.
    ///
    /// # Errors
    ///
    /// Lecture impossible.
    pub fn read(&self, id: &str) -> Result<Option<MirrorLetter>, AppError> {
        self.conn
            .query_row(
                "SELECT id, mailbox, uid, message_id, from_name, from_address, subject, sent_on,
                        text, html, seen, notice, notice_address, folder_role, choice_of, choice_ids,
                        body_state
                 FROM message WHERE id = ?1",
                [id],
                letter_row,
            )
            .optional()
            .map_err(AppError::from)
    }

    /// Passe le message en lu dans le miroir. `false` : déjà lu, ou absent.
    ///
    /// # Errors
    ///
    /// Écriture impossible.
    pub fn mark_seen(&self, id: &str) -> Result<bool, AppError> {
        let updated = self.conn.execute(
            "UPDATE message SET seen = 1 WHERE id = ?1 AND seen = 0",
            [id],
        )?;
        Ok(updated == 1)
    }

    /// Dossier du serveur et UID, pour marquer lu sur le compte.
    ///
    /// # Errors
    ///
    /// Lecture impossible.
    pub fn place(&self, id: &str) -> Result<Option<SeenPlace>, AppError> {
        self.conn
            .query_row(
                "SELECT mailbox, uid, seen FROM message WHERE id = ?1",
                [id],
                |row| {
                    let seen: i64 = row.get(2)?;
                    Ok(SeenPlace {
                        mailbox: row.get(0)?,
                        uid: row.get(1)?,
                        seen: seen == 1,
                    })
                },
            )
            .optional()
            .map_err(AppError::from)
    }

    /// Noms de dossiers déjà vus. Les autres ne sont pas inventés.
    ///
    /// # Errors
    ///
    /// Lecture impossible.
    pub fn folder_names(&self) -> Result<Vec<String>, AppError> {
        let mut stmt = self.conn.prepare(
            "SELECT name FROM mailbox
             UNION
             SELECT mailbox FROM message
             ORDER BY 1",
        )?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
    }

    /// Dernière relève aboutie, et la phrase de l'échec encore en cours.
    /// L'échec n'efface pas l'heure.
    ///
    /// # Errors
    ///
    /// Lecture impossible.
    pub fn releve_stamp(&self) -> Result<(Option<String>, Option<String>), AppError> {
        self.conn
            .query_row("SELECT at, error FROM releve WHERE id = 1", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .optional()
            .map(|row| row.unwrap_or((None, None)))
            .map_err(AppError::from)
    }

    /// Note l'heure d'une relève aboutie, ou la phrase d'un échec.
    /// `at` absent garde l'heure précédente. `error` absent l'efface.
    ///
    /// # Errors
    ///
    /// Écriture impossible.
    pub fn note_releve(&self, at: Option<&str>, error: Option<&str>) -> Result<(), AppError> {
        self.conn.execute(
            "INSERT INTO releve (id, at, error) VALUES (1, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET
                at = COALESCE(excluded.at, releve.at),
                error = excluded.error",
            params![at, error],
        )?;
        Ok(())
    }

    /// Corps encore absent pour cette ligne. `None` s'il est déjà là, ou si la ligne n'existe pas.
    ///
    /// # Errors
    ///
    /// Lecture impossible.
    pub fn pending_of(&self, id: &str) -> Result<Option<PendingBody>, AppError> {
        self.conn
            .query_row(
                "SELECT mailbox, uid, uidvalidity FROM message
                 WHERE id = ?1 AND body_state = 'pending'",
                [id],
                |row| {
                    Ok(PendingBody {
                        mailbox: row.get(0)?,
                        uid: row.get(1)?,
                        uidvalidity: row.get(2)?,
                    })
                },
            )
            .optional()
            .map_err(AppError::from)
    }

    /// Corps encore absents, les plus récemment déposés d'abord.
    ///
    /// # Errors
    ///
    /// Lecture impossible.
    pub fn pending_bodies(&self, limit: u32) -> Result<Vec<PendingBody>, AppError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut stmt = self.conn.prepare(
            "SELECT mailbox, uid, uidvalidity FROM message
             WHERE body_state = 'pending'
             ORDER BY rowid DESC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map([i64::from(limit)], |row| {
            Ok(PendingBody {
                mailbox: row.get(0)?,
                uid: row.get(1)?,
                uidvalidity: row.get(2)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
    }

    /// État du corps. `None` si la ligne n'est pas là.
    ///
    /// # Errors
    ///
    /// Lecture impossible.
    pub fn body_state_of(
        &self,
        mailbox: &str,
        uidvalidity: i64,
        uid: i64,
    ) -> Result<Option<String>, AppError> {
        self.conn
            .query_row(
                "SELECT body_state FROM message
                 WHERE mailbox = ?1 AND uidvalidity = ?2 AND uid = ?3",
                params![mailbox, uidvalidity, uid],
                |row| row.get(0),
            )
            .optional()
            .map_err(AppError::from)
    }

    /// Écrit le corps d'une ligne déjà posée par ses en-têtes.
    ///
    /// # Errors
    ///
    /// Écriture impossible.
    #[allow(clippy::too_many_arguments)]
    pub fn fill_body(
        &self,
        mailbox: &str,
        uidvalidity: i64,
        uid: i64,
        text: &str,
        html: Option<&str>,
        body_state: &str,
        notice: Option<&str>,
        notice_address: Option<&str>,
    ) -> Result<bool, AppError> {
        let updated = self.conn.execute(
            "UPDATE message
             SET text = ?1, html = ?2, body_state = ?3, notice = ?4, notice_address = ?5
             WHERE mailbox = ?6 AND uidvalidity = ?7 AND uid = ?8",
            params![
                text,
                html,
                body_state,
                notice,
                notice_address,
                mailbox,
                uidvalidity,
                uid
            ],
        )?;
        Ok(updated == 1)
    }

    /// Met à jour le drapeau lu d'une ligne déjà indexée.
    ///
    /// # Errors
    ///
    /// Écriture impossible.
    pub fn set_seen_uid(
        &self,
        mailbox: &str,
        uidvalidity: i64,
        uid: i64,
        seen: bool,
    ) -> Result<(), AppError> {
        self.conn.execute(
            "UPDATE message SET seen = ?1
             WHERE mailbox = ?2 AND uidvalidity = ?3 AND uid = ?4",
            params![i64::from(seen), mailbox, uidvalidity, uid],
        )?;
        Ok(())
    }
}

/// Lettre du miroir, corps compris. Le coffre comptable ne la reçoit pas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirrorLetter {
    pub id: String,
    pub mailbox: String,
    pub uid: i64,
    pub message_id: Option<String>,
    pub from_name: Option<String>,
    pub from_address: Option<String>,
    pub subject: String,
    pub sent_on: Option<String>,
    pub text: String,
    pub html: Option<String>,
    pub seen: bool,
    pub notice: Option<String>,
    pub notice_address: Option<String>,
    pub folder_role: String,
    pub choice_of: Option<String>,
    pub choice_ids: Option<String>,
    /// `pending`, `ready`, `too_large` ou `unreadable`.
    pub body_state: String,
}

/// Corps encore à demander au serveur.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingBody {
    pub mailbox: String,
    pub uid: i64,
    pub uidvalidity: i64,
}

/// Où marquer lu sur le serveur.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeenPlace {
    pub mailbox: String,
    pub uid: i64,
    pub seen: bool,
}

fn letter_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MirrorLetter> {
    let seen: i64 = row.get(10)?;
    Ok(MirrorLetter {
        id: row.get(0)?,
        mailbox: row.get(1)?,
        uid: row.get(2)?,
        message_id: row.get(3)?,
        from_name: row.get(4)?,
        from_address: row.get(5)?,
        subject: row.get(6)?,
        sent_on: row.get(7)?,
        text: row.get(8)?,
        html: row.get(9)?,
        seen: seen == 1,
        notice: row.get(11)?,
        notice_address: row.get(12)?,
        folder_role: row.get(13)?,
        choice_of: row.get(14)?,
        choice_ids: row.get(15)?,
        body_state: row.get(16)?,
    })
}
