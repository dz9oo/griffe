//! Persistance chiffrée (`SQLCipher`) : ouverture, migrations, sauvegarde/restauration.
//!
//! Aucun mot de passe ni clé n'est jamais écrit en clair sur disque, ni dans l'environnement du
//! process. Un coffre au format courant (sidecar v3, lot 24) est chiffré par une clé maître
//! aléatoire, scellée dans le sidecar par la clé qu'Argon2id dérive de la passphrase (voir
//! [`kdf`]) ; la clé du coffre n'est mise en cache dans le trousseau du système d'exploitation
//! que sur demande explicite ([`Store::remember`]), et toujours avec une expiration bornée que
//! [`Store::open_cached`] vérifie à chaque appel — une entrée expirée est purgée, pas seulement
//! ignorée. Cette ouverture cœur ne fait jamais d'IO interactive (pas de prompt, pas d'exécution
//! de commande externe) : c'est la responsabilité de l'adaptateur appelant (CLI, GUI) d'acquérir
//! la passphrase, puis de la remettre ici.
//!
//! [`Store::change_passphrase`] sur un coffre v3 ré-enveloppe la clé maître dans le seul
//! sidecar, atomiquement — la base n'est pas touchée. Sur un coffre v1/v2 (clé dérivée de la
//! passphrase), elle ré-chiffre l'intégralité du coffre sous une clé maître neuve et migre le
//! sidecar en v3. Elle n'émet jamais `PRAGMA rekey` : dans `SQLCipher` 4.14, `sqlite3_rekey_v2`
//! renvoie inconditionnellement `SQLITE_OK` même quand la transaction interne échoue (busy, page
//! illisible, commit raté) — un rekey raté serait donc indiscernable d'un rekey réussi. À la
//! place, une copie ré-chiffrée est écrite dans un fichier temporaire (même mécanisme que
//! [`Store::backup_to`], déjà éprouvé), puis deux `rename()` la basculent en place : d'abord la
//! base, puis son sidecar. Un sidecar en attente (`<db>.kdf.new`) rend la fenêtre entre les deux
//! renames récupérable après une interruption, sans intervention de l'utilisateur — voir
//! [`Store::open_with_passphrase_with`].

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
use rusqlite::backup::StepResult;
use subtle::ConstantTimeEq;
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
        /// `1` (coffre pas encore migré), `2` (clé dérivée de la passphrase), ou `3` (clé
        /// maître enveloppée — voir le doc de module de `store::kdf`).
        sidecar_version: u8,
        /// `Some` si une session est en cache dans le trousseau et n'est pas expirée.
        session_expires_at: Option<OffsetDateTime>,
        /// Un changement de passphrase a été commencé et interrompu avant sa bascule finale :
        /// la prochaine ouverture avec la NOUVELLE passphrase le terminera d'elle-même, ou
        /// l'ancienne redevient utilisable en le restaurant depuis la sauvegarde préalable.
        passphrase_change_pending: bool,
    },
}

