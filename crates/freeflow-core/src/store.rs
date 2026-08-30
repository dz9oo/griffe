//! Persistance chiffrée (`SQLCipher`) : ouverture, migrations, sauvegarde/restauration.
//!
//! Aucun mot de passe ni clé n'est jamais écrit en clair sur disque, ni dans l'environnement du
//! process. La clé de chiffrement, dérivée par Argon2id (voir [`kdf`]), n'est mise en cache dans
//! le trousseau du système d'exploitation que sur demande explicite ([`Store::remember`]), et
//! toujours avec une expiration bornée que [`Store::open_cached`] vérifie à chaque appel — une
//! entrée expirée est purgée, pas seulement ignorée. Cette ouverture cœur ne fait jamais d'IO
//! interactive (pas de prompt, pas d'exécution de commande externe) : c'est la responsabilité de
//! l'adaptateur appelant (CLI, GUI) d'acquérir la passphrase, puis de la remettre ici.

mod error;
mod kdf;
mod key;
mod migrations;
mod secret;

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::Connection;
use time::OffsetDateTime;
use zeroize::Zeroize;

pub use error::StoreError;
#[cfg(any(test, feature = "test-support"))]
pub use key::InMemoryKeyCache;
pub use key::{KeyCache, OsKeyring};
pub use secret::{Passphrase, VaultKey};

/// État d'un emplacement de coffre, sans le déverrouiller — pour `vault status` et l'écran
/// d'accueil de la GUI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultStatus {
    /// Aucun `.db`/`.kdf` à ce chemin : rien à déverrouiller, seulement à créer.
    Absent,
    Exists {
        /// `1` (coffre pas encore migré) ou `2`.
        sidecar_version: u8,
        /// `Some` si une session est en cache dans le trousseau et n'est pas expirée.
        session_expires_at: Option<OffsetDateTime>,
    },
}

/// Un coffre `FreeFlow` ouvert : une connexion `SQLCipher` déverrouillée.
pub struct Store {
    conn: Connection,
    db_path: PathBuf,
    key: VaultKey,
    vault_id: kdf::VaultId,
}

impl fmt::Debug for Store {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Store")
            .field("db_path", &self.db_path)
            .finish_non_exhaustive()
    }
}

impl Store {
    /// Ouvre le coffre en utilisant uniquement une clé déjà mise en cache et non expirée dans le
    /// trousseau OS. Ne demande jamais de passphrase, n'exécute jamais rien d'interactif : c'est
    /// le chemin emprunté par les appels non interactifs (serveur MCP, console de la GUI).
    ///
    /// # Errors
    ///
    /// [`StoreError::VaultNotFound`] si aucun coffre n'existe à `db_path` ;
    /// [`StoreError::Locked`] si aucune session valide n'est en cache.
    pub fn open_cached(db_path: &Path) -> Result<Self, StoreError> {
        Self::open_cached_with(db_path, &OsKeyring)
    }

    /// # Errors
    pub fn open_cached_with(db_path: &Path, cache: &dyn KeyCache) -> Result<Self, StoreError> {
        let sidecar =
            kdf::read(db_path)?.ok_or_else(|| StoreError::VaultNotFound(db_path.to_path_buf()))?;
        let account = sidecar.vault_id.as_account();
        let cached = cache.load(&account).ok_or(StoreError::Locked)?;
        if !sidecar.verify(&cached.key) {
            // La session en cache ne correspond plus au sidecar (coffre recréé entre-temps,
            // par exemple) : la purger plutôt que d'ouvrir un coffre incohérent.
            let _ = cache.forget(&account);
            return Err(StoreError::Locked);
        }
        Self::open_with_key(db_path, cached.key, sidecar.vault_id)
    }

    /// Crée un coffre neuf. Échoue si un `.db` ou un `.kdf` existe déjà à cet emplacement — une
    /// faute de frappe dans le chemin ne crée donc jamais silencieusement un coffre vide.
    ///
    /// # Errors
    ///
    /// [`StoreError::VaultAlreadyExists`] si le coffre existe déjà.
    pub fn create(db_path: &Path, passphrase: &Passphrase) -> Result<Self, StoreError> {
        if db_path.exists() {
            return Err(StoreError::VaultAlreadyExists(db_path.to_path_buf()));
        }
        let (sidecar, key) = kdf::create(db_path, passphrase)?;
        Self::open_with_key(db_path, key, sidecar.vault_id)
    }

