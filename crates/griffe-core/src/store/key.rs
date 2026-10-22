//! Cache de la clé dérivée dans le trousseau du système d'exploitation (Keychain macOS / Secret
//! Service Linux) — **opt-in** (jamais automatique) et **borné dans le temps** (une entrée porte
//! sa propre expiration, vérifiée à chaque lecture ; une entrée expirée est purgée, pas
//! seulement ignorée).
//!
//! L'accès au trousseau est derrière le trait [`KeyCache`] plutôt qu'appelé en dur : le
//! trousseau est un service externe absent du bac à sable Nix et de tout runner CI (aucun
//! Secret Service/D-Bus n'y tourne), et les tests d'aujourd'hui écrivent réellement dans le
//! trousseau de la machine de développement sans jamais nettoyer. Ce n'est pas la doctrine « pas
//! de mock » du dépôt qui est en jeu ici — elle vise le SGBD, où un double masquerait les bugs
//! de migration/concurrence qu'on cherche justement à attraper — mais un service tiers dont
//! l'absence est la norme, pas l'exception. `InMemoryKeyCache` (derrière la feature
//! `test-support`) est une implémentation triviale du même trait, pas une simulation du SGBD.

#[cfg(any(test, feature = "test-support"))]
use std::sync::Mutex;

use time::OffsetDateTime;
use zeroize::Zeroize;

use super::secret::VaultKey;

/// Nom écrit à partir de cette version. L'ancien nom n'est plus qu'une lecture de secours.
const SERVICE: &str = "dev.griffe.vault";
/// Session « 12 h » posée sous l'ancien nom de service.
const LEGACY_SERVICE: &str = "dev.freeflow.vault";

/// Un service du trousseau : lire, écrire, effacer un compte. Le trousseau du système et le
/// double de test parlent la même langue, pour que la reprise de l'ancien nom se prouve sans
/// Secret Service.
trait NamedSecret {
    fn read(&self, service: &str, account: &str) -> Option<Vec<u8>>;
    fn write(&self, service: &str, account: &str, bytes: &[u8]) -> bool;
    fn delete(&self, service: &str, account: &str) -> bool;
}

/// Clé en cache et l'instant auquel elle cesse d'être valide.
#[derive(Clone)]
pub struct CachedKey {
    pub key: VaultKey,
    pub expires_at: OffsetDateTime,
}

pub trait KeyCache: Send + Sync {
    /// Renvoie la clé en cache pour `account`, si elle existe et n'est pas expirée. Une entrée
    /// expirée doit être purgée par l'implémentation avant de renvoyer `None`.
    fn load(&self, account: &str) -> Option<CachedKey>;
    /// Met `key` en cache pour `account` jusqu'à `expires_at`. Renvoie `false` — silencieusement
    /// pour l'appelant, mais observable — si le trousseau est indisponible.
    fn store(&self, account: &str, key: &VaultKey, expires_at: OffsetDateTime) -> bool;
    /// Purge l'entrée de `account`, si elle existe. `true` si l'état résultant est bien
    /// « aucune entrée », y compris quand il n'y en avait déjà pas.
    fn forget(&self, account: &str) -> bool;
}

/// Implémentation par défaut : le trousseau réel de l'OS, via le crate `keyring`.
pub struct OsKeyring;

/// Trousseau du système, un service à la fois.
struct OsSecrets;

impl NamedSecret for OsSecrets {
    fn read(&self, service: &str, account: &str) -> Option<Vec<u8>> {
        let entry = keyring::Entry::new(service, account).ok()?;
        entry.get_secret().ok()
    }

    fn write(&self, service: &str, account: &str, bytes: &[u8]) -> bool {
        let Ok(entry) = keyring::Entry::new(service, account) else {
            return false;
        };
        entry.set_secret(bytes).is_ok()
    }

    fn delete(&self, service: &str, account: &str) -> bool {
        let Ok(entry) = keyring::Entry::new(service, account) else {
            return true;
        };
        matches!(
            entry.delete_credential(),
            Ok(()) | Err(keyring::Error::NoEntry)
        )
    }
}

