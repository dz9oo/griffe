//! Mutations du journal de relances. Aucune IO fichier : le `.eml` est renvoyé en octets.

use std::collections::HashSet;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use time::{Date, OffsetDateTime};

use crate::app::{AppError, Command};
use crate::company::{CompanyProfile, company_profile};
use crate::domain::{
    self, EmlDraft, FollowUpEvent, FollowUpEventId, FollowUpFact, FollowUpKind, FollowUpSubject,
    InteractionKind, TemplateContext, derive_cursor_with, parse_email, phrase_from_editor,
    render_eml, render_template,
};
use crate::prospection::{LogInteraction, opportunity_by_id};

use super::error::FollowUpError;
use super::queries::{FollowUpCard, card_for, load_subject};
use super::row;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetFollowUpSender {
    pub email: String,
    pub name: Option<String>,
}

impl Command for SetFollowUpSender {
    type Output = ();
    const NAME: &'static str = "follow_up.set_sender";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let email = parse_email(&self.email).map_err(FollowUpError::from)?;
        let name = self
            .name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        row::upsert_settings(conn, &email, name)?;
        Ok(())
    }
}

/// Réécrit le nom, le sujet et le corps d'un moment. L'écart et l'ordre ne bougent pas.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhraseRewrite {
    pub key: String,
    pub label: String,
    pub subject: String,
    pub body: String,
    pub revision: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RewriteProspectPhrases {
    pub phrases: Vec<PhraseRewrite>,
}

impl Command for RewriteProspectPhrases {
    type Output = Vec<super::row::ProspectPhrase>;
    const NAME: &'static str = "follow_up.rewrite_phrases";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let current = row::list_phrases(conn)?;
        for edit in &self.phrases {
            let label = edit.label.trim();
            let subject = phrase_from_editor(edit.subject.trim());
            let body = phrase_from_editor(&edit.body);
            if label.is_empty() {
                return Err(FollowUpError::EmptyMoment.into());
            }
            if subject.trim().is_empty() {
                return Err(FollowUpError::EmptySubject.into());
            }
            if body.trim().is_empty() {
                return Err(FollowUpError::EmptyLetter.into());
            }
            let Some(row) = current.iter().find(|phrase| phrase.key == edit.key) else {
                return Err(FollowUpError::UnknownPhrase.into());
            };
            let same = row.label == label && row.subject == subject && row.body == body;
            if row.revision != edit.revision {
                if same {
                    continue;
                }
                return Err(FollowUpError::StalePhrases.into());
            }
            if same {
                continue;
            }
            if !row::update_phrase(conn, &edit.key, label, &subject, &body, edit.revision)? {
                return Err(FollowUpError::StalePhrases.into());
            }
        }
        row::list_phrases(conn)
    }
}

/// Un moment de la série vivante, tel qu'on veut le laisser.
///
/// `key` absent : un moment nouveau. L'écart du premier moment reste 0 :
/// c'est le jour déjà posé sur le dossier.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MomentDraft {
    pub key: Option<String>,
    pub label: String,
    pub offset_days: i64,
    pub subject: String,
    pub body: String,
    pub revision: Option<i64>,
}

impl MomentDraft {
    #[must_use]
    pub fn from_phrase(phrase: &row::ProspectPhrase) -> Self {
        Self {
            key: Some(phrase.key.clone()),
            label: phrase.label.clone(),
            offset_days: phrase.offset_days,
            subject: phrase.subject.clone(),
            body: phrase.body.clone(),
            revision: Some(phrase.revision),
        }
    }
}

/// Au plus dix ans. Au-delà, ce n'est plus un écart qu'on relit.
const MAX_GAP_DAYS: i64 = 3_660;

/// Remplace la série vivante.
///
/// Tant que l'ordre, le nombre et les écarts ne bougent pas, les mots se
/// réécrivent pour tout le monde. Au premier changement de structure, chaque
/// conversation ouverte qui n'a pas encore de copie garde la série d'avant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArrangeProspectPhrases {
    pub moments: Vec<MomentDraft>,
}

struct PreparedMoment {
    key: String,
    is_new: bool,
    label: String,
    offset_days: i64,
    subject: String,
    body: String,
    revision: Option<i64>,
}

