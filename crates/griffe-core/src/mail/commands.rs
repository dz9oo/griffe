//! Commandes du courrier. Aucune ne contacte un serveur.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use time::{Date, Duration, OffsetDateTime};

use super::error::MailError;
use super::model::{ICLOUD_HOST, LetterInk, MailPreset, PROBE_OK_SENTENCE, TlsMode, UNDO_SECS};
use super::present::close_letter;
use super::secret::MailSecret;
use super::store::{self, NewLetter};
use crate::app::{AppError, Command};
use crate::domain::parse_email;
use crate::follow_up::SetFollowUpSender;

fn now_stamp() -> Result<String, AppError> {
    store::stamp(OffsetDateTime::now_utc())
}

fn checked_account(
    from_address: &str,
    host: &str,
    port: u16,
    tls: &str,
    username: &str,
    preset: &str,
) -> Result<(String, String, u16, TlsMode, String, MailPreset), AppError> {
    let from_address = parse_email(from_address.trim()).map_err(|_| MailError::Address)?;
    let username = parse_email(username.trim()).map_err(|_| MailError::Address)?;
    let preset = MailPreset::parse(preset);
    let (host, port, tls) = if preset == MailPreset::Icloud {
        (ICLOUD_HOST.to_string(), 587, TlsMode::StartTls)
    } else {
        let host = host.trim().to_string();
        if host.is_empty() || host.contains("://") || host.contains(char::is_whitespace) {
            return Err(MailError::Incomplete.into());
        }
        let tls = TlsMode::parse(tls)?;
        if port != tls.port() {
            return Err(MailError::Cleartext.into());
        }
        (host, port, tls)
    };
    Ok((from_address, host, port, tls, username, preset))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveMailAccount {
    pub from_name: Option<String>,
    pub from_address: String,
    pub host: String,
    pub port: u16,
    pub tls: String,
    pub username: String,
    pub preset: String,
    /// Hôte des copies. Ignoré pour iCloud. Vide : pas de copie.
    #[serde(default)]
    pub imap_host: String,
    /// Port des copies. 993, ou ignoré quand l'hôte est vide.
    #[serde(default = "default_imap_port")]
    pub imap_port: u16,
}

fn default_imap_port() -> u16 {
    super::model::IMAP_PORT
}

fn checked_copy(
    preset: MailPreset,
    host: &str,
    port: u16,
) -> Result<(Option<String>, Option<u16>), AppError> {
    if preset == MailPreset::Icloud {
        return Ok((None, None));
    }
    let host = host.trim();
    if host.is_empty() {
        return Ok((None, None));
    }
    if host.contains("://") || host.contains(char::is_whitespace) {
        return Err(MailError::Incomplete.into());
    }
    if port != super::model::IMAP_PORT {
        return Err(MailError::CopyPort.into());
    }
    Ok((Some(host.to_string()), Some(super::model::IMAP_PORT)))
}

impl Command for SaveMailAccount {
    type Output = ();
    const NAME: &'static str = "mail.save_account";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let (from_address, host, port, tls, username, preset) = checked_account(
            &self.from_address,
            &self.host,
            self.port,
            &self.tls,
            &self.username,
            &self.preset,
        )?;
        let (imap_host, imap_port) = checked_copy(preset, &self.imap_host, self.imap_port)?;
        let name = self
            .from_name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string);
        let now = now_stamp()?;
        store::save_account(
            conn,
            name.as_deref(),
            &from_address,
            &host,
            port,
            tls,
            &username,
            preset,
            imap_host.as_deref(),
            imap_port,
            &now,
        )?;
        SetFollowUpSender {
            email: from_address,
            name,
        }
        .apply(conn)?;
        Ok(())
    }
}

/// Le secret n'entre pas dans le JSON d'audit : le champ est ignoré par serde.
#[derive(Debug, Serialize, Deserialize)]
pub struct SaveMailSecret {
    #[serde(skip)]
    pub secret: MailSecret,
}