/// Résultat d'un [`Store::change_passphrase`] réussi : où trouver la sauvegarde préalable, et
/// sous quels paramètres Argon2 la nouvelle clé a été dérivée (utile pour l'événement d'audit et
/// l'affichage `--json`, sans avoir à exposer publiquement le type interne `kdf::Argon2Cost`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PassphraseChangeReport {
    pub backup_path: PathBuf,
    pub argon2_m_cost: u32,
    pub argon2_t_cost: u32,
    pub argon2_p_cost: u32,
    /// `true` si la base a été ré-chiffrée page à page (coffre v1/v2 migrant vers v3) ; `false`
    /// pour un coffre déjà v3, où seul le sidecar — l'enveloppe de la clé maître — a été
    /// réécrit, la base restant intacte à l'octet près.
    pub reencrypted: bool,
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
        let Some(cached) = cache.load(&account) else {
            return Err(interrupted_or_locked(db_path)?);
        };
        if !sidecar.verify(&cached.key) {
            // La session en cache ne correspond plus au sidecar (coffre recréé entre-temps,
            // par exemple) : la purger plutôt que d'ouvrir un coffre incohérent.
            let _ = cache.forget(&account);
            return Err(interrupted_or_locked(db_path)?);
        }
        match Self::open_with_key(db_path, cached.key, sidecar.vault_id) {
            Ok(store) => {
                discard_rekey_orphans(db_path);
                Ok(store)
            }
            // La clé en cache est valide pour le sidecar committé mais la base la refuse : la
            // seule façon légitime dont ça arrive est une base déjà basculée sur une nouvelle
            // clé pendant un changement de passphrase interrompu avant la bascule du sidecar
            // (fenêtre F4). Une session en cache ne peut alors plus rien ouvrir — seule
            // `open_with_passphrase_with`, qui a la passphrase en main, peut se rétablir seule.
            Err(_) if kdf::read_staged(db_path)?.is_some() => Err(
                StoreError::PassphraseChangeInterrupted(db_path.to_path_buf()),
            ),
            Err(e) => Err(e),
        }
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
        // Atomique vu de l'extérieur (lot 36) : si la base ne peut pas être créée (répertoire
        // non inscriptible, disque plein), le sidecar déjà écrit est retiré avec ce qui a pu
        // l'être — sinon un `.kdf` orphelin faisait croire à un coffre existant, et bloquait
        // toute nouvelle tentative au même chemin.
        Self::open_with_key(db_path, key, sidecar.vault_id).inspect_err(|_| {
            let mut candidates = vec![db_path.to_path_buf(), kdf::sidecar_path(db_path)];
            for suffix in ["-wal", "-shm", "-journal"] {
                let mut name = db_path.as_os_str().to_owned();
                name.push(suffix);
                candidates.push(PathBuf::from(name));
            }
            for path in candidates {
                // Seulement ce que cette création a pu écrire : jamais un lien symbolique ou
                // un répertoire préexistant à cet emplacement.
                if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_file()) {
                    let _ = fs::remove_file(path);
                }
            }
        })
    }

    /// Ouvre un coffre **existant**. Ne crée jamais rien — contrairement au comportement
    /// précédent, qui créait silencieusement un coffre vide sur un chemin inexistant. Migre
    /// automatiquement un sidecar v1 vers v2 après une ouverture réussie (même sel, mêmes
    /// paramètres : la clé ne change pas).
    ///
    /// # Errors
    ///
    /// [`StoreError::VaultNotFound`] si `db_path` n'existe pas ; [`StoreError::WrongPassphrase`]
    /// si la passphrase ne correspond pas au coffre ;
    /// [`StoreError::PassphraseChangeInterrupted`] si un changement de passphrase a été
    /// interrompu et que `passphrase` ne correspond ni à l'ancienne ni à la nouvelle.
    pub fn open_with_passphrase(
        db_path: &Path,
        passphrase: &Passphrase,
    ) -> Result<Self, StoreError> {
        Self::open_with_passphrase_with(db_path, passphrase, &OsKeyring)
    }

    /// # Errors
    ///
    /// Voir [`Self::open_with_passphrase`].
    pub fn open_with_passphrase_with(
        db_path: &Path,
        passphrase: &Passphrase,
        cache: &dyn KeyCache,
    ) -> Result<Self, StoreError> {
        if !db_path.exists() {
            return Err(StoreError::VaultNotFound(db_path.to_path_buf()));
        }
        let sidecar =
            kdf::read(db_path)?.ok_or_else(|| StoreError::VaultNotFound(db_path.to_path_buf()))?;

        // `key_from_passphrase` dispatche selon le format : clé dérivée vérifiée (v2), clé
        // maître déballée de son enveloppe AEAD (v3), ou clé dérivée sans garantie (v1 — seule
        // la lecture SQLCipher tranchera).
        match sidecar.key_from_passphrase(passphrase) {
            Ok(key) => {
                return match Self::finish_open_with_committed_sidecar(db_path, key, sidecar, cache)
                {
                    Ok(store) => Ok(store),
                    // Le sidecar committé accepte cette passphrase mais la base elle-même la
                    // refuse : la seule façon dont ça arrive légitimement est une base déjà
                    // basculée sur une nouvelle clé pendant un changement de passphrase
                    // interrompu *avant* la bascule du sidecar (fenêtre F4, migration v2 → v3),
                    // où l'appelant vient justement de fournir l'ANCIENNE passphrase. Sans ce
                    // contrôle, ce cas ressortirait comme un trompeur `WrongPassphrase`.
                    Err(_) if kdf::read_staged(db_path)?.is_some() => Err(
                        StoreError::PassphraseChangeInterrupted(db_path.to_path_buf()),
                    ),
                    Err(_) => Err(StoreError::WrongPassphrase),
                };
            }
            Err(StoreError::WrongPassphrase) => {}
            Err(e) => return Err(e),
        }

        // La passphrase ne correspond pas au sidecar committé : avant de conclure qu'elle est
        // fausse, essayer le sidecar en attente d'un changement interrompu — c'est l'autre moitié
        // de la fenêtre F4, où l'appelant fournit cette fois la NOUVELLE passphrase.
        if let Some(staged) = kdf::read_staged(db_path)? {
            return match staged.key_from_passphrase(passphrase) {
                Ok(staged_key) => {
                    if let Ok(store) =
                        Self::open_with_key(db_path, staged_key, staged.vault_id.clone())
                    {
                        // La base ouvre déjà sous la clé du sidecar en attente : le changement
                        // avait réellement abouti, seule la bascule du sidecar restait à faire.
                        // La terminer silencieusement — aucune action requise de l'utilisateur.
                        kdf::commit_staged(db_path)?;
                        Ok(store)
                    } else {
                        Err(StoreError::PassphraseChangeInterrupted(
                            db_path.to_path_buf(),
                        ))
                    }
                }
                Err(StoreError::WrongPassphrase) => Err(StoreError::PassphraseChangeInterrupted(
                    db_path.to_path_buf(),
                )),
                Err(e) => Err(e),
            };
        }

        Err(StoreError::WrongPassphrase)
    }

    /// Termine l'ouverture avec une passphrase déjà vérifiée contre le sidecar committé :
    /// migration v1→v2 si nécessaire, ouverture effective, puis nettoyage d'un éventuel
    /// orphelin de changement de passphrase (sidecar en attente ou copie temporaire) — sûr à
    /// faire puisque le sidecar committé vient d'ouvrir la base avec succès.
    fn finish_open_with_committed_sidecar(
        db_path: &Path,
        key: VaultKey,
        sidecar: kdf::Sidecar,
        cache: &dyn KeyCache,
    ) -> Result<Self, StoreError> {
        let sidecar = if sidecar.is_legacy() {
            let legacy_account = sidecar.vault_id.as_account();
            let upgraded = kdf::upgrade_v1_to_v2(db_path, &sidecar, &key)?;
            // La session éventuellement en cache sous l'ancien compte (hash du chemin) ne
            // pourra plus jamais être retrouvée après la migration : autant la purger tout
            // de suite plutôt que de la laisser traîner indéfiniment.
            let _ = cache.forget(&legacy_account);
            upgraded
        } else {
            sidecar
        };
        let store = Self::open_with_key(db_path, key, sidecar.vault_id)?;
        discard_rekey_orphans(db_path);
        Ok(store)
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
        let sidecar_version = sidecar.version();
        let session_expires_at = cache
            .load(&sidecar.vault_id.as_account())
            .map(|c| c.expires_at);
        let passphrase_change_pending = kdf::read_staged(db_path)?.is_some();
        Ok(VaultStatus::Exists {
            sidecar_version,
            session_expires_at,
            passphrase_change_pending,
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
    /// une curiosité si les deux fichiers sont ouverts et « mémorisés » en parallèle. Autre
    /// conséquence à connaître : une sauvegarde écrite après un changement de passphrase
    /// s'ouvre avec la NOUVELLE passphrase, mais toute sauvegarde plus ancienne — y compris
    /// celle qu'écrit obligatoirement [`Self::change_passphrase`] juste avant de changer quoi
    /// que ce soit — reste chiffrée avec l'ANCIENNE.
    ///
    /// # Errors
    ///
    /// Retourne une erreur si l'écriture du fichier de sauvegarde ou son chiffrement échoue.
    pub fn backup_to(&self, dest_db_path: &Path) -> Result<(), StoreError> {
        // Refuse d'écraser un fichier existant : sans ce garde-fou, la copie du sidecar écrasait
        // d'abord le `.kdf` d'un éventuel coffre à cette destination (détruisant son sel et son
        // vérificateur, donc le rendant illisible) avant même que la copie de la base n'échoue.
        // Une destination de sauvegarde est toujours un fichier neuf.
        if dest_db_path.exists() || kdf::sidecar_path(dest_db_path).exists() {
            return Err(StoreError::BackupDestinationExists(
                dest_db_path.to_path_buf(),
            ));
        }
        if let Some(parent) = dest_db_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut dest = Connection::open(dest_db_path)?;
        unlock(&dest, &self.key)?;

        let backup = rusqlite::backup::Backup::new(&self.conn, &mut dest)?;
        backup.run_to_completion(16, Duration::from_millis(250), None)?;
        // Le sidecar n'est copié qu'après le succès de la copie de la base : une base sauvegardée
        // sans son `.kdf` serait inutilisable, mais on préfère ne rien laisser d'incohérent à la
        // destination si le backup lui-même échoue.
        fs::copy(
            kdf::sidecar_path(&self.db_path),
            kdf::sidecar_path(dest_db_path),
        )?;
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
        // Ne jamais écraser un coffre existant à la destination : restaurer se fait vers un
        // chemin neuf, explicite (cf. `freeflow backup restore --to`). Sans ce contrôle, un
        // `fs::copy` inconditionnel remplaçait silencieusement le coffre visé.
        if dest_db_path.exists() || kdf::sidecar_path(dest_db_path).exists() {
            return Err(StoreError::BackupDestinationExists(
                dest_db_path.to_path_buf(),
            ));
        }
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
        let stamp = timestamp_now()?;
        let dest = backups_dir.join(format!("backup-{stamp}.db"));
        // `backup_to` refuse d'écraser une destination existante (garde-fou contre l'écrasement
        // d'un coffre tiers par un `backup create --out` maladroit). Ici, une collision ne peut
        // provenir que d'une auto-sauvegarde de la *même seconde* dans notre propre répertoire :
        // c'est notre fichier, on le remplace délibérément avant de réécrire.
        if dest.exists() || kdf::sidecar_path(&dest).exists() {
            let _ = fs::remove_file(&dest);
            let _ = fs::remove_file(kdf::sidecar_path(&dest));
        }
        self.backup_to(&dest)?;
        Ok(Some(dest))
    }

    /// Change la passphrase du coffre. Deux régimes selon le format du sidecar :
    ///
    /// - **coffre v3** (clé maître enveloppée) : ré-enveloppe la même clé maître sous la
    ///   nouvelle passphrase (sel neuf, paramètres Argon2 courants) et réécrit atomiquement le
    ///   seul sidecar — la base n'est pas touchée, il n'existe aucune fenêtre d'interruption à
    ///   gérer ;
    /// - **coffre v1/v2** (clé dérivée de la passphrase) : ré-chiffre l'intégralité des pages
    ///   sous une clé maître aléatoire neuve et migre le coffre au format v3 — voir le doc de
    ///   module pour le pourquoi de la méthode copie + double `rename()`.
    ///
    /// Dans les deux cas, écrit d'abord dans `backups_dir` une sauvegarde **obligatoire** dont
    /// le chemin est renvoyé ; un échec de cette sauvegarde abandonne le changement sans rien
    /// modifier au coffre. Cette sauvegarde, comme toute sauvegarde antérieure, reste ouvrable
    /// avec l'ANCIENNE passphrase (son `.kdf` copié n'est pas réécrit).
    ///
    /// `current` est exigée **même si ce `Store` est déjà déverrouillé** : ni une session en
    /// cache dans le trousseau OS, ni une fenêtre déjà ouverte, ne peuvent tenir lieu de preuve
    /// de possession de la passphrase — l'une comme l'autre pourraient être détenues par un
    /// agent ou laissées actives sur une machine sans surveillance, et accepter l'une d'elles
    /// permettrait d'enfermer l'utilisateur hors de son propre coffre.
    ///
    /// # Errors
    ///
    /// [`StoreError::WrongPassphrase`] si `current` ne correspond pas au coffre ;
    /// [`StoreError::VaultBusy`] si une autre connexion a une transaction d'écriture en cours au
    /// moment du changement — une connexion simplement ouverte mais inactive ailleurs (fenêtre
    /// GUI qui ne fait rien à cet instant précis, par exemple) ne tient aucun verrou en mode WAL
    /// et n'est donc pas détectable : elle continuera de lire une vue figée jusqu'à sa prochaine
    /// écriture, qui échouera alors normalement puisque le fichier a changé sous elle ;
    /// [`StoreError::PassphraseChangeInterrupted`] si un changement précédent n'a pas été mené à
    /// son terme (le terminer, ou restaurer la sauvegarde qu'il a écrite, avant d'en lancer un
    /// autre) ; une erreur de dérivation si `new` est vide ou identique à `current`.
    pub fn change_passphrase(
        &mut self,
        current: &Passphrase,
        new: &Passphrase,
        backups_dir: &Path,
    ) -> Result<PassphraseChangeReport, StoreError> {
        self.change_passphrase_with(current, new, backups_dir, &OsKeyring)
    }

    /// # Errors
    ///
    /// Voir [`Self::change_passphrase`].
    pub fn change_passphrase_with(
        &mut self,
        current: &Passphrase,
        new: &Passphrase,
        backups_dir: &Path,
        cache: &dyn KeyCache,
    ) -> Result<PassphraseChangeReport, StoreError> {
        let committed = kdf::read(&self.db_path)?
            .ok_or_else(|| StoreError::VaultNotFound(self.db_path.clone()))?;
        // Preuve d'ancienne passphrase, dispatchée par format : vérificateur (v2), tag AEAD de
        // l'enveloppe (v3). Pour v1/v2, `current_key` est la clé dérivée — celle que le test
        // d'égalité de passphrases plus bas attend.
        let current_key = committed.key_from_passphrase(current)?;

        // Un changement précédent inachevé doit être terminé (en rouvrant avec la nouvelle
        // passphrase) ou restauré depuis sa sauvegarde avant d'en commencer un autre : deux
        // sidecars en attente à la fois ne pourraient plus être départagés à la reprise.
        if kdf::read_staged(&self.db_path)?.is_some() {
            return Err(StoreError::PassphraseChangeInterrupted(
                self.db_path.clone(),
            ));
        }

        if new.is_empty() {
            return Err(StoreError::KeyDerivation(
                "la nouvelle passphrase ne peut pas être vide".to_string(),
            ));
        }

        // Coffre v3 : la clé maître ne change pas, seul le sidecar est réécrit — un unique
        // `write_atomic`, pas de fenêtre F4, pas de garde d'exclusivité (la base n'est pas
        // touchée, une écriture concurrente d'un autre process ne gêne en rien). La sauvegarde
        // préalable reste obligatoire : c'est le seul filet si la NOUVELLE passphrase est
        // oubliée aussitôt (son `.kdf` copié enveloppe encore la clé maître sous l'ancienne),
        // et un `.kdf` de sauvegarde est désormais le seul objet au monde qui porte la clé de
        // cette copie-là.
        if committed.wraps_master_key() {
            let stamp = timestamp_now()?;
            let backup_path = backups_dir.join(format!("pre-passphrase-change-{stamp}.db"));
            self.backup_to(&backup_path)?;

            // `rewrap` refait sa propre preuve (défense en profondeur — le unwrap est la seule
            // opération qui touche la clé maître) et détecte « nouvelle identique à l'ancienne ».
            let rewrapped = committed.rewrap(&self.db_path, current, new)?;
            let cost = rewrapped.cost();
            // Même sémantique qu'un changement avec re-chiffrement : changer de passphrase
            // déconnecte les sessions mémorisées, même si la clé maître en cache resterait
            // techniquement valide — une session ne doit jamais survivre à la preuve qu'elle
            // ne peut pas fournir.
            let _ = cache.forget(&self.vault_id.as_account());
            return Ok(PassphraseChangeReport {
                backup_path,
                argon2_m_cost: cost.m_cost,
                argon2_t_cost: cost.t_cost,
                argon2_p_cost: cost.p_cost,
                reencrypted: false,
            });
        }

        // Coffre v1/v2 : la clé du coffre dérive encore de la passphrase, en changer exige de
        // ré-chiffrer la base — et c'est l'occasion unique de migrer vers v3 (clé maître
        // aléatoire), ce qu'une migration silencieuse au déverrouillage ne pourrait pas faire.
        let new_under_current_params = kdf::derive_key(new, committed.salt(), committed.cost())?;
        if bool::from(
            new_under_current_params
                .as_bytes()
                .ct_eq(current_key.as_bytes()),
        ) {
            return Err(StoreError::KeyDerivation(
                "la nouvelle passphrase est identique à l'ancienne".to_string(),
            ));
        }

        // Exclusivité : `journal_mode = DELETE` échoue (ou est ignorée, mode inchangé) tant
        // qu'une autre connexion tient une transaction en cours — et son effet de bord
        // (checkpoint + suppression du WAL) est de toute façon requis avant de renommer un
        // nouveau fichier par-dessus celui-ci plus bas. Une contention réelle peut ressortir
        // soit comme une erreur SQLite (`busy`/`locked`), soit comme un mode simplement resté
        // inchangé : les deux sont traités comme `VaultBusy`.
        let mode: String = self
            .conn
            .query_row("PRAGMA journal_mode = DELETE", [], |row| row.get(0))
            .map_err(classify_busy)?;
        if !mode.eq_ignore_ascii_case("delete") {
            return Err(StoreError::VaultBusy);
        }
        self.conn
            .execute_batch("PRAGMA locking_mode = EXCLUSIVE;")?;
        self.conn
            .execute_batch("BEGIN EXCLUSIVE; COMMIT;")
            .map_err(|_| StoreError::VaultBusy)?;

        // Sauvegarde obligatoire : point de non-retour si elle échoue, rien n'a encore été
        // modifié sur le coffre lui-même.
        let stamp = timestamp_now()?;
        let backup_path = backups_dir.join(format!("pre-passphrase-change-{stamp}.db"));
        self.backup_to(&backup_path)?;

        let rekeyed_path = rekeyed_path(&self.db_path);
        let result = self.rekey_in_place(&committed.vault_id, new, &rekeyed_path, cache);
        if result.is_err() {
            // Le coffre courant (encore sur l'ancienne clé tant que le sidecar n'a pas basculé)
            // reste la source de vérité : ne laisser traîner ni copie ni sidecar en attente.
            kdf::discard_staged(&self.db_path);
            let _ = fs::remove_file(&rekeyed_path);
        }
        result.map(|(m_cost, t_cost, p_cost)| PassphraseChangeReport {
            backup_path,
            argon2_m_cost: m_cost,
            argon2_t_cost: t_cost,
            argon2_p_cost: p_cost,
            reencrypted: true,
        })
    }

    /// Le cœur du changement de passphrase, une fois la sauvegarde obligatoire écrite : sidecar
    /// en attente, copie ré-chiffrée, puis bascule en deux `rename()` (base, puis sidecar). Ne
    /// touche `self` qu'au tout dernier moment, une fois le nouveau fichier de base réellement
    /// en place et rouvert avec succès. Renvoie les paramètres Argon2 (m/t/p cost) sous lesquels
    /// la nouvelle clé a été dérivée — de simples `u32`, pour ne pas avoir à exposer publiquement
    /// le type interne `kdf::Argon2Cost`.
    fn rekey_in_place(
        &mut self,
        vault_id: &kdf::VaultId,
        new: &Passphrase,
        rekeyed_path: &Path,
        cache: &dyn KeyCache,
    ) -> Result<(u32, u32, u32), StoreError> {
        // Sidecar en attente : sel neuf, coût courant, même `vault_id`. Écrit et synchronisé sur
        // disque avant de toucher la base — c'est ce qui rend la fenêtre suivante récupérable.
        let (staged, new_key) = kdf::stage_rekey(&self.db_path, vault_id, new)?;
        let cost = staged.cost();

        // Copie ré-chiffrée page à page vers un fichier temporaire, sous la nouvelle clé.
        let _ = fs::remove_file(rekeyed_path);
        {
            let mut dest = Connection::open(rekeyed_path)?;
            apply_key(&dest, &new_key)?;
            let backup = rusqlite::backup::Backup::new(&self.conn, &mut dest)?;
            match backup.step(-1)? {
                StepResult::Done => {}
                // `More`/`Busy`/`Locked` : `step(-1)` demande explicitement toutes les pages
                // restantes en un seul appel, donc n'importe quoi d'autre que `Done` signifie
                // que la copie n'a pas pu aboutir d'un coup — traité comme « coffre occupé »,
                // quelle que soit la variante exacte (l'énumération est `#[non_exhaustive]`).
                _ => return Err(StoreError::VaultBusy),
            }
            drop(backup);
            dest.execute_batch("PRAGMA foreign_keys = ON;")?;
            tighten_permissions(rekeyed_path);
            dest.close().map_err(|(_, e)| StoreError::from(e))?;
        }
        sync_dir(rekeyed_path.parent())?;

        // Fermer la connexion actuelle (libère le verrou exclusif) avant de renommer son
        // fichier par-dessus : POSIX autorise un rename sur un chemin dont un descripteur reste
        // ouvert ailleurs, mais rien n'est gagné à le laisser ouvert ici. `Connection` ne peut
        // pas être « vidée » sans valeur de remplacement : une connexion en mémoire jetable sert
        // de valeur temporaire, remplacée pour de bon quelques lignes plus bas.
        let old_conn = std::mem::replace(&mut self.conn, Connection::open_in_memory()?);
        drop(old_conn);
        remove_stale_wal_files(&self.db_path);

        // Bascule de la base — entre en fenêtre F4 (base neuve, sidecar encore ancien).
        fs::rename(rekeyed_path, &self.db_path)?;
        sync_dir(self.db_path.parent())?;

        // Bascule du sidecar — sort de la fenêtre F4, c'est LE point de commit.
        kdf::commit_staged(&self.db_path)?;

        // Réouverture sous la nouvelle clé.
        let mut conn = Connection::open(&self.db_path)?;
        apply_key(&conn, &new_key)?;
        configure(&conn)?;
        migrations::migrations().to_latest(&mut conn)?;
        tighten_permissions(&self.db_path);

        self.conn = conn;
        self.key = new_key;
        // `vault_id` est inchangé (c'est l'identité du coffre, pas de la clé) : le compte du
        // trousseau reste le même, donc `forget` retrouve bien la session à purger.
        let _ = cache.forget(&self.vault_id.as_account());

        Ok((cost.m_cost, cost.t_cost, cost.p_cost))
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

/// `StoreError::Locked`, sauf si un changement de passphrase est resté en attente — auquel cas
/// c'est ce qui mérite d'être rapporté plutôt qu'un verrouillage ordinaire.
fn interrupted_or_locked(db_path: &Path) -> Result<StoreError, StoreError> {
    Ok(if kdf::read_staged(db_path)?.is_some() {
        StoreError::PassphraseChangeInterrupted(db_path.to_path_buf())
    } else {
        StoreError::Locked
    })
}

/// Supprime un sidecar en attente et une copie temporaire de ré-chiffrement orphelins — sûr à
/// faire dès que le sidecar committé vient d'ouvrir la base avec succès, dans les deux cas où
/// ça peut se produire : un changement jamais commencé (rien à faire), ou déjà mené à son terme
/// (l'un ou l'autre a pu ne pas être nettoyé si le process a été interrompu juste après).
fn discard_rekey_orphans(db_path: &Path) {
    kdf::discard_staged(db_path);
    let _ = fs::remove_file(rekeyed_path(db_path));
}

/// Chemin de la copie temporaire ré-chiffrée qu'utilise [`Store::change_passphrase`] avant de la
/// renommer par-dessus le fichier de base.
fn rekeyed_path(db_path: &Path) -> PathBuf {
    let mut os_string = db_path.as_os_str().to_owned();
    os_string.push(".rekeyed");
    PathBuf::from(os_string)
}

/// Horodatage `YYYYMMDD-HHMMSS` pour un nom de fichier de sauvegarde.
///
/// # Panics
///
/// Ne panique jamais en pratique : le format de date est un littéral constant, toujours valide.
fn timestamp_now() -> Result<String, StoreError> {
    let format =
        time::format_description::parse_borrowed::<2>("[year][month][day]-[hour][minute][second]")
            .expect("format de date littéral, toujours valide");
    OffsetDateTime::now_utc()
        .format(&format)
        .map_err(|e| StoreError::KeyDerivation(format!("horodatage de sauvegarde : {e}")))
}

/// Supprime les fichiers `-wal`/`-shm` résiduels d'un `.db` — après `PRAGMA journal_mode =
/// DELETE` réussi, ils ne devraient déjà plus exister ; ce n'est qu'un filet de sécurité avant
/// de renommer un nouveau fichier par-dessus.
fn remove_stale_wal_files(db_path: &Path) {
    for suffix in ["-wal", "-shm"] {
        let mut os_string = db_path.as_os_str().to_owned();
        os_string.push(suffix);
        let _ = fs::remove_file(PathBuf::from(os_string));
    }
}

/// Applique la clé `SQLCipher` à la connexion et valide qu'elle permet bien de lire le contenu
/// déjà chiffré : `SQLCipher` ne vérifie rien avant une vraie lecture des pages.
fn apply_key(conn: &Connection, key: &VaultKey) -> Result<(), StoreError> {
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
    .map_err(|e| classify_key_check_failure(&e))?;
    Ok(())
}

/// Distingue une vraie mauvaise passphrase d'un coffre simplement occupé par un autre process —
/// sans ce contrôle, une commande concurrente pendant un changement de passphrase annoncerait
/// à tort « passphrase incorrecte ».
fn classify_key_check_failure(e: &rusqlite::Error) -> StoreError {
    if is_busy_or_locked(e) {
        return StoreError::VaultBusy;
    }
    StoreError::WrongPassphrase
}

/// `VaultBusy` si `e` est une erreur SQLite `busy`/`locked` (contention réelle avec une autre
/// connexion), sinon l'erreur d'origine convertie normalement.
fn classify_busy(e: rusqlite::Error) -> StoreError {
    if is_busy_or_locked(&e) {
        return StoreError::VaultBusy;
    }
    StoreError::from(e)
}

fn is_busy_or_locked(e: &rusqlite::Error) -> bool {
    matches!(e, rusqlite::Error::SqliteFailure(err, _)
        if matches!(err.code, rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked))
}

fn configure(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")?;
    conn.busy_timeout(Duration::from_secs(5))?;
    Ok(())
}

/// Applique la clé puis configure la connexion — le chemin normal d'ouverture. Le changement de
/// passphrase, lui, a besoin des deux étapes séparément (voir [`Store::rekey_in_place`]).
fn unlock(conn: &Connection, key: &VaultKey) -> Result<(), StoreError> {
    apply_key(conn, key)?;
    configure(conn)
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

/// Fsync d'un répertoire — pour rendre un `rename()` qui vient d'avoir lieu dedans réellement
/// durable, pas seulement visible. Best-effort côté appelants qui ne peuvent pas se permettre
/// d'échouer sur ce détail (`tighten_permissions`) ; propagé en erreur là où l'appelant construit
/// justement une garantie de durabilité dessus (changement de passphrase).
#[cfg(unix)]
fn sync_dir(dir: Option<&Path>) -> Result<(), StoreError> {
    if let Some(dir) = dir {
        fs::File::open(dir)?.sync_all()?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn sync_dir(_dir: Option<&Path>) -> Result<(), StoreError> {
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

    /// Fabrique un coffre **v2** authentique — `kdf::create` ne produit plus que du v3 (lot 24),
    /// le seul chemin restant passe par un sidecar v1 écrit à la main (le seul format dont la
    /// clé se calcule hors de `kdf`), une base créée sous cette clé dérivée, puis la migration
    /// v1 → v2 d'une ouverture normale. C'est le point de départ des tests de migration v2 → v3.
    fn create_v2(db_path: &Path, passphrase: &str) -> Store {
        let salt = [5u8; kdf::SALT_LEN];
        let mut bytes = vec![1u8];
        bytes.extend_from_slice(&salt);
        fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        fs::write(kdf::sidecar_path(db_path), &bytes).unwrap();
        let v1_sidecar = kdf::read(db_path).unwrap().unwrap();
        let key = kdf::derive_key(
            &Passphrase::from(passphrase),
            v1_sidecar.salt(),
            v1_sidecar.cost(),
        )
        .unwrap();
        Store::open_with_key(db_path, key, v1_sidecar.vault_id.clone()).unwrap();
        let store = open(db_path, passphrase).unwrap(); // migre v1 -> v2 au passage
        assert_eq!(kdf::read(db_path).unwrap().unwrap().version(), 2);
        store
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
            "fiscal_years",
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
    fn backup_and_restore_refuse_to_overwrite_an_existing_destination() {
        let source_path = temp_db_path("overwrite-source");
        let victim_path = temp_db_path("overwrite-victim");
        let source = create(&source_path, "s3cret");
        // Un second coffre légitime à la destination : il ne doit jamais être écrasé.
        let _victim = create(&victim_path, "other-secret");

        let backup_err = source.backup_to(&victim_path).unwrap_err();
        assert!(matches!(backup_err, StoreError::BackupDestinationExists(_)));
        let restore_err = Store::restore_from(
            &source_path,
            &victim_path,
            &Passphrase::from("other-secret"),
        )
        .unwrap_err();
        assert!(matches!(
            restore_err,
            StoreError::BackupDestinationExists(_)
        ));

        // Le coffre visé reste intact et ouvrable avec SA passphrase.
        Store::open_with_passphrase(&victim_path, &Passphrase::from("other-secret")).unwrap();
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

    fn backups_dir_for(label: &str) -> PathBuf {
        temp_db_path(label).parent().unwrap().join("backups")
    }

    #[test]
    fn changing_the_passphrase_only_lets_the_new_one_open_the_vault() {
        let db_path = temp_db_path("change-reencrypts");
        let mut store = create(&db_path, "old-s3cret");
        store
            .connection()
            .execute("INSERT INTO clients (id, name, created_at) VALUES ('c1', 'Argon Digital', '2026-08-29T00:00:00Z')", [])
            .unwrap();

        let backup_path = store
            .change_passphrase(
                &Passphrase::from("old-s3cret"),
                &Passphrase::from("new-s3cret"),
                &backups_dir_for("change-reencrypts"),
            )
            .unwrap()
            .backup_path;
        assert!(backup_path.exists());

        let err = open(&db_path, "old-s3cret").unwrap_err();
        assert!(matches!(err, StoreError::WrongPassphrase));

        let reopened = open(&db_path, "new-s3cret").unwrap();
        let name: String = reopened
            .connection()
            .query_row("SELECT name FROM clients WHERE id = 'c1'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(name, "Argon Digital");
        assert_eq!(
            crate::app::verify_chain(reopened.connection()).unwrap(),
            crate::app::ChainStatus::Intact
        );
    }

    #[test]
    fn changing_the_passphrase_of_a_v3_vault_rewrites_the_sidecar_but_never_the_database() {
        // Le cœur du lot 24 : un coffre neuf est v3 (clé maître enveloppée), et son changement
        // de passphrase est la réécriture atomique du seul sidecar — la base reste identique à
        // l'octet près, aucun sidecar en attente ni copie `.rekeyed` n'apparaît jamais.
        let db_path = temp_db_path("v3-change-no-reencrypt");
        let mut store = create(&db_path, "old-s3cret");
        assert_eq!(kdf::read(&db_path).unwrap().unwrap().version(), 3);
        store
            .connection()
            .execute("INSERT INTO clients (id, name, created_at) VALUES ('c1', 'Argon Digital', '2026-08-29T00:00:00Z')", [])
            .unwrap();

        let db_bytes_before = fs::read(&db_path).unwrap();
        let sidecar_before = fs::read(kdf::sidecar_path(&db_path)).unwrap();
        let account_before = kdf::read(&db_path).unwrap().unwrap().vault_id.as_account();

        let report = store
            .change_passphrase(
                &Passphrase::from("old-s3cret"),
                &Passphrase::from("new-s3cret"),
                &backups_dir_for("v3-change-no-reencrypt"),
            )
            .unwrap();
        assert!(!report.reencrypted);
        assert!(report.backup_path.exists());

        assert_eq!(
            fs::read(&db_path).unwrap(),
            db_bytes_before,
            "la base n'est jamais réécrite par un changement de passphrase v3"
        );
        assert_ne!(
            fs::read(kdf::sidecar_path(&db_path)).unwrap(),
            sidecar_before
        );
        assert!(kdf::read_staged(&db_path).unwrap().is_none());
        assert!(!rekeyed_path(&db_path).exists());

        let after = kdf::read(&db_path).unwrap().unwrap();
        assert_eq!(after.version(), 3);
        assert_eq!(after.vault_id.as_account(), account_before);

        drop(store);
        let err = open(&db_path, "old-s3cret").unwrap_err();
        assert!(matches!(err, StoreError::WrongPassphrase));
        let reopened = open(&db_path, "new-s3cret").unwrap();
        let name: String = reopened
            .connection()
            .query_row("SELECT name FROM clients WHERE id = 'c1'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(name, "Argon Digital");

        // La sauvegarde préalable, elle, s'ouvre toujours avec l'ANCIENNE passphrase : son
        // `.kdf` copié enveloppe encore la clé maître sous l'ancienne enveloppe.
        Store::restore_from(
            &report.backup_path,
            &temp_db_path("v3-change-no-reencrypt-restored"),
            &Passphrase::from("old-s3cret"),
        )
        .unwrap();
    }

    #[test]
    fn changing_the_passphrase_of_a_v2_vault_migrates_it_to_v3_by_reencrypting() {
        let db_path = temp_db_path("v2-migrates-to-v3");
        let mut store = create_v2(&db_path, "old-s3cret");
        store
            .connection()
            .execute("INSERT INTO clients (id, name, created_at) VALUES ('c1', 'Argon Digital', '2026-08-29T00:00:00Z')", [])
            .unwrap();
        let db_bytes_before = fs::read(&db_path).unwrap();

        let report = store
            .change_passphrase(
                &Passphrase::from("old-s3cret"),
                &Passphrase::from("new-s3cret"),
                &backups_dir_for("v2-migrates-to-v3"),
            )
            .unwrap();
        assert!(report.reencrypted);

        assert_ne!(
            fs::read(&db_path).unwrap(),
            db_bytes_before,
            "la migration v2 -> v3 ré-chiffre réellement la base sous la clé maître neuve"
        );
        let after = kdf::read(&db_path).unwrap().unwrap();
        assert_eq!(after.version(), 3);

        drop(store);
        let reopened = open(&db_path, "new-s3cret").unwrap();
        let name: String = reopened
            .connection()
            .query_row("SELECT name FROM clients WHERE id = 'c1'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(name, "Argon Digital");
    }

    #[test]
    fn changing_the_passphrase_requires_the_current_one_even_on_an_already_open_store() {
        let db_path = temp_db_path("change-requires-current");
        let mut store = create(&db_path, "old-s3cret");
        let sidecar_before = fs::read(kdf::sidecar_path(&db_path)).unwrap();

        let err = store
            .change_passphrase(
                &Passphrase::from("totally-wrong"),
                &Passphrase::from("new-s3cret"),
                &backups_dir_for("change-requires-current"),
            )
            .unwrap_err();
        assert!(matches!(err, StoreError::WrongPassphrase));

        let sidecar_after = fs::read(kdf::sidecar_path(&db_path)).unwrap();
        assert_eq!(sidecar_before, sidecar_after);
        assert!(open(&db_path, "old-s3cret").is_ok());
    }

    #[test]
    fn changing_the_passphrase_writes_a_mandatory_backup_that_still_opens_with_the_old_one() {
        let db_path = temp_db_path("change-mandatory-backup");
        let mut store = create(&db_path, "old-s3cret");

        let backup_path = store
            .change_passphrase(
                &Passphrase::from("old-s3cret"),
                &Passphrase::from("new-s3cret"),
                &backups_dir_for("change-mandatory-backup"),
            )
            .unwrap()
            .backup_path;

        assert!(
            backup_path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("pre-passphrase-change-")
        );
        Store::restore_from(
            &backup_path,
            &temp_db_path("change-mandatory-backup-restored"),
            &Passphrase::from("old-s3cret"),
        )
        .unwrap();
    }

    #[test]
    fn changing_the_passphrase_upgrades_legacy_argon2_parameters_and_draws_a_fresh_salt() {
        let db_path = temp_db_path("change-upgrades-legacy");
        // Sidecar v1 fabriqué à la main, comme les tests de `kdf` — jamais produit par
        // `Store::create`, qui écrit v2 depuis toujours. Le coffre chiffré derrière est
        // construit avec les mêmes primitives internes que `open_with_key`, pour obtenir un
        // vrai coffre v1 authentique plutôt qu'un fichier vide qui ne prouverait rien.
        let salt = [5u8; kdf::SALT_LEN];
        let mut bytes = vec![1u8];
        bytes.extend_from_slice(&salt);
        fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        fs::write(kdf::sidecar_path(&db_path), &bytes).unwrap();

        let v1_sidecar = kdf::read(&db_path).unwrap().unwrap();
        assert!(v1_sidecar.is_legacy());
        let key = kdf::derive_key(
            &Passphrase::from("old-s3cret"),
            v1_sidecar.salt(),
            v1_sidecar.cost(),
        )
        .unwrap();
        Store::open_with_key(&db_path, key, v1_sidecar.vault_id.clone()).unwrap();

        let mut store = open(&db_path, "old-s3cret").unwrap(); // migre v1 -> v2 au passage
        let migrated = kdf::read(&db_path).unwrap().unwrap();
        assert!(!migrated.is_legacy());
        assert_eq!(
            migrated.salt(),
            &salt,
            "la migration v1->v2 conserve le sel"
        );

        let backup_path = store
            .change_passphrase(
                &Passphrase::from("old-s3cret"),
                &Passphrase::from("new-s3cret"),
                &backups_dir_for("change-upgrades-legacy"),
            )
            .unwrap()
            .backup_path;
        assert!(backup_path.exists());

        let after = kdf::read(&db_path).unwrap().unwrap();
        assert!(!after.is_legacy());
        assert_eq!(after.cost().m_cost, kdf::Argon2Cost::CURRENT.m_cost);
        assert_ne!(after.salt(), &salt);
    }

    #[test]
    fn changing_the_passphrase_keeps_the_vault_id_and_purges_the_cached_session() {
        let db_path = temp_db_path("change-keeps-vault-id");
        let cache = InMemoryKeyCache::new();
        let mut store = create(&db_path, "old-s3cret");
        let account_before = kdf::read(&db_path).unwrap().unwrap().vault_id.as_account();
        store
            .remember_with(Duration::from_secs(3600), &cache)
            .unwrap();
        assert!(Store::open_cached_with(&db_path, &cache).is_ok());

        store
            .change_passphrase_with(
                &Passphrase::from("old-s3cret"),
                &Passphrase::from("new-s3cret"),
                &backups_dir_for("change-keeps-vault-id"),
                &cache,
            )
            .unwrap();

        let account_after = kdf::read(&db_path).unwrap().unwrap().vault_id.as_account();
        assert_eq!(account_before, account_after);
        let err = Store::open_cached_with(&db_path, &cache).unwrap_err();
        assert!(matches!(err, StoreError::Locked));
    }

    #[test]
    fn two_processes_cannot_rekey_a_v2_vault_that_is_still_open_elsewhere() {
        // Une connexion simplement ouverte mais inactive ne tient aucun verrou en mode WAL — ce
        // n'est vrai que le temps d'une transaction en cours. La sonde `journal_mode = DELETE`
        // ne peut donc détecter qu'une contention réelle, pas la seule présence d'un `Store`
        // ouvert ailleurs : ce test simule la vraie contention (une transaction d'écriture en
        // cours sur une seconde connexion), la seule que SQLite lui-même expose. Elle ne
        // concerne que le changement **avec re-chiffrement** (coffre v2) : un coffre v3 ne
        // touche pas la base et n'a aucune exclusivité à prendre — voir le test suivant.
        let db_path = temp_db_path("change-vault-busy");
        let mut store = create_v2(&db_path, "old-s3cret");
        store
            .connection()
            .execute("INSERT INTO clients (id, name, created_at) VALUES ('c1', 'Argon Digital', '2026-08-29T00:00:00Z')", [])
            .unwrap();
        let second = open(&db_path, "old-s3cret").unwrap();
        second
            .connection()
            .execute_batch("BEGIN IMMEDIATE;")
            .unwrap();

        let err = store
            .change_passphrase(
                &Passphrase::from("old-s3cret"),
                &Passphrase::from("new-s3cret"),
                &backups_dir_for("change-vault-busy"),
            )
            .unwrap_err();
        assert!(matches!(err, StoreError::VaultBusy));

        second.connection().execute_batch("ROLLBACK;").unwrap();
        assert!(kdf::read_staged(&db_path).unwrap().is_none());
        assert!(!rekeyed_path(&db_path).exists());
        let reopened = open(&db_path, "old-s3cret").unwrap();
        let count: i64 = reopened
            .connection()
            .query_row("SELECT count(*) FROM clients", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn a_v3_vault_changes_passphrase_even_while_another_connection_is_writing() {
        // Contrepartie du test précédent : un coffre v3 change de passphrase sans toucher la
        // base, donc une transaction d'écriture en cours ailleurs ne le gêne pas — plus de
        // `VaultBusy` possible sur ce chemin.
        let db_path = temp_db_path("v3-change-while-writing");
        let mut store = create(&db_path, "old-s3cret");
        let second = open(&db_path, "old-s3cret").unwrap();
        second
            .connection()
            .execute_batch("BEGIN IMMEDIATE; INSERT INTO clients (id, name, created_at) VALUES ('c1', 'Argon Digital', '2026-08-29T00:00:00Z');")
            .unwrap();

        let report = store
            .change_passphrase(
                &Passphrase::from("old-s3cret"),
                &Passphrase::from("new-s3cret"),
                &backups_dir_for("v3-change-while-writing"),
            )
            .unwrap();
        assert!(!report.reencrypted);

        // L'écriture concurrente aboutit normalement : la clé du coffre n'a pas changé.
        second.connection().execute_batch("COMMIT;").unwrap();
        drop(second);
        drop(store);

        let reopened = open(&db_path, "new-s3cret").unwrap();
        let count: i64 = reopened
            .connection()
            .query_row("SELECT count(*) FROM clients", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn an_interrupted_v2_to_v3_migration_is_finished_on_the_next_open_with_the_new_passphrase() {
        // La fenêtre F4 (base basculée, sidecar pas encore) n'existe plus que pour un changement
        // **avec re-chiffrement**, c'est-à-dire la migration v2 → v3 : un coffre déjà v3 change
        // de passphrase par un unique `write_atomic`, sans jamais toucher la base — reconstruire
        // « son » état F4 par copies de fichiers redonnerait simplement un coffre valide sous
        // l'ancienne passphrase plus un orphelin `.kdf.new`, pas une interruption.
        let db_path = temp_db_path("interrupted-finishes");
        let mut store = create_v2(&db_path, "old-s3cret");
        store
            .connection()
            .execute("INSERT INTO clients (id, name, created_at) VALUES ('c1', 'Argon Digital', '2026-08-29T00:00:00Z')", [])
            .unwrap();
        let backups_dir = backups_dir_for("interrupted-finishes");
        let backup_path = store
            .change_passphrase(
                &Passphrase::from("old-s3cret"),
                &Passphrase::from("new-s3cret"),
                &backups_dir,
            )
            .unwrap()
            .backup_path;

        // Reconstruire l'état F4 (base neuve, sidecar encore ancien) à partir d'un changement
        // déjà terminé : copier le sidecar désormais committé (nouveau, v3) vers `.kdf.new`,
        // puis restaurer l'ancien sidecar (celui de la sauvegarde préalable, v2) comme committé.
        fs::copy(
            kdf::sidecar_path(&db_path),
            kdf::staged_sidecar_path(&db_path),
        )
        .unwrap();
        fs::copy(kdf::sidecar_path(&backup_path), kdf::sidecar_path(&db_path)).unwrap();
        drop(store);

        let reopened = open(&db_path, "new-s3cret").unwrap();
        assert!(kdf::read_staged(&db_path).unwrap().is_none());
        let name: String = reopened
            .connection()
            .query_row("SELECT name FROM clients WHERE id = 'c1'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(name, "Argon Digital");
        let committed = kdf::read(&db_path).unwrap().unwrap();
        assert_eq!(committed.version(), 3, "la bascule terminée est bien la v3");
        assert!(
            committed
                .key_from_passphrase(&Passphrase::from("new-s3cret"))
                .is_ok()
        );
    }

    #[test]
    fn an_interrupted_v2_to_v3_migration_reports_itself_rather_than_a_wrong_passphrase() {
        let db_path = temp_db_path("interrupted-reports-itself");
        let mut store = create_v2(&db_path, "old-s3cret");
        let backups_dir = backups_dir_for("interrupted-reports-itself");
        let backup_path = store
            .change_passphrase(
                &Passphrase::from("old-s3cret"),
                &Passphrase::from("new-s3cret"),
                &backups_dir,
            )
            .unwrap()
            .backup_path;

        fs::copy(
            kdf::sidecar_path(&db_path),
            kdf::staged_sidecar_path(&db_path),
        )
        .unwrap();
        fs::copy(kdf::sidecar_path(&backup_path), kdf::sidecar_path(&db_path)).unwrap();
        drop(store);

        let err = open(&db_path, "old-s3cret").unwrap_err();
        assert!(matches!(err, StoreError::PassphraseChangeInterrupted(_)));
        assert!(
            kdf::read_staged(&db_path).unwrap().is_some(),
            "l'échec avec l'ancienne passphrase ne doit pas consommer le sidecar en attente"
        );
    }

    #[test]
    fn an_orphan_staged_sidecar_is_discarded_on_the_next_successful_open() {
        let db_path = temp_db_path("orphan-staged-discarded");
        let store = create(&db_path, "s3cret");
        let sidecar = kdf::read(&db_path).unwrap().unwrap();
        kdf::stage_rekey(&db_path, &sidecar.vault_id, &Passphrase::from("unused")).unwrap();
        drop(store);

        assert!(kdf::read_staged(&db_path).unwrap().is_some());
        open(&db_path, "s3cret").unwrap();
        assert!(kdf::read_staged(&db_path).unwrap().is_none());
    }

    #[test]
    fn data_survives_a_passphrase_change_across_every_table() {
        let db_path = temp_db_path("change-data-survives");
        let mut store = create(&db_path, "old-s3cret");
        store
            .connection()
            .execute("INSERT INTO clients (id, name, created_at) VALUES ('c1', 'Argon Digital', '2026-08-29T00:00:00Z')", [])
            .unwrap();
        let before_tables: i64 = store
            .connection()
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        store
            .change_passphrase(
                &Passphrase::from("old-s3cret"),
                &Passphrase::from("new-s3cret"),
                &backups_dir_for("change-data-survives"),
            )
            .unwrap();

        let after_tables: i64 = store
            .connection()
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(before_tables, after_tables);
        let name: String = store
            .connection()
            .query_row("SELECT name FROM clients WHERE id = 'c1'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(name, "Argon Digital");
        assert_eq!(
            crate::app::verify_chain(store.connection()).unwrap(),
            crate::app::ChainStatus::Intact
        );
    }

    /// Lot 36 : si la base ne peut pas être créée après l'écriture du sidecar, rien ne reste —
    /// un `.kdf` orphelin faisait croire à un coffre existant et bloquait toute nouvelle
    /// tentative. Le cas est reproduit avec un lien symbolique vers un répertoire **non
    /// inscriptible** : le sidecar (à côté du lien) s'écrit, la base (derrière le lien) ne peut
    /// pas être créée.
    #[cfg(unix)]
    #[test]
    fn a_failed_creation_leaves_no_orphan_sidecar_behind() {
        use std::os::unix::fs::PermissionsExt;
        let db_path = temp_db_path("atomic-create");
        let dir = db_path.parent().unwrap();
        let read_only = dir.join("read-only");
        fs::create_dir_all(&read_only).unwrap();
        fs::set_permissions(&read_only, fs::Permissions::from_mode(0o500)).unwrap();
        if fs::File::create(read_only.join("probe")).is_ok() {
            // root ignore les permissions : le scénario n'est pas reproductible ici.
            return;
        }
        std::os::unix::fs::symlink(read_only.join("vault.db"), &db_path).unwrap();

        let err = Store::create(&db_path, &Passphrase::from("s3cret")).unwrap_err();
        assert!(
            !matches!(err, StoreError::VaultAlreadyExists(_)),
            "l'échec vient de la base, pas d'un coffre existant : {err}"
        );
        assert!(
            !kdf::sidecar_path(&db_path).exists(),
            "le sidecar écrit avant l'échec doit avoir été retiré"
        );
        // Une seconde tentative échoue pour la même raison — jamais « coffre existant ».
        let again = Store::create(&db_path, &Passphrase::from("s3cret")).unwrap_err();
        assert!(
            !matches!(again, StoreError::VaultAlreadyExists(_)),
            "{again}"
        );
    }
}
