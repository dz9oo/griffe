//! Dépôt IMAP de la copie, et lecture des lettres nouvelles.
//! `BODY.PEEK` : le relevé ne marque pas lu. La liaison est chiffrée sur le
//! port 993. Aucun dossier n'est créé, rien n'est détruit.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

use griffe_core::mail::{
    BODY_CAP_BYTES, HEADER_BATCH, ICLOUD_SENT_MAILBOX, IMAP_PORT, ImapEndpoint, MailSecret,
    MailSubmitError, PullEvent, PullRole, PullTick, PulledBody, PulledMailbox, PulledMessage,
    SentCopyStatus,
};
use griffe_core::mailbox::MailboxCursor;
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, StreamOwned};
use zeroize::Zeroize;

const TIMEOUT: Duration = Duration::from_secs(60);
const MAX_PREFIX: usize = 256 * 1024;
const HEADER_LITERAL: usize = 1024 * 1024;
const SEARCH_PREFIX: usize = 8 * 1024 * 1024;
const MAX_UNTAGGED: usize = 200_000;
const OVERSIZE_MARK: &[u8] = b" GRF-OVERSIZE";

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

/// Lit les messages nouveaux de la boîte de réception et d'Envoyés.
/// `BODY.PEEK` : le relevé ne marque pas lu.
///
/// # Errors
///
/// La liaison, l'identifiant ou la lecture échoue. Le mot de passe n'est pas dans le message.
pub fn fetch_new(
    endpoint: &ImapEndpoint,
    secret: &MailSecret,
    cursors: &[MailboxCursor],
) -> Result<Vec<PulledMailbox>, MailSubmitError> {
    let mut batches = Vec::new();
    fetch_reporting(endpoint, secret, cursors, &mut |event| {
        if let PullEvent::Batch(batch) = event {
            batches.push(batch);
        }
    })?;
    Ok(batches)
}

/// Comme [`fetch_new`], et annonce chaque lot dès qu'il est lu.
/// Le rappel ne reçoit ni mot de passe ni corps.
///
/// # Errors
///
/// La liaison, l'identifiant ou la lecture échoue. Le mot de passe n'est pas dans le message.
pub fn fetch_reporting(
    endpoint: &ImapEndpoint,
    secret: &MailSecret,
    cursors: &[MailboxCursor],
    on: &mut dyn FnMut(PullEvent),
) -> Result<(), MailSubmitError> {
    let (mut session, _) = login(endpoint, secret)?;
    sync_folders(&mut session, endpoint, secret, cursors, on)?;
    session.logout();
    Ok(())
}

/// Corps d'un message déjà indexé. Au-delà du plafond, les en-têtes restent.
///
/// # Errors
///
/// La liaison ou la lecture échoue.
pub fn fetch_body(
    endpoint: &ImapEndpoint,
    secret: &MailSecret,
    mailbox: &str,
    uid: u32,
) -> Result<PulledMessage, MailSubmitError> {
    let (mut session, _) = login(endpoint, secret)?;
    let message = session.fetch_one_body(secret.expose(), mailbox, uid)?;
    session.logout();
    Ok(message)
}

/// Corps des lignes encore `pending`, dans une seule liaison.
///
/// # Errors
///
/// La liaison ou la lecture échoue.
pub fn fetch_pending(
    endpoint: &ImapEndpoint,
    secret: &MailSecret,
    pending: &[griffe_core::mailbox::PendingBody],
) -> Result<Vec<PulledMailbox>, MailSubmitError> {
    if pending.is_empty() {
        return Ok(Vec::new());
    }
    let (mut session, _) = login(endpoint, secret)?;
    let mut batches = Vec::with_capacity(pending.len());
    for item in pending {
        let Ok(uid) = u32::try_from(item.uid) else {
            continue;
        };
        let message = session.fetch_one_body(secret.expose(), &item.mailbox, uid)?;
        batches.push(PulledMailbox {
            name: item.mailbox.clone(),
            uidvalidity: item.uidvalidity,
            resync: false,
            role: "inbox".to_string(),
            messages: vec![message],
            modseq: None,
        });
    }
    session.logout();
    Ok(batches)
}

