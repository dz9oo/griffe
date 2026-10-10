//! Mutations du journal de relances. Aucune IO fichier : le `.eml` est renvoyé en octets.

use std::collections::HashSet;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use time::{Date, OffsetDateTime};

use crate::app::{AppError, Command};
use crate::company::company_profile;
use crate::domain::{
    self, EmlDraft, FollowUpEvent, FollowUpEventId, FollowUpFact, FollowUpKind, FollowUpSubject,
    InteractionKind, derive_cursor_with, parse_email, phrase_from_editor, render_eml,
    render_template,
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
        // Un moment ajouté reçoit, dans chaque genre, les mots par défaut.
        // Un moment retiré quitte les genres vivants. La copie, elle, le garde.
        row::sync_genre_words(conn)?;
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

fn trimmed_genre_name(raw: &str) -> Result<String, AppError> {
    let name = raw.trim();
    if name.is_empty() {
        Err(FollowUpError::EmptyGenre.into())
    } else {
        Ok(name.to_string())
    }
}

fn blank_id(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|id| !id.is_empty())
}

fn opportunity_subject(subject: FollowUpSubject) -> Result<domain::OpportunityId, AppError> {
    match subject {
        FollowUpSubject::Opportunity(id) => Ok(id),
        FollowUpSubject::Invoice(_) => Err(FollowUpError::NotAConversation.into()),
    }
}

/// Crée un genre en copiant les sujets et les corps. Pas les libellés, pas les écarts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateProspectGenre {
    pub name: String,
    /// Identifiant du genre dont on copie les mots. Absent : les phrases par défaut.
    pub copy_from: Option<String>,
}

impl Command for CreateProspectGenre {
    type Output = row::ProspectGenre;
    const NAME: &'static str = "follow_up.create_genre";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let name = trimmed_genre_name(&self.name)?;
        if row::genre_by_name(conn, &name)?.is_some() {
            return Err(FollowUpError::DuplicateGenre.into());
        }
        let source = blank_id(self.copy_from.as_deref());
        if let Some(source) = source
            && row::genre_by_id(conn, source)?.is_none()
        {
            return Err(FollowUpError::UnknownGenre.into());
        }
        let genre = row::insert_genre(conn, &name)?;
        row::copy_genre_words(conn, &genre.id, source)?;
        Ok(genre)
    }
}

/// Retire un genre. Les fiches qui le portaient redeviennent sans genre.
/// Les lettres classées restent. Le compteur ne repart pas.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DropProspectGenre {
    pub id: String,
}

impl Command for DropProspectGenre {
    type Output = ();
    const NAME: &'static str = "follow_up.drop_genre";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let id = self.id.trim();
        if row::genre_by_id(conn, id)?.is_none() {
            return Err(FollowUpError::UnknownGenre.into());
        }
        row::delete_genre(conn, id)
    }
}

/// Pose un genre sur le dossier, ou l'enlève si le nom est vide.
/// Un nom inconnu crée le genre en copiant les phrases par défaut.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetDossierGenre {
    pub subject: FollowUpSubject,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenreAssignment {
    pub id: Option<String>,
    pub name: Option<String>,
    pub created: bool,
}

impl Command for SetDossierGenre {
    type Output = GenreAssignment;
    const NAME: &'static str = "follow_up.set_genre";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let opportunity_id = opportunity_subject(self.subject)?;
        if opportunity_by_id(conn, opportunity_id)?.is_none() {
            return Err(FollowUpError::UnknownConversation.into());
        }
        let name = self.name.trim();
        if name.is_empty() {
            if !row::set_opportunity_genre(conn, opportunity_id, None)? {
                return Err(FollowUpError::UnknownConversation.into());
            }
            return Ok(GenreAssignment {
                id: None,
                name: None,
                created: false,
            });
        }
        let (genre, created) = if let Some(existing) = row::genre_by_name(conn, name)? {
            (existing, false)
        } else {
            let created = row::insert_genre(conn, name)?;
            row::copy_genre_words(conn, &created.id, None)?;
            (created, true)
        };
        if !row::set_opportunity_genre(conn, opportunity_id, Some(&genre.id))? {
            return Err(FollowUpError::UnknownConversation.into());
        }
        Ok(GenreAssignment {
            id: Some(genre.id),
            name: Some(genre.name),
            created,
        })
    }
}

