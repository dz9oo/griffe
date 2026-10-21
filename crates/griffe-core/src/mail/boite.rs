//! Lecture de la boîte et classement humain. Aucune socket.

use std::collections::HashMap;
use std::path::Path;

use rusqlite::OptionalExtension;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc2822;

use super::commands::{FileMessage, FilingSource, HoldReply};
use crate::app::{AppError, ExecutionContext, Executor, Outcome};
use crate::mailbox::{self, Mailbox, MirrorLetter, MirrorMessage};
use crate::store::Store;

/// Vue de la page. Un message peut en occuper plusieurs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoxVue {
    Lire,
    Dossiers,
    Envoyees,
    Tout,
    Echec,
}

impl BoxVue {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Lire => "lire",
            Self::Dossiers => "dossiers",
            Self::Envoyees => "envoyees",
            Self::Tout => "tout",
            Self::Echec => "echec",
        }
    }

    #[must_use]
    pub fn parse(raw: &str) -> Self {
        match raw {
            "dossiers" => Self::Dossiers,
            "envoyees" => Self::Envoyees,
            "tout" => Self::Tout,
            "echec" => Self::Echec,
            _ => Self::Lire,
        }
    }
}

/// Carte de la liste, sans le corps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxCard {
    pub id: String,
    pub mailbox: String,
    pub who: String,
    pub sent_on: Option<String>,
    pub subject: String,
    pub seen: bool,
    pub notice: Option<String>,
    pub role: String,
    pub on_dossier: bool,
    pub choice: bool,
}

/// Où ranger, proposé à l'humain. Pas une devinette.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassChoice {
    pub client_id: String,
    pub opportunity_id: Option<String>,
    pub label: String,
}

/// Rattachement déjà connu, ou un choix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LetterPlace {
    Filed {
        client_id: String,
        opportunity_id: Option<String>,
        name: String,
    },
    Pick {
        choices: Vec<ClassChoice>,
    },
    Quiet,
}

/// Lettre ouverte. Le corps reste hors du coffre comptable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxLetter {
    pub id: String,
    pub message_id: Option<String>,
    pub from_name: Option<String>,
    pub from_address: Option<String>,
    pub subject: String,
    pub sent_on: Option<String>,
    pub text: String,
    pub html: Option<String>,
    pub role: String,
    pub notice: Option<String>,
    pub notice_address: Option<String>,
    pub place: LetterPlace,
    /// Adresse en échec certain, quand la fiche est unique.
    pub dead_address: Option<String>,
}

/// Compteurs des cinq vues, avant la recherche.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BoxCounts {
    pub lire: u32,
    pub dossiers: u32,
    pub envoyees: u32,
    pub tout: u32,
    pub echec: u32,
}

/// Page. `open` est la lettre demandée, ou la première de la liste.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxPage {
    pub dossier_letters: u32,
    pub failed_addresses: u32,
    pub counts: BoxCounts,
    pub folders: Vec<String>,
    pub cards: Vec<BoxCard>,
    pub open: Option<BoxLetter>,
}

/// Ce qu'on demande à la page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoxQuery<'a> {
    pub vue: BoxVue,
    pub q: &'a str,
    pub letter: Option<&'a str>,
    pub folder: Option<&'a str>,
}

/// Vrai si la carte appartient à la vue. La recherche est à part.
#[must_use]
pub fn in_view(card: &BoxCard, vue: BoxVue) -> bool {
    match vue {
        BoxVue::Tout => true,
        BoxVue::Echec => card.notice.as_deref() == Some("certain"),
        BoxVue::Envoyees => card.role == "sent",
        BoxVue::Lire => !card.seen && card.role != "sent",
        BoxVue::Dossiers => card.on_dossier && card.role != "sent",
    }
}

/// Réponses de dossier pas encore ouvertes, plus les adresses en échec certain.
/// Un avis probable ne compte pas. Zéro : le sceau ne s'allume pas.
///
/// # Errors
///
/// Lecture impossible.
pub fn seal_count(vault: &Store) -> Result<u32, AppError> {
    let failed = failed_addresses(vault.connection())?;
    let replies = if mailbox_exists(vault) {
        let mailbox = vault.open_mailbox()?;
        unread_replies(vault.connection(), &mailbox)?
    } else {
        0
    };
    Ok(failed.saturating_add(replies))
}