impl KeyCache for OsKeyring {
    fn load(&self, account: &str) -> Option<CachedKey> {
        load_account(&OsSecrets, account)
    }

    fn store(&self, account: &str, key: &VaultKey, expires_at: OffsetDateTime) -> bool {
        store_account(&OsSecrets, account, key, expires_at)
    }

    fn forget(&self, account: &str) -> bool {
        forget_account(&OsSecrets, account)
    }
}

/// Lit `dev.griffe.vault`, puis l'ancien nom pour le même compte. Une session encore valide
/// sous l'ancien nom est recopiée, relue, et seulement alors l'ancienne entrée disparaît.
fn load_account(store: &impl NamedSecret, account: &str) -> Option<CachedKey> {
    if let Some(cached) = live_entry(store, SERVICE, account) {
        return Some(cached);
    }
    let cached = live_entry(store, LEGACY_SERVICE, account)?;
    let mut bytes = encode(&cached.key, cached.expires_at);
    let wrote = store.write(SERVICE, account, &bytes);
    bytes.zeroize();
    if !wrote {
        return Some(cached);
    }
    match live_entry(store, SERVICE, account) {
        Some(reread) => {
            let _ = store.delete(LEGACY_SERVICE, account);
            Some(reread)
        }
        None => Some(cached),
    }
}

/// Écrit seulement le nom actuel. L'ancien nom ne reçoit plus de session neuve.
fn store_account(
    store: &impl NamedSecret,
    account: &str,
    key: &VaultKey,
    expires_at: OffsetDateTime,
) -> bool {
    let mut bytes = encode(key, expires_at);
    let ok = store.write(SERVICE, account, &bytes);
    bytes.zeroize();
    ok
}

/// Efface les deux noms pour ce compte. Un autre coffre, donc un autre compte, reste.
fn forget_account(store: &impl NamedSecret, account: &str) -> bool {
    let current = store.delete(SERVICE, account);
    let legacy = store.delete(LEGACY_SERVICE, account);
    current && legacy
}

/// Entrée vivante, ou `None` si elle manque, si elle est illisible, ou si elle est expirée.
/// Une entrée expirée est purgée sur ce service seul.
fn live_entry(store: &impl NamedSecret, service: &str, account: &str) -> Option<CachedKey> {
    let mut bytes = store.read(service, account)?;
    let cached = decode(&bytes);
    bytes.zeroize();
    let cached = cached?;
    if cached.expires_at <= OffsetDateTime::now_utc() {
        let _ = store.delete(service, account);
        return None;
    }
    Some(cached)
}

/// `expires_at` (secondes Unix, `i64` little-endian) suivi de la clé (32 octets) — 40 octets.
/// Toute autre longueur (y compris l'ancien format 32 octets sans expiration) est traitée comme
/// une entrée absente : au pire, une passphrase est redemandée une fois.
fn encode(key: &VaultKey, expires_at: OffsetDateTime) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(8 + VaultKey::LEN);
    bytes.extend_from_slice(&expires_at.unix_timestamp().to_le_bytes());
    bytes.extend_from_slice(key.as_bytes());
    bytes
}

fn decode(bytes: &[u8]) -> Option<CachedKey> {
    if bytes.len() != 8 + VaultKey::LEN {
        return None;
    }
    let timestamp = i64::from_le_bytes(bytes[0..8].try_into().ok()?);
    let expires_at = OffsetDateTime::from_unix_timestamp(timestamp).ok()?;
    let mut key_bytes = [0u8; VaultKey::LEN];
    key_bytes.copy_from_slice(&bytes[8..]);
    Some(CachedKey {
        key: VaultKey::new(key_bytes),
        expires_at,
    })
}

/// Magasin de clés en mémoire, pour les tests d'autres crates : jamais de trousseau OS réel
/// touché, jamais de mutation de l'environnement du process.
#[cfg(any(test, feature = "test-support"))]
#[derive(Default)]
pub struct InMemoryKeyCache {
    entries: Mutex<std::collections::HashMap<String, CachedKey>>,
}

