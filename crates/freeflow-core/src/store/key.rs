//! Dérivation de clé (Argon2id) et mise en cache dans le trousseau OS (Keychain macOS /
//! Secret Service Linux via `keyring`). La clé dérivée n'est jamais écrite en clair sur disque.

use argon2::{Algorithm, Argon2, Params, Version};
use rand::Rng;

const SERVICE: &str = "dev.freeflow.vault";

pub const KEY_LEN: usize = 32;
pub const SALT_LEN: usize = 16;

/// Paramètres Argon2id : volontairement coûteux (~19 Mio, 2 itérations, ~300-600 ms sur un
/// poste récent). La clé n'est dérivée qu'à la première ouverture ou après un verrouillage
/// explicite ; les ouvertures suivantes réutilisent la clé mise en cache dans le trousseau OS.
fn argon2() -> Argon2<'static> {
    let params = Params::new(19_456, 2, 1, Some(KEY_LEN))
        .expect("paramètres Argon2id fixes toujours valides");
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

#[must_use]
pub fn random_salt() -> [u8; SALT_LEN] {
    let mut salt = [0u8; SALT_LEN];
    rand::rng().fill_bytes(&mut salt);
    salt
}

/// Dérive une clé de chiffrement de 32 octets à partir d'une passphrase et d'un sel.
///
/// # Errors
///
/// Retourne une erreur textuelle si la dérivation échoue (ne devrait jamais arriver avec les
/// paramètres fixes de cette fonction, mais Argon2id peut en théorie rejeter une passphrase
/// vide selon la configuration).
pub fn derive_key(passphrase: &str, salt: &[u8; SALT_LEN]) -> Result<[u8; KEY_LEN], String> {
    let mut key = [0u8; KEY_LEN];
    argon2()
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|e| e.to_string())?;
    Ok(key)
}

/// Clé mise en cache dans le trousseau OS pour ce coffre, si le trousseau est disponible et la
/// contient. `None` dans tous les autres cas (absente, trousseau indisponible) : l'appelant
/// retombe alors sur la dérivation depuis la passphrase — c'est le repli documenté pour les
/// systèmes sans trousseau accessible.
#[must_use]
pub fn load_cached_key(vault_id: &str) -> Option<[u8; KEY_LEN]> {
    let entry = keyring::Entry::new(SERVICE, vault_id).ok()?;
    let bytes = entry.get_secret().ok()?;
    if bytes.len() != KEY_LEN {
        return None;
    }
    let mut key = [0u8; KEY_LEN];
    key.copy_from_slice(&bytes);
    Some(key)
}

/// Met la clé en cache dans le trousseau OS si un trousseau est accessible ; ne fait rien
/// silencieusement sinon — l'ouverture du coffre reste possible dans tous les cas, seule la
/// mise en cache pour les prochaines ouvertures est perdue.
pub fn cache_key(vault_id: &str, key: &[u8; KEY_LEN]) {
    if let Ok(entry) = keyring::Entry::new(SERVICE, vault_id) {
        let _ = entry.set_secret(key);
    }
}

/// Purge la clé du trousseau OS, si elle y était.
#[must_use]
pub fn forget_key(vault_id: &str) -> bool {
    let Ok(entry) = keyring::Entry::new(SERVICE, vault_id) else {
        return true;
    };
    matches!(
        entry.delete_credential(),
        Ok(()) | Err(keyring::Error::NoEntry)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_passphrase_and_salt_derive_the_same_key() {
        let salt = random_salt();
        let a = derive_key("correct horse battery staple", &salt).unwrap();
        let b = derive_key("correct horse battery staple", &salt).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn different_passphrases_derive_different_keys() {
        let salt = random_salt();
        let a = derive_key("passphrase-a", &salt).unwrap();
        let b = derive_key("passphrase-b", &salt).unwrap();
        assert_ne!(a, b);
    }
}
