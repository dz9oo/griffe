//! Range des messages déjà lus. Aucune socket : l'adaptateur a rapporté les octets.

use rusqlite::OptionalExtension;

use super::commands::{AddressFault, FileMessage, FilingSource, HoldReply, MarkAddress};
use super::inbound::{DossierLink, FicheAddress, KnownLetter, Notice, classify, read_message};
use super::lecture::PullTick;
use super::model::ImapEndpoint;
use super::secret::MailSecret;
use super::{probe_material, store};
use crate::app::{AppError, ExecutionContext, Executor, Outcome};
use crate::mailbox::{Mailbox, MailboxCursor, MirrorInsert, MirrorMessage};

/// Écart entre deux relèves de la fenêtre, en secondes. Défaut : trois minutes.
pub const RELEVE_EVERY_SECS: u64 = 180;

/// Périodes acceptées : une minute, trois, quinze, trente.
pub const RELEVE_CHOICES_SECS: [u64; 4] = [60, 180, 900, 1_800];

/// Corps gardé dans le miroir, en octets. Au-delà, les en-têtes restent.
pub const BODY_CAP_BYTES: u64 = 15 * 1024 * 1024;

/// Corps récents demandés après l'index des en-têtes.
pub const RECENT_BODIES: u32 = 40;

/// En-têtes demandés ensemble.
pub const HEADER_BATCH: usize = 100;

/// Ce que la liaison a déjà du corps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PulledBody {
    /// En-têtes seuls. Le corps viendra.
    Pending,
    /// `raw` est le message entier.
    Ready,
    /// Au-delà de [`BODY_CAP_BYTES`]. `raw` est les en-têtes.
    TooLarge,
    /// En-têtes illisibles.
    Unreadable,
    /// Drapeaux seulement. La ligne existe déjà.
    Flags,
}

impl PulledBody {
    /// Valeur rangée dans le miroir.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Ready | Self::Flags => "ready",
            Self::TooLarge => "too_large",
            Self::Unreadable => "unreadable",
        }
    }
}

/// Un dossier du serveur, tel que la liaison l'a rapporté.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PulledMailbox {
    pub name: String,
    pub uidvalidity: i64,
    pub resync: bool,
    /// `inbox`, `sent` ou `other`.
    pub role: String,
    pub messages: Vec<PulledMessage>,
    /// `HIGHESTMODSEQ` lu au `SELECT`. Absent si le serveur ne le dit pas.
    pub modseq: Option<i64>,
}

/// Un message rapporté, pas encore rangé.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PulledMessage {
    pub uid: i64,
    pub seen: bool,
    pub raw: String,
    pub body: PulledBody,
    /// `RFC822.SIZE`, quand le serveur l'a donné.
    pub size: Option<i64>,
}

impl PulledMessage {
    /// Message entier, tel que les tests et l'ancien lecteur le posaient.
    #[must_use]
    pub fn full(uid: i64, seen: bool, raw: impl Into<String>) -> Self {
        Self {
            uid,
            seen,
            raw: raw.into(),
            body: PulledBody::Ready,
            size: None,
        }
    }
}

/// Annonce de la liaison, ou un lot à ranger tout de suite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PullEvent {
    /// `EXISTS`, puis chaque message analysé.
    Tick(PullTick),
    /// En-têtes, corps, ou drapeaux d'un dossier.
    Batch(PulledMailbox),
}

/// Ce qu'il faut pour ouvrir la liaison. Le secret s'efface avec cette valeur.
pub struct RelevePlan {
    pub endpoint: ImapEndpoint,
    pub secret: MailSecret,
    pub cursors: Vec<MailboxCursor>,
    /// Adresse du compte. Pas un secret.
    pub address: String,
}

/// Compte du passage. Les corps n'y figurent pas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
pub struct AbsorbReport {
    pub stored: u32,
    pub held: u32,
    pub failed: u32,
    pub choices: u32,
    /// Lettres posées sur un dossier pendant ce passage.
    pub on_dossier: u32,
}

/// Ligne montrée par `griffe boite`, sans le corps.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct BoxLine {
    pub id: String,
    pub view: String,
    pub from_name: Option<String>,
    pub from_address: Option<String>,
    pub subject: String,
    pub dossier: Option<String>,
    pub notice: Option<String>,
    pub seen: bool,
    pub choice: bool,
}