/// Monte la page depuis le miroir déjà rempli.
///
/// # Errors
///
/// Lecture impossible.
pub fn read_box(vault: &Store, query: BoxQuery<'_>) -> Result<BoxPage, AppError> {
    let failed_addresses = failed_addresses(vault.connection())?;
    if !mailbox_exists(vault) {
        return Ok(BoxPage {
            dossier_letters: 0,
            failed_addresses,
            counts: BoxCounts::default(),
            folders: Vec::new(),
            cards: Vec::new(),
            open: None,
        });
    }
    let mailbox = vault.open_mailbox()?;
    let filings = filings_of(vault.connection())?;
    let mut cards = Vec::new();
    let mut dossier_letters = 0_u32;
    for message in mailbox.list()? {
        let filing = filing_of(&message, &filings);
        let on_dossier = filing
            .as_ref()
            .is_some_and(|row| row.opportunity_id.is_some());
        if on_dossier && message.folder_role != "sent" {
            dossier_letters = dossier_letters.saturating_add(1);
        }
        cards.push(card_of(&message, on_dossier));
    }
    cards.sort_by_key(|card| std::cmp::Reverse(sent_stamp(card.sent_on.as_deref())));
    let counts = count_views(&cards);
    let folders = mailbox.folder_names()?;
    let shown = shown_cards(&cards, query);
    let open_id = query
        .letter
        .or_else(|| shown.first().map(|card| card.id.as_str()));
    let open = open_id
        .map(|id| open_letter(vault, &mailbox, &filings, id))
        .transpose()?
        .flatten();
    let cards = if let Some(letter) = &open {
        stick(&shown, &cards, &letter.id)
    } else {
        shown
    };
    Ok(BoxPage {
        dossier_letters,
        failed_addresses,
        counts,
        folders,
        cards,
        open,
    })
}

/// Range le message, puis retient la relance si un dossier est choisi.
/// Un agent dépose la confirmation et ne retient rien tant qu'elle n'est pas donnée.
///
/// # Errors
///
/// Le message, la fiche ou le dossier est illisible, ou l'écriture échoue.
pub fn file_human(
    vault: &mut Store,
    ctx: &ExecutionContext,
    message_id: &str,
    client_id: &str,
    opportunity_id: Option<String>,
) -> Result<Outcome<()>, AppError> {
    let message_id = message_id.trim().trim_matches(['<', '>']).to_string();
    if message_id.is_empty() {
        return Err(AppError::Domain("Ce message n'a pas d'identifiant.".into()));
    }
    let opportunity_id = opportunity_id.filter(|id| !id.trim().is_empty());
    let outcome = Executor::new(vault).execute(
        &FileMessage {
            message_id,
            client_id: client_id.to_string(),
            opportunity_id: opportunity_id.clone(),
            source: FilingSource::Human,
        },
        ctx,
    )?;
    if matches!(outcome, Outcome::Applied(())) {
        hold_filed(vault, ctx, opportunity_id)?;
    }
    Ok(outcome)
}

/// Retient la relance d'un classement déjà confirmé.
///
/// # Errors
///
/// Le dossier est introuvable, ou l'écriture échoue.
pub fn hold_filed(
    vault: &mut Store,
    ctx: &ExecutionContext,
    opportunity_id: Option<String>,
) -> Result<(), AppError> {
    let Some(opportunity_id) = opportunity_id.filter(|id| !id.trim().is_empty()) else {
        return Ok(());
    };
    Executor::new(vault).execute(&HoldReply { opportunity_id }, ctx)?;
    Ok(())
}

/// Où marquer lu. `remote` est vide quand l'UID ne tient pas dans un ordre serveur.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeenTarget {
    pub mailbox: String,
    pub uid: u32,
}

/// Travail avant de montrer la lettre.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SeenMark {
    Absent,
    Already,
    Due { remote: Option<SeenTarget> },
}

