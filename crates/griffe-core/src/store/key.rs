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

const SERVICE: &str = "dev.freeflow.vault";

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

impl KeyCache for OsKeyring {
    fn load(&self, account: &str) -> Option<CachedKey> {
        let entry = keyring::Entry::new(SERVICE, account).ok()?;
        let bytes = entry.get_secret().ok()?;
        let cached = decode(&bytes)?;
        if cached.expires_at <= OffsetDateTime::now_utc() {
            let _ = self.forget(account);
            return None;
        }
        Some(cached)
    }

    fn store(&self, account: &str, key: &VaultKey, expires_at: OffsetDateTime) -> bool {
        let Ok(entry) = keyring::Entry::new(SERVICE, account) else {
            return false;
        };
        let mut bytes = encode(key, expires_at);
        let ok = entry.set_secret(&bytes).is_ok();
        bytes.zeroize();
        ok
    }

    fn forget(&self, account: &str) -> bool {
        let Ok(entry) = keyring::Entry::new(SERVICE, account) else {
            return true;
        };
        matches!(
            entry.delete_credential(),
            Ok(()) | Err(keyring::Error::NoEntry)
        )
    }
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
}