/// Phrase de la période. Les quatre valeurs, rien d'autre.
#[must_use]
pub fn releve_cadence(seconds: u64) -> &'static str {
    match seconds {
        60 => "Toutes les minutes.",
        900 => "Toutes les 15 minutes.",
        1_800 => "Toutes les 30 minutes.",
        _ => "Toutes les 3 minutes.",
    }
}

/// Heure locale d'une relève aboutie, « 14 h 32 ».
#[must_use]
pub fn releve_clock(at: &str) -> Option<String> {
    let parsed =
        time::OffsetDateTime::parse(at, &time::format_description::well_known::Rfc3339).ok()?;
    let local = time::UtcOffset::local_offset_at(parsed)
        .ok()
        .map_or(parsed, |offset| parsed.to_offset(offset));
    Some(format!("{} h {:02}", local.hour(), local.minute()))
}

/// Mot des quatre périodes.
#[must_use]
pub fn releve_word(seconds: u64) -> &'static str {
    match seconds {
        60 => "1 minute",
        900 => "15 minutes",
        1_800 => "30 minutes",
        _ => "3 minutes",
    }
}

/// Période retenue, ou trois minutes si le compte n'a rien choisi de valide.
///
/// # Errors
///
/// Lecture impossible.
pub fn releve_every(vault: &crate::store::Store) -> Result<u64, AppError> {
    let raw = store::releve_every_raw(vault.connection())?;
    Ok(raw
        .and_then(|seconds| u64::try_from(seconds).ok())
        .filter(|seconds| RELEVE_CHOICES_SECS.contains(seconds))
        .unwrap_or(RELEVE_EVERY_SECS))
}

/// Heure d'une relève aboutie. Efface la phrase d'échec.
///
/// # Errors
///
/// Le miroir ne s'écrit pas.
pub fn note_releve_success(vault: &crate::store::Store, at: &str) -> Result<(), AppError> {
    vault.open_mailbox()?.note_releve(Some(at), None)
}

/// Phrase d'un échec. L'heure de la dernière réussite reste.
///
/// # Errors
///
/// Le miroir ne s'écrit pas.
pub fn note_releve_failure(vault: &crate::store::Store, error: &str) -> Result<(), AppError> {
    let error = error.trim();
    if error.is_empty() {
        return Ok(());
    }
    vault.open_mailbox()?.note_releve(None, Some(error))
}

/// Prépare la liaison. `None` : le compte ne permet pas de lire.
///
/// # Errors
///
/// Lecture du coffre ou du miroir impossible.
pub fn releve_plan(vault: &crate::store::Store) -> Result<Option<RelevePlan>, AppError> {
    let Some(material) = probe_material(vault.connection())? else {
        return Ok(None);
    };
    let Some(endpoint) = material.copy else {
        return Ok(None);
    };
    let mailbox = vault.open_mailbox()?;
    let cursors = mailbox.cursors()?;
    Ok(Some(RelevePlan {
        endpoint,
        secret: material.secret,
        cursors,
        address: material.address,
    }))
}

/// Range le lot. Une ligne humaine déjà posée n'est pas reprise.
///
/// # Errors
///
/// Écriture impossible.
pub fn absorb(
    vault: &mut crate::store::Store,
    mailbox: &mut Mailbox,
    pulled: &[PulledMailbox],
    ctx: &ExecutionContext,
) -> Result<AbsorbReport, AppError> {
    if ctx.dry_run {
        return Ok(AbsorbReport::default());
    }
    let known = known_letters(vault.connection())?;
    let fiches = fiche_addresses(vault.connection())?;
    let own = own_addresses(vault.connection())?;
    let open = open_dossiers(vault.connection())?;
    let books = Books {
        known: &known,
        fiches: &fiches,
        own: &own,
        open: &open,
    };
    let mut report = AbsorbReport::default();
    for folder in pulled {
        absorb_folder(vault, mailbox, folder, ctx, &books, &mut report)?;
    }
    Ok(report)
}

struct Books<'a> {
    known: &'a [KnownLetter],
    fiches: &'a [FicheAddress],
    own: &'a [String],
    open: &'a [OpenDossier],
}

