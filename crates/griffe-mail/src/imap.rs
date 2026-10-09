//! Dépôt IMAP de la copie. Pas de lecture de la boîte de réception.
//! La liaison est chiffrée sur le port 993. Aucun dossier n'est créé.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

use griffe_core::mail::{
    ICLOUD_SENT_MAILBOX, IMAP_PORT, ImapEndpoint, MailSecret, MailSubmitError, SentCopyStatus,
};
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, StreamOwned};
use zeroize::Zeroize;

const TIMEOUT: Duration = Duration::from_secs(20);
const MAX_LINE: usize = 1024 * 1024;

/// Noms d'essai, sans doublon : celui qui a déjà marché, la partie avant `@`,
/// puis l'adresse complète.
#[must_use]
pub fn login_names(remembered: Option<&str>, smtp_username: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut push = |value: &str| {
        let value = value.trim();
        if value.is_empty()
            || names
                .iter()
                .any(|known: &String| known.eq_ignore_ascii_case(value))
        {
            return;
        }
        names.push(value.to_string());
    };
    if let Some(remembered) = remembered {
        push(remembered);
    }
    if let Some((local, _)) = smtp_username.split_once('@') {
        push(local);
    }
    push(smtp_username);
    names
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedMailbox {
    pub name: String,
    pub sent: bool,
}

/// Le dossier marqué `\Sent`, sinon le nom de repli connu.
#[must_use]
pub fn choose_sent_mailbox(entries: &[ListedMailbox], icloud: bool) -> Option<String> {
    if let Some(found) = entries.iter().find(|entry| entry.sent) {
        return Some(found.name.clone());
    }
    let fallbacks = if icloud {
        vec![ICLOUD_SENT_MAILBOX]
    } else {
        vec!["Sent", ICLOUD_SENT_MAILBOX]
    };
    for candidate in fallbacks {
        if let Some(found) = entries
            .iter()
            .find(|entry| entry.name.eq_ignore_ascii_case(candidate))
        {
            return Some(found.name.clone());
        }
    }
    None
}

/// Une ligne `LIST`. `None` si ce n'est pas une liste de dossiers.
#[must_use]
pub fn parse_list_line(line: &str) -> Option<ListedMailbox> {
    let rest = line.trim().strip_prefix("* LIST ")?;
    let rest = rest.trim_start();
    if !rest.starts_with('(') {
        return None;
    }
    let end = rest.find(')')?;
    let sent = rest[1..end]
        .split_whitespace()
        .any(|attr| attr.eq_ignore_ascii_case("\\Sent"));
    let mut rest = rest[end + 1..].trim_start();
    rest = skip_astring(rest)?.trim_start();
    let (name, _) = parse_astring(rest)?;
    Some(ListedMailbox { name, sent })
}

/// Dépose `bytes` dans Envoyés s'il n'y est pas déjà. N'envoie rien par SMTP.
pub fn deposit(
    endpoint: &ImapEndpoint,
    secret: &MailSecret,
    message_id: &str,
    bytes: &[u8],
) -> SentCopyStatus {
    match deposit_result(endpoint, secret, message_id, bytes) {
        Ok(username) => SentCopyStatus::Saved { username },
        Err(error) => SentCopyStatus::Failed(copy_sentence(&error)),
    }
}

/// Ouvre la session et vérifie que le dossier Envoyés existe. Ne sélectionne pas INBOX.
pub fn probe_copy(endpoint: &ImapEndpoint, secret: &MailSecret) -> Result<String, MailSubmitError> {
    let (mut session, username) = login(endpoint, secret)?;
    let entries = session.list(secret.expose())?;
    if choose_sent_mailbox(&entries, endpoint.icloud).is_none() {
        return Err(MailSubmitError::Other(
            "Le dossier Envoyés est introuvable.".into(),
        ));
    }
    session.logout();
    Ok(username)
}

fn deposit_result(
    endpoint: &ImapEndpoint,
    secret: &MailSecret,
    message_id: &str,
    bytes: &[u8],
) -> Result<String, MailSubmitError> {
    let (mut session, username) = login(endpoint, secret)?;
    let entries = session.list(secret.expose())?;
    let Some(mailbox) = choose_sent_mailbox(&entries, endpoint.icloud) else {
        return Err(MailSubmitError::Other(
            "Le dossier Envoyés est introuvable.".into(),
        ));
    };
    session.select(&mailbox, secret.expose())?;
    if !session.already_there(message_id, secret.expose())? {
        session.append(&mailbox, bytes, secret.expose())?;
    }
    session.logout();
    Ok(username)
}

fn copy_sentence(error: &MailSubmitError) -> String {
    match error {
        MailSubmitError::Auth => "Le serveur a refusé le mot de passe.".to_string(),
        MailSubmitError::Other(message) | MailSubmitError::Refused(message)
            if message.contains("Envoyés") =>
        {
            message.clone()
        }
        MailSubmitError::Timeout => "Le serveur n'a pas répondu.".to_string(),
        MailSubmitError::Tls => "La liaison chiffrée a échoué.".to_string(),
        _ => "La copie n'a pas pu être déposée.".to_string(),
    }
}

fn login(
    endpoint: &ImapEndpoint,
    secret: &MailSecret,
) -> Result<(Session, String), MailSubmitError> {
    if endpoint.port != IMAP_PORT || endpoint.host.is_empty() {
        return Err(MailSubmitError::Tls);
    }
    let names = login_names(endpoint.remembered.as_deref(), &endpoint.username);
    if names.is_empty() {
        return Err(MailSubmitError::Auth);
    }
    let mut last = MailSubmitError::Auth;
    for name in names {
        let mut session = connect(endpoint)?;
        let mut password = secret.expose().to_string();
        let result = session.login(&name, &password);
        password.zeroize();
        match result {
            Ok(()) => return Ok((session, name)),
            Err(error) => last = error,
        }
    }
    Err(last)
}

fn connect(endpoint: &ImapEndpoint) -> Result<Session, MailSubmitError> {
    let config = tls_config()?;
    let name = ServerName::try_from(endpoint.host.clone()).map_err(|_| MailSubmitError::Tls)?;
    let connection = ClientConnection::new(config, name).map_err(|_| MailSubmitError::Tls)?;
    let sock = TcpStream::connect((endpoint.host.as_str(), endpoint.port))
        .map_err(|_| MailSubmitError::Timeout)?;
    sock.set_read_timeout(Some(TIMEOUT))
        .map_err(|_| MailSubmitError::Timeout)?;
    sock.set_write_timeout(Some(TIMEOUT))
        .map_err(|_| MailSubmitError::Timeout)?;
    let stream = StreamOwned::new(connection, sock);
    let mut session = Session { stream, tag: 0 };
    let greeting = session.read_line()?;
    let upper = greeting.to_ascii_uppercase();
    if !upper.contains(" OK") && !upper.starts_with("* OK") && !upper.contains("PREAUTH") {
        return Err(MailSubmitError::Tls);
    }
    Ok(session)
}

fn tls_config() -> Result<Arc<ClientConfig>, MailSubmitError> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
        .map_err(|_| MailSubmitError::Tls)?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(config))
}

