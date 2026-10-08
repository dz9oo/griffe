//! Soumission SMTP. Seule crate qui connaît `lettre`.
//! Le cœur décide du moment et du texte ; ici on ouvre la liaison et on la referme.

use std::time::Duration;

use griffe_core::mail::{
    DeliveryOutcome, MailSecret, MailSubmitError, OutboundMail, OutboundMessage, SmtpEndpoint,
    SubmissionBatch, SubmissionReceipt, TlsMode,
};
use lettre::message::{Mailbox, Message, header::ContentType};
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::response::Response;
use lettre::{SmtpTransport, Transport};
use zeroize::Zeroize;

const TIMEOUT: Duration = Duration::from_secs(20);

/// Transport préparé pour un compte. Le secret s'efface avec cette valeur.
pub struct LettreMail {
    endpoint: SmtpEndpoint,
    secret: MailSecret,
}

impl LettreMail {
    /// # Errors
    ///
    /// Le port ne correspond pas au chiffrement demandé.
    pub fn new(endpoint: SmtpEndpoint, secret: MailSecret) -> Result<Self, MailSubmitError> {
        if endpoint.host.is_empty() || endpoint.port != endpoint.tls.port() {
            return Err(MailSubmitError::Tls);
        }
        Ok(Self { endpoint, secret })
    }
}

impl OutboundMail for LettreMail {
    fn submit(&self, message: &OutboundMessage) -> Result<SubmissionReceipt, MailSubmitError> {
        let email = compose(message)?;
        let transport = open_transport(&self.endpoint, &self.secret)?;
        match transport.send(&email) {
            Ok(response) => Ok(SubmissionReceipt {
                message_id: message.message_id.clone(),
                smtp_response: shorten(&response_text(&response), self.secret.expose()),
            }),
            Err(error) => Err(classify_lettre(&error, self.secret.expose())),
        }
    }
}

/// Ouvre la liaison, la chiffre et authentifie. N'envoie pas de lettre.
///
/// # Errors
///
/// Le port ne correspond pas au chiffrement, ou le serveur a refusé.
pub fn probe(endpoint: &SmtpEndpoint, secret: &MailSecret) -> Result<(), MailSubmitError> {
    if endpoint.host.is_empty() || endpoint.port != endpoint.tls.port() {
        return Err(MailSubmitError::Tls);
    }
    let transport = open_transport(endpoint, secret)?;
    match transport.test_connection() {
        Ok(true) => Ok(()),
        Ok(false) => Err(MailSubmitError::Timeout),
        Err(error) => Err(classify_lettre(&error, secret.expose())),
    }
}

/// Soumet le lot puis oublie le secret. Aucune lettre n'est retentée ici.
#[must_use]
pub fn submit_batch(batch: SubmissionBatch) -> Vec<DeliveryOutcome> {
    let SubmissionBatch {
        endpoint,
        secret,
        letters,
    } = batch;
    let mail = match LettreMail::new(endpoint, secret) {
        Ok(mail) => mail,
        Err(error) => {
            return letters
                .into_iter()
                .map(|letter| DeliveryOutcome {
                    id: letter.id,
                    result: Err(error.clone()),
                })
                .collect();
        }
    };
    let outcomes = letters
        .into_iter()
        .map(|letter| DeliveryOutcome {
            id: letter.id,
            result: mail.submit(&letter.message),
        })
        .collect();
    drop(mail);
    outcomes
}

fn compose(message: &OutboundMessage) -> Result<Message, MailSubmitError> {
    let from = mailbox(message.from_name.as_deref(), &message.from_address)?;
    let to = mailbox(None, &message.to_address)?;
    Message::builder()
        .from(from)
        .to(to)
        .message_id(Some(message.message_id.clone()))
        .subject(message.subject.clone())
        .header(ContentType::TEXT_PLAIN)
        .body(message.text.clone())
        .map_err(|error| MailSubmitError::Refused(shorten(&error.to_string(), "")))
}

fn mailbox(name: Option<&str>, address: &str) -> Result<Mailbox, MailSubmitError> {
    let parsed = address
        .parse()
        .map_err(|_| MailSubmitError::Refused("L'adresse est illisible.".to_string()))?;
    let name = name.map(str::trim).filter(|value| !value.is_empty());
    Ok(Mailbox::new(name.map(ToString::to_string), parsed))
}

fn open_transport(
    endpoint: &SmtpEndpoint,
    secret: &MailSecret,
) -> Result<SmtpTransport, MailSubmitError> {
    let builder = match endpoint.tls {
        TlsMode::StartTls => SmtpTransport::starttls_relay(&endpoint.host),
        TlsMode::Implicit => SmtpTransport::relay(&endpoint.host),
    }
    .map_err(|_| MailSubmitError::Tls)?;
    let mut password = secret.expose().to_string();
    let credentials = Credentials::new(endpoint.username.clone(), password.clone());
    password.zeroize();
    Ok(builder
        .port(endpoint.port)
        .credentials(credentials)
        .timeout(Some(TIMEOUT))
        .build())
}