/// `Due` : le miroir n'a pas encore la lettre comme lue.
///
/// # Errors
///
/// Lecture impossible.
pub fn prepare_seen(vault: &Store, id: &str) -> Result<SeenMark, AppError> {
    if !mailbox_exists(vault) {
        return Ok(SeenMark::Absent);
    }
    let mailbox = vault.open_mailbox()?;
    let Some(place) = mailbox.place(id)? else {
        return Ok(SeenMark::Absent);
    };
    if place.seen {
        return Ok(SeenMark::Already);
    }
    let remote = u32::try_from(place.uid).ok().map(|uid| SeenTarget {
        mailbox: place.mailbox,
        uid,
    });
    Ok(SeenMark::Due { remote })
}

/// Identifiant de message à ranger. Un identifiant du miroir est traduit. Les chevrons sautent.
///
/// # Errors
///
/// Le message du miroir n'a pas d'identifiant, ou la lecture échoue.
pub fn resolve_message_id(vault: &Store, token: &str) -> Result<String, AppError> {
    let token = token.trim();
    if token.is_empty() {
        return Err(AppError::Domain("Ce message n'a pas d'identifiant.".into()));
    }
    if mailbox_exists(vault) {
        let mailbox = vault.open_mailbox()?;
        if let Some(letter) = mailbox.read(token)? {
            let Some(id) = letter.message_id.filter(|id| !id.trim().is_empty()) else {
                return Err(AppError::Domain("Ce message n'a pas d'identifiant.".into()));
            };
            return Ok(id);
        }
    }
    Ok(token.trim_matches(['<', '>']).to_string())
}

fn mailbox_exists(vault: &Store) -> bool {
    mailbox::mailbox_file(Path::new(vault.db_path())).exists()
}

fn failed_addresses(conn: &rusqlite::Connection) -> Result<u32, AppError> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM address_fault WHERE status = 'failed'",
        [],
        |row| row.get(0),
    )?;
    Ok(u32::try_from(count).unwrap_or(u32::MAX))
}

fn unread_replies(conn: &rusqlite::Connection, mailbox: &Mailbox) -> Result<u32, AppError> {
    let filings = filings_of(conn)?;
    let mut count = 0_u32;
    for message in mailbox.list()? {
        if is_unopened_reply(&message, filing_of(&message, &filings).as_ref()) {
            count = count.saturating_add(1);
        }
    }
    Ok(count)
}

fn is_unopened_reply(message: &MirrorMessage, filing: Option<&FilingRow>) -> bool {
    !message.seen
        && message.folder_role != "sent"
        && message.notice.is_none()
        && filing.is_some_and(|row| row.opportunity_id.is_some())
}

#[derive(Clone)]
struct FilingRow {
    client_id: String,
    opportunity_id: Option<String>,
}

fn filings_of(conn: &rusqlite::Connection) -> Result<HashMap<String, FilingRow>, AppError> {
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
    let mut map = HashMap::new();
    for row in rows {
        let (id, filing) = row?;
        map.insert(id, filing);
    }
    Ok(map)
}

fn filing_of(message: &MirrorMessage, filings: &HashMap<String, FilingRow>) -> Option<FilingRow> {
    message
        .message_id
        .as_deref()
        .and_then(|id| filings.get(&bare(id)))
        .cloned()
}

fn card_of(message: &MirrorMessage, on_dossier: bool) -> BoxCard {
    BoxCard {
        id: message.id.clone(),
        mailbox: message.mailbox.clone(),
        who: message
            .from_name
            .clone()
            .or_else(|| message.from_address.clone())
            .unwrap_or_else(|| "—".to_string()),
        sent_on: message.sent_on.clone(),
        subject: message.subject.clone(),
        seen: message.seen,
        notice: message.notice.clone(),
        role: message.folder_role.clone(),
        on_dossier,
        choice: message.choice_ids.is_some(),
    }
}

fn sent_stamp(raw: Option<&str>) -> i64 {
    raw.and_then(|value| OffsetDateTime::parse(value, &Rfc2822).ok())
        .map_or(0, OffsetDateTime::unix_timestamp)
}

