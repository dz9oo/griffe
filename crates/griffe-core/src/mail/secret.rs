//! Mot de passe du serveur d'envoi. Effacé en mémoire, illisible au journal.

use std::fmt;

use zeroize::Zeroize;

/// Secret d'authentification SMTP. `Debug` ne montre pas la valeur.
#[derive(Default)]
pub struct MailSecret {
    value: String,
}

impl MailSecret {
    /// # Errors
    ///
    /// Chaîne vide après trim.
    pub fn new(value: impl Into<String>) -> Result<Self, super::MailError> {
        let mut value = value.into();
        let trimmed = value.trim();
        if trimmed.is_empty() {
            value.zeroize();
            return Err(super::MailError::EmptySecret);
        }
        if trimmed.len() != value.len() {
            let kept = trimmed.to_string();
            value.zeroize();
            value = kept;
        }
        Ok(Self { value })
    }

    #[must_use]
    pub fn expose(&self) -> &str {
        &self.value
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.value.is_empty()
    }
}

impl Drop for MailSecret {
    fn drop(&mut self) {
        self.value.zeroize();
    }
}

impl fmt::Debug for MailSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MailSecret([redacted])")
    }
}