    /// Ouvre un coffre **existant**. Ne crée jamais rien — contrairement au comportement
    /// précédent, qui créait silencieusement un coffre vide sur un chemin inexistant. Migre
    /// automatiquement un sidecar v1 vers v2 après une ouverture réussie (même sel, mêmes
    /// paramètres : la clé ne change pas).
    ///
    /// # Errors
    ///
    /// [`StoreError::VaultNotFound`] si `db_path` n'existe pas ; [`StoreError::WrongPassphrase`]
    /// si la passphrase ne correspond pas au coffre.
    pub fn open_with_passphrase(
        db_path: &Path,
        passphrase: &Passphrase,
    ) -> Result<Self, StoreError> {
        if !db_path.exists() {
            return Err(StoreError::VaultNotFound(db_path.to_path_buf()));
        }
        let sidecar =
            kdf::read(db_path)?.ok_or_else(|| StoreError::VaultNotFound(db_path.to_path_buf()))?;
        let key = kdf::derive_key(passphrase, sidecar.salt(), sidecar.cost())?;
        if !sidecar.verify(&key) {
            return Err(StoreError::WrongPassphrase);
        }

        let sidecar = if sidecar.is_legacy() {
            let legacy_account = sidecar.vault_id.as_account();
            let upgraded = kdf::upgrade_v1_to_v2(db_path, &sidecar, &key)?;
            // La session éventuellement en cache sous l'ancien compte (hash du chemin) ne
            // pourra plus jamais être retrouvée après la migration : autant la purger tout
            // de suite plutôt que de la laisser traîner indéfiniment.
            let _ = OsKeyring.forget(&legacy_account);
            upgraded
        } else {
            sidecar
        };

        Self::open_with_key(db_path, key, sidecar.vault_id)
    }

    fn open_with_key(
        db_path: &Path,
        key: VaultKey,
        vault_id: kdf::VaultId,
    ) -> Result<Self, StoreError> {
        if let Some(parent) = db_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut conn = Connection::open(db_path)?;
        unlock(&conn, &key)?;
        migrations::migrations().to_latest(&mut conn)?;
        tighten_permissions(db_path);
        Ok(Self {
            conn,
            db_path: db_path.to_path_buf(),
            key,
            vault_id,
        })
    }

    /// Met la clé de ce coffre déjà ouvert en cache dans le trousseau OS, pour `ttl` — jamais
    /// automatique, toujours à la demande explicite de l'appelant (`--remember` en CLI, case à
    /// cocher en GUI).
    ///
    /// # Errors
    ///
    /// [`StoreError::KeychainUnavailable`] si le trousseau OS n'est pas accessible : le coffre
    /// reste utilisable, seule la mise en cache pour la prochaine ouverture échoue.
    pub fn remember(&self, ttl: Duration) -> Result<(), StoreError> {
        self.remember_with(ttl, &OsKeyring)
    }

    /// # Errors
    pub fn remember_with(&self, ttl: Duration, cache: &dyn KeyCache) -> Result<(), StoreError> {
        let seconds = i64::try_from(ttl.as_secs()).unwrap_or(i64::MAX);
        let expires_at = OffsetDateTime::now_utc() + time::Duration::seconds(seconds);
        let account = self.vault_id.as_account();
        if cache.store(&account, &self.key, expires_at) {
            Ok(())
        } else {
            Err(StoreError::KeychainUnavailable)
        }
    }

    /// Purge la session en cache dans le trousseau OS pour le coffre situé à `db_path` : la
    /// prochaine ouverture devra fournir à nouveau la passphrase (ou une autre source non
    /// interactive). `true` si l'état résultant est bien « aucune session », y compris quand il
    /// n'y en avait déjà pas.
    #[must_use]
    pub fn lock(db_path: &Path) -> bool {
        Self::lock_with(db_path, &OsKeyring)
    }