impl Command for SaveMailSecret {
    type Output = ();
    const NAME: &'static str = "mail.save_secret";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        if self.secret.is_empty() {
            return Err(MailError::EmptySecret.into());
        }
        store::save_secret(conn, self.secret.expose(), &now_stamp()?)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClearMailSecret;

impl Command for ClearMailSecret {
    type Output = ();
    const NAME: &'static str = "mail.clear_secret";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        store::clear_secret(conn, &now_stamp()?)
    }
}

/// Verdict d'une liaison déjà tentée par l'adaptateur. N'ouvre aucune socket.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordMailProbe {
    pub ok: bool,
    pub detail: String,
}

impl Command for RecordMailProbe {
    type Output = ();
    const NAME: &'static str = "mail.record_probe";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let detail = checked_detail(self.ok, &self.detail);
        store::record_probe(conn, self.ok, &detail, &now_stamp()?)
    }
}

/// Retient l'identifiant IMAP qui a ouvert la session. Pas un secret.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RememberImapUsername {
    pub username: String,
}

impl Command for RememberImapUsername {
    type Output = ();
    const NAME: &'static str = "mail.remember_imap_username";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let Some(username) = checked_imap_username(&self.username) else {
            return Ok(());
        };
        store::remember_username(conn, &username)
    }
}

fn checked_imap_username(raw: &str) -> Option<String> {
    let text = raw.trim();
    if text.is_empty()
        || text.chars().count() > 200
        || text.chars().any(|ch| ch.is_whitespace() || ch.is_control())
    {
        return None;
    }
    Some(text.to_string())
}

/// Résultat du dépôt dans Envoyés. La lettre est déjà partie.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct RecordSentCopy {
    pub id: String,
    pub status: String,
    pub detail: String,
    pub imap_username: Option<String>,
}

impl Command for RecordSentCopy {
    type Output = ();
    const NAME: &'static str = "mail.record_copy";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        if !matches!(self.status.as_str(), "saved" | "failed" | "skipped") {
            return Err(MailError::Missing.into());
        }
        let detail = {
            let text: String = self
                .detail
                .chars()
                .filter(|ch| !ch.is_control())
                .take(160)
                .collect();
            let text = text.trim();
            (!text.is_empty()).then(|| text.to_string())
        };
        let username = self
            .imap_username
            .as_deref()
            .and_then(checked_imap_username);
        store::record_copy(
            conn,
            &self.id,
            &self.status,
            detail.as_deref(),
            username.as_deref(),
            &now_stamp()?,
        )
    }
}

fn checked_detail(ok: bool, raw: &str) -> String {
    let text: String = raw
        .chars()
        .filter(|ch| !ch.is_control())
        .take(160)
        .collect();
    let text = text.trim();
    if !text.is_empty() {
        return text.to_string();
    }
    if ok {
        PROBE_OK_SENTENCE.to_string()
    } else {
        "Le serveur n'a pas répondu.".to_string()
    }
}

const SIGNATURE_MAX: usize = 2_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveMailSignature {
    pub signature: String,
}

impl Command for SaveMailSignature {
    type Output = ();
    const NAME: &'static str = "mail.save_signature";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let text = checked_signature(&self.signature)?;
        let stored = if text.is_empty() {
            None
        } else {
            Some(text.as_str())
        };
        store::save_signature(conn, stored, &now_stamp()?)
    }
}

const METIER_MAX: usize = 80;
const SITE_MAX: usize = 300;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveLetterface {
    pub ink: String,
    pub metier: String,
    pub site: String,
}

impl Command for SaveLetterface {
    type Output = ();
    const NAME: &'static str = "mail.save_letterface";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let ink = LetterInk::parse(self.ink.trim()).ok_or(MailError::Ink)?;
        let metier = checked_metier(&self.metier)?;
        let site = checked_site(&self.site)?;
        store::save_letterface(conn, ink.as_str(), &metier, &site, &now_stamp()?)
    }
}