fn count_views(cards: &[BoxCard]) -> BoxCounts {
    let mut counts = BoxCounts::default();
    for card in cards {
        counts.tout = counts.tout.saturating_add(1);
        if in_view(card, BoxVue::Lire) {
            counts.lire = counts.lire.saturating_add(1);
        }
        if in_view(card, BoxVue::Dossiers) {
            counts.dossiers = counts.dossiers.saturating_add(1);
        }
        if in_view(card, BoxVue::Envoyees) {
            counts.envoyees = counts.envoyees.saturating_add(1);
        }
        if in_view(card, BoxVue::Echec) {
            counts.echec = counts.echec.saturating_add(1);
        }
    }
    counts
}

fn shown_cards(cards: &[BoxCard], query: BoxQuery<'_>) -> Vec<BoxCard> {
    cards
        .iter()
        .filter(|card| {
            folder_ok(card, query.folder) && view_ok(card, query) && search_ok(card, query.q)
        })
        .cloned()
        .collect()
}

fn folder_ok(card: &BoxCard, folder: Option<&str>) -> bool {
    match folder.map(str::trim).filter(|name| !name.is_empty()) {
        None => true,
        Some(name) => card.mailbox.eq_ignore_ascii_case(name),
    }
}

fn view_ok(card: &BoxCard, query: BoxQuery<'_>) -> bool {
    if query.folder.is_some_and(|name| !name.is_empty()) {
        return true;
    }
    in_view(card, query.vue)
}

fn search_ok(card: &BoxCard, q: &str) -> bool {
    let q = q.trim();
    if q.is_empty() {
        return true;
    }
    let q = q.to_lowercase();
    card.who.to_lowercase().contains(&q) || card.subject.to_lowercase().contains(&q)
}

fn stick(shown: &[BoxCard], all: &[BoxCard], id: &str) -> Vec<BoxCard> {
    if shown.iter().any(|card| card.id == id) {
        return shown.to_vec();
    }
    let Some(card) = all.iter().find(|card| card.id == id) else {
        return shown.to_vec();
    };
    let mut cards = Vec::with_capacity(shown.len() + 1);
    cards.push(card.clone());
    cards.extend(shown.iter().cloned());
    cards
}

fn open_letter(
    vault: &Store,
    mailbox: &Mailbox,
    filings: &HashMap<String, FilingRow>,
    id: &str,
) -> Result<Option<BoxLetter>, AppError> {
    let Some(letter) = mailbox.read(id)? else {
        return Ok(None);
    };
    let names = client_names(vault.connection())?;
    let open = open_dossiers(vault.connection(), &names)?;
    let filing = letter
        .message_id
        .as_deref()
        .and_then(|message_id| filings.get(&bare(message_id)));
    let place = place_of(&letter, filing, &names, &open);
    let dead_address = dead_of(vault.connection(), &letter, &place)?;
    Ok(Some(BoxLetter {
        id: letter.id,
        message_id: letter.message_id,
        from_name: letter.from_name,
        from_address: letter.from_address,
        subject: letter.subject,
        sent_on: letter.sent_on,
        text: letter.text,
        html: letter.html,
        role: letter.folder_role,
        notice: letter.notice,
        notice_address: letter.notice_address,
        place,
        dead_address,
    }))
}

fn place_of(
    letter: &MirrorLetter,
    filing: Option<&FilingRow>,
    names: &HashMap<String, String>,
    open: &[ClassChoice],
) -> LetterPlace {
    if letter.folder_role == "sent" || letter.message_id.as_deref().is_none_or(str::is_empty) {
        return LetterPlace::Quiet;
    }
    if let Some(filing) = filing {
        return LetterPlace::Filed {
            name: names
                .get(&filing.client_id)
                .cloned()
                .unwrap_or_else(|| "Dossier".to_string()),
            client_id: filing.client_id.clone(),
            opportunity_id: filing.opportunity_id.clone(),
        };
    }
    let choices = match letter.choice_ids.as_deref() {
        Some(raw) if letter.choice_of.as_deref() == Some("dossier") => {
            choices_for(&parse_ids(raw), open)
        }
        Some(raw) => fiches_for(&parse_ids(raw), names),
        None => open.to_vec(),
    };
    LetterPlace::Pick { choices }
}

fn choices_for(ids: &[String], open: &[ClassChoice]) -> Vec<ClassChoice> {
    ids.iter()
        .filter_map(|id| {
            open.iter()
                .find(|choice| choice.opportunity_id.as_deref() == Some(id))
                .cloned()
        })
        .collect()
}