    #[must_use]
    pub fn lock_with(db_path: &Path, cache: &dyn KeyCache) -> bool {
        match kdf::read(db_path) {
            Ok(Some(sidecar)) => cache.forget(&sidecar.vault_id.as_account()),
            Ok(None) => true,
            Err(_) => false,
        }
    }

    /// État du coffre à `db_path`, sans le déverrouiller.
    ///
    /// # Errors
    pub fn status(db_path: &Path) -> Result<VaultStatus, StoreError> {
        Self::status_with(db_path, &OsKeyring)
    }

    /// # Errors
    pub fn status_with(db_path: &Path, cache: &dyn KeyCache) -> Result<VaultStatus, StoreError> {
        let Some(sidecar) = kdf::read(db_path)? else {
            return Ok(VaultStatus::Absent);
        };
        let sidecar_version = if sidecar.is_legacy() { 1 } else { 2 };
        let session_expires_at = cache
            .load(&sidecar.vault_id.as_account())
            .map(|c| c.expires_at);
        Ok(VaultStatus::Exists {
            sidecar_version,
            session_expires_at,
        })
    }

    /// Emplacement de coffre par défaut, si l'appelant n'en fournit pas explicitement :
    /// `$XDG_DATA_HOME/freeflow/vault.db` sur Linux (défaut `~/.local/share/freeflow/vault.db`),
    /// `~/Library/Application Support/FreeFlow/vault.db` sur macOS.
    ///
    /// # Errors
    ///
    /// [`StoreError::NoDefaultVaultPath`] si le système ne fournit aucun répertoire de données
    /// utilisateur (`HOME` absent, par exemple).
    pub fn default_vault_path() -> Result<PathBuf, StoreError> {
        let data_dir = dirs::data_dir().ok_or(StoreError::NoDefaultVaultPath)?;
        let app_dir = if cfg!(target_os = "macos") {
            "FreeFlow"
        } else {
            "freeflow"
        };
        Ok(data_dir.join(app_dir).join("vault.db"))
    }