/// Réécrit le sujet et le corps d'un genre, pour les moments donnés.
/// Ne copie pas de série et ne change pas les phrases par défaut.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenreWordDraft {
    pub key: String,
    pub subject: String,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RewriteGenreWords {
    pub genre_id: String,
    pub words: Vec<GenreWordDraft>,
}

impl Command for RewriteGenreWords {
    type Output = ();
    const NAME: &'static str = "follow_up.rewrite_genre";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        write_genre_words(conn, self.genre_id.trim(), &self.words)?;
        row::touch_genre(conn, self.genre_id.trim())?;
        Ok(())
    }
}

fn write_genre_words(
    conn: &Connection,
    genre_id: &str,
    words: &[GenreWordDraft],
) -> Result<(), AppError> {
    if row::genre_by_id(conn, genre_id)?.is_none() {
        return Err(FollowUpError::UnknownGenre.into());
    }
    let living = row::list_phrases(conn)?;
    for word in words {
        let subject = phrase_from_editor(word.subject.trim());
        let body = phrase_from_editor(&word.body);
        if subject.trim().is_empty() {
            return Err(FollowUpError::EmptySubject.into());
        }
        if body.trim().is_empty() {
            return Err(FollowUpError::EmptyLetter.into());
        }
        if !living.iter().any(|phrase| phrase.key == word.key) {
            return Err(FollowUpError::UnknownPhrase.into());
        }
        row::upsert_genre_word(conn, genre_id, &word.key, &subject, &body)?;
    }
    Ok(())
}

/// Enregistre la lettre d'un genre.
///
/// Le libellé, l'ordre et les écarts sont le procédé commun. Le sujet et le
/// corps ne changent que ce genre. Un moment nouveau prend, pour tout le
/// monde, les mots écrits ici : ce sont les mots par défaut de ce moment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveGenreLetter {
    pub genre_id: String,
    pub moments: Vec<MomentDraft>,
}

impl Command for SaveGenreLetter {
    type Output = ();
    const NAME: &'static str = "follow_up.save_genre";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let genre_id = self.genre_id.trim();
        if row::genre_by_id(conn, genre_id)?.is_none() {
            return Err(FollowUpError::UnknownGenre.into());
        }
        let current = row::list_phrases(conn)?;
        let prepared = prepare_moments(&current, &self.moments)?;
        let structure = structure_drafts(&current, &prepared)?;
        ArrangeProspectPhrases { moments: structure }.apply(conn)?;
        let words = prepared
            .iter()
            .filter(|moment| !moment.is_new)
            .map(|moment| GenreWordDraft {
                key: moment.key.clone(),
                subject: moment.subject.clone(),
                body: moment.body.clone(),
            })
            .collect::<Vec<_>>();
        write_genre_words(conn, genre_id, &words)?;
        row::touch_genre(conn, genre_id)?;
        Ok(())
    }
}

fn structure_drafts(
    current: &[row::ProspectPhrase],
    prepared: &[PreparedMoment],
) -> Result<Vec<MomentDraft>, AppError> {
    let mut drafts = Vec::with_capacity(prepared.len());
    for want in prepared {
        if want.is_new {
            drafts.push(MomentDraft {
                key: None,
                label: want.label.clone(),
                offset_days: want.offset_days,
                subject: want.subject.clone(),
                body: want.body.clone(),
                revision: None,
            });
            continue;
        }
        let Some(row) = current.iter().find(|phrase| phrase.key == want.key) else {
            return Err(FollowUpError::UnknownPhrase.into());
        };
        drafts.push(MomentDraft {
            key: Some(want.key.clone()),
            label: want.label.clone(),
            offset_days: want.offset_days,
            subject: row.subject.clone(),
            body: row.body.clone(),
            revision: want.revision.or(Some(row.revision)),
        });
    }
    Ok(drafts)
}