fn fiches_for(ids: &[String], names: &HashMap<String, String>) -> Vec<ClassChoice> {
    ids.iter()
        .filter_map(|id| {
            names.get(id).map(|name| ClassChoice {
                client_id: id.clone(),
                opportunity_id: None,
                label: name.clone(),
            })
        })
        .collect()
}

fn dead_of(
    conn: &rusqlite::Connection,
    letter: &MirrorLetter,
    place: &LetterPlace,
) -> Result<Option<String>, AppError> {
    let LetterPlace::Filed { client_id, .. } = place else {
        return Ok(None);
    };
    if letter.notice.as_deref() == Some("certain")
        && let Some(address) = letter
            .notice_address
            .as_deref()
            .filter(|value| !value.is_empty())
    {
        if address_status(conn, client_id, address)?.as_deref() == Some("confirmed") {
            return Ok(None);
        }
        return Ok(Some(address.to_string()));
    }
    let found: Option<String> = conn
        .query_row(
            "SELECT address FROM address_fault
             WHERE client_id = ?1 AND status = 'failed'
             LIMIT 1",
            [client_id],
            |row| row.get(0),
        )
        .optional()?;
    Ok(found)
}

fn address_status(
    conn: &rusqlite::Connection,
    client_id: &str,
    address: &str,
) -> Result<Option<String>, AppError> {
    conn.query_row(
        "SELECT status FROM address_fault WHERE client_id = ?1 AND address = ?2",
        [client_id, address],
        |row| row.get(0),
    )
    .optional()
    .map_err(AppError::from)
}

fn client_names(conn: &rusqlite::Connection) -> Result<HashMap<String, String>, AppError> {
    let mut stmt = conn.prepare("SELECT id, name FROM clients")?;
    let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    let mut names = HashMap::new();
    for row in rows {
        let (id, name) = row?;
        names.insert(id, name);
    }
    Ok(names)
}

fn open_dossiers(
    conn: &rusqlite::Connection,
    names: &HashMap<String, String>,
) -> Result<Vec<ClassChoice>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, client_id, name FROM opportunities
         WHERE archived_at IS NULL AND stage NOT IN ('won', 'lost')
         ORDER BY name",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut choices = Vec::new();
    for row in rows {
        let (id, client_id, opportunity) = row?;
        let Some(name) = names.get(&client_id) else {
            continue;
        };
        let label = if opportunity.is_empty() || opportunity.eq_ignore_ascii_case(name) {
            name.clone()
        } else {
            format!("{name} — {opportunity}")
        };
        choices.push(ClassChoice {
            client_id,
            opportunity_id: Some(id),
            label,
        });
    }
    Ok(choices)
}

fn parse_ids(raw: &str) -> Vec<String> {
    serde_json::from_str(raw).unwrap_or_default()
}