struct Session {
    stream: StreamOwned<ClientConnection, TcpStream>,
    tag: u32,
}

impl Session {
    fn login(&mut self, user: &str, password: &str) -> Result<(), MailSubmitError> {
        if contains_break(user) || contains_break(password) {
            return Err(MailSubmitError::Auth);
        }
        let mut body = format!("LOGIN {} {}", quote(user), quote(password));
        let result = self.command(&body, password);
        body.zeroize();
        result.map(|_| ())
    }

    fn list(&mut self, secret: &str) -> Result<Vec<ListedMailbox>, MailSubmitError> {
        let lines = self.command("LIST \"\" \"*\"", secret)?;
        Ok(lines
            .iter()
            .filter_map(|line| parse_list_line(line))
            .collect())
    }

    fn select(&mut self, mailbox: &str, secret: &str) -> Result<(), MailSubmitError> {
        self.command(&format!("SELECT {}", quote(mailbox)), secret)
            .map(|_| ())
    }

    fn already_there(&mut self, message_id: &str, secret: &str) -> Result<bool, MailSubmitError> {
        let lines = self.command(
            &format!("SEARCH HEADER Message-ID {}", quote(message_id)),
            secret,
        )?;
        Ok(lines.iter().any(|line| {
            line.trim()
                .strip_prefix("* SEARCH")
                .is_some_and(|rest| !rest.trim().is_empty())
        }))
    }