impl Command for ArrangeProspectPhrases {
    type Output = Vec<row::ProspectPhrase>;
    const NAME: &'static str = "follow_up.arrange_phrases";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let current = row::list_phrases(conn)?;
        let prepared = prepare_moments(&current, &self.moments)?;
        if series_matches(&current, &prepared) {
            return Ok(current);
        }
        refuse_stale(&current, &prepared)?;
        if structure_changed(&current, &prepared) {
            pin_open_cycles(conn)?;
        }
        row::replace_phrases(conn, &phrases_to_write(&current, &prepared)?)?;
        row::list_phrases(conn)
    }
}

fn prepare_moments(
    current: &[row::ProspectPhrase],
    drafts: &[MomentDraft],
) -> Result<Vec<PreparedMoment>, AppError> {
    if drafts.is_empty() {
        return Err(FollowUpError::LastMoment.into());
    }
    let mut seen = HashSet::new();
    let mut prepared = Vec::with_capacity(drafts.len());
    for (index, draft) in drafts.iter().enumerate() {
        let label = draft.label.trim().to_string();
        let subject = phrase_from_editor(draft.subject.trim());
        let body = phrase_from_editor(&draft.body);
        if label.is_empty() {
            return Err(FollowUpError::EmptyMoment.into());
        }
        if subject.trim().is_empty() {
            return Err(FollowUpError::EmptySubject.into());
        }
        if body.trim().is_empty() {
            return Err(FollowUpError::EmptyLetter.into());
        }
        let offset_days = gap_days(index, draft.offset_days)?;
        let (key, is_new) = moment_key(current, &mut seen, draft.key.as_deref())?;
        prepared.push(PreparedMoment {
            key,
            is_new,
            label,
            offset_days,
            subject,
            body,
            revision: draft.revision,
        });
    }
    Ok(prepared)
}

fn gap_days(index: usize, days: i64) -> Result<i64, AppError> {
    if index == 0 {
        if days == 0 {
            Ok(0)
        } else {
            Err(FollowUpError::FirstMomentIsTheDay.into())
        }
    } else if (0..=MAX_GAP_DAYS).contains(&days) {
        Ok(days)
    } else {
        Err(FollowUpError::GapNotANumber.into())
    }
}

fn moment_key(
    current: &[row::ProspectPhrase],
    seen: &mut HashSet<String>,
    requested: Option<&str>,
) -> Result<(String, bool), AppError> {
    let Some(key) = requested.map(str::trim).filter(|key| !key.is_empty()) else {
        return Ok((uuid::Uuid::now_v7().to_string(), true));
    };
    if !seen.insert(key.to_string()) {
        return Err(FollowUpError::DuplicateMoment.into());
    }
    if current.iter().any(|phrase| phrase.key == key) {
        Ok((key.to_string(), false))
    } else {
        Err(FollowUpError::UnknownPhrase.into())
    }
}

fn series_matches(current: &[row::ProspectPhrase], prepared: &[PreparedMoment]) -> bool {
    current.len() == prepared.len()
        && current.iter().zip(prepared).all(|(row, want)| {
            !want.is_new
                && row.key == want.key
                && row.label == want.label
                && row.offset_days == want.offset_days
                && row.subject == want.subject
                && row.body == want.body
        })
}

fn structure_changed(current: &[row::ProspectPhrase], prepared: &[PreparedMoment]) -> bool {
    current.len() != prepared.len()
        || current.iter().zip(prepared).any(|(row, want)| {
            want.is_new || row.key != want.key || row.offset_days != want.offset_days
        })
}

fn refuse_stale(
    current: &[row::ProspectPhrase],
    prepared: &[PreparedMoment],
) -> Result<(), AppError> {
    for want in prepared {
        if want.is_new {
            continue;
        }
        let Some(row) = current.iter().find(|phrase| phrase.key == want.key) else {
            return Err(FollowUpError::UnknownPhrase.into());
        };
        let same_words =
            row.label == want.label && row.subject == want.subject && row.body == want.body;
        if !same_words && want.revision != Some(row.revision) {
            return Err(FollowUpError::StalePhrases.into());
        }
    }
    Ok(())
}

fn phrases_to_write(
    current: &[row::ProspectPhrase],
    prepared: &[PreparedMoment],
) -> Result<Vec<row::ProspectPhrase>, AppError> {
    let mut written = Vec::with_capacity(prepared.len());
    for (index, want) in prepared.iter().enumerate() {
        let Ok(position) = i64::try_from(index) else {
            return Err(FollowUpError::GapNotANumber.into());
        };
        let revision = current
            .iter()
            .find(|phrase| phrase.key == want.key)
            .map_or(1, |row| {
                let changed = row.label != want.label
                    || row.subject != want.subject
                    || row.body != want.body
                    || row.offset_days != want.offset_days;
                if changed {
                    row.revision + 1
                } else {
                    row.revision
                }
            });
        written.push(row::ProspectPhrase {
            key: want.key.clone(),
            position,
            label: want.label.clone(),
            offset_days: want.offset_days,
            subject: want.subject.clone(),
            body: want.body.clone(),
            revision,
        });
    }
    Ok(written)
}

