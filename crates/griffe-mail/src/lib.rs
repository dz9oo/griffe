//! Soumission SMTP, puis dépôt de la même lettre dans Envoyés.
//! Seule crate qui connaît `lettre` et IMAP.
//! Le cœur décide du moment et du texte ; ici on ouvre la liaison et on la referme.

mod imap;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use time::OffsetDateTime;

use griffe_core::mail::{
    DeliveryOutcome, ICLOUD_HOST, ImapEndpoint, LetterParts, MailSecret, MailSubmitError,
    OutboundMail, OutboundMessage, PROBE_OK_SENTENCE, ProbeVerdict, ReadyLetter, SentCopyStatus,
    SmtpEndpoint, SubmissionBatch, SubmissionReceipt, TlsMode, close_letter, french_submit_error,
    letter_html,
};
use lettre::message::{Mailbox, Message, MultiPart, SinglePart};
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

impl LettreMail {
    fn secret(&self) -> &MailSecret {
        &self.secret
    }

    fn send_bytes(
        &self,
        email: &Message,
        bytes: &[u8],
    ) -> Result<SubmissionReceipt, MailSubmitError> {
        let transport = open_transport(&self.endpoint, &self.secret)?;
        match transport.send_raw(email.envelope(), bytes) {
            Ok(response) => Ok(SubmissionReceipt {
                message_id: String::new(),
                smtp_response: shorten(&response_text(&response), self.secret.expose()),
            }),
            Err(error) => Err(classify_lettre(&error, self.secret.expose())),
        }
    }
}

impl OutboundMail for LettreMail {
    fn submit(&self, message: &OutboundMessage) -> Result<SubmissionReceipt, MailSubmitError> {
        let email = compose(message)?;
        let bytes = email.formatted();
        let mut receipt = self.send_bytes(&email, &bytes)?;
        receipt.message_id = message.message_id.clone();
        Ok(receipt)
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

/// Soumet le lot puis oublie le secret. La copie suit un SMTP réussi.
/// Un dépôt manqué ne renvoie pas la lettre.
#[must_use]
pub fn submit_batch(batch: SubmissionBatch) -> Vec<DeliveryOutcome> {
    let SubmissionBatch {
        endpoint,
        secret,
        imap,
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
                    copy: SentCopyStatus::Skipped,
                })
                .collect();
        }
    };
    let outcomes = letters
        .into_iter()
        .map(|letter| submit_letter(&mail, imap.as_ref(), letter))
        .collect();
    drop(mail);
    outcomes
}

/// Soumet une lettre déjà engagée. Le secret reste dans `mail`.
#[must_use]
pub fn submit_letter(
    mail: &LettreMail,
    imap: Option<&ImapEndpoint>,
    letter: ReadyLetter,
) -> DeliveryOutcome {
    let id = letter.id;
    match compose(&letter.message) {
        Err(error) => DeliveryOutcome {
            id,
            result: Err(error),
            copy: SentCopyStatus::Skipped,
        },
        Ok(email) => {
            let bytes = email.formatted();
            match mail.send_bytes(&email, &bytes) {
                Err(error) => DeliveryOutcome {
                    id,
                    result: Err(error),
                    copy: SentCopyStatus::Skipped,
                },
                Ok(response) => {
                    let copy =
                        copy_after_send(imap, mail.secret(), &letter.message.message_id, &bytes);
                    DeliveryOutcome {
                        id,
                        result: Ok(SubmissionReceipt {
                            message_id: letter.message.message_id,
                            smtp_response: response.smtp_response,
                        }),
                        copy,
                    }
                }
            }
        }
    }
}

/// Dépose des lettres déjà parties. N'ouvre pas de session SMTP.
#[must_use]
pub fn file_copies(batch: SubmissionBatch) -> Vec<DeliveryOutcome> {
    let SubmissionBatch {
        secret,
        imap,
        letters,
        ..
    } = batch;
    let Some(imap) = imap else {
        return letters
            .into_iter()
            .map(|letter| DeliveryOutcome {
                id: letter.id,
                result: Ok(SubmissionReceipt {
                    message_id: letter.message.message_id,
                    smtp_response: String::new(),
                }),
                copy: SentCopyStatus::Skipped,
            })
            .collect();
    };
    letters
        .into_iter()
        .map(|letter| {
            let id = letter.id;
            let message_id = letter.message.message_id.clone();
            let copy = match letter_bytes(&letter.message) {
                Ok(bytes) => imap::deposit(&imap, &secret, &message_id, &bytes),
                Err(_) => SentCopyStatus::Failed("La copie n'a pas pu être déposée.".into()),
            };
            DeliveryOutcome {
                id,
                result: Ok(SubmissionReceipt {
                    message_id,
                    smtp_response: String::new(),
                }),
                copy,
            }
        })
        .collect()
}

fn copy_after_send(
    imap: Option<&ImapEndpoint>,
    secret: &MailSecret,
    message_id: &str,
    bytes: &[u8],
) -> SentCopyStatus {
    let Some(imap) = imap else {
        return SentCopyStatus::Skipped;
    };
    imap::deposit(imap, secret, message_id, bytes)
}

