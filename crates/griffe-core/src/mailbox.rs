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
    last_uid INTEGER NOT NULL
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
    UNIQUE (mailbox, uidvalidity, uid)
);
";

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
        if version > 1 {
            return Err(StoreError::SchemaTooNew);
        }
        if version == 0 {
            conn.execute_batch(SCHEMA)?;
            conn.execute_batch("PRAGMA user_version = 1")?;
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
            .prepare("SELECT name, uidvalidity, last_uid FROM mailbox ORDER BY name")?;
        let rows = stmt.query_map([], |row| {
            Ok(MailboxCursor {
                name: row.get(0)?,
                uidvalidity: row.get(1)?,
                last_uid: row.get(2)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
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
                folder_role, choice_of, choice_ids
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17
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
            ],
        )?;
        Ok(())
    }

    /// Avance le curseur. `last_uid` ne recule pas, sauf après un oubli du dossier.
    ///
    /// # Errors
    ///
    /// Écriture impossible.
    pub fn remember(&self, name: &str, uidvalidity: i64, last_uid: i64) -> Result<(), AppError> {
        self.conn.execute(
            "INSERT INTO mailbox (name, uidvalidity, last_uid) VALUES (?1, ?2, ?3)
             ON CONFLICT(name) DO UPDATE SET
                uidvalidity = excluded.uidvalidity,
                last_uid = MAX(mailbox.last_uid, excluded.last_uid)",
            params![name, uidvalidity, last_uid],
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
                        text, html, seen, notice, notice_address, folder_role, choice_of, choice_ids
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
    })
}