fn absorb_folder(
    vault: &mut crate::store::Store,
    mailbox: &mut Mailbox,
    folder: &PulledMailbox,
    ctx: &ExecutionContext,
    books: &Books<'_>,
    report: &mut AbsorbReport,
) -> Result<(), AppError> {
    if folder.resync {
        mailbox.clear_mailbox(&folder.name)?;
    }
    let mut last_uid = 0_i64;
    for message in &folder.messages {
        last_uid = last_uid.max(message.uid);
        if mailbox.has_uid(&folder.name, folder.uidvalidity, message.uid)? {
            if message.body == PulledBody::Flags {
                mailbox.set_seen_uid(
                    &folder.name,
                    folder.uidvalidity,
                    message.uid,
                    message.seen,
                )?;
            } else if message.body != PulledBody::Pending {
                fill_if_pending(vault, mailbox, folder, message, ctx, books, report)?;
            }
            continue;
        }
        if message.body == PulledBody::Flags {
            continue;
        }
        let placed = place(vault, mailbox, folder, message, ctx, books)?;
        report.stored += 1;
        report.held += u32::from(placed.held);
        report.failed += u32::from(placed.failed);
        report.choices += u32::from(placed.choice);
        report.on_dossier += u32::from(placed.on_dossier);
    }
    mailbox.remember(&folder.name, folder.uidvalidity, last_uid, folder.modseq)?;
    Ok(())
}

fn fill_if_pending(
    vault: &mut crate::store::Store,
    mailbox: &mut Mailbox,
    folder: &PulledMailbox,
    message: &PulledMessage,
    ctx: &ExecutionContext,
    books: &Books<'_>,
    report: &mut AbsorbReport,
) -> Result<(), AppError> {
    let Some(state) = mailbox.body_state_of(&folder.name, folder.uidvalidity, message.uid)? else {
        return Ok(());
    };
    if state != PulledBody::Pending.as_str() {
        return Ok(());
    }
    let parsed = read_message(&message.raw);
    let own = own_refs(books.own);
    let filing = classify(&parsed, books.known, books.fiches, &own);
    let (link, _, _, _) = resolve(&filing.link, filing.hold, books.open);
    let failed = mark_notice(
        vault,
        ctx,
        parsed.message_id.as_ref(),
        &link,
        filing.notice.as_ref(),
    )?;
    report.failed += u32::from(failed);
    mailbox.fill_body(
        &folder.name,
        folder.uidvalidity,
        message.uid,
        &parsed.text,
        parsed.html.as_deref(),
        message.body.as_str(),
        notice_label(filing.notice.as_ref()).as_deref(),
        notice_address(filing.notice.as_ref()).as_deref(),
    )?;
    Ok(())
}

#[allow(clippy::struct_excessive_bools)]
struct Placed {
    held: bool,
    failed: bool,
    choice: bool,
    on_dossier: bool,
}

struct OpenDossier {
    id: String,
    client_id: String,
}

fn place(
    vault: &mut crate::store::Store,
    mailbox: &mut Mailbox,
    folder: &PulledMailbox,
    message: &PulledMessage,
    ctx: &ExecutionContext,
    books: &Books<'_>,
) -> Result<Placed, AppError> {
    let parsed = read_message(&message.raw);
    let defer_notice = message.body == PulledBody::Pending && parsed.delivery_report;
    let already = match parsed.message_id.as_deref() {
        Some(id) => store::filing_source(vault.connection(), id)?.is_some(),
        None => false,
    };
    let ours = parsed.message_id.as_deref().is_some_and(|id| {
        books
            .known
            .iter()
            .any(|letter| same_id(&letter.message_id, id))
    });
    let own = own_refs(books.own);
    let filing = classify(&parsed, books.known, books.fiches, &own);
    let (link, hold, choice_of, choice_ids) = resolve(&filing.link, filing.hold, books.open);
    let mut placed = Placed {
        held: false,
        failed: false,
        choice: choice_ids.is_some(),
        on_dossier: false,
    };
    if !already && !ours {
        placed.held = file_link(vault, ctx, parsed.message_id.as_ref(), &link, hold)?;
        placed.on_dossier = parsed.message_id.is_some()
            && matches!(
                link,
                DossierLink::One {
                    opportunity_id: Some(_),
                    ..
                }
            );
        placed.failed = !defer_notice
            && mark_notice(
                vault,
                ctx,
                parsed.message_id.as_ref(),
                &link,
                filing.notice.as_ref(),
            )?;
    }
    mailbox.insert(&MirrorInsert {
        mailbox: folder.name.clone(),
        uid: message.uid,
        uidvalidity: folder.uidvalidity,
        message_id: parsed.message_id,
        from_name: parsed.from_name,
        from_address: parsed.from_address,
        subject: if parsed.subject.is_empty() {
            "Sans objet".to_string()
        } else {
            parsed.subject
        },
        sent_on: parsed.sent_on,
        text: parsed.text,
        html: parsed.html,
        seen: message.seen,
        notice: if defer_notice {
            None
        } else {
            notice_label(filing.notice.as_ref())
        },
        notice_address: if defer_notice {
            None
        } else {
            notice_address(filing.notice.as_ref())
        },
        folder_role: folder.role.clone(),
        choice_of,
        choice_ids,
        body_state: message.body.as_str().to_string(),
    })?;
    Ok(placed)
}

