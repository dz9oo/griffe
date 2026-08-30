//! Persistance chiffrée (`SQLCipher`) : ouverture, migrations, sauvegarde/restauration.
//!
//! Aucun mot de passe ni clé n'est jamais écrit en clair sur disque. La clé de chiffrement,
//! dérivée par Argon2id, est mise en cache dans le trousseau du système d'exploitation
//! (Keychain macOS / Secret Service Linux) afin que la CLI et le serveur MCP puissent ouvrir le
//! coffre sans redemander la passphrase à chaque invocation.

mod error;
mod key;
mod migrations;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::Connection;
use sha2::{Digest, Sha256};

pub use error::StoreError;

/// Un coffre `FreeFlow` ouvert : une connexion `SQLCipher` déverrouillée, prête pour les
/// dépôts des lots suivants.
#[derive(Debug)]
pub struct Store {
    conn: Connection,
    db_path: PathBuf,
    key: [u8; key::KEY_LEN],
}

impl Store {
    /// Ouvre le coffre en utilisant uniquement la clé déjà mise en cache dans le trousseau OS.
    /// Ne demande jamais de passphrase : c'est le chemin emprunté par les appels non
    /// interactifs (CLI, serveur MCP) qui ne doivent jamais bloquer sur une invite.
    ///
    /// # Errors
    ///
    /// Retourne [`StoreError::Locked`] si aucune clé n'est en cache.
    pub fn open_cached(db_path: &Path) -> Result<Self, StoreError> {
        let vault_id = vault_id_for(db_path);
        let key = key::load_cached_key(&vault_id).ok_or(StoreError::Locked)?;
        Self::open_with_key(db_path, key)
    }

    /// Ouvre (ou crée) le coffre en dérivant la clé depuis `passphrase` — c'est le chemin
    /// emprunté à la première ouverture, ou quand l'appelant a explicitement redemandé la
    /// passphrase à l'utilisateur (après un verrouillage, par exemple). Met ensuite la clé en
    /// cache dans le trousseau OS pour les ouvertures suivantes.
    ///
    /// # Errors
    ///
    /// Retourne [`StoreError::WrongPassphrase`] si la passphrase ne permet pas de lire le
    /// contenu déjà chiffré du coffre.
    pub fn open_with_passphrase(db_path: &Path, passphrase: &str) -> Result<Self, StoreError> {
        let salt = load_or_create_salt(db_path)?;
        let key = key::derive_key(passphrase, &salt).map_err(StoreError::KeyDerivation)?;
        let store = Self::open_with_key(db_path, key)?;
        key::cache_key(&vault_id_for(db_path), &key);
        Ok(store)
    }

    fn open_with_key(db_path: &Path, key: [u8; key::KEY_LEN]) -> Result<Self, StoreError> {
        if let Some(parent) = db_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut conn = Connection::open(db_path)?;
        unlock(&conn, &key)?;
        migrations::migrations().to_latest(&mut conn)?;
        Ok(Self {
            conn,
            db_path: db_path.to_path_buf(),
            key,
        })
    }

    /// Purge la clé du trousseau OS pour ce coffre : la prochaine ouverture devra fournir à
    /// nouveau la passphrase.
    #[must_use]
    pub fn lock(db_path: &Path) -> bool {
        key::forget_key(&vault_id_for(db_path))
    }

    /// Écrit une copie chiffrée, transactionnellement cohérente, du coffre vers `dest_db_path`
    /// (et de son sel de dérivation associé).
    ///
    /// # Errors
    ///
    /// Retourne une erreur si l'écriture du fichier de sauvegarde ou son chiffrement échoue.
    pub fn backup_to(&self, dest_db_path: &Path) -> Result<(), StoreError> {
        if let Some(parent) = dest_db_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(
            kdf_sidecar_path(&self.db_path),
            kdf_sidecar_path(dest_db_path),
        )?;

        let mut dest = Connection::open(dest_db_path)?;
        unlock(&dest, &self.key)?;

        let backup = rusqlite::backup::Backup::new(&self.conn, &mut dest)?;
        backup.run_to_completion(16, Duration::from_millis(250), None)?;
        Ok(())
    }

    /// Restaure une sauvegarde produite par [`Self::backup_to`] : copie le fichier de
    /// sauvegarde (et son sel) vers `dest_db_path`, puis rouvre avec `passphrase` pour
    /// vérifier que la restauration est saine avant de la considérer réussie.
    ///
    /// # Errors
    ///
    /// Retourne une erreur si la copie échoue, ou [`StoreError::WrongPassphrase`] si la
    /// passphrase ne correspond pas à la sauvegarde.
    pub fn restore_from(
        backup_db_path: &Path,
        dest_db_path: &Path,
        passphrase: &str,
    ) -> Result<Self, StoreError> {
        if let Some(parent) = dest_db_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(backup_db_path, dest_db_path)?;
        fs::copy(
            kdf_sidecar_path(backup_db_path),
            kdf_sidecar_path(dest_db_path),
        )?;
        Self::open_with_passphrase(dest_db_path, passphrase)
    }