#[cfg(any(test, feature = "test-support"))]
impl InMemoryKeyCache {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[cfg(any(test, feature = "test-support"))]
impl KeyCache for InMemoryKeyCache {
    fn load(&self, account: &str) -> Option<CachedKey> {
        let mut entries = self.entries.lock().expect("mutex empoisonné");
        let cached = entries.get(account)?.clone();
        if cached.expires_at <= OffsetDateTime::now_utc() {
            entries.remove(account);
            return None;
        }
        Some(cached)
    }

    fn store(&self, account: &str, key: &VaultKey, expires_at: OffsetDateTime) -> bool {
        self.entries.lock().expect("mutex empoisonné").insert(
            account.to_string(),
            CachedKey {
                key: key.clone(),
                expires_at,
            },
        );
        true
    }

    fn forget(&self, account: &str) -> bool {
        self.entries
            .lock()
            .expect("mutex empoisonné")
            .remove(account);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stored_key_round_trips() {
        let cache = InMemoryKeyCache::new();
        let key = VaultKey::new([3u8; VaultKey::LEN]);
        let expires_at = OffsetDateTime::now_utc() + time::Duration::hours(1);
        assert!(cache.store("acct", &key, expires_at));
        let loaded = cache.load("acct").unwrap();
        assert_eq!(loaded.key.as_bytes(), key.as_bytes());
    }

    #[test]
    fn an_expired_entry_is_refused_and_purged() {
        let cache = InMemoryKeyCache::new();
        let key = VaultKey::new([3u8; VaultKey::LEN]);
        let already_past = OffsetDateTime::now_utc() - time::Duration::seconds(1);
        cache.store("acct", &key, already_past);
        assert!(cache.load("acct").is_none());
        // Purgée : une seconde lecture ne trouve toujours rien (et ne panique pas).
        assert!(cache.load("acct").is_none());
    }

    #[test]
    fn forget_is_idempotent_on_a_missing_entry() {
        let cache = InMemoryKeyCache::new();
        assert!(cache.forget("never-stored"));
        assert!(cache.forget("never-stored"));
    }

    /// Deux services, sans Secret Service. `refuse_write` fait échouer la copie.
    /// `fail_reread` fait échouer la relecture du nom actuel juste après l'écriture.
    struct MemorySecrets {
        entries: Mutex<std::collections::HashMap<(String, String), Vec<u8>>>,
        refuse_write: Option<String>,
        fail_reread: bool,
        reread_armed: Mutex<bool>,
    }

    impl MemorySecrets {
        fn new() -> Self {
            Self {
                entries: Mutex::new(std::collections::HashMap::new()),
                refuse_write: None,
                fail_reread: false,
                reread_armed: Mutex::new(false),
            }
        }

        fn insert(&self, service: &str, account: &str, key: &VaultKey, expires_at: OffsetDateTime) {
            self.entries.lock().expect("mutex empoisonné").insert(
                (service.to_string(), account.to_string()),
                encode(key, expires_at),
            );
        }

        fn contains(&self, service: &str, account: &str) -> bool {
            self.entries
                .lock()
                .expect("mutex empoisonné")
                .contains_key(&(service.to_string(), account.to_string()))
        }
    }

    impl NamedSecret for MemorySecrets {
        fn read(&self, service: &str, account: &str) -> Option<Vec<u8>> {
            if service == SERVICE && *self.reread_armed.lock().expect("mutex empoisonné") {
                return None;
            }
            self.entries
                .lock()
                .expect("mutex empoisonné")
                .get(&(service.to_string(), account.to_string()))
                .cloned()
        }

        fn write(&self, service: &str, account: &str, bytes: &[u8]) -> bool {
            if self.refuse_write.as_deref() == Some(service) {
                return false;
            }
            self.entries
                .lock()
                .expect("mutex empoisonné")
                .insert((service.to_string(), account.to_string()), bytes.to_vec());
            if self.fail_reread && service == SERVICE {
                *self.reread_armed.lock().expect("mutex empoisonné") = true;
            }
            true
        }

        fn delete(&self, service: &str, account: &str) -> bool {
            self.entries
                .lock()
                .expect("mutex empoisonné")
                .remove(&(service.to_string(), account.to_string()));
            true
        }
    }

    fn hour_from_now() -> OffsetDateTime {
        OffsetDateTime::now_utc() + time::Duration::hours(1)
    }

    #[test]
    fn a_legacy_session_is_copied_then_the_old_name_disappears() {
        let store = MemorySecrets::new();
        let key = VaultKey::new([7u8; VaultKey::LEN]);
        let expires_at = hour_from_now();
        store.insert(LEGACY_SERVICE, "v2:aa", &key, expires_at);

        let loaded = load_account(&store, "v2:aa").expect("session reprise");

        assert_eq!(loaded.key.as_bytes(), key.as_bytes());
        assert!(store.contains(SERVICE, "v2:aa"));
        assert!(!store.contains(LEGACY_SERVICE, "v2:aa"));
    }

    #[test]
    fn a_failed_copy_keeps_the_legacy_entry_and_still_returns_the_key() {
        let mut store = MemorySecrets::new();
        store.refuse_write = Some(SERVICE.to_string());
        let key = VaultKey::new([8u8; VaultKey::LEN]);
        store.insert(LEGACY_SERVICE, "v2:aa", &key, hour_from_now());

        let loaded = load_account(&store, "v2:aa").expect("session quand même");

        assert_eq!(loaded.key.as_bytes(), key.as_bytes());
        assert!(!store.contains(SERVICE, "v2:aa"));
        assert!(store.contains(LEGACY_SERVICE, "v2:aa"));
    }

    #[test]
    fn a_failed_reread_keeps_the_legacy_entry_and_still_returns_the_key() {
        let mut store = MemorySecrets::new();
        store.fail_reread = true;
        let key = VaultKey::new([6u8; VaultKey::LEN]);
        store.insert(LEGACY_SERVICE, "v2:aa", &key, hour_from_now());

        let loaded = load_account(&store, "v2:aa").expect("session quand même");

        assert_eq!(loaded.key.as_bytes(), key.as_bytes());
        assert!(store.contains(LEGACY_SERVICE, "v2:aa"));
    }

    #[test]
    fn an_expired_legacy_entry_is_purged_and_not_copied() {
        let store = MemorySecrets::new();
        let key = VaultKey::new([5u8; VaultKey::LEN]);
        let already_past = OffsetDateTime::now_utc() - time::Duration::seconds(1);
        store.insert(LEGACY_SERVICE, "v2:aa", &key, already_past);

        assert!(load_account(&store, "v2:aa").is_none());
        assert!(!store.contains(SERVICE, "v2:aa"));
        assert!(!store.contains(LEGACY_SERVICE, "v2:aa"));
    }

    #[test]
    fn forget_clears_both_names_for_that_account_only() {
        let store = MemorySecrets::new();
        let key = VaultKey::new([4u8; VaultKey::LEN]);
        let expires_at = hour_from_now();
        store.insert(SERVICE, "v2:aa", &key, expires_at);
        store.insert(LEGACY_SERVICE, "v2:aa", &key, expires_at);
        store.insert(SERVICE, "v2:bb", &key, expires_at);
        store.insert(LEGACY_SERVICE, "v2:bb", &key, expires_at);

        assert!(forget_account(&store, "v2:aa"));
        assert!(!store.contains(SERVICE, "v2:aa"));
        assert!(!store.contains(LEGACY_SERVICE, "v2:aa"));
        assert!(store.contains(SERVICE, "v2:bb"));
        assert!(store.contains(LEGACY_SERVICE, "v2:bb"));
    }

    #[test]
    fn store_writes_the_current_name_only() {
        let store = MemorySecrets::new();
        let key = VaultKey::new([2u8; VaultKey::LEN]);
        assert!(store_account(&store, "v2:aa", &key, hour_from_now()));
        assert!(store.contains(SERVICE, "v2:aa"));
        assert!(!store.contains(LEGACY_SERVICE, "v2:aa"));
    }
}