fn sync_folders(
    session: &mut Session,
    endpoint: &ImapEndpoint,
    secret: &MailSecret,
    cursors: &[MailboxCursor],
    on: &mut dyn FnMut(PullEvent),
) -> Result<(), MailSubmitError> {
    let password = secret.expose();
    let listed = session.list(password)?;
    let mut names = Vec::new();
    let inbox = listed
        .iter()
        .find(|entry| entry.name.eq_ignore_ascii_case("INBOX"))
        .map(|entry| entry.name.clone())
        .unwrap_or_else(|| "INBOX".to_string());
    names.push(inbox);
    if let Some(sent) = choose_sent_mailbox(&listed, endpoint.icloud)
        && !names.iter().any(|name| name.eq_ignore_ascii_case(&sent))
    {
        names.push(sent);
    }
    for name in names {
        let role = if name.eq_ignore_ascii_case("INBOX") {
            "inbox"
        } else {
            "sent"
        };
        session.pull_headers(password, &name, role, cursors, on)?;
    }
    Ok(())
}

/// Marque un message lu sur le serveur.
///
/// # Errors
///
/// La liaison ou l'écriture de l'indicateur échoue.
pub fn store_seen(
    endpoint: &ImapEndpoint,
    secret: &MailSecret,
    mailbox: &str,
    uid: u32,
) -> Result<(), MailSubmitError> {
    let (mut session, _) = login(endpoint, secret)?;
    let password = secret.expose();
    session.select(mailbox, password)?;
    session
        .command(&format!("UID STORE {uid} +FLAGS.SILENT (\\Seen)"), password)
        .map(|_| ())?;
    session.logout();
    Ok(())
}

/// `EXISTS` lu dans la réponse `SELECT`. `* 14 EXISTS`.
#[must_use]
pub fn parse_exists(lines: &[String]) -> Option<u32> {
    for line in lines {
        let Some(rest) = line.trim().strip_prefix('*') else {
            continue;
        };
        let rest = rest.trim_start();
        let Some((number, tail)) = rest.split_once(char::is_whitespace) else {
            continue;
        };
        if tail.trim().eq_ignore_ascii_case("EXISTS")
            && let Ok(value) = number.parse()
        {
            return Some(value);
        }
    }
    None
}

/// `UIDVALIDITY` lu dans la réponse `SELECT`.
#[must_use]
pub fn parse_uidvalidity(lines: &[String]) -> Option<u64> {
    for line in lines {
        let Some(at) = line.find("UIDVALIDITY") else {
            continue;
        };
        let rest = line[at + "UIDVALIDITY".len()..].trim_start();
        let number: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(value) = number.parse() {
            return Some(value);
        }
    }
    None
}

/// Une ligne `FETCH` dont le littéral a déjà été absorbé.
#[must_use]
pub fn parse_fetch_line(line: &[u8]) -> Option<PulledMessage> {
    let fetch_at = line.windows(7).position(|window| window == b" FETCH ")?;
    let marker = line[fetch_at..]
        .windows(4)
        .position(|window| window == b"BODY")?
        + fetch_at;
    let after = &line[marker..];
    let brace = after.iter().position(|byte| *byte == b'{')?;
    let close = after[brace..].iter().position(|byte| *byte == b'}')?;
    let number = std::str::from_utf8(&after[brace + 1..brace + close]).ok()?;
    let len: usize = number.parse().ok()?;
    let start = marker + brace + close + 1;
    let end = start.checked_add(len)?;
    if end > line.len() {
        return None;
    }
    let prefix = std::str::from_utf8(&line[..marker]).ok()?;
    Some(PulledMessage {
        uid: i64::from(uid_of(prefix)?),
        seen: prefix.contains("\\Seen"),
        raw: String::from_utf8_lossy(&line[start..end]).into_owned(),
        body: PulledBody::Ready,
        size: rfc822_size(prefix),
    })
}