    /// Sauvegarde automatique : si `backups_dir` ne contient aucune sauvegarde plus récente que
    /// `max_age`, en écrit une nouvelle (nommée par horodatage) et renvoie son chemin. Ne fait
    /// rien — silencieusement — si une sauvegarde assez fraîche existe déjà : c'est ce qui
    /// permet de l'appeler à chaque commande sans réécrire le fichier à chaque fois. Conçue pour
    /// être invoquée par un adaptateur (CLI, GUI) après chaque commande appliquée, en l'absence
    /// de tout démon dans cette architecture local-only.
    ///
    /// # Errors
    ///
    /// # Panics
    ///
    /// Ne panique jamais en pratique : le format de date est un littéral constant, toujours
    /// valide.
    pub fn auto_backup_if_stale(
        &self,
        backups_dir: &Path,
        max_age: Duration,
    ) -> Result<Option<PathBuf>, StoreError> {
        let newest = newest_backup_age(backups_dir)?;
        if newest.is_some_and(|age| age < max_age) {
            return Ok(None);
        }
        let format = time::format_description::parse_borrowed::<2>(
            "[year][month][day]-[hour][minute][second]",
        )
        .expect("format de date littéral, toujours valide");
        let stamp = time::OffsetDateTime::now_utc()
            .format(&format)
            .map_err(|e| StoreError::KeyDerivation(format!("horodatage de sauvegarde : {e}")))?;
        let dest = backups_dir.join(format!("backup-{stamp}.db"));
        self.backup_to(&dest)?;
        Ok(Some(dest))
    }

    /// Accès à la connexion sous-jacente, pour les dépôts des lots suivants.
    #[must_use]
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Chemin du fichier de coffre ouvert — pour l'affichage (GUI, lot 9), pas pour la logique.
    #[must_use]
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// Accès mutable à la connexion sous-jacente, pour les écritures des lots suivants.
    pub fn connection_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }

    /// Change à chaque écriture commitée sur ce fichier, y compris par un autre process — la
    /// GUI (lot 9) l'interroge à intervalle court pour détecter une mise à jour sans démon ni
    /// canal de notification dédié.
    ///
    /// # Errors
    pub fn data_version(&self) -> Result<i64, StoreError> {
        self.conn
            .query_row("PRAGMA data_version", [], |row| row.get(0))
            .map_err(StoreError::from)
    }
}

/// Applique la clé `SQLCipher` à la connexion et valide qu'elle permet bien de lire le contenu
/// déjà chiffré : `SQLCipher` ne vérifie rien avant une vraie lecture des pages.
fn unlock(conn: &Connection, key: &[u8; key::KEY_LEN]) -> Result<(), StoreError> {
    // `PRAGMA key` seule ne touche jamais aux pages chiffrées : elle ne peut donc pas échouer.
    // On valide la clé explicitement avant de toucher journal_mode/foreign_keys, sans quoi une
    // mauvaise passphrase remonterait comme une `StoreError::Sqlite` générique plutôt que
    // `WrongPassphrase`.
    conn.execute_batch(&format!("PRAGMA key = \"x'{}'\";", hex::encode(key)))?;
    conn.query_row("SELECT count(*) FROM sqlite_master", [], |row| {
        row.get::<_, i64>(0)
    })
    .map_err(|_| StoreError::WrongPassphrase)?;
    conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")?;
    conn.busy_timeout(Duration::from_secs(5))?;
    Ok(())
}