/// Remplace le sujet et le corps de ce moment, pour ce genre seulement.
///
/// Ne classe rien, n'envoie rien, ne déplace pas le curseur, et ne réécrit
/// ni une lettre déjà classée ni un brouillon déjà préparé.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeepGenreWords {
    pub subject: FollowUpSubject,
    #[serde(with = "crate::domain::serde_date::date")]
    pub today: Date,
    /// Absent : ce champ n'a pas été modifié, on garde le modèle.
    #[serde(default)]
    pub subject_line: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    /// Quand le dossier n'a pas de genre. Un nom nouveau le crée.
    #[serde(default)]
    pub genre_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeptWords {
    pub genre_id: String,
    pub genre_name: String,
    pub key: String,
    pub subject: String,
    pub body: String,
}

impl Command for KeepGenreWords {
    type Output = KeptWords;
    const NAME: &'static str = "follow_up.keep_words";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let opportunity_id = opportunity_subject(self.subject)?;
        let loaded = load_subject(conn, self.subject, self.today)?;
        if loaded.cursor.exhausted {
            return Err(FollowUpError::Exhausted.into());
        }
        let step = loaded.cursor.step.clone().ok_or(FollowUpError::Exhausted)?;
        let living = row::list_phrases(conn)?;
        if !living.iter().any(|phrase| phrase.key == step.key) {
            return Err(FollowUpError::UnknownPhrase.into());
        }
        let ctx = loaded.template_context(conn)?;
        let rendered_subject = render_template(&step.subject, &ctx);
        let rendered_body = render_template(&step.body, &ctx);
        let subject = kept_field(
            self.subject_line.as_deref(),
            &rendered_subject,
            &step.subject,
            FollowUpError::EmptySubject,
        )?;
        let body = kept_field(
            self.body.as_deref(),
            &rendered_body,
            &step.body,
            FollowUpError::EmptyLetter,
        )?;
        if row::genre_of_opportunity(conn, opportunity_id)?.is_none() {
            let Some(name) = self
                .genre_name
                .as_deref()
                .map(str::trim)
                .filter(|n| !n.is_empty())
            else {
                return Err(FollowUpError::GenreRequired.into());
            };
            SetDossierGenre {
                subject: self.subject,
                name: name.to_string(),
            }
            .apply(conn)?;
        }
        let Some(genre) = row::genre_of_opportunity(conn, opportunity_id)? else {
            return Err(FollowUpError::GenreRequired.into());
        };
        row::upsert_genre_word(conn, &genre.id, &step.key, &subject, &body)?;
        row::touch_genre(conn, &genre.id)?;
        Ok(KeptWords {
            genre_id: genre.id,
            genre_name: genre.name,
            key: step.key,
            subject,
            body,
        })
    }
}

fn kept_field(
    submitted: Option<&str>,
    rendered: &str,
    template: &str,
    empty: FollowUpError,
) -> Result<String, AppError> {
    match submitted {
        None => Ok(template.to_string()),
        Some(text) if text == rendered || text.trim() == rendered.trim() => {
            Ok(template.to_string())
        }
        Some(text) => {
            let stored = phrase_from_editor(text);
            if stored.trim().is_empty() {
                Err(empty.into())
            } else {
                Ok(stored)
            }
        }
    }
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
        let body = body_override.or_else(|| draft.as_ref().map(|(_, b)| b.clone()));
        let signature = crate::mail::profile(conn)
            .map(|account| account.signature)
            .unwrap_or_default();
        (
            subject_override.or_else(|| draft.as_ref().map(|(s, _)| s.clone())),
            body.map(|text| crate::mail::close_letter(&text, &signature)),
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