fn bare(value: &str) -> String {
    value
        .trim()
        .trim_matches(|ch: char| ch == '<' || ch == '>')
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::{BoxQuery, BoxVue, file_human, hold_filed, in_view, read_box, seal_count};
    use crate::app::{Actor, ExecutionContext, recent_audit_entries};
    use crate::mailbox::MirrorInsert;
    use crate::store::testing::test_store;

    fn human() -> ExecutionContext {
        ExecutionContext::new(Actor::Human, false)
    }

    fn agent() -> ExecutionContext {
        ExecutionContext::new(
            Actor::Agent {
                session: "test".into(),
            },
            false,
        )
    }

    fn seed_people(store: &mut crate::store::Store) {
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
                 INSERT INTO mail_filing (message_id, client_id, opportunity_id, source) VALUES
                    ('camille-1@riviere.test', 'c1', 'o1', 'auto'),
                    ('bounce@mx.example', 'c2', 'o2', 'auto');
                 INSERT INTO address_fault (client_id, address, status, message_id) VALUES
                    ('c2', 'contact@atelier-du-mail.fr', 'failed', 'bounce@mx.example');",
            )
            .unwrap();
    }

    #[allow(clippy::too_many_arguments)]
    fn plant(
        store: &crate::store::Store,
        uid: i64,
        message_id: &str,
        notice: Option<&str>,
        seen: bool,
        role: &str,
        subject: &str,
        text: &str,
    ) {
        let mailbox = store.open_mailbox().unwrap();
        mailbox
            .insert(&MirrorInsert {
                mailbox: "INBOX".into(),
                uid,
                uidvalidity: 1,
                message_id: Some(message_id.into()),
                from_name: Some(subject.into()),
                from_address: Some("qui@exemple.test".into()),
                subject: subject.into(),
                sent_on: Some("Sat, 5 Sep 2026 09:00:00 +0200".into()),
                text: text.into(),
                html: None,
                seen,
                notice: notice.map(str::to_string),
                notice_address: (notice == Some("certain"))
                    .then(|| "contact@atelier-du-mail.fr".into()),
                folder_role: role.into(),
                choice_of: None,
                choice_ids: None,
            })
            .unwrap();
        mailbox.remember("INBOX", 1, uid).unwrap();
    }

    #[test]
    fn the_seal_counts_an_unopened_reply_and_a_failed_address_only() {
        let mut store = test_store("boite-seal");
        assert_eq!(seal_count(&store).unwrap(), 0);
        seed_people(&mut store);
        plant(
            &store,
            1,
            "<camille-1@riviere.test>",
            None,
            false,
            "inbox",
            "Reponse",
            "PHRASE-SECRETE-REPONSE",
        );
        plant(
            &store,
            2,
            "<bounce@mx.example>",
            Some("certain"),
            false,
            "inbox",
            "Echec",
            "PHRASE-SECRETE-AVIS",
        );
        plant(
            &store,
            3,
            "<peut-etre@mx.example>",
            Some("probable"),
            false,
            "inbox",
            "Peut-etre",
            "indice",
        );
        assert_eq!(
            seal_count(&store).unwrap(),
            2,
            "probable et avis certain non lu ne doublonnent pas"
        );
        let page = read_box(
            &store,
            BoxQuery {
                vue: BoxVue::Echec,
                q: "",
                letter: None,
                folder: None,
            },
        )
        .unwrap();
        assert_eq!(page.cards.len(), 1);
        assert_eq!(page.cards[0].subject, "Echec");
        assert!(page.cards.iter().all(|card| in_view(card, BoxVue::Echec)));
        assert_eq!(page.counts.lire, 3);
        let mailbox = store.open_mailbox().unwrap();
        let id = mailbox
            .list()
            .unwrap()
            .into_iter()
            .find(|line| line.subject == "Reponse")
            .unwrap()
            .id;
        mailbox.mark_seen(&id).unwrap();
        assert_eq!(seal_count(&store).unwrap(), 1);
    }

    #[test]
    fn filing_by_a_human_holds_the_relance_and_leaves_the_body_out() {
        let mut store = test_store("boite-file");
        seed_people(&mut store);
        let pending = file_human(
            &mut store,
            &agent(),
            "<autre@exemple.test>",
            "c1",
            Some("o1".into()),
        )
        .unwrap();
        assert!(matches!(
            pending,
            crate::app::Outcome::PendingConfirmation(_)
        ));
        let held: i64 = store
            .connection()
            .query_row(
                "SELECT reply_hold FROM opportunities WHERE id = 'o1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(held, 0);
        file_human(
            &mut store,
            &human(),
            "<autre@exemple.test>",
            "c1",
            Some("o1".into()),
        )
        .unwrap();
        let held: i64 = store
            .connection()
            .query_row(
                "SELECT reply_hold FROM opportunities WHERE id = 'o1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(held, 1);
        let entries = recent_audit_entries(store.connection(), 0, 20).unwrap();
        let filed = entries
            .iter()
            .find(|entry| entry.command_name == "mail.file")
            .unwrap();
        assert!(!filed.command_json.contains("PHRASE"));
        assert!(filed.command_json.contains("autre@exemple.test"));
        hold_filed(&mut store, &human(), Some("o1".into())).unwrap();
        let held: i64 = store
            .connection()
            .query_row(
                "SELECT reply_hold FROM opportunities WHERE id = 'o1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(held, 1);
    }
}