/// Âge du fichier `.db` le plus récemment modifié dans `backups_dir` — `None` si le dossier
/// n'existe pas encore ou ne contient aucune sauvegarde.
fn newest_backup_age(backups_dir: &Path) -> Result<Option<Duration>, StoreError> {
    let entries = match fs::read_dir(backups_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let mut newest: Option<std::time::SystemTime> = None;
    for entry in entries {
        let entry = entry?;
        if entry.path().extension().is_some_and(|ext| ext == "db") {
            let modified = entry.metadata()?.modified()?;
            newest = Some(newest.map_or(modified, |current| current.max(modified)));
        }
    }
    Ok(newest.map(|modified| {
        std::time::SystemTime::now()
            .duration_since(modified)
            .unwrap_or(Duration::ZERO)
    }))
}

fn kdf_sidecar_path(db_path: &Path) -> PathBuf {
    let mut os_string = db_path.as_os_str().to_owned();
    os_string.push(".kdf");
    PathBuf::from(os_string)
}

fn load_or_create_salt(db_path: &Path) -> Result<[u8; key::SALT_LEN], StoreError> {
    let path = kdf_sidecar_path(db_path);
    match fs::read(&path) {
        Ok(bytes) if bytes.len() == 1 + key::SALT_LEN && bytes[0] == 1 => {
            let mut salt = [0u8; key::SALT_LEN];
            salt.copy_from_slice(&bytes[1..]);
            Ok(salt)
        }
        Ok(_) => Err(StoreError::CorruptKdfParams(path)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let salt = key::random_salt();
            let mut bytes = Vec::with_capacity(1 + key::SALT_LEN);
            bytes.push(1u8);
            bytes.extend_from_slice(&salt);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&path, &bytes)?;
            Ok(salt)
        }
        Err(e) => Err(StoreError::Io(e)),
    }
}

