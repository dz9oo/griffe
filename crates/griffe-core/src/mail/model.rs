//! Types d'envoi. Aucune crate SMTP.

use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use super::secret::MailSecret;

/// Hôte iCloud. Le préréglage ne fait que remplir ces valeurs.
pub const ICLOUD_HOST: &str = "smtp.mail.me.com";

/// Soumissions acceptées par heure de coffre ouvert.
pub const HOURLY_CAP: i64 = 30;

/// Délai pendant lequel Annuler gagne encore.
pub const UNDO_SECS: i64 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TlsMode {
    StartTls,
    Implicit,
}

impl TlsMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::StartTls => "starttls",
            Self::Implicit => "implicit",
        }
    }

    /// # Errors
    ///
    /// Toute autre valeur, y compris une connexion en clair.
    pub fn parse(value: &str) -> Result<Self, super::MailError> {
        match value {
            "starttls" => Ok(Self::StartTls),
            "implicit" => Ok(Self::Implicit),
            _ => Err(super::MailError::Cleartext),
        }
    }

    #[must_use]
    pub const fn port(self) -> u16 {
        match self {
            Self::StartTls => 587,
            Self::Implicit => 465,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MailPreset {
    Icloud,
    Custom,
}

impl MailPreset {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Icloud => "icloud",
            Self::Custom => "custom",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Self {
        match value {
            "icloud" => Self::Icloud,
            _ => Self::Custom,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutboundStatus {
    Armed,
    Sending,
    Sent,
    Failed,
    Uncertain,
    Cancelled,
}

impl OutboundStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Armed => "armed",
            Self::Sending => "sending",
            Self::Sent => "sent",
            Self::Failed => "failed",
            Self::Uncertain => "uncertain",
            Self::Cancelled => "cancelled",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Self {
        match value {
            "armed" => Self::Armed,
            "sending" => Self::Sending,
            "sent" => Self::Sent,
            "uncertain" => Self::Uncertain,
            "cancelled" => Self::Cancelled,
            // Inconnu ou « failed » : la lettre n'est pas réputée partie.
            _ => Self::Failed,
        }
    }
}

/// Compte tel que l'écran le montre. Jamais le secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailProfile {
    pub from_name: String,
    pub from_address: String,
    pub host: String,
    pub port: u16,
    pub tls: TlsMode,
    pub username: String,
    pub has_secret: bool,
    pub preset: MailPreset,
    pub auto_send: bool,
    pub ready: bool,
    /// Lignes qui ferment la lettre. Vides : la formule « Bien à vous ».
    pub signature: String,
}

impl Default for MailProfile {
    fn default() -> Self {
        Self {
            from_name: String::new(),
            from_address: String::new(),
            host: String::new(),
            port: 587,
            tls: TlsMode::StartTls,
            username: String::new(),
            has_secret: false,
            preset: MailPreset::Custom,
            auto_send: false,
            ready: false,
            signature: String::new(),
        }
    }
}

/// Ce que la fenêtre montre d'une lettre en cours.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboundView {
    pub id: String,
    pub status: OutboundStatus,
    pub subject: String,
    pub to_address: String,
    pub error: Option<String>,
    pub seconds_left: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmtpEndpoint {
    pub host: String,
    pub port: u16,
    pub tls: TlsMode,
    pub username: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboundMessage {
    pub from_name: Option<String>,
    pub from_address: String,
    pub to_address: String,
    pub subject: String,
    pub text: String,
    pub message_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmissionReceipt {
    pub message_id: String,
    pub smtp_response: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MailSubmitError {
    Auth,
    Tls,
    Timeout,
    Refused(String),
    Other(String),
}

/// Lettre réclamée, prête à être soumise par l'adaptateur.
pub struct ReadyLetter {
    pub id: String,
    pub message: OutboundMessage,
}

/// Lot sorti du coffre. Le secret s'efface avec le lot.
pub struct SubmissionBatch {
    pub endpoint: SmtpEndpoint,
    pub secret: MailSecret,
    pub letters: Vec<ReadyLetter>,
}

#[derive(Debug)]
pub struct DeliveryOutcome {
    pub id: String,
    pub result: Result<SubmissionReceipt, MailSubmitError>,
}

/// Transport d'envoi. Une implémentation par crate, interchangeable.
pub trait OutboundMail {
    /// # Errors
    ///
    /// Le serveur a refusé, ou la liaison n'a pas abouti.
    fn submit(&self, message: &OutboundMessage) -> Result<SubmissionReceipt, MailSubmitError>;
}

/// Transport de test : enregistre les lettres, n'ouvre aucune connexion.
#[derive(Default)]
pub struct RecordingMail {
    sent: Mutex<Vec<OutboundMessage>>,
    fail: Option<MailSubmitError>,
}

impl RecordingMail {
    #[must_use]
    pub fn failing(error: MailSubmitError) -> Self {
        Self {
            sent: Mutex::new(Vec::new()),
            fail: Some(error),
        }
    }

    /// # Panics
    ///
    /// Si le verrou a été empoisonné.
    #[must_use]
    pub fn sent(&self) -> Vec<OutboundMessage> {
        self.sent
            .lock()
            .expect("le verrou du transport de test reste sain")
            .clone()
    }
}

impl OutboundMail for RecordingMail {
    fn submit(&self, message: &OutboundMessage) -> Result<SubmissionReceipt, MailSubmitError> {
        if let Some(error) = &self.fail {
            return Err(error.clone());
        }
        self.sent
            .lock()
            .expect("le verrou du transport de test reste sain")
            .push(message.clone());
        Ok(SubmissionReceipt {
            message_id: message.message_id.clone(),
            smtp_response: "250 ok".to_string(),
        })
    }
}