/// Essaie le SMTP, puis la copie. Un SMTP refusé n'ouvre pas IMAP.
/// Si le SMTP passe et la copie non, le compte reste accepté.
#[must_use]
pub fn probe_account(
    endpoint: &SmtpEndpoint,
    copy: Option<&ImapEndpoint>,
    secret: &MailSecret,
) -> ProbeVerdict {
    let icloud = endpoint.host == ICLOUD_HOST;
    match probe(endpoint, secret) {
        Err(error) => ProbeVerdict {
            ok: false,
            detail: french_submit_error(&error, icloud),
            imap_username: None,
        },
        Ok(()) => {
            let (sentence, username) = match copy {
                None => ("La copie n'ira pas dans Envoyés.", None),
                Some(imap) => match imap::probe_copy(imap, secret) {
                    Ok(username) => ("La copie ira dans Envoyés.", Some(username)),
                    Err(_) => ("La copie dans Envoyés n'a pas pu être vérifiée.", None),
                },
            };
            ProbeVerdict {
                ok: true,
                detail: format!("{PROBE_OK_SENTENCE} {sentence}"),
                imap_username: username,
            }
        }
    }
}

/// Les octets de la lettre. Deux appels avec les mêmes champs donnent les mêmes octets.
///
/// # Errors
///
/// L'adresse est illisible.
pub fn letter_bytes(message: &OutboundMessage) -> Result<Vec<u8>, MailSubmitError> {
    Ok(compose(message)?.formatted())
}

fn compose(message: &OutboundMessage) -> Result<Message, MailSubmitError> {
    let from = mailbox(message.from_name.as_deref(), &message.from_address)?;
    let to = mailbox(None, &message.to_address)?;
    let text = close_letter(&message.text, &message.chrome.signature);
    let html = letter_html(&LetterParts {
        from_name: message.from_name.as_deref().unwrap_or(""),
        subject: &message.subject,
        body: &text,
        chrome: &message.chrome,
    });
    let plain = SinglePart::plain(text);
    let rich = SinglePart::html(html);
    Message::builder()
        .date(system_time(message.at))
        .from(from)
        .to(to)
        .message_id(Some(message.message_id.clone()))
        .subject(message.subject.clone())
        .multipart(
            MultiPart::alternative()
                .boundary(letter_boundary(&message.message_id))
                .singlepart(plain)
                .singlepart(rich),
        )
        .map_err(|error| MailSubmitError::Refused(shorten(&error.to_string(), "")))
}

/// Frontière MIME stable. `lettre` en tire une au hasard, ce qui changerait
/// les octets d'une même lettre d'un appel à l'autre.
fn letter_boundary(message_id: &str) -> String {
    let mut boundary = String::from("griffe");
    for ch in message_id.chars() {
        if ch.is_ascii_alphanumeric() {
            boundary.push(ch);
        }
    }
    boundary
}

fn system_time(at: OffsetDateTime) -> SystemTime {
    match u64::try_from(at.unix_timestamp()) {
        Ok(seconds) => UNIX_EPOCH + Duration::from_secs(seconds),
        Err(_) => UNIX_EPOCH,
    }
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
    use super::{
        LettreMail, PROBE_OK_SENTENCE, classify_text, compose, letter_bytes, open_transport, probe,
        probe_account,
    };
    use griffe_core::mail::{
        LetterChrome, MailSecret, MailSubmitError, OutboundMessage, SmtpEndpoint, TlsMode,
    };

    fn endpoint(port: u16, tls: TlsMode) -> SmtpEndpoint {
        SmtpEndpoint {
            host: "smtp.invalid.test".into(),
            port,
            tls,
            username: "ada@studio.test".into(),
        }
    }

    #[test]
    fn a_refused_smtp_does_not_talk_about_the_sent_folder() {
        let secret = MailSecret::new("secret-value").unwrap();
        let verdict = probe_account(&endpoint(465, TlsMode::StartTls), None, &secret);
        assert!(!verdict.ok);
        assert!(verdict.imap_username.is_none());
        assert!(!verdict.detail.contains("Envoyés"));
    }

    #[test]
    fn the_copy_sentences_stay_within_the_probe_limit() {
        for sentence in [
            "La copie ira dans Envoyés.",
            "La copie dans Envoyés n'a pas pu être vérifiée.",
            "La copie n'ira pas dans Envoyés.",
        ] {
            let detail = format!("{PROBE_OK_SENTENCE} {sentence}");
            assert!(detail.chars().count() <= 160, "{detail}");
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
    fn the_letter_is_html_with_a_plain_fallback() {
        let secret = MailSecret::new("secret-value").unwrap();
        assert!(LettreMail::new(endpoint(587, TlsMode::StartTls), secret).is_ok());
        let message = OutboundMessage {
            from_name: Some("Ada".into()),
            from_address: "ada@studio.test".into(),
            to_address: "marie@acme.test".into(),
            subject: "Bonjour".into(),
            text: "Une ligne.".into(),
            chrome: LetterChrome::default(),
            message_id: "<lettre@griffe.local>".into(),
            at: time::OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap(),
        };
        let raw = String::from_utf8(compose(&message).unwrap().formatted()).unwrap();
        assert_eq!(
            letter_bytes(&message).unwrap(),
            letter_bytes(&message).unwrap()
        );
        assert!(raw.contains("multipart/alternative"));
        assert!(raw.contains("text/plain"));
        assert!(raw.contains("text/html"));
        assert!(raw.contains("<lettre@griffe.local>"));
        assert!(raw.contains("Une ligne."));
        assert!(!raw.contains("secret-value"));
        assert!(!raw.contains("<img"));
        assert!(!raw.contains("@import"));
        assert!(!raw.contains("url("));
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