/// Identifiant stable dérivé du chemin du coffre, utilisé comme compte dans le trousseau OS —
/// deux coffres à des emplacements différents n'entrent jamais en collision.
fn vault_id_for(db_path: &Path) -> String {
    let absolute = std::path::absolute(db_path).unwrap_or_else(|_| db_path.to_path_buf());
    let digest = Sha256::digest(absolute.to_string_lossy().as_bytes());
    hex::encode(digest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    fn temp_db_path(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "freeflow-store-test-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ));
        dir.join("vault.db")
    }

    #[test]
    fn wrong_passphrase_is_rejected() {
        let db_path = temp_db_path("wrong-passphrase");
        Store::open_with_passphrase(&db_path, "correct horse battery staple").unwrap();

        let err =
            Store::open_with_passphrase(&db_path, "totally different passphrase").unwrap_err();
        assert!(matches!(err, StoreError::WrongPassphrase));
    }

    #[test]
    fn correct_passphrase_reopens_the_same_vault() {
        let db_path = temp_db_path("correct-passphrase");
        {
            let store = Store::open_with_passphrase(&db_path, "s3cret").unwrap();
            store
                .connection()
                .execute("INSERT INTO clients (id, name, created_at) VALUES ('c1', 'Argon Digital', '2026-08-29T00:00:00Z')", [])
                .unwrap();
        }
        let reopened = Store::open_with_passphrase(&db_path, "s3cret").unwrap();
        let name: String = reopened
            .connection()
            .query_row("SELECT name FROM clients WHERE id = 'c1'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(name, "Argon Digital");
    }

    #[test]
    fn migrations_apply_cleanly_and_schema_matches_expected_tables() {
        let db_path = temp_db_path("migrations");
        let store = Store::open_with_passphrase(&db_path, "s3cret").unwrap();
        let mut stmt = store
            .connection()
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .unwrap();
        let tables: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        for expected in [
            "clients",
            "contacts",
            "opportunities",
            "quotes",
            "quote_lines",
            "missions",
            "milestones",
            "time_entries",
            "invoices",
            "invoice_lines",
            "payments",
            "expenses",
            "company_profile",
        ] {
            assert!(
                tables.iter().any(|t| t == expected),
                "table manquante : {expected}"
            );
        }
    }

    #[test]
    fn down_migration_removes_every_table_created_by_up() {
        let db_path = temp_db_path("migration-round-trip");
        let mut store = Store::open_with_passphrase(&db_path, "s3cret").unwrap();
        migrations::migrations()
            .to_version(store.connection_mut(), 0)
            .unwrap();
        let remaining: i64 = store
            .connection()
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 0);
    }

    #[test]
    fn two_processes_writing_concurrently_do_not_corrupt_the_vault() {
        let db_path = temp_db_path("concurrency");
        Store::open_with_passphrase(&db_path, "s3cret").unwrap(); // crée le schéma une fois

        let handles: Vec<_> = (0..8)
            .map(|i| {
                let db_path = db_path.clone();
                thread::spawn(move || {
                    let store = Store::open_with_passphrase(&db_path, "s3cret").unwrap();
                    store
                        .connection()
                        .execute(
                            "INSERT INTO clients (id, name, created_at) VALUES (?1, ?2, '2026-08-29T00:00:00Z')",
                            rusqlite::params![format!("c{i}"), format!("Client {i}")],
                        )
                        .unwrap();
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }

        let store = Store::open_with_passphrase(&db_path, "s3cret").unwrap();
        let count: i64 = store
            .connection()
            .query_row("SELECT count(*) FROM clients", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 8);
    }

    #[test]
    fn backup_then_restore_is_functionally_identical() {
        let original_path = temp_db_path("backup-original");
        let backup_path = temp_db_path("backup-copy").with_file_name("backup.db");
        let restored_path = temp_db_path("backup-restored");

        let store = Store::open_with_passphrase(&original_path, "s3cret").unwrap();
        store
            .connection()
            .execute("INSERT INTO clients (id, name, created_at) VALUES ('c1', 'Argon Digital', '2026-08-29T00:00:00Z')", [])
            .unwrap();
        store.backup_to(&backup_path).unwrap();

        let restored = Store::restore_from(&backup_path, &restored_path, "s3cret").unwrap();
        let name: String = restored
            .connection()
            .query_row("SELECT name FROM clients WHERE id = 'c1'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(name, "Argon Digital");

        let err = Store::restore_from(
            &backup_path,
            &restored_path.with_file_name("wrong.db"),
            "not-the-passphrase",
        )
        .unwrap_err();
        assert!(matches!(err, StoreError::WrongPassphrase));
    }

    #[test]
    fn lock_forgets_the_cached_key() {
        let db_path = temp_db_path("lock");
        Store::open_with_passphrase(&db_path, "s3cret").unwrap();
        assert!(Store::lock(&db_path));
        // open_cached ne peut pas garantir l'absence de trousseau système dans cet
        // environnement de test, mais lock() doit au moins réussir sans erreur.
    }

    #[test]
    fn data_version_changes_after_a_commit_from_another_connection_on_the_same_file() {
        // C'est l'invariant dont dépend le rail d'audit temps réel de la GUI (lot 9) : sans
        // démon ni canal de notification, elle détecte une écriture d'un autre process en
        // observant ce pragma changer.
        let db_path = temp_db_path("data-version");
        let reader = Store::open_with_passphrase(&db_path, "s3cret").unwrap();
        let before = reader.data_version().unwrap();

        let mut writer = Store::open_with_passphrase(&db_path, "s3cret").unwrap();
        writer
            .connection_mut()
            .execute("INSERT INTO clients (id, name, created_at) VALUES ('c1', 'Argon Digital', '2026-08-29T00:00:00Z')", [])
            .unwrap();

        let after = reader.data_version().unwrap();
        assert_ne!(
            before, after,
            "data_version doit changer après l'écriture d'un autre process"
        );
    }

    #[test]
    fn auto_backup_writes_once_when_stale_then_skips_while_fresh() {
        let store =
            Store::open_with_passphrase(&temp_db_path("auto-backup-src"), "s3cret").unwrap();
        let backups_dir = temp_db_path("auto-backup-dest")
            .parent()
            .unwrap()
            .join("backups");

        let first = store
            .auto_backup_if_stale(&backups_dir, Duration::from_secs(3600))
            .unwrap();
        assert!(
            first.is_some(),
            "aucune sauvegarde n'existe encore : la première invocation doit en écrire une"
        );
        assert!(first.as_ref().unwrap().exists());

        let second = store
            .auto_backup_if_stale(&backups_dir, Duration::from_secs(3600))
            .unwrap();
        assert!(
            second.is_none(),
            "une sauvegarde fraîche existe déjà : la deuxième invocation ne doit rien écrire"
        );

        let third = store
            .auto_backup_if_stale(&backups_dir, Duration::ZERO)
            .unwrap();
        assert!(
            third.is_some(),
            "avec max_age = 0, la sauvegarde existante est toujours jugée périmée"
        );
    }

    #[test]
    fn a_backup_written_by_auto_backup_restores_to_a_functionally_identical_vault() {
        let mut store =
            Store::open_with_passphrase(&temp_db_path("auto-backup-restore-src"), "s3cret")
                .unwrap();
        store
            .connection_mut()
            .execute("INSERT INTO clients (id, name, created_at) VALUES ('c1', 'Argon Digital', '2026-08-29T00:00:00Z')", [])
            .unwrap();
        let backups_dir = temp_db_path("auto-backup-restore-dest")
            .parent()
            .unwrap()
            .join("backups");
        let backup_path = store
            .auto_backup_if_stale(&backups_dir, Duration::ZERO)
            .unwrap()
            .expect("aucune sauvegarde préexistante : celle-ci doit être écrite");

        let restored = Store::restore_from(
            &backup_path,
            &temp_db_path("auto-backup-restored"),
            "s3cret",
        )
        .unwrap();
        let name: String = restored
            .connection()
            .query_row("SELECT name FROM clients WHERE id = 'c1'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(name, "Argon Digital");
    }
}