fn rfc822_size(prefix: &str) -> Option<i64> {
    let upper = prefix.to_ascii_uppercase();
    let at = upper.find("RFC822.SIZE")?;
    let rest = prefix[at + "RFC822.SIZE".len()..].trim_start();
    let number: String = rest.chars().take_while(char::is_ascii_digit).collect();
    number.parse().ok()
}

/// `UIDNEXT` ou `HIGHESTMODSEQ` dans une réponse `SELECT`.
#[must_use]
pub fn parse_code_number(lines: &[String], key: &str) -> Option<u64> {
    let needle = format!("[{key} ");
    for line in lines {
        let Some(at) = line.find(&needle) else {
            continue;
        };
        let rest = &line[at + needle.len()..];
        let number: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(value) = number.parse() {
            return Some(value);
        }
    }
    None
}

/// UID renvoyés par `UID SEARCH`.
#[must_use]
pub fn parse_search_uids(lines: &[String]) -> Vec<i64> {
    let mut uids = Vec::new();
    for line in lines {
        let Some(rest) = line.trim().strip_prefix("* SEARCH") else {
            continue;
        };
        for token in rest.split_whitespace() {
            if let Ok(uid) = token.parse() {
                uids.push(uid);
            }
        }
    }
    uids
}

fn uid_of(prefix: &str) -> Option<u32> {
    let mut rest = prefix;
    while let Some(at) = rest.find("UID") {
        let after = &rest[at + 3..];
        if let Some(stripped) = after.strip_prefix("VALIDITY") {
            rest = stripped;
            continue;
        }
        let after = after.trim_start();
        if !after.starts_with(|ch: char| ch.is_ascii_digit()) {
            rest = &rest[at + 3..];
            continue;
        }
        let number: String = after.chars().take_while(char::is_ascii_digit).collect();
        return number.parse().ok();
    }
    None
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

pub(crate) fn open_session(
    endpoint: &ImapEndpoint,
    secret: &MailSecret,
) -> Result<Session, MailSubmitError> {
    login(endpoint, secret).map(|(session, _)| session)
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
    let stream = BufReader::new(StreamOwned::new(connection, sock));
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

pub(crate) struct Session {
    stream: BufReader<StreamOwned<ClientConnection, TcpStream>>,
    tag: u32,
}

struct FlagWatch<'a> {
    secret: &'a str,
    name: &'a str,
    role: &'a str,
    uidvalidity: i64,
    cursor: Option<&'a MailboxCursor>,
    highest: Option<i64>,
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
        self.select_lines(mailbox, secret).map(|_| ())
    }

    fn select_lines(
        &mut self,
        mailbox: &str,
        secret: &str,
    ) -> Result<Vec<String>, MailSubmitError> {
        self.command(&format!("SELECT {}", quote(mailbox)), secret)
    }

    fn pull_headers(
        &mut self,
        secret: &str,
        name: &str,
        role: &str,
        cursors: &[MailboxCursor],
        on: &mut dyn FnMut(PullEvent),
    ) -> Result<(), MailSubmitError> {
        let lines = self.select_for_pull(name, secret)?;
        let total = parse_exists(&lines);
        let role_tick = PullRole::parse(role);
        on(PullEvent::Tick(PullTick {
            role: role_tick,
            read: 0,
            total,
        }));
        let uidvalidity = parse_uidvalidity(&lines)
            .and_then(|value| i64::try_from(value).ok())
            .ok_or_else(|| unread(secret, ""))?;
        let uidnext =
            parse_code_number(&lines, "UIDNEXT").and_then(|value| i64::try_from(value).ok());
        let highest =
            parse_code_number(&lines, "HIGHESTMODSEQ").and_then(|value| i64::try_from(value).ok());
        let cursor = cursors
            .iter()
            .find(|cursor| cursor.name.eq_ignore_ascii_case(name));
        let resync = cursor.is_some_and(|cursor| cursor.uidvalidity != uidvalidity);
        let start = match cursor {
            Some(cursor) if !resync => cursor.last_uid.saturating_add(1).max(1),
            _ => 1,
        };
        let watch = FlagWatch {
            secret,
            name,
            role,
            uidvalidity,
            cursor,
            highest,
        };
        if !resync && uidnext.is_some_and(|next| start >= next) {
            if !self.pull_flag_changes(&watch, on)? {
                emit_batch(on, name, uidvalidity, false, role, Vec::new(), highest);
            }
            return Ok(());
        }
        let searched =
            self.command_wide(&format!("UID SEARCH UID {start}:*"), secret, SEARCH_PREFIX)?;
        let mut uids = parse_search_uids(&searched);
        uids.sort_unstable();
        uids.dedup();
        if uids.is_empty() {
            if resync {
                emit_batch(on, name, uidvalidity, true, role, Vec::new(), highest);
            } else if !self.pull_flag_changes(&watch, on)? {
                emit_batch(on, name, uidvalidity, false, role, Vec::new(), highest);
            }
            return Ok(());
        }
        let mut read = 0_u32;
        let chunks: Vec<&[i64]> = uids.chunks(HEADER_BATCH).collect();
        for (index, chunk) in chunks.iter().enumerate() {
            let fetched = self.fetch(
                &format!(
                    "UID FETCH {} (UID FLAGS RFC822.SIZE BODY.PEEK[HEADER])",
                    uid_set(chunk)
                ),
                secret,
                HEADER_LITERAL,
            )?;
            let mut messages = Vec::new();
            for line in &fetched {
                if let Some(message) = message_from_header(line) {
                    messages.push(message);
                }
            }
            self.complete_reports(secret, &mut messages)?;
            read = read.saturating_add(u32::try_from(messages.len()).unwrap_or(u32::MAX));
            on(PullEvent::Tick(PullTick {
                role: role_tick,
                read,
                total,
            }));
            emit_batch(
                on,
                name,
                uidvalidity,
                resync && index == 0,
                role,
                messages,
                highest.filter(|_| index + 1 == chunks.len()),
            );
        }
        if !resync {
            self.pull_flag_changes(&watch, on)?;
        }
        Ok(())
    }

    fn select_for_pull(
        &mut self,
        mailbox: &str,
        secret: &str,
    ) -> Result<Vec<String>, MailSubmitError> {
        match self.command(&format!("SELECT {} (CONDSTORE)", quote(mailbox)), secret) {
            Ok(lines) => Ok(lines),
            Err(
                error @ (MailSubmitError::Auth | MailSubmitError::Tls | MailSubmitError::Timeout),
            ) => Err(error),
            Err(_) => self.select_lines(mailbox, secret),
        }
    }

    fn pull_flag_changes(
        &mut self,
        watch: &FlagWatch<'_>,
        on: &mut dyn FnMut(PullEvent),
    ) -> Result<bool, MailSubmitError> {
        let Some(previous) = watch.cursor.and_then(|cursor| cursor.modseq) else {
            return Ok(false);
        };
        let Some(highest) = watch.highest else {
            return Ok(false);
        };
        if previous >= highest {
            return Ok(false);
        }
        let fetched = self.fetch(
            &format!("UID FETCH 1:* (UID FLAGS) (CHANGEDSINCE {previous})"),
            watch.secret,
            HEADER_LITERAL,
        )?;
        let messages: Vec<_> = fetched
            .iter()
            .filter_map(|line| message_from_flags(line))
            .collect();
        if messages.is_empty() {
            return Ok(false);
        }
        emit_batch(
            on,
            watch.name,
            watch.uidvalidity,
            false,
            watch.role,
            messages,
            Some(highest),
        );
        Ok(true)
    }

    fn complete_reports(
        &mut self,
        secret: &str,
        messages: &mut [PulledMessage],
    ) -> Result<(), MailSubmitError> {
        for message in messages.iter_mut() {
            if message.body != PulledBody::Pending
                || !message
                    .raw
                    .to_ascii_lowercase()
                    .contains("multipart/report")
            {
                continue;
            }
            let Ok(uid) = u32::try_from(message.uid) else {
                continue;
            };
            let fetched = self.fetch(
                &format!("UID FETCH {uid} (UID FLAGS BODY.PEEK[])"),
                secret,
                body_cap(),
            )?;
            if let Some(line) = fetched
                .iter()
                .find(|line| std::str::from_utf8(line).is_ok_and(|text| text.contains(" FETCH ")))
            {
                apply_full_body(message, line);
            }
        }
        Ok(())
    }

    fn fetch_one_body(
        &mut self,
        secret: &str,
        mailbox: &str,
        uid: u32,
    ) -> Result<PulledMessage, MailSubmitError> {
        self.select(mailbox, secret)?;
        let fetched = self.fetch(
            &format!("UID FETCH {uid} (UID FLAGS RFC822.SIZE BODY.PEEK[])"),
            secret,
            body_cap(),
        )?;
        let line = fetched
            .into_iter()
            .find(|line| std::str::from_utf8(line).is_ok_and(|text| text.contains(" FETCH ")))
            .ok_or_else(|| unread(secret, ""))?;
        let mut message = PulledMessage {
            uid: i64::from(uid),
            seen: false,
            raw: String::new(),
            body: PulledBody::Pending,
            size: None,
        };
        apply_full_body(&mut message, &line);
        if message.body == PulledBody::Pending {
            return Err(unread(secret, ""));
        }
        Ok(message)
    }

    fn fetch(
        &mut self,
        body: &str,
        secret: &str,
        max_literal: usize,
    ) -> Result<Vec<Vec<u8>>, MailSubmitError> {
        self.write_command(body)?;
        self.read_tagged_bytes(secret, max_literal)
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
        self.write_raw(bytes)?;
        self.write_raw(b"\r\n")?;
        self.read_tagged(secret).map(|_| ())
    }

    fn logout(&mut self) {
        let _ = self.write_command("LOGOUT");
    }

    pub(crate) fn still_there(&mut self) -> bool {
        self.command("NOOP", "").is_ok()
    }

    pub(crate) fn close(&mut self) {
        self.logout();
    }

    pub(crate) fn read_folders(
        &mut self,
        endpoint: &ImapEndpoint,
        secret: &MailSecret,
        cursors: &[MailboxCursor],
        on: &mut dyn FnMut(PullEvent),
    ) -> Result<(), MailSubmitError> {
        sync_folders(self, endpoint, secret, cursors, on)
    }

    pub(crate) fn read_one(
        &mut self,
        secret: &str,
        mailbox: &str,
        uid: u32,
    ) -> Result<PulledMessage, MailSubmitError> {
        self.fetch_one_body(secret, mailbox, uid)
    }

    fn command(&mut self, body: &str, secret: &str) -> Result<Vec<String>, MailSubmitError> {
        self.command_wide(body, secret, MAX_PREFIX)
    }

    fn command_wide(
        &mut self,
        body: &str,
        secret: &str,
        max_prefix: usize,
    ) -> Result<Vec<String>, MailSubmitError> {
        self.write_command(body)?;
        self.read_tagged_capped(secret, max_prefix)
    }

    fn write_command(&mut self, body: &str) -> Result<(), MailSubmitError> {
        self.tag += 1;
        let mut line = format!("A{} {body}\r\n", self.tag);
        let result = self.write_raw(line.as_bytes());
        line.zeroize();
        result
    }

    fn write_raw(&mut self, bytes: &[u8]) -> Result<(), MailSubmitError> {
        let stream = self.stream.get_mut();
        stream.write_all(bytes).map_err(io_error)?;
        stream.flush().map_err(io_error)?;
        Ok(())
    }

    fn read_tagged(&mut self, secret: &str) -> Result<Vec<String>, MailSubmitError> {
        self.read_tagged_capped(secret, MAX_PREFIX)
    }

    fn read_tagged_capped(
        &mut self,
        secret: &str,
        max_prefix: usize,
    ) -> Result<Vec<String>, MailSubmitError> {
        let tag = format!("A{}", self.tag);
        let mut lines = Vec::new();
        loop {
            let line = self.read_line_capped(max_prefix)?;
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
        self.read_line_capped(MAX_PREFIX)
    }

    fn read_line_capped(&mut self, max_prefix: usize) -> Result<String, MailSubmitError> {
        let bytes = read_logical_line(&mut self.stream, HEADER_LITERAL, max_prefix)?;
        String::from_utf8(bytes)
            .map_err(|_| MailSubmitError::Other("La copie n'a pas pu être déposée.".into()))
    }

    fn read_tagged_bytes(
        &mut self,
        secret: &str,
        max_literal: usize,
    ) -> Result<Vec<Vec<u8>>, MailSubmitError> {
        let tag = format!("A{}", self.tag);
        let mut lines = Vec::new();
        loop {
            let line = read_logical_line(&mut self.stream, max_literal, MAX_PREFIX)?;
            let text = String::from_utf8_lossy(&line);
            if let Some(rest) = text.strip_prefix(&tag) {
                let rest = rest.trim_start();
                if rest.starts_with("OK") {
                    return Ok(lines);
                }
                return Err(unread(secret, rest));
            }
            if text.starts_with("+ ") || text == "+" {
                return Err(unread(secret, &text));
            }
            lines.push(line);
            if lines.len() > MAX_UNTAGGED {
                return Err(unread(secret, ""));
            }
        }
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

fn unread(secret: &str, line: &str) -> MailSubmitError {
    match classify(line, secret) {
        MailSubmitError::Auth => MailSubmitError::Auth,
        MailSubmitError::Tls => MailSubmitError::Tls,
        MailSubmitError::Timeout => MailSubmitError::Timeout,
        MailSubmitError::Refused(message) => {
            if !secret.is_empty() && message.contains(secret) {
                MailSubmitError::Other("La boîte n'a pas pu être lue.".into())
            } else {
                MailSubmitError::Refused(message)
            }
        }
        MailSubmitError::Other(_) => MailSubmitError::Other("La boîte n'a pas pu être lue.".into()),
    }
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

fn trailing_literal(line: &[u8]) -> Result<Option<usize>, MailSubmitError> {
    if !line.ends_with(b"}") {
        return Ok(None);
    }
    let Some(open) = line.iter().rposition(|byte| *byte == b'{') else {
        return Ok(None);
    };
    if !quotes_even(&line[..open]) {
        return Ok(None);
    }
    let Ok(raw) = std::str::from_utf8(&line[open + 1..line.len() - 1]) else {
        return Ok(None);
    };
    let raw = raw.strip_suffix('+').unwrap_or(raw);
    if raw.is_empty() || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return Ok(None);
    }
    raw.parse::<usize>().map(Some).map_err(|_| unread("", ""))
}

fn read_logical_line(
    reader: &mut impl BufRead,
    max_literal: usize,
    max_prefix: usize,
) -> Result<Vec<u8>, MailSubmitError> {
    let mut buf = Vec::new();
    loop {
        if buf.len() > max_literal.saturating_add(max_prefix) {
            return Err(unread("", ""));
        }
        let segment = buf.len();
        loop {
            let byte = read_one(reader)?;
            buf.push(byte);
            if buf.len() - segment > max_prefix {
                return Err(unread("", ""));
            }
            if buf.ends_with(b"\r\n") {
                buf.truncate(buf.len() - 2);
                break;
            }
        }
        let Some(len) = trailing_literal(&buf)? else {
            return Ok(buf);
        };
        let over = len > max_literal
            || buf.len().saturating_add(len) > max_literal.saturating_add(max_prefix);
        if over {
            discard_exact(reader, len)?;
            buf.extend_from_slice(OVERSIZE_MARK);
            continue;
        }
        let mut rest = vec![0_u8; len];
        reader.read_exact(&mut rest).map_err(io_error)?;
        buf.extend(rest);
    }
}

fn read_one(reader: &mut impl BufRead) -> Result<u8, MailSubmitError> {
    let mut byte = [0_u8; 1];
    reader.read_exact(&mut byte).map_err(io_error)?;
    Ok(byte[0])
}

fn discard_exact(reader: &mut impl BufRead, mut len: usize) -> Result<(), MailSubmitError> {
    let mut chunk = [0_u8; 8 * 1024];
    while len > 0 {
        let take = len.min(chunk.len());
        reader.read_exact(&mut chunk[..take]).map_err(io_error)?;
        len -= take;
    }
    Ok(())
}

fn body_cap() -> usize {
    usize::try_from(BODY_CAP_BYTES).unwrap_or(usize::MAX)
}

fn body_cap_i64() -> i64 {
    i64::try_from(BODY_CAP_BYTES).unwrap_or(i64::MAX)
}

fn uid_set(uids: &[i64]) -> String {
    let mut out = String::new();
    for (index, uid) in uids.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&uid.to_string());
    }
    out
}

fn emit_batch(
    on: &mut dyn FnMut(PullEvent),
    name: &str,
    uidvalidity: i64,
    resync: bool,
    role: &str,
    messages: Vec<PulledMessage>,
    modseq: Option<i64>,
) {
    on(PullEvent::Batch(PulledMailbox {
        name: name.to_string(),
        uidvalidity,
        resync,
        role: role.to_string(),
        messages,
        modseq,
    }));
}

fn message_from_header(line: &[u8]) -> Option<PulledMessage> {
    let text = std::str::from_utf8(line).ok()?;
    if !text.contains(" FETCH ") {
        return None;
    }
    let uid = i64::from(uid_of(text)?);
    let seen = text.contains("\\Seen");
    let size = rfc822_size(text);
    if text.contains("GRF-OVERSIZE") {
        return Some(PulledMessage {
            uid,
            seen,
            raw: String::new(),
            body: PulledBody::Unreadable,
            size,
        });
    }
    let Some(parsed) = parse_fetch_line(line) else {
        return Some(PulledMessage {
            uid,
            seen,
            raw: String::new(),
            body: PulledBody::Unreadable,
            size,
        });
    };
    let too_large = size.is_some_and(|bytes| bytes > body_cap_i64());
    Some(PulledMessage {
        uid: parsed.uid,
        seen: parsed.seen,
        raw: parsed.raw,
        body: if too_large {
            PulledBody::TooLarge
        } else {
            PulledBody::Pending
        },
        size: size.or(parsed.size),
    })
}

fn message_from_flags(line: &[u8]) -> Option<PulledMessage> {
    let text = std::str::from_utf8(line).ok()?;
    if !text.contains(" FETCH ") || text.contains("BODY") {
        return None;
    }
    Some(PulledMessage {
        uid: i64::from(uid_of(text)?),
        seen: text.contains("\\Seen"),
        raw: String::new(),
        body: PulledBody::Flags,
        size: None,
    })
}

fn apply_full_body(message: &mut PulledMessage, line: &[u8]) {
    let text = String::from_utf8_lossy(line);
    message.seen = text.contains("\\Seen");
    message.size = message.size.or_else(|| rfc822_size(&text));
    if text.contains("GRF-OVERSIZE") || message.size.is_some_and(|bytes| bytes > body_cap_i64()) {
        message.body = PulledBody::TooLarge;
        return;
    }
    if let Some(parsed) = parse_fetch_line(line) {
        message.uid = parsed.uid;
        message.seen = parsed.seen;
        message.raw = parsed.raw;
        message.body = PulledBody::Ready;
        message.size = message.size.or(parsed.size);
        return;
    }
    message.body = PulledBody::Unreadable;
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
    use super::{
        choose_sent_mailbox, login_names, parse_exists, parse_fetch_line, parse_list_line,
        parse_uidvalidity, probe_copy,
    };
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

    #[test]
    fn a_select_response_gives_the_uid_validity() {
        let lines = [
            "* FLAGS (\\Seen)".to_string(),
            "* OK [UIDVALIDITY 3857529045] UIDs valid".to_string(),
            "A1 OK [READ-WRITE] SELECT completed".to_string(),
        ];
        assert_eq!(parse_uidvalidity(&lines), Some(3_857_529_045));
        assert_eq!(parse_exists(&lines), None);
    }

    #[test]
    fn a_select_response_gives_exists() {
        let lines = [
            "* 14 EXISTS".to_string(),
            "* 3 RECENT".to_string(),
            "* OK [UIDVALIDITY 3857529045] UIDs valid".to_string(),
            "A1 OK [READ-WRITE] SELECT completed".to_string(),
        ];
        assert_eq!(parse_exists(&lines), Some(14));
        assert_eq!(parse_exists(&["* 0 EXISTS".to_string()]), Some(0));
        assert_eq!(parse_exists(&["A1 OK SELECT".to_string()]), None);
        assert_eq!(parse_uidvalidity(&lines), Some(3_857_529_045));
    }

    #[test]
    fn a_peek_line_keeps_the_body_and_the_seen_flag() {
        let mut line = b"* 1 FETCH (UID 42 FLAGS (\\Seen) BODY[] {10}".to_vec();
        line.extend(b"Bonjour\r\n.");
        line.extend(b")");
        let message = parse_fetch_line(&line).unwrap();
        assert_eq!(message.uid, 42);
        assert!(message.seen);
        assert_eq!(message.raw, "Bonjour\r\n.");
        let fresh = b"* 2 FETCH (UID 7 FLAGS () BODY[] {4}lu ?)";
        let message = parse_fetch_line(fresh).unwrap();
        assert_eq!(message.uid, 7);
        assert!(!message.seen);
        assert_eq!(message.raw, "lu ?");
    }

    #[test]
    fn an_oversized_literal_does_not_swallow_the_next_letter() {
        let mut raw = b"* 1 FETCH (UID 1 BODY[HEADER] {4}\r\n".to_vec();
        raw.extend(b"H: a)\r\n");
        raw.extend(b"* 2 FETCH (UID 2 BODY[HEADER] {24}\r\n");
        raw.extend(std::iter::repeat_n(b'X', 24));
        raw.extend(b")\r\n");
        raw.extend(b"* 3 FETCH (UID 3 BODY[HEADER] {4}\r\n");
        raw.extend(b"H: c)\r\n");
        let mut cursor = std::io::Cursor::new(raw);
        let first = super::read_logical_line(&mut cursor, 8, 256).unwrap();
        let second = super::read_logical_line(&mut cursor, 8, 256).unwrap();
        let third = super::read_logical_line(&mut cursor, 8, 256).unwrap();
        let one = super::message_from_header(&first).unwrap();
        let two = super::message_from_header(&second).unwrap();
        let three = super::message_from_header(&third).unwrap();
        assert_eq!(one.uid, 1);
        assert_eq!(one.body, griffe_core::mail::PulledBody::Pending);
        assert_eq!(one.raw, "H: a");
        assert_eq!(two.uid, 2);
        assert_eq!(two.body, griffe_core::mail::PulledBody::Unreadable);
        assert!(!String::from_utf8_lossy(&second).contains("XXXX"));
        assert_eq!(three.uid, 3);
        assert_eq!(three.raw, "H: c");
    }

    #[test]
    fn a_header_heavier_than_the_cap_stays_a_header() {
        let size = griffe_core::mail::BODY_CAP_BYTES + 1;
        let mut line =
            format!("* 1 FETCH (UID 5 FLAGS () RFC822.SIZE {size} BODY[HEADER] {{5}}").into_bytes();
        line.extend(b"Hello)");
        let message = super::message_from_header(&line).unwrap();
        assert_eq!(message.body, griffe_core::mail::PulledBody::TooLarge);
        assert_eq!(message.raw, "Hello");
        assert_eq!(message.uid, 5);
    }

    #[test]
    fn uidnext_and_search_are_read_from_the_server_lines() {
        let lines = [
            "* OK [UIDNEXT 42] Predicted next UID".to_string(),
            "* OK [HIGHESTMODSEQ 7] Highest".to_string(),
            "* SEARCH 3 8 12".to_string(),
            "A1 OK SEARCH done".to_string(),
        ];
        assert_eq!(super::parse_code_number(&lines, "UIDNEXT"), Some(42));
        assert_eq!(super::parse_code_number(&lines, "HIGHESTMODSEQ"), Some(7));
        assert_eq!(super::parse_search_uids(&lines), vec![3, 8, 12]);
    }
}