fn file_link(
    vault: &mut crate::store::Store,
    ctx: &ExecutionContext,
    message_id: Option<&String>,
    link: &DossierLink,
    hold: bool,
) -> Result<bool, AppError> {
    let DossierLink::One {
        client_id,
        opportunity_id,
    } = link
    else {
        return Ok(false);
    };
    let Some(message_id) = message_id.cloned() else {
        return Ok(false);
    };
    let mut retained = false;
    if hold && let Some(opportunity_id) = opportunity_id.clone() {
        let outcome = Executor::new(vault).execute(&HoldReply { opportunity_id }, ctx)?;
        retained = matches!(outcome, Outcome::Applied(()));
    }
    Executor::new(vault).execute(
        &FileMessage {
            message_id,
            client_id: client_id.clone(),
            opportunity_id: opportunity_id.clone(),
            source: FilingSource::Auto,
        },
        ctx,
    )?;
    Ok(retained)
}

fn mark_notice(
    vault: &mut crate::store::Store,
    ctx: &ExecutionContext,
    message_id: Option<&String>,
    link: &DossierLink,
    notice: Option<&Notice>,
) -> Result<bool, AppError> {
    let DossierLink::One { client_id, .. } = link else {
        return Ok(false);
    };
    let (address, status, failed) = match notice {
        Some(Notice::Certain { address }) => (address.clone(), AddressFault::Failed, true),
        Some(Notice::Probable) => {
            let Some(address) = switchboard(vault.connection(), client_id)? else {
                return Ok(false);
            };
            (address, AddressFault::Probable, false)
        }
        None => return Ok(false),
    };
    Executor::new(vault).execute(
        &MarkAddress {
            client_id: client_id.clone(),
            address,
            status,
            message_id: message_id.cloned(),
        },
        ctx,
    )?;
    Ok(failed)
}