fn checked_metier(raw: &str) -> Result<String, AppError> {
    let text = raw.replace('\r', "").trim().to_string();
    if text.contains('\n') || text.chars().count() > METIER_MAX {
        return Err(MailError::Metier.into());
    }
    Ok(text)
}

fn checked_site(raw: &str) -> Result<String, AppError> {
    let text = raw.trim().to_string();
    if text.is_empty() {
        return Ok(text);
    }
    let http = text.starts_with("https://") || text.starts_with("http://");
    if !http || text.chars().count() > SITE_MAX || text.contains(char::is_whitespace) {
        return Err(MailError::Site.into());
    }
    Ok(text)
}

fn checked_signature(raw: &str) -> Result<String, AppError> {
    let text = raw.replace('\r', "").trim().to_string();
    if text.contains("{{signature}}") || text.contains("<signature>") {
        return Err(MailError::SignatureToken.into());
    }
    if text.chars().count() > SIGNATURE_MAX {
        return Err(MailError::SignatureLength.into());
    }
    Ok(text)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetAutomaticSend {
    pub enabled: bool,
}

impl Command for SetAutomaticSend {
    type Output = ();
    const NAME: &'static str = "mail.set_automatic";

    fn requires_confirmation(&self) -> bool {
        true
    }

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        store::set_auto(conn, self.enabled, &now_stamp()?)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArmOutbound {
    pub kind: String,
    pub anchor: Option<String>,
    pub to_address: String,
    pub subject: String,
    pub body: String,
    pub delay_secs: i64,
    pub session_token: Option<String>,
    pub follow_subject: Option<String>,
    pub follow_subject_id: Option<String>,
    pub follow_cycle: Option<String>,
    pub follow_step: Option<String>,
    /// Fiche dont le courriel est relu au moment de l'envoi. `None` : lettre d'essai
    /// ou lettre sans fiche, l'adresse saisie reste.
    #[serde(default)]
    pub client_id: Option<String>,
}

impl Command for ArmOutbound {
    type Output = String;
    const NAME: &'static str = "mail.arm";

    fn requires_confirmation(&self) -> bool {
        true
    }

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let to_address = parse_email(self.to_address.trim()).map_err(|_| MailError::Address)?;
        let subject = self.subject.trim().to_string();
        let signature = store::profile(conn)?.signature;
        let body = close_letter(self.body.trim(), &signature);
        if subject.is_empty() || body.is_empty() {
            return Err(MailError::Incomplete.into());
        }
        let delay = self.delay_secs.clamp(0, UNDO_SECS);
        let due = OffsetDateTime::now_utc() + Duration::seconds(delay);
        let now = store::stamp(OffsetDateTime::now_utc())?;
        store::insert_letter(
            conn,
            &NewLetter {
                kind: self.kind.clone(),
                anchor: self.anchor.clone(),
                to_address,
                subject,
                body,
                send_after: store::stamp(due)?,
                session_token: self.session_token.clone(),
                follow_subject: self.follow_subject.clone(),
                follow_subject_id: self.follow_subject_id.clone(),
                follow_cycle: self.follow_cycle.clone(),
                follow_step: self.follow_step.clone(),
                client_id: self.client_id.clone(),
            },
            &now,
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelOutbound {
    pub id: String,
}

impl Command for CancelOutbound {
    type Output = ();
    const NAME: &'static str = "mail.cancel";

    fn requires_confirmation(&self) -> bool {
        true
    }

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        store::cancel(conn, &self.id, &now_stamp()?)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct CommitOutbound {
    pub id: String,
}

impl Command for CommitOutbound {
    type Output = bool;
    const NAME: &'static str = "mail.commit";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        store::commit_outbound(conn, &self.id, &now_stamp()?)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct AbandonHeld;

impl Command for AbandonHeld {
    type Output = ();
    const NAME: &'static str = "mail.abandon_held";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        store::abandon_held(conn, &now_stamp()?)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct ReleaseStaleArmed {
    #[serde(with = "time::serde::rfc3339")]
    pub now: OffsetDateTime,
    pub session_token: String,
}

impl Command for ReleaseStaleArmed {
    type Output = ();
    const NAME: &'static str = "mail.release_stale";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        store::release_stale(conn, &self.session_token, &store::stamp(self.now)?)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct FreezeInterrupted {
    #[serde(with = "time::serde::rfc3339")]
    pub now: OffsetDateTime,
}

impl Command for FreezeInterrupted {
    type Output = ();
    const NAME: &'static str = "mail.freeze";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        store::freeze_sending(conn, &store::stamp(self.now)?)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct EnrollToday {
    #[serde(with = "crate::domain::serde_date::date")]
    pub today: Date,
    #[serde(with = "time::serde::rfc3339")]
    pub now: OffsetDateTime,
    pub session_token: String,
}

impl Command for EnrollToday {
    type Output = ();
    const NAME: &'static str = "mail.enroll_today";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        store::enroll(
            conn,
            self.today,
            &store::stamp(self.now)?,
            &self.session_token,
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct ClaimDue {
    #[serde(with = "time::serde::rfc3339")]
    pub now: OffsetDateTime,
    pub session_token: String,
}

impl Command for ClaimDue {
    type Output = Vec<String>;
    const NAME: &'static str = "mail.claim";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        store::claim(
            conn,
            &self.session_token,
            &store::stamp(self.now)?,
            &store::hour_ago(self.now)?,
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) enum DeliveryDetail {
    Sent { response: String },
    Failed { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct RecordDelivery {
    pub id: String,
    #[serde(with = "crate::domain::serde_date::date")]
    pub today: Date,
    pub detail: DeliveryDetail,
}

impl Command for RecordDelivery {
    type Output = ();
    const NAME: &'static str = "mail.record";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let now = now_stamp()?;
        match &self.detail {
            DeliveryDetail::Failed { message } => store::mark_failed(conn, &self.id, message, &now),
            DeliveryDetail::Sent { response } => {
                let link = store::follow_link(conn, &self.id)?;
                store::mark_sent(conn, &self.id, response, &now)?;
                if let Some(link) = link {
                    let marked = crate::follow_up::MarkFollowUpSent {
                        subject: link.subject,
                        today: self.today,
                        subject_line: Some(link.subject_line),
                        body: Some(link.body),
                    }
                    .apply(conn);
                    if let Err(error) = marked {
                        store::note(conn, &self.id, &error.to_string())?;
                    }
                }
                Ok(())
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolveUncertain {
    pub id: String,
    pub sent: bool,
    #[serde(with = "crate::domain::serde_date::date")]
    pub today: Date,
}

impl Command for ResolveUncertain {
    type Output = ();
    const NAME: &'static str = "mail.resolve";

    fn requires_confirmation(&self) -> bool {
        true
    }

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let now = now_stamp()?;
        store::resolve(conn, &self.id, self.sent, &now)?;
        if self.sent
            && let Some(link) = store::follow_link(conn, &self.id)?
        {
            let marked = crate::follow_up::MarkFollowUpSent {
                subject: link.subject,
                today: self.today,
                subject_line: Some(link.subject_line),
                body: Some(link.body),
            }
            .apply(conn);
            if let Err(error) = marked {
                store::note(conn, &self.id, &error.to_string())?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryOutbound {
    pub id: String,
    pub session_token: Option<String>,
}

impl Command for RetryOutbound {
    type Output = String;
    const NAME: &'static str = "mail.retry";

    fn requires_confirmation(&self) -> bool {
        true
    }

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let now = OffsetDateTime::now_utc();
        store::retry(
            conn,
            &self.id,
            &store::stamp(now)?,
            &store::stamp(now)?,
            self.session_token.as_deref(),
        )
    }
}
