//! Types de secret qui ne s'affichent jamais en clair et s'effacent de la mémoire à leur perte.
//!
//! Aucun des deux types ci-dessous n'implémente `Display` ni `Serialize` : la seule façon d'en
//! lire le contenu est l'appel explicite `expose()`, volontairement visible dans une revue de
//! code. `Debug` est implémenté à la main pour ne jamais imprimer le secret.

use std::fmt;

use zeroize::{Zeroize, ZeroizeOnDrop};

/// Une passphrase utilisateur en mémoire.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct Passphrase(String);

impl Passphrase {
    #[must_use]
    pub fn new(value: String) -> Self {
        Self(value)
    }

    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Passphrase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Passphrase(***)")
    }
}

impl From<String> for Passphrase {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for Passphrase {
    fn from(value: &str) -> Self {
        Self::new(value.to_string())
    }
}

/// Clé de chiffrement dérivée : 32 octets bruts, jamais affichés, effacés au `Drop`.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct VaultKey([u8; Self::LEN]);

impl VaultKey {
    pub const LEN: usize = 32;

    #[must_use]
    pub fn new(bytes: [u8; Self::LEN]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8; Self::LEN] {
        &self.0
    }
}

impl fmt::Debug for VaultKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("VaultKey(***)")
    }
}