    fn append(&mut self, mailbox: &str, bytes: &[u8], secret: &str) -> Result<(), MailSubmitError> {
        let head = format!("APPEND {} (\\Seen) {{{}}}", quote(mailbox), bytes.len());
        self.write_command(&head)?;
        let cont = self.read_line()?;
        if !cont.starts_with('+') {
            return Err(classify(&cont, secret));
        }
        self.stream.write_all(bytes).map_err(io_error)?;
        self.stream.write_all(b"\r\n").map_err(io_error)?;
        self.stream.flush().map_err(io_error)?;
        self.read_tagged(secret).map(|_| ())
    }

    fn logout(&mut self) {
        let _ = self.write_command("LOGOUT");
    }

    fn command(&mut self, body: &str, secret: &str) -> Result<Vec<String>, MailSubmitError> {
        self.write_command(body)?;
        self.read_tagged(secret)
    }

    fn write_command(&mut self, body: &str) -> Result<(), MailSubmitError> {
        self.tag += 1;
        let mut line = format!("A{} {body}\r\n", self.tag);
        let result = self.stream.write_all(line.as_bytes()).map_err(io_error);
        line.zeroize();
        self.stream.flush().map_err(io_error)?;
        result
    }

    fn read_tagged(&mut self, secret: &str) -> Result<Vec<String>, MailSubmitError> {
        let tag = format!("A{}", self.tag);
        let mut lines = Vec::new();
        loop {
            let line = self.read_line()?;
            if let Some(rest) = line.strip_prefix(&tag) {
                let rest = rest.trim_start();
                if rest.starts_with("OK") {
                    lines.push(line);
                    return Ok(lines);
                }
                return Err(classify(rest, secret));
            }
            if line.starts_with("+ ") || line == "+" {
                return Err(MailSubmitError::Other(
                    "La copie n'a pas pu être déposée.".into(),
                ));
            }
            lines.push(line);
            if lines.len() > 5_000 {
                return Err(MailSubmitError::Other(
                    "La copie n'a pas pu être déposée.".into(),
                ));
            }
        }
    }

    fn read_line(&mut self) -> Result<String, MailSubmitError> {
        let mut buf = Vec::new();
        loop {
            let byte = self.read_byte()?;
            buf.push(byte);
            if buf.len() > MAX_LINE {
                return Err(MailSubmitError::Other(
                    "La copie n'a pas pu être déposée.".into(),
                ));
            }
            if buf.ends_with(b"\r\n") {
                buf.truncate(buf.len() - 2);
                if let Some(len) = trailing_literal(&buf) {
                    let mut rest = vec![0_u8; len];
                    self.stream.read_exact(&mut rest).map_err(io_error)?;
                    buf.extend(rest);
                    continue;
                }
                return String::from_utf8(buf).map_err(|_| {
                    MailSubmitError::Other("La copie n'a pas pu être déposée.".into())
                });
            }
        }
    }

    fn read_byte(&mut self) -> Result<u8, MailSubmitError> {
        let mut byte = [0_u8; 1];
        self.stream.read_exact(&mut byte).map_err(io_error)?;
        Ok(byte[0])
    }
}

fn contains_break(value: &str) -> bool {
    value
        .chars()
        .any(|ch| ch == '\r' || ch == '\n' || ch == '\0')
}

fn quote(value: &str) -> String {
    let mut out = String::from("\"");
    for ch in value.chars() {
        if ch == '"' || ch == '\\' {
            out.push('\\');
        }
        out.push(ch);
    }
    out.push('"');
    out
}

fn classify(line: &str, secret: &str) -> MailSubmitError {
    let lower = line.to_ascii_lowercase();
    if lower.contains("authentication")
        || lower.contains("authentification")
        || lower.contains("login")
        || lower.contains("credentials")
        || lower.contains("password")
    {
        return MailSubmitError::Auth;
    }
    if !secret.is_empty() && line.contains(secret) {
        return MailSubmitError::Other("La copie n'a pas pu être déposée.".into());
    }
    if lower.contains("tls") || lower.contains("certificate") {
        return MailSubmitError::Tls;
    }
    MailSubmitError::Other("La copie n'a pas pu être déposée.".into())
}