fn pin_open_cycles(conn: &Connection) -> Result<(), AppError> {
    let living = row::list_phrases(conn)?;
    for opportunity in crate::prospection::list_open_opportunities(conn)? {
        let subject = FollowUpSubject::Opportunity(opportunity.id);
        let events = row::events_for(conn, subject)?;
        let cycle = crate::domain::entered_cycle_key(&events);
        row::copy_series(conn, opportunity.id, &cycle, &living)?;
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrepareFollowUp {
    pub subject: FollowUpSubject,
    #[serde(with = "crate::domain::serde_date::date")]
    pub today: Date,
    /// Sujet libre. Absent ou vide : le modèle de cadence.
    #[serde(default)]
    pub subject_line: Option<String>,
    /// Corps libre. Absent ou vide : le modèle de cadence.
    #[serde(default)]
    pub body: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreparedFollowUp {
    pub event_id: FollowUpEventId,
    pub draft: EmlDraft,
    pub to_email: String,
    pub subject_line: String,
}

impl Command for PrepareFollowUp {
    type Output = PreparedFollowUp;
    const NAME: &'static str = "follow_up.prepare";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let loaded = load_subject(conn, self.subject, self.today)?;
        if loaded.cursor.exhausted {
            return Err(FollowUpError::Exhausted.into());
        }
        let step = loaded.cursor.step.clone().ok_or(FollowUpError::Exhausted)?;
        let (from_name, from_email) = sender(conn)?;
        let (to_name, to_email) = recipient(&loaded)?;
        let ctx = loaded.template_context(conn)?;
        let subject_line = optional_letter_part(self.subject_line.clone())
            .unwrap_or_else(|| render_template(&step.subject, &ctx));
        let body = optional_letter_part(self.body.clone())
            .unwrap_or_else(|| render_template(&step.body, &ctx));
        let event_id = FollowUpEventId::new();
        let at = OffsetDateTime::now_utc();
        let draft = render_eml(
            (&from_name, &from_email),
            (&to_name, &to_email),
            &subject_line,
            &body,
            at,
            &format!("ff-{event_id}"),
        );
        let event = FollowUpEvent {
            id: event_id,
            subject: self.subject,
            fact: FollowUpFact::DraftPrepared,
            at,
            until: None,
            rendered_subject: Some(subject_line.clone()),
            rendered_body: Some(body),
            retracts: None,
            interaction_id: None,
        };
        row::insert_event(conn, &event)?;
        Ok(PreparedFollowUp {
            event_id,
            draft,
            to_email,
            subject_line,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarkFollowUpSent {
    pub subject: FollowUpSubject,
    #[serde(with = "crate::domain::serde_date::date")]
    pub today: Date,
    /// Rectification au classement. Absent : le dernier brouillon.
    #[serde(default)]
    pub subject_line: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
}

impl Command for MarkFollowUpSent {
    type Output = FollowUpCard;
    const NAME: &'static str = "follow_up.mark_sent";

    fn requires_confirmation(&self) -> bool {
        true
    }

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        append_advancing(
            conn,
            self.subject,
            self.today,
            FollowUpFact::MarkedSent,
            optional_letter_part(self.subject_line.clone()),
            optional_letter_part(self.body.clone()),
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkipFollowUpStep {
    pub subject: FollowUpSubject,
    #[serde(with = "crate::domain::serde_date::date")]
    pub today: Date,
}

impl Command for SkipFollowUpStep {
    type Output = FollowUpCard;
    const NAME: &'static str = "follow_up.skip";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        append_advancing(
            conn,
            self.subject,
            self.today,
            FollowUpFact::StepSkipped,
            None,
            None,
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnoozeFollowUp {
    pub subject: FollowUpSubject,
    #[serde(with = "crate::domain::serde_date::date")]
    pub until: Date,
    #[serde(with = "crate::domain::serde_date::date")]
    pub today: Date,
}

impl Command for SnoozeFollowUp {
    type Output = FollowUpCard;
    const NAME: &'static str = "follow_up.snooze";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        if self.until <= self.today {
            return Err(FollowUpError::DateNotInFuture.into());
        }
        append_override(
            conn,
            self.subject,
            self.today,
            FollowUpFact::Snoozed,
            self.until,
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetFollowUpDate {
    pub subject: FollowUpSubject,
    #[serde(with = "crate::domain::serde_date::date")]
    pub on: Date,
    #[serde(with = "crate::domain::serde_date::date")]
    pub today: Date,
}

impl Command for SetFollowUpDate {
    type Output = FollowUpCard;
    const NAME: &'static str = "follow_up.set_date";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        if self.on < self.today {
            return Err(FollowUpError::DateNotInFuture.into());
        }
        append_override(
            conn,
            self.subject,
            self.today,
            FollowUpFact::DateSet,
            self.on,
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetractLastFollowUp {
    pub subject: FollowUpSubject,
    #[serde(with = "crate::domain::serde_date::date")]
    pub today: Date,
}

impl Command for RetractLastFollowUp {
    type Output = FollowUpCard;
    const NAME: &'static str = "follow_up.retract";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let loaded = load_subject(conn, self.subject, self.today)?;
        let last = domain::active_events(&loaded.events)
            .into_iter()
            .next_back()
            .ok_or(FollowUpError::NothingToRetract)?;
        let retracts = last.id;
        let interaction_id = last.interaction_id;
        let event = FollowUpEvent {
            id: FollowUpEventId::new(),
            subject: self.subject,
            fact: FollowUpFact::Retracted,
            at: OffsetDateTime::now_utc(),
            until: None,
            rendered_subject: None,
            rendered_body: None,
            retracts: Some(retracts),
            interaction_id: None,
        };
        row::insert_event(conn, &event)?;
        if let Some(id) = interaction_id
            && let FollowUpSubject::Opportunity(_) = self.subject
        {
            conn.execute("DELETE FROM interactions WHERE id = ?1", [id.to_string()])?;
        }
        project_opportunity(conn, self.subject, self.today)?;
        card_for(conn, self.subject, self.today)
    }
}

/// Midi du jour du coffre, ou une seconde après le dernier fait s'il est déjà plus tard.
/// Ainsi une reprise posée le jour d'une lettre reste après elle, et la lettre suivante aussi.
pub(crate) fn stamp_after(today: Date, events: &[FollowUpEvent]) -> OffsetDateTime {
    let noon = today.with_hms(12, 0, 0).map_or_else(
        |_| OffsetDateTime::now_utc(),
        time::PrimitiveDateTime::assume_utc,
    );
    // Le brouillon est horodaté à l'horloge de la machine. On ne suit que les faits
    // du jour du coffre, pour ne pas décaler la cadence quand les deux divergent.
    let on_this_day = events
        .iter()
        .map(|event| event.at)
        .filter(|at| at.date() == today);
    match on_this_day.max() {
        Some(last) if last >= noon => {
            let next = last + time::Duration::seconds(1);
            if next.date() == today { next } else { last }
        }
        _ => noon,
    }
}

fn optional_letter_part(value: Option<String>) -> Option<String> {
    value.filter(|s| !s.trim().is_empty())
}

fn last_draft_letter(
    events: &[FollowUpEvent],
    cursor: &domain::FollowUpCursor,
) -> Option<(String, String)> {
    if !cursor.drafted {
        return None;
    }
    domain::active_events(events)
        .into_iter()
        .rev()
        .find(|e| e.fact == FollowUpFact::DraftPrepared)
        .and_then(|e| Some((e.rendered_subject.clone()?, e.rendered_body.clone()?)))
}

fn append_advancing(
    conn: &Connection,
    subject: FollowUpSubject,
    today: Date,
    fact: FollowUpFact,
    subject_override: Option<String>,
    body_override: Option<String>,
) -> Result<FollowUpCard, AppError> {
    let loaded = load_subject(conn, subject, today)?;
    if loaded.cursor.exhausted {
        return Err(FollowUpError::Exhausted.into());
    }
    let step = loaded.cursor.step.clone();
    let mut interaction_id = None;
    if fact == FollowUpFact::MarkedSent
        && let FollowUpSubject::Opportunity(oid) = subject
    {
        let note = step.map_or_else(|| "lettre".to_string(), |s| s.label.clone());
        let id = LogInteraction {
            opportunity_id: oid,
            kind: InteractionKind::Email,
            note,
            occurred_at: None,
        }
        .apply(conn)?;
        interaction_id = Some(id);
    }
    let (rendered_subject, rendered_body) = if fact == FollowUpFact::MarkedSent {
        let draft = last_draft_letter(&loaded.events, &loaded.cursor);
        (
            subject_override.or_else(|| draft.as_ref().map(|(s, _)| s.clone())),
            body_override.or_else(|| draft.as_ref().map(|(_, b)| b.clone())),
        )
    } else {
        (None, None)
    };
    let event = FollowUpEvent {
        id: FollowUpEventId::new(),
        subject,
        fact,
        at: stamp_after(today, &loaded.events),
        until: None,
        rendered_subject,
        rendered_body,
        retracts: None,
        interaction_id,
    };
    row::insert_event(conn, &event)?;
    project_opportunity(conn, subject, today)?;
    card_for(conn, subject, today)
}

fn append_override(
    conn: &Connection,
    subject: FollowUpSubject,
    today: Date,
    fact: FollowUpFact,
    until: Date,
) -> Result<FollowUpCard, AppError> {
    let _loaded = load_subject(conn, subject, today)?;
    let event = FollowUpEvent {
        id: FollowUpEventId::new(),
        subject,
        fact,
        at: OffsetDateTime::now_utc(),
        until: Some(until),
        rendered_subject: None,
        rendered_body: None,
        retracts: None,
        interaction_id: None,
    };
    row::insert_event(conn, &event)?;
    project_opportunity(conn, subject, today)?;
    card_for(conn, subject, today)
}

fn project_opportunity(
    conn: &Connection,
    subject: FollowUpSubject,
    today: Date,
) -> Result<(), AppError> {
    let FollowUpSubject::Opportunity(id) = subject else {
        return Ok(());
    };
    let opportunity = opportunity_by_id(conn, id)?.ok_or(FollowUpError::OpportunityInactive)?;
    let events = row::events_for(conn, subject)?;
    let anchor = opportunity.next_action_at.unwrap_or(today);
    let steps = row::steps_for_opportunity(conn, id, &events)?;
    let cursor = derive_cursor_with(FollowUpKind::Prospect, &steps, &events, anchor, today);
    crate::prospection::set_next_action_at(conn, id, cursor.due_on)?;
    Ok(())
}

fn sender(conn: &Connection) -> Result<(String, String), AppError> {
    let settings = row::settings(conn)?;
    let email = settings.sender_email.ok_or(FollowUpError::SenderMissing)?;
    let profile = company_profile(conn)?;
    let name = settings
        .sender_name
        .or_else(|| profile.as_ref().and_then(|p| p.president_name.clone()))
        .or_else(|| profile.map(|p| p.name))
        .unwrap_or_else(|| "moi".to_string());
    Ok((name, email))
}

fn recipient(loaded: &super::queries::LoadedSubject) -> Result<(String, String), FollowUpError> {
    loaded
        .recipient
        .clone()
        .ok_or(FollowUpError::RecipientMissing)
}

impl super::queries::LoadedSubject {
    fn template_context(&self, conn: &Connection) -> Result<TemplateContext, AppError> {
        let profile: Option<CompanyProfile> = company_profile(conn)?;
        let settings = row::settings(conn)?;
        let moi = settings
            .sender_name
            .clone()
            .or_else(|| profile.as_ref().and_then(|p| p.president_name.clone()))
            .or_else(|| profile.as_ref().map(|p| p.name.clone()))
            .unwrap_or_default();
        let societe = profile.map(|p| p.name).unwrap_or_default();
        let (contact, _) = self
            .recipient
            .clone()
            .unwrap_or_else(|| (self.party.clone(), String::new()));
        let mut ctx = TemplateContext {
            sujet: self.title.clone(),
            moi,
            societe,
            facture: self.invoice_number.clone().unwrap_or_default(),
            echeance: self
                .invoice_due_on
                .map(domain::format_date_fr)
                .unwrap_or_default(),
            retard: self.cursor.days_until(self.today).map_or_else(
                || "0".to_string(),
                |d| d.saturating_neg().max(0).to_string(),
            ),
            ..TemplateContext::default()
        };
        ctx = ctx
            .with_names(&contact, &self.party)
            .with_amount(self.amount);
        if let Some(outstanding) = self.outstanding {
            ctx.solde = outstanding.to_string();
        }
        Ok(ctx)
    }
}