fn switchboard(conn: &rusqlite::Connection, client_id: &str) -> Result<Option<String>, AppError> {
    conn.query_row(
        "SELECT switchboard_email FROM clients WHERE id = ?1",
        [client_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(AppError::from)
}

fn resolve(
    link: &DossierLink,
    hold: bool,
    open: &[OpenDossier],
) -> (DossierLink, bool, Option<String>, Option<String>) {
    match link {
        DossierLink::One {
            client_id,
            opportunity_id: None,
        } => {
            let dossiers: Vec<_> = open
                .iter()
                .filter(|dossier| dossier.client_id == *client_id)
                .map(|dossier| dossier.id.clone())
                .collect();
            match dossiers.as_slice() {
                [one] => (
                    DossierLink::One {
                        client_id: client_id.clone(),
                        opportunity_id: Some(one.clone()),
                    },
                    hold,
                    None,
                    None,
                ),
                [] => (
                    DossierLink::One {
                        client_id: client_id.clone(),
                        opportunity_id: None,
                    },
                    false,
                    None,
                    None,
                ),
                many => (
                    DossierLink::Choice {
                        client_ids: vec![client_id.clone()],
                    },
                    false,
                    Some("dossier".to_string()),
                    Some(serde_json::to_string(many).unwrap_or_else(|_| "[]".to_string())),
                ),
            }
        }
        DossierLink::Choice { client_ids } => (
            link.clone(),
            false,
            Some("fiche".to_string()),
            Some(serde_json::to_string(client_ids).unwrap_or_else(|_| "[]".to_string())),
        ),
        DossierLink::One { .. } | DossierLink::Unlinked => (link.clone(), hold, None, None),
    }
}

fn notice_label(notice: Option<&Notice>) -> Option<String> {
    match notice {
        Some(Notice::Certain { .. }) => Some("certain".to_string()),
        Some(Notice::Probable) => Some("probable".to_string()),
        None => None,
    }
}

fn notice_address(notice: Option<&Notice>) -> Option<String> {
    match notice {
        Some(Notice::Certain { address }) => Some(address.clone()),
        Some(Notice::Probable) | None => None,
    }
}

fn same_id(stored: &str, parsed: &str) -> bool {
    bare(stored) == bare(parsed)
}

fn bare(value: &str) -> String {
    value
        .trim()
        .trim_matches(|ch| ch == '<' || ch == '>')
        .to_ascii_lowercase()
}

fn own_refs(own: &[String]) -> Vec<&str> {
    own.iter().map(String::as_str).collect()
}

fn known_letters(conn: &rusqlite::Connection) -> Result<Vec<KnownLetter>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT message_id, client_id, anchor FROM outbound_mail
         WHERE status = 'sent' AND client_id IS NOT NULL",
    )?;
    let rows = stmt.query_map([], |row| {
        let anchor: Option<String> = row.get(2)?;
        Ok(KnownLetter {
            message_id: row.get(0)?,
            client_id: row.get(1)?,
            opportunity_id: anchor
                .and_then(|value| value.strip_prefix("opportunity:").map(ToString::to_string)),
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
}

fn fiche_addresses(conn: &rusqlite::Connection) -> Result<Vec<FicheAddress>, AppError> {
    let mut found = Vec::new();
    let mut boards = conn
        .prepare("SELECT id, switchboard_email FROM clients WHERE switchboard_email IS NOT NULL")?;
    let boards = boards.query_map([], |row| {
        Ok(FicheAddress {
            client_id: row.get(0)?,
            address: row.get(1)?,
        })
    })?;
    for row in boards {
        found.push(row?);
    }
    let mut people =
        conn.prepare("SELECT client_id, email FROM contacts WHERE email IS NOT NULL")?;
    let people = people.query_map([], |row| {
        Ok(FicheAddress {
            client_id: row.get(0)?,
            address: row.get(1)?,
        })
    })?;
    for row in people {
        found.push(row?);
    }
    Ok(found)
}

fn own_addresses(conn: &rusqlite::Connection) -> Result<Vec<String>, AppError> {
    let address: Option<Option<String>> = conn
        .query_row(
            "SELECT from_address FROM mail_account WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    Ok(address.flatten().into_iter().collect())
}

fn open_dossiers(conn: &rusqlite::Connection) -> Result<Vec<OpenDossier>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, client_id FROM opportunities
         WHERE archived_at IS NULL AND stage NOT IN ('won', 'lost')",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(OpenDossier {
            id: row.get(0)?,
            client_id: row.get(1)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
}

/// Liste le miroir. Le corps reste dans le fichier.
///
/// # Errors
///
/// Lecture impossible.
pub fn list_box(vault: &crate::store::Store, mailbox: &Mailbox) -> Result<Vec<BoxLine>, AppError> {
    let filings = filings_of(vault.connection())?;
    let mut lines = Vec::new();
    for message in mailbox.list()? {
        lines.push(line_of(&message, &filings));
    }
    Ok(lines)
}

struct FilingRow {
    client_id: String,
    opportunity_id: Option<String>,
}

fn filings_of(
    conn: &rusqlite::Connection,
) -> Result<std::collections::HashMap<String, FilingRow>, AppError> {
    let mut stmt = conn.prepare("SELECT message_id, client_id, opportunity_id FROM mail_filing")?;
    let rows = stmt.query_map([], |row| {
        Ok((
            bare(&row.get::<_, String>(0)?),
            FilingRow {
                client_id: row.get(1)?,
                opportunity_id: row.get(2)?,
            },
        ))
    })?;
    let mut map = std::collections::HashMap::new();
    for row in rows {
        let (id, filing) = row?;
        map.insert(id, filing);
    }
    Ok(map)
}

fn line_of(
    message: &MirrorMessage,
    filings: &std::collections::HashMap<String, FilingRow>,
) -> BoxLine {
    let filing = message
        .message_id
        .as_deref()
        .and_then(|id| filings.get(&bare(id)));
    let choice = message.choice_ids.is_some();
    let dossier = filing
        .and_then(|row| row.opportunity_id.clone())
        .or_else(|| filing.map(|row| row.client_id.clone()));
    BoxLine {
        view: view_of(message, filing.is_some()).to_string(),
        id: message.id.clone(),
        from_name: message.from_name.clone(),
        from_address: message.from_address.clone(),
        subject: message.subject.clone(),
        dossier,
        notice: message.notice.clone(),
        seen: message.seen,
        choice,
    }
}

fn view_of(message: &MirrorMessage, filed: bool) -> &'static str {
    if message.notice.as_deref() == Some("certain") {
        return "En échec";
    }
    if message.folder_role == "sent" {
        return "Envoyées";
    }
    if !message.seen {
        return "À lire";
    }
    if filed {
        return "Avec les dossiers";
    }
    "Tout"
}

/// Vrai quand la fenêtre doit relire. `last` absent : à l'ouverture.
#[must_use]
pub fn releve_due(last: Option<time::OffsetDateTime>, now: time::OffsetDateTime) -> bool {
    match last {
        None => true,
        Some(last) => {
            (now - last).whole_seconds() >= i64::try_from(RELEVE_EVERY_SECS).unwrap_or(i64::MAX)
        }
    }
}

#[cfg(test)]
mod tests {
    use time::OffsetDateTime;

    use super::{
        PulledBody, PulledMailbox, PulledMessage, absorb, list_box, note_releve_failure,
        note_releve_success, releve_due, releve_every,
    };
    use crate::app::{Actor, ExecutionContext, Executor, recent_audit_entries};
    use crate::mail::SetReleveInterval;
    use crate::store::testing::test_store;

    const REPLY: &str = "\
From: Camille <camille@riviere.test>\n\
To: nicolas@lumen.test\n\
Subject: Re: Votre proposition\n\
Message-ID: <camille-1@riviere.test>\n\
In-Reply-To: <abc@griffe.local>\n\
Date: Sat, 5 Sep 2026 09:00:00 +0200\n\
\n\
PHRASE-SECRETE-REPONSE\n";

    const OURS: &str = "\
From: Nicolas <nicolas@lumen.test>\n\
To: camille@riviere.test\n\
Subject: Votre proposition\n\
Message-ID: <abc@griffe.local>\n\
\n\
La lettre partie.\n";

    const DSN: &str = "\
From: Mail Delivery System <MAILER-DAEMON@mx.example>\n\
Subject: Undelivered Mail Returned to Sender\n\
Message-ID: <bounce@mx.example>\n\
Content-Type: multipart/report; report-type=delivery-status; boundary=\"bound\"\n\
\n\
--bound\n\
Content-Type: text/plain; charset=utf-8\n\
\n\
PHRASE-SECRETE-AVIS\n\
--bound\n\
Content-Type: message/delivery-status\n\
\n\
Final-Recipient: rfc822; contact@atelier-du-mail.fr\n\
Action: failed\n\
Status: 5.1.1\n\
\n\
--bound\n\
Content-Type: message/rfc822\n\
\n\
Message-ID: <dead@griffe.local>\n\
\n\
--bound--\n";

    fn system() -> ExecutionContext {
        ExecutionContext::new(Actor::System, false)
    }

    fn seed(store: &mut crate::store::Store) {
        store
            .connection()
            .execute_batch(
                "INSERT INTO clients (id, name, created_at, switchboard_email) VALUES
                    ('c1', 'Camille', '2026-09-01T00:00:00Z', 'camille@riviere.test'),
                    ('c2', 'Atelier', '2026-09-01T00:00:00Z', 'contact@atelier-du-mail.fr');
                 INSERT INTO opportunities (
                    id, client_id, name, stage, amount_cents, probability_percent,
                    next_action_at, created_at
                 ) VALUES
                    ('o1', 'c1', 'Site', 'discovery', 0, 40, '2026-09-08', '2026-09-01T00:00:00Z'),
                    ('o2', 'c2', 'Enseigne', 'discovery', 0, 40, '2026-09-08', '2026-09-01T00:00:00Z');
                 INSERT INTO outbound_mail (
                    id, kind, status, from_address, to_address, subject, body, message_id,
                    send_after, created_at, updated_at, client_id, anchor
                 ) VALUES (
                    'm1', 'letter', 'sent', 'nicolas@lumen.test', 'camille@riviere.test',
                    'Votre proposition', 'La lettre.', '<abc@griffe.local>',
                    '2026-09-02T00:00:00Z', '2026-09-02T00:00:00Z', '2026-09-02T00:00:00Z',
                    'c1', 'opportunity:o1'
                 ), (
                    'm2', 'letter', 'sent', 'nicolas@lumen.test', 'contact@atelier-du-mail.fr',
                    'Devis', 'La lettre.', '<dead@griffe.local>',
                    '2026-09-02T00:00:00Z', '2026-09-02T00:00:00Z', '2026-09-02T00:00:00Z',
                    'c2', 'opportunity:o2'
                 );",
            )
            .unwrap();
    }

    fn folder(uidvalidity: i64, resync: bool, messages: Vec<PulledMessage>) -> PulledMailbox {
        PulledMailbox {
            name: "INBOX".to_string(),
            uidvalidity,
            resync,
            role: "inbox".to_string(),
            messages,
            modseq: None,
        }
    }

    fn pulled(uid: i64, raw: &str) -> PulledMessage {
        PulledMessage::full(uid, false, raw)
    }

    #[test]
    fn a_reply_is_held_once_and_its_body_stays_out_of_the_audit() {
        let mut vault = test_store("releve-hold");
        seed(&mut vault);
        let mut mailbox = vault.open_mailbox().unwrap();
        let batch = [folder(7, false, vec![pulled(4, REPLY), pulled(5, OURS)])];
        let report = absorb(&mut vault, &mut mailbox, &batch, &system()).unwrap();
        assert_eq!(report.stored, 2);
        assert_eq!(report.held, 1);
        assert_eq!(report.on_dossier, 1);
        assert_eq!(mailbox.count().unwrap(), 2);
        assert_eq!(mailbox.recent_ids(report.stored).unwrap().len(), 2);
        let again = absorb(&mut vault, &mut mailbox, &batch, &system()).unwrap();
        assert_eq!(again.stored, 0);
        assert_eq!(again.held, 0);
        assert_eq!(again.on_dossier, 0);
        let hold: i64 = vault
            .connection()
            .query_row(
                "SELECT reply_hold FROM opportunities WHERE id = 'o1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(hold, 1);
        let entries = recent_audit_entries(vault.connection(), 0, 40).unwrap();
        let holds = entries
            .iter()
            .filter(|entry| entry.command_name == "mail.hold_reply")
            .count();
        assert_eq!(holds, 1);
        assert!(entries.iter().all(|entry| {
            !entry.command_json.contains("PHRASE-SECRETE-REPONSE")
                && !entry.command_json.contains("La lettre partie")
        }));
        let lines = list_box(&vault, &mailbox).unwrap();
        assert!(
            lines
                .iter()
                .any(|line| line.view == "À lire" && line.dossier.as_deref() == Some("o1"))
        );
        assert!(
            lines
                .iter()
                .all(|line| line.subject != "PHRASE-SECRETE-REPONSE")
        );
    }

    #[test]
    fn a_certain_notice_marks_the_address_without_the_body() {
        let mut vault = test_store("releve-dsn");
        seed(&mut vault);
        let mut mailbox = vault.open_mailbox().unwrap();
        let batch = folder(3, false, vec![pulled(9, DSN)]);
        let report = absorb(&mut vault, &mut mailbox, &[batch], &system()).unwrap();
        assert_eq!(report.failed, 1);
        assert_eq!(report.held, 0);
        let status: String = vault
            .connection()
            .query_row(
                "SELECT status FROM address_fault WHERE client_id = 'c2'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(status, "failed");
        let entries = recent_audit_entries(vault.connection(), 0, 20).unwrap();
        let marked = entries
            .iter()
            .find(|entry| entry.command_name == "mail.mark_address")
            .expect("marque");
        assert!(marked.command_json.contains("contact@atelier-du-mail.fr"));
        assert!(!marked.command_json.contains("PHRASE-SECRETE-AVIS"));
        let lines = list_box(&vault, &mailbox).unwrap();
        assert_eq!(lines[0].view, "En échec");
    }

    #[test]
    fn a_new_uidvalidity_replaces_the_folder_and_does_not_hold_again() {
        let mut vault = test_store("releve-uid");
        seed(&mut vault);
        let mut mailbox = vault.open_mailbox().unwrap();
        let first = folder(1, false, vec![pulled(4, REPLY)]);
        absorb(&mut vault, &mut mailbox, &[first], &system()).unwrap();
        let second = folder(2, true, vec![pulled(4, REPLY)]);
        let report = absorb(&mut vault, &mut mailbox, &[second], &system()).unwrap();
        assert_eq!(report.stored, 1);
        assert_eq!(report.held, 0);
        assert_eq!(mailbox.list().unwrap().len(), 1);
        let holds = recent_audit_entries(vault.connection(), 0, 40)
            .unwrap()
            .iter()
            .filter(|entry| entry.command_name == "mail.hold_reply")
            .count();
        assert_eq!(holds, 1);
    }

    #[test]
    fn a_backup_carries_the_mirror() {
        let mut vault = test_store("releve-backup");
        seed(&mut vault);
        let mut mailbox = vault.open_mailbox().unwrap();
        absorb(
            &mut vault,
            &mut mailbox,
            &[folder(1, false, vec![pulled(4, REPLY)])],
            &system(),
        )
        .unwrap();
        drop(mailbox);
        let backup = vault.db_path().with_file_name("saved.db");
        vault.backup_to(&backup).unwrap();
        let restored = crate::store::Store::restore_from(
            &backup,
            &vault.db_path().with_file_name("restored.db"),
            &crate::store::Passphrase::from("s3cret"),
        )
        .unwrap();
        let mailbox = restored.open_mailbox().unwrap();
        assert_eq!(mailbox.list().unwrap().len(), 1);
        let lines = list_box(&restored, &mailbox).unwrap();
        assert_eq!(lines[0].subject, "Re: Votre proposition");
    }

    #[test]
    fn headers_are_stored_before_the_body_and_a_report_waits() {
        let mut vault = test_store("releve-headers");
        seed(&mut vault);
        let mut mailbox = vault.open_mailbox().unwrap();
        let reply_headers = format!("{}\n\n", REPLY.split("\n\n").next().unwrap_or(REPLY));
        let other = "\
From: Ada <ada@example.test>\n\
Subject: Bonjour\n\
Message-ID: <ada@example.test>\n\
\n";
        let report_headers = format!("{}\n\n", DSN.split("\n\n").next().unwrap_or(DSN));
        let batch = folder(
            7,
            false,
            vec![
                PulledMessage {
                    uid: 1,
                    seen: false,
                    raw: reply_headers,
                    body: PulledBody::Pending,
                    size: Some(2_000_000),
                },
                PulledMessage {
                    uid: 2,
                    seen: true,
                    raw: other.to_string(),
                    body: PulledBody::Pending,
                    size: Some(400),
                },
                PulledMessage {
                    uid: 3,
                    seen: false,
                    raw: report_headers,
                    body: PulledBody::Pending,
                    size: Some(800),
                },
            ],
        );
        let report = absorb(&mut vault, &mut mailbox, &[batch], &system()).unwrap();
        assert_eq!(report.stored, 3);
        assert_eq!(report.held, 1);
        assert_eq!(report.on_dossier, 1);
        assert_eq!(report.failed, 0);
        assert_eq!(
            mailbox.body_state_of("INBOX", 7, 1).unwrap().as_deref(),
            Some("pending")
        );
        let filled = absorb(
            &mut vault,
            &mut mailbox,
            &[folder(7, false, vec![PulledMessage::full(3, false, DSN)])],
            &system(),
        )
        .unwrap();
        assert_eq!(filled.stored, 0);
        assert_eq!(filled.failed, 1);
        assert_eq!(
            mailbox.body_state_of("INBOX", 7, 3).unwrap().as_deref(),
            Some("ready")
        );
    }

    #[test]
    fn the_interval_accepts_four_values_and_the_stamp_keeps_the_hour() {
        let mut vault = test_store("releve-interval");
        let human = ExecutionContext::new(Actor::Human, false);
        for seconds in [60_u64, 180, 900, 1_800] {
            Executor::new(&mut vault)
                .execute(&SetReleveInterval { seconds }, &human)
                .unwrap();
            assert_eq!(releve_every(&vault).unwrap(), seconds);
        }
        let refused = Executor::new(&mut vault)
            .execute(&SetReleveInterval { seconds: 2 }, &human)
            .unwrap_err();
        assert!(refused.to_string().contains("1, 3, 15 ou 30"), "{refused}");
        assert_eq!(releve_every(&vault).unwrap(), 1_800);
        note_releve_success(&vault, "2026-10-22T12:32:00Z").unwrap();
        note_releve_failure(&vault, "Le serveur n'a pas répondu.").unwrap();
        let mailbox = vault.open_mailbox().unwrap();
        let (at, error) = mailbox.releve_stamp().unwrap();
        assert_eq!(at.as_deref(), Some("2026-10-22T12:32:00Z"));
        assert_eq!(error.as_deref(), Some("Le serveur n'a pas répondu."));
        note_releve_success(&vault, "2026-10-22T12:35:00Z").unwrap();
        let mailbox = vault.open_mailbox().unwrap();
        let (at, error) = mailbox.releve_stamp().unwrap();
        assert_eq!(at.as_deref(), Some("2026-10-22T12:35:00Z"));
        assert_eq!(error, None);
    }

    #[test]
    fn the_window_waits_three_minutes_between_pulls() {
        let now = OffsetDateTime::now_utc();
        assert!(releve_due(None, now));
        assert!(!releve_due(Some(now), now));
        assert!(releve_due(Some(now - time::Duration::seconds(180)), now));
    }
}