fn io_error(error: std::io::Error) -> MailSubmitError {
    match error.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => MailSubmitError::Timeout,
        _ => MailSubmitError::Other("La copie n'a pas pu être déposée.".into()),
    }
}

fn trailing_literal(line: &[u8]) -> Option<usize> {
    if !line.ends_with(b"}") {
        return None;
    }
    let open = line.iter().rposition(|byte| *byte == b'{')?;
    if !quotes_even(&line[..open]) {
        return None;
    }
    let number = std::str::from_utf8(&line[open + 1..line.len() - 1]).ok()?;
    let len = number.parse().ok()?;
    (len <= MAX_LINE).then_some(len)
}

fn quotes_even(bytes: &[u8]) -> bool {
    let mut quoted = false;
    let mut escape = false;
    for byte in bytes {
        if escape {
            escape = false;
            continue;
        }
        if *byte == b'\\' && quoted {
            escape = true;
            continue;
        }
        if *byte == b'"' {
            quoted = !quoted;
        }
    }
    !quoted
}

fn parse_astring(input: &str) -> Option<(String, &str)> {
    let input = input.trim_start();
    if let Some(rest) = input.strip_prefix('"') {
        let mut out = String::new();
        let mut chars = rest.chars();
        while let Some(ch) = chars.next() {
            if ch == '\\' {
                out.push(chars.next()?);
            } else if ch == '"' {
                return Some((out, chars.as_str()));
            } else {
                out.push(ch);
            }
        }
        return None;
    }
    let end = input
        .find(|ch: char| ch.is_whitespace())
        .unwrap_or(input.len());
    if end == 0 {
        return None;
    }
    Some((input[..end].to_string(), &input[end..]))
}

fn skip_astring(input: &str) -> Option<&str> {
    parse_astring(input).map(|(_, rest)| rest)
}

#[cfg(test)]
mod tests {
    use super::{choose_sent_mailbox, login_names, parse_list_line, probe_copy};
    use griffe_core::mail::{ImapEndpoint, MailSecret, MailSubmitError};

    #[test]
    fn login_tries_the_remembered_name_then_the_local_part() {
        let names = login_names(Some("ada"), "ada@icloud.com");
        assert_eq!(names, ["ada", "ada@icloud.com"]);
        let fresh = login_names(None, "ada@icloud.com");
        assert_eq!(fresh, ["ada", "ada@icloud.com"]);
        let full = login_names(Some("ada@icloud.com"), "ada@icloud.com");
        assert_eq!(full, ["ada@icloud.com", "ada"]);
    }

    #[test]
    fn the_sent_flag_wins_over_the_fallback_name() {
        let lines = [
            r#"* LIST (\HasNoChildren) "/" INBOX"#,
            r#"* LIST (\HasNoChildren \Sent) "/" "Sent Messages""#,
            r#"* LIST (\HasNoChildren) "/" Sent"#,
        ];
        let entries: Vec<_> = lines
            .iter()
            .filter_map(|line| parse_list_line(line))
            .collect();
        assert_eq!(
            choose_sent_mailbox(&entries, false).as_deref(),
            Some("Sent Messages")
        );
        let plain = [parse_list_line(r#"* LIST (\HasNoChildren) "/" Sent"#).unwrap()];
        assert_eq!(choose_sent_mailbox(&plain, false).as_deref(), Some("Sent"));
        assert_eq!(choose_sent_mailbox(&plain, true).as_deref(), None);
        let icloud = [parse_list_line(r#"* LIST (\HasNoChildren) "/" "Sent Messages""#).unwrap()];
        assert_eq!(
            choose_sent_mailbox(&icloud, true).as_deref(),
            Some("Sent Messages")
        );
    }

    #[test]
    fn a_port_other_than_993_is_refused_before_a_socket() {
        let endpoint = ImapEndpoint {
            host: "127.0.0.1".into(),
            port: 143,
            username: "ada@exemple.test".into(),
            remembered: None,
            icloud: false,
        };
        let secret = MailSecret::new("secret-de-test").unwrap();
        let error = probe_copy(&endpoint, &secret).unwrap_err();
        assert_eq!(error, MailSubmitError::Tls);
    }
}