    /// Écrit une copie chiffrée, transactionnellement cohérente, du coffre vers `dest_db_path`
    /// (et de son sel de dérivation associé).
    ///
    /// Limite connue : la sauvegarde partage le `vault_id` — donc le compte trousseau — de
    /// l'original, puisque le sidecar est copié tel quel. Ce n'est pas un problème de
    /// confidentialité (la clé copiée est authentiquement valide pour cette copie), seulement
    /// une curiosité si les deux fichiers sont ouverts et « mémorisés » en parallèle.
    ///
    /// # Errors
    ///
    /// Retourne une erreur si l'écriture du fichier de sauvegarde ou son chiffrement échoue.
    pub fn backup_to(&self, dest_db_path: &Path) -> Result<(), StoreError> {
        if let Some(parent) = dest_db_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(
            kdf::sidecar_path(&self.db_path),
            kdf::sidecar_path(dest_db_path),
        )?;

        let mut dest = Connection::open(dest_db_path)?;
        unlock(&dest, &self.key)?;

        let backup = rusqlite::backup::Backup::new(&self.conn, &mut dest)?;
        backup.run_to_completion(16, Duration::from_millis(250), None)?;
        tighten_permissions(dest_db_path);
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
        passphrase: &Passphrase,
    ) -> Result<Self, StoreError> {
        if let Some(parent) = dest_db_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(backup_db_path, dest_db_path)?;
        fs::copy(
            kdf::sidecar_path(backup_db_path),
            kdf::sidecar_path(dest_db_path),
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

    /// Chemin du fichier de coffre ouvert — pour l'affichage (GUI), pas pour la logique.
    #[must_use]
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// Accès mutable à la connexion sous-jacente, pour les écritures.
    pub fn connection_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }

    /// Change à chaque écriture commitée sur ce fichier, y compris par un autre process — la
    /// GUI l'interroge à intervalle court pour détecter une mise à jour sans démon ni canal de
    /// notification dédié.
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
fn unlock(conn: &Connection, key: &VaultKey) -> Result<(), StoreError> {
    // `PRAGMA key` seule ne touche jamais aux pages chiffrées : elle ne peut donc pas échouer.
    // On valide la clé explicitement avant de toucher journal_mode/foreign_keys, sans quoi une
    // mauvaise passphrase remonterait comme une `StoreError::Sqlite` générique plutôt que
    // `WrongPassphrase`.
    let mut pragma = format!("PRAGMA key = \"x'{}'\";", hex::encode(key.as_bytes()));
    let result = conn.execute_batch(&pragma);
    pragma.zeroize();
    result?;

    conn.query_row("SELECT count(*) FROM sqlite_master", [], |row| {
        row.get::<_, i64>(0)
    })
    .map_err(|_| StoreError::WrongPassphrase)?;
    conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")?;
    conn.busy_timeout(Duration::from_secs(5))?;
    Ok(())
}

/// Resserre les permissions du `.db` et de son sidecar `.kdf` à 0600 (Unix uniquement) : les
/// deux peuvent avoir été créés avec un umask plus permissif que ce que ce coffre exige.
/// Best-effort, ne fait jamais échouer l'ouverture.
fn tighten_permissions(db_path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for path in [db_path.to_path_buf(), kdf::sidecar_path(db_path)] {
            let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = db_path; // pas d'équivalent portable simple ; documenté comme limite Windows.
    }
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

    fn create(db_path: &Path, passphrase: &str) -> Store {
        Store::create(db_path, &Passphrase::from(passphrase)).unwrap()
    }

    fn open(db_path: &Path, passphrase: &str) -> Result<Store, StoreError> {
        Store::open_with_passphrase(db_path, &Passphrase::from(passphrase))
    }

    #[test]
    fn wrong_passphrase_is_rejected() {
        let db_path = temp_db_path("wrong-passphrase");
        create(&db_path, "correct horse battery staple");

        let err = open(&db_path, "totally different passphrase").unwrap_err();
        assert!(matches!(err, StoreError::WrongPassphrase));
    }

    #[test]
    fn wrong_passphrase_is_rejected_even_on_a_freshly_created_empty_vault() {
        // Avant le vérificateur du sidecar v2, une clé quelconque pouvait sembler valide sur un
        // fichier `.db` neuf et vide : `SQLCipher` ne vérifie rien avant de lire une page déjà
        // chiffrée. C'est précisément ce que corrige le sidecar v2.
        let db_path = temp_db_path("wrong-passphrase-empty");
        create(&db_path, "s3cret");

        let err = open(&db_path, "not-s3cret").unwrap_err();
        assert!(matches!(err, StoreError::WrongPassphrase));
    }

    #[test]
    fn open_with_passphrase_never_creates_a_vault() {
        let db_path = temp_db_path("no-silent-create");
        let err = open(&db_path, "s3cret").unwrap_err();
        assert!(matches!(err, StoreError::VaultNotFound(_)));
        assert!(!db_path.exists());
    }

    #[test]
    fn create_refuses_an_existing_vault() {
        let db_path = temp_db_path("create-twice");
        create(&db_path, "s3cret");
        let err = Store::create(&db_path, &Passphrase::from("other")).unwrap_err();
        assert!(matches!(err, StoreError::VaultAlreadyExists(_)));
    }

    #[test]
    fn correct_passphrase_reopens_the_same_vault() {
        let db_path = temp_db_path("correct-passphrase");
        {
            let store = create(&db_path, "s3cret");
            store
                .connection()
                .execute("INSERT INTO clients (id, name, created_at) VALUES ('c1', 'Argon Digital', '2026-08-29T00:00:00Z')", [])
                .unwrap();
        }
        let reopened = open(&db_path, "s3cret").unwrap();
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
        let store = create(&db_path, "s3cret");
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
        let mut store = create(&db_path, "s3cret");
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
        create(&db_path, "s3cret"); // crée le schéma une fois

        let handles: Vec<_> = (0..8)
            .map(|i| {
                let db_path = db_path.clone();
                thread::spawn(move || {
                    let store = open(&db_path, "s3cret").unwrap();
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

        let store = open(&db_path, "s3cret").unwrap();
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

        let store = create(&original_path, "s3cret");
        store
            .connection()
            .execute("INSERT INTO clients (id, name, created_at) VALUES ('c1', 'Argon Digital', '2026-08-29T00:00:00Z')", [])
            .unwrap();
        store.backup_to(&backup_path).unwrap();

        let restored =
            Store::restore_from(&backup_path, &restored_path, &Passphrase::from("s3cret")).unwrap();
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
            &Passphrase::from("not-the-passphrase"),
        )
        .unwrap_err();
        assert!(matches!(err, StoreError::WrongPassphrase));
    }

    #[test]
    fn lock_purges_the_cached_session_and_open_cached_then_fails() {
        let db_path = temp_db_path("lock");
        let cache = InMemoryKeyCache::new();
        let store = create(&db_path, "s3cret");
        store
            .remember_with(Duration::from_secs(3600), &cache)
            .unwrap();
        assert!(Store::open_cached_with(&db_path, &cache).is_ok());

        assert!(Store::lock_with(&db_path, &cache));
        let err = Store::open_cached_with(&db_path, &cache).unwrap_err();
        assert!(matches!(err, StoreError::Locked));
    }

    #[test]
    fn remembering_requires_an_explicit_call_open_with_passphrase_never_caches() {
        let db_path = temp_db_path("no-implicit-cache");
        let cache = InMemoryKeyCache::new();
        create(&db_path, "s3cret");
        assert!(Store::open_cached_with(&db_path, &cache).is_err());

        let store = open(&db_path, "s3cret").unwrap();
        assert!(
            Store::open_cached_with(&db_path, &InMemoryKeyCache::new()).is_err(),
            "open_with_passphrase ne doit jamais mettre la clé en cache implicitement"
        );
        store
            .remember_with(Duration::from_secs(60), &cache)
            .unwrap();
        assert!(Store::open_cached_with(&db_path, &cache).is_ok());
    }

    #[test]
    fn an_expired_session_is_refused_and_purged() {
        let db_path = temp_db_path("expired-session");
        let cache = InMemoryKeyCache::new();
        let store = create(&db_path, "s3cret");
        // TTL de zéro : déjà expiré au moment de la lecture.
        store.remember_with(Duration::ZERO, &cache).unwrap();

        let err = Store::open_cached_with(&db_path, &cache).unwrap_err();
        assert!(matches!(err, StoreError::Locked));
    }

    #[test]
    fn a_vault_recreated_at_the_same_path_never_reuses_the_previous_session() {
        let db_path = temp_db_path("recreate-no-reuse");
        let cache = InMemoryKeyCache::new();
        {
            let store = create(&db_path, "s3cret");
            store
                .remember_with(Duration::from_secs(3600), &cache)
                .unwrap();
        }
        fs::remove_file(&db_path).unwrap();
        fs::remove_file(kdf::sidecar_path(&db_path)).unwrap();
        create(&db_path, "another-passphrase");

        let err = Store::open_cached_with(&db_path, &cache).unwrap_err();
        assert!(matches!(err, StoreError::Locked));
    }

    #[test]
    fn store_debug_never_prints_key_material() {
        let db_path = temp_db_path("debug-redacted");
        let store = create(&db_path, "s3cret");
        let rendered = format!("{store:?}");
        assert!(!rendered.contains(&hex::encode(store.key.as_bytes())));
    }

    #[test]
    fn data_version_changes_after_a_commit_from_another_connection_on_the_same_file() {
        // C'est l'invariant dont dépend le rail d'audit temps réel de la GUI : sans démon ni
        // canal de notification, elle détecte une écriture d'un autre process en observant ce
        // pragma changer.
        let db_path = temp_db_path("data-version");
        let reader = create(&db_path, "s3cret");
        let before = reader.data_version().unwrap();

        let mut writer = open(&db_path, "s3cret").unwrap();
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
        let store = create(&temp_db_path("auto-backup-src"), "s3cret");
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
        let mut store = create(&temp_db_path("auto-backup-restore-src"), "s3cret");
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
            &Passphrase::from("s3cret"),
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