fn classify_lettre(error: &lettre::transport::smtp::Error, secret: &str) -> MailSubmitError {
    if error.is_timeout() {
        return MailSubmitError::Timeout;
    }
    if error.is_tls() {
        return MailSubmitError::Tls;
    }
    let code = error.status().map(u16::from);
    classify_text(code, &error.to_string(), secret)
}

fn classify_text(code: Option<u16>, text: &str, secret: &str) -> MailSubmitError {
    let lower = text.to_ascii_lowercase();
    let auth = matches!(code, Some(530 | 534 | 535))
        || lower.contains("authentication")
        || lower.contains("authentification")
        || lower.contains("535");
    if auth {
        return MailSubmitError::Auth;
    }
    if lower.contains("certificate") || lower.contains("tls") {
        return MailSubmitError::Tls;
    }
    let message = shorten(text, secret);
    let message = if message.is_empty() {
        "Le serveur a refusé la lettre.".to_string()
    } else {
        message
    };
    if matches!(code, Some(code) if (500..600).contains(&code)) {
        MailSubmitError::Refused(message)
    } else {
        MailSubmitError::Other(message)
    }
}

fn response_text(response: &Response) -> String {
    let code = response.code();
    match response.first_line() {
        Some(line) if !line.is_empty() => format!("{code} {line}"),
        _ => code.to_string(),
    }
}

fn shorten(text: &str, secret: &str) -> String {
    if !secret.is_empty() && text.contains(secret) {
        return "Le serveur a renvoyé un message que Griffe ne recopie pas.".to_string();
    }
    text.chars()
        .filter(|ch| !ch.is_control())
        .take(160)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{LettreMail, classify_text, compose, open_transport, probe};
    use griffe_core::mail::{MailSecret, MailSubmitError, OutboundMessage, SmtpEndpoint, TlsMode};

    fn endpoint(port: u16, tls: TlsMode) -> SmtpEndpoint {
        SmtpEndpoint {
            host: "smtp.invalid.test".into(),
            port,
            tls,
            username: "ada@studio.test".into(),
        }
    }

    #[test]
    fn probe_refuses_a_mismatched_port_without_the_password() {
        let secret = MailSecret::new("secret-value").unwrap();
        let error = probe(&endpoint(465, TlsMode::StartTls), &secret).unwrap_err();
        assert_eq!(error, MailSubmitError::Tls);
        assert!(!format!("{error:?}").contains("secret-value"));
    }

    #[test]
    fn a_mismatched_port_is_refused_before_any_socket() {
        let secret = MailSecret::new("secret-value").unwrap();
        let built = LettreMail::new(endpoint(465, TlsMode::StartTls), secret);
        assert!(built.is_err());
    }

    #[test]
    fn the_letter_is_plain_text_and_keeps_its_message_id() {
        let secret = MailSecret::new("secret-value").unwrap();
        assert!(LettreMail::new(endpoint(587, TlsMode::StartTls), secret).is_ok());
        let message = OutboundMessage {
            from_name: Some("Ada".into()),
            from_address: "ada@studio.test".into(),
            to_address: "marie@acme.test".into(),
            subject: "Bonjour".into(),
            text: "Une ligne.".into(),
            message_id: "<lettre@griffe.local>".into(),
        };
        let raw = String::from_utf8(compose(&message).unwrap().formatted()).unwrap();
        assert!(raw.contains("text/plain"));
        assert!(raw.contains("<lettre@griffe.local>"));
        assert!(raw.contains("Une ligne."));
        assert!(!raw.contains("secret-value"));
        assert!(!raw.to_ascii_lowercase().contains("text/html"));
    }

    #[test]
    fn building_the_transport_does_not_connect() {
        let secret = MailSecret::new("secret-value").unwrap();
        let transport = open_transport(&endpoint(587, TlsMode::StartTls), &secret);
        assert!(transport.is_ok());
        assert!(!format!("{transport:?}").contains("secret-value"));
    }

    #[test]
    fn authentication_failure_is_named_without_the_password() {
        let error = classify_text(
            Some(535),
            "535 5.7.8 authentication failed secret-value",
            "secret-value",
        );
        assert_eq!(error, MailSubmitError::Auth);
        let other = classify_text(Some(550), "550 user unknown", "");
        assert!(matches!(other, MailSubmitError::Refused(_)));
        let bare = classify_text(None, "the author of the letter", "");
        assert!(matches!(bare, MailSubmitError::Other(_)));
    }
}
