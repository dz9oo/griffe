//! Relances : cadence, journal de faits, rendu de brouillon `.eml`. Aucune IO.

use std::collections::HashSet;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::{Date, Duration, OffsetDateTime, Weekday};

use super::calendar::add_weekdays;

use super::client::Civility;
use super::ids::{FollowUpEventId, InteractionId, InvoiceId, OpportunityId};
use super::money::Money;
use super::serde_date;

/// Famille de relance — deux horloges distinctes (voir [`derive_cursor`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FollowUpKind {
    Prospect,
    Invoice,
}

impl FollowUpKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Prospect => "prospect",
            Self::Invoice => "invoice",
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
#[error("famille de relance inconnue : {0}")]
pub struct UnknownFollowUpKind(pub String);

impl FromStr for FollowUpKind {
    type Err = UnknownFollowUpKind;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "prospect" => Ok(Self::Prospect),
            "invoice" => Ok(Self::Invoice),
            other => Err(UnknownFollowUpKind(other.to_string())),
        }
    }
}

/// Sujet d'une relance : une opportunité ouverte, ou une facture encore due.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum FollowUpSubject {
    Opportunity(OpportunityId),
    Invoice(InvoiceId),
}

impl FollowUpSubject {
    #[must_use]
    pub const fn kind(self) -> FollowUpKind {
        match self {
            Self::Opportunity(_) => FollowUpKind::Prospect,
            Self::Invoice(_) => FollowUpKind::Invoice,
        }
    }
}

impl fmt::Display for FollowUpSubject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Opportunity(id) => write!(f, "opportunité {id}"),
            Self::Invoice(id) => write!(f, "facture {id}"),
        }
    }
}

/// Fait du journal. Seuls [`Self::MarkedSent`] et [`Self::StepSkipped`] avancent l'étape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FollowUpFact {
    DraftPrepared,
    MarkedSent,
    StepSkipped,
    Snoozed,
    DateSet,
    Retracted,
    /// Frontière de reprise : les faits d'avant restent dans l'historique, la cadence repart.
    CycleOpened,
}

impl FollowUpFact {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DraftPrepared => "draft_prepared",
            Self::MarkedSent => "marked_sent",
            Self::StepSkipped => "step_skipped",
            Self::Snoozed => "snoozed",
            Self::DateSet => "date_set",
            Self::Retracted => "retracted",
            Self::CycleOpened => "cycle_opened",
        }
    }

    #[must_use]
    pub const fn advances(self) -> bool {
        matches!(self, Self::MarkedSent | Self::StepSkipped)
    }

    #[must_use]
    pub const fn overrides_due(self) -> bool {
        matches!(self, Self::Snoozed | Self::DateSet)
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
#[error("fait de relance inconnu : {0}")]
pub struct UnknownFollowUpFact(pub String);

impl FromStr for FollowUpFact {
    type Err = UnknownFollowUpFact;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "draft_prepared" => Ok(Self::DraftPrepared),
            "marked_sent" => Ok(Self::MarkedSent),
            "step_skipped" => Ok(Self::StepSkipped),
            "snoozed" => Ok(Self::Snoozed),
            "date_set" => Ok(Self::DateSet),
            "retracted" => Ok(Self::Retracted),
            "cycle_opened" => Ok(Self::CycleOpened),
            other => Err(UnknownFollowUpFact(other.to_string())),
        }
    }
}

/// Une étape de cadence : écart en jours et modèles de message.
///
/// Pour une conversation, l'écart compte les jours du lundi au vendredi.
/// Pour une facture, il compte les jours de calendrier depuis l'échéance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CadenceStep {
    pub key: &'static str,
    pub offset_days: i64,
    pub label: &'static str,
    pub subject: &'static str,
    pub body: &'static str,
}

impl CadenceStep {
    #[must_use]
    pub const fn cadence(kind: FollowUpKind) -> &'static [Self] {
        match kind {
            FollowUpKind::Prospect => &PROSPECT_CADENCE,
            FollowUpKind::Invoice => &INVOICE_CADENCE,
        }
    }
}

/// Premier message dès la date déjà posée, puis +3 / +7 / +14 jours ouvrés
/// (du lundi au vendredi) depuis le dernier geste.
pub const PROSPECT_CADENCE: [CadenceStep; 4] = [
    CadenceStep {
        key: "hello",
        offset_days: 0,
        label: "Premier message",
        subject: "{{sujet}}",
        body: "Bonjour {{prenom}},\n\nJe me permets de revenir vers vous au sujet de {{sujet}}.\n\nAuriez-vous un créneau cette semaine pour en parler ?\n\n{{signature}}\n",
    },
    CadenceStep {
        key: "bump",
        offset_days: 3,
        label: "Petit rappel",
        subject: "{{sujet}} — je me permets un rappel",
        body: "Bonjour {{prenom}},\n\nJe me permets un court rappel au sujet de {{sujet}}.\n\nDites-moi simplement si le moment n'est pas le bon.\n\n{{signature}}\n",
    },
    CadenceStep {
        key: "value",
        offset_days: 7,
        label: "Relance utile",
        subject: "{{sujet}}",
        body: "Bonjour {{prenom}},\n\nJe reviens vers vous concernant {{sujet}} ({{montant}}).\n\nJe reste disponible pour un échange de quinze minutes, à votre convenance.\n\n{{signature}}\n",
    },
    CadenceStep {
        key: "close",
        offset_days: 14,
        label: "Dernier mot",
        subject: "{{sujet}} — je clos le dossier de mon côté ?",
        body: "Bonjour {{prenom}},\n\nJe n'ai pas eu de retour au sujet de {{sujet}}.\n\nSi le timing n'est pas le bon, dites-le-moi et je referme le dossier sans insister.\n\n{{signature}}\n",
    },
];

/// J+0 (échéance), J+7, J+15, J+30 — depuis la date d'échéance de la facture.
pub const INVOICE_CADENCE: [CadenceStep; 4] = [
    CadenceStep {
        key: "due",
        offset_days: 0,
        label: "Échéance",
        subject: "Facture {{facture}} — échéance le {{echeance}}",
        body: "Bonjour {{prenom}},\n\nLa facture {{facture}} ({{solde}}) arrive à échéance le {{echeance}}.\n\nMerci de me confirmer la mise en paiement, ou de me signaler si un document manque.\n\n{{signature}}\n",
    },
    CadenceStep {
        key: "gentle",
        offset_days: 7,
        label: "Premier rappel",
        subject: "Facture {{facture}} — {{solde}} en attente",
        body: "Bonjour {{prenom}},\n\nLa facture {{facture}} ({{solde}}), échue le {{echeance}}, ne m'est pas encore parvenue.\n\nPouvez-vous me confirmer la date de virement ?\n\n{{signature}}\n",
    },
    CadenceStep {
        key: "firm",
        offset_days: 15,
        label: "Deuxième rappel",
        subject: "Relance — facture {{facture}} ({{solde}})",
        body: "Bonjour {{prenom}},\n\nSauf erreur de ma part, la facture {{facture}} ({{solde}}) reste due depuis le {{echeance}} (J+{{retard}}).\n\nMerci de régulariser dès que possible, ou de me dire si un point bloque.\n\n{{signature}}\n",
    },
    CadenceStep {
        key: "last",
        offset_days: 30,
        label: "Dernière relance",
        subject: "Dernière relance — facture {{facture}}",
        body: "Bonjour {{prenom}},\n\nLa facture {{facture}} ({{solde}}) est échue depuis le {{echeance}}.\n\nSans règlement ou échange de votre part, je considérerai ce dossier comme à traiter en priorité.\n\n{{signature}}\n",
    },
];

/// Entrée du journal (pure). L'adaptateur n'écrit que ces faits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FollowUpEvent {
    pub id: FollowUpEventId,
    pub subject: FollowUpSubject,
    pub fact: FollowUpFact,
    #[serde(with = "serde_date::datetime")]
    pub at: OffsetDateTime,
    #[serde(default, with = "serde_date::date::option")]
    pub until: Option<Date>,
    pub rendered_subject: Option<String>,
    pub rendered_body: Option<String>,
    pub retracts: Option<FollowUpEventId>,
    pub interaction_id: Option<InteractionId>,
}

/// Un moment de cadence, possédé : les constantes, ou une phrase lue dans le coffre.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhraseStep {
    pub key: String,
    pub offset_days: i64,
    pub label: String,
    pub subject: String,
    pub body: String,
}

impl From<&CadenceStep> for PhraseStep {
    fn from(step: &CadenceStep) -> Self {
        Self {
            key: step.key.to_string(),
            offset_days: step.offset_days,
            label: step.label.to_string(),
            subject: step.subject.to_string(),
            body: step.body.to_string(),
        }
    }
}

impl FollowUpKind {
    /// La série compilée. La prospection affichée vient du coffre ; la facture reste ici.
    #[must_use]
    pub fn steps(self) -> Vec<PhraseStep> {
        CadenceStep::cadence(self)
            .iter()
            .map(PhraseStep::from)
            .collect()
    }
}

/// Position dérivée dans la cadence, à une date `today` donnée.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FollowUpCursor {
    pub position: usize,
    pub step: Option<PhraseStep>,
    pub due_on: Option<Date>,
    pub drafted: bool,
    pub exhausted: bool,
}

impl FollowUpCursor {
    /// Jours jusqu'à l'échéance : négatif = en retard. `None` si pas d'échéance.
    #[must_use]
    pub fn days_until(&self, today: Date) -> Option<i64> {
        self.due_on.map(|due| (due - today).whole_days())
    }
}

/// Les faits encore « vrais » : on ignore les rétractations et les faits qu'elles visent.
#[must_use]
pub fn active_events(events: &[FollowUpEvent]) -> Vec<&FollowUpEvent> {
    let retracted: HashSet<FollowUpEventId> = events
        .iter()
        .filter(|e| e.fact == FollowUpFact::Retracted)
        .filter_map(|e| e.retracts)
        .collect();
    let mut active: Vec<&FollowUpEvent> = events
        .iter()
        .filter(|e| e.fact != FollowUpFact::Retracted && !retracted.contains(&e.id))
        .collect();
    active.sort_by(|a, b| a.at.cmp(&b.at).then(a.id.as_uuid().cmp(&b.id.as_uuid())));
    active
}

/// Cycle sur lequel la conversation est entrée.
///
/// Chaîne vide : le cycle d'origine, avant toute reprise. Sinon l'identifiant
/// du dernier `cycle_opened` encore vrai.
#[must_use]
pub fn entered_cycle_key(events: &[FollowUpEvent]) -> String {
    active_events(events)
        .iter()
        .rev()
        .find(|event| event.fact == FollowUpFact::CycleOpened)
        .map(|event| event.id.to_string())
        .unwrap_or_default()
}

/// Dérive où l'on en est sur la série compilée du genre.
///
/// La fenêtre de prospection passe par [`derive_cursor_with`] et les phrases du coffre.
#[must_use]
pub fn derive_cursor(
    kind: FollowUpKind,
    events: &[FollowUpEvent],
    anchor: Date,
    today: Date,
) -> FollowUpCursor {
    derive_cursor_with(kind, &kind.steps(), events, anchor, today)
}

/// Dérive où l'on en est. `anchor` = `next_action_at` (prospect) ou `due_on` (facture).
/// `steps` est la série de cette conversation : les phrases du coffre, ou la cadence facture.
///
/// Après un envoi ou un saut, l'écart d'une conversation avance en jours du lundi au
/// vendredi. L'écart d'une facture reste un nombre de jours de calendrier. Un report
/// ou une date posée à la main l'emporte sur les deux.
#[must_use]
pub fn derive_cursor_with(
    kind: FollowUpKind,
    cadence: &[PhraseStep],
    events: &[FollowUpEvent],
    anchor: Date,
    _today: Date,
) -> FollowUpCursor {
    let active_all = active_events(events);
    // Une reprise garde les lettres d'avant et remet le compteur à zéro.
    let active: Vec<&FollowUpEvent> = match active_all
        .iter()
        .rposition(|event| event.fact == FollowUpFact::CycleOpened)
    {
        Some(idx) => active_all[idx + 1..].to_vec(),
        None => active_all,
    };
    let position = active.iter().filter(|e| e.fact.advances()).count();
    let exhausted = position >= cadence.len();
    let step = cadence.get(position).cloned();

    let last_advancing = active.iter().rev().find(|e| e.fact.advances()).copied();
    let last_override = active
        .iter()
        .rev()
        .find(|e| e.fact.overrides_due() && last_advancing.is_none_or(|adv| e.at >= adv.at))
        .copied();

    let due_on = if let Some(over) = last_override {
        over.until
    } else if exhausted {
        None
    } else if kind == FollowUpKind::Invoice {
        Some(anchor.saturating_add(Duration::days(cadence[position].offset_days)))
    } else if let Some(adv) = last_advancing {
        Some(add_weekdays(adv.at.date(), cadence[position].offset_days))
    } else {
        Some(anchor)
    };

    let drafted = active.iter().rev().any(|e| {
        e.fact == FollowUpFact::DraftPrepared && last_advancing.is_none_or(|adv| e.at >= adv.at)
    });

    FollowUpCursor {
        position,
        step,
        due_on,
        drafted,
        exhausted,
    }
}

/// Corps d'un moment ajouté. La fin est `<signature>`.
pub const NEW_MOMENT_BODY: &str = "Bonjour {{prenom}},\n\n{{sujet}}\n\n{{signature}}\n";

/// Contexte de substitution des modèles — toutes les clés `{{…}}` connues.
#[derive(Debug, Clone, Default)]
pub struct TemplateContext {
    pub prenom: String,
    /// Nom de famille. Vide : la balise `<nom>` disparaît.
    pub family_name: String,
    /// « Monsieur », « Madame », ou vide.
    pub civility: String,
    pub contact: String,
    pub client: String,
    pub sujet: String,
    pub montant: String,
    /// Constat propre à ce prospect. Vide : la ligne qui ne contient que `{{constat}}` disparaît.
    pub constat: String,
    pub moi: String,
    pub societe: String,
    pub facture: String,
    pub echeance: String,
    pub retard: String,
    pub solde: String,
    /// Texte saisi dans Le courrier. Vide : la formule « Bien à vous ».
    pub signature: String,
}

impl TemplateContext {
    /// Prénom issu de « Qui répond » quand il diffère de « Qui ».
    /// Sinon, premier mot de « Qui » seulement si ce n'est pas une enseigne.
    #[must_use]
    pub fn with_names(mut self, contact: &str, client: &str) -> Self {
        let contact = contact.trim();
        let client = client.trim();
        self.prenom = given_name(contact, client);
        self.contact = if contact.is_empty() {
            client.to_string()
        } else {
            contact.to_string()
        };
        self.client = client.to_string();
        self
    }

    /// Prénom, nom et genre déjà séparés. Un prénom vide retombe sur [`given_name`].
    #[must_use]
    pub fn with_respondent(
        mut self,
        given: &str,
        family: &str,
        civility_label: &str,
        contact: &str,
        client: &str,
    ) -> Self {
        self = self.with_names(contact, client);
        let given = given.trim();
        if !given.is_empty() {
            self.prenom = given.to_string();
        }
        self.family_name = family.trim().to_string();
        self.civility = civility_label.trim().to_string();
        self
    }

    #[must_use]
    pub fn with_amount(mut self, amount: Money) -> Self {
        self.montant = amount.to_string();
        self.solde = amount.to_string();
        self
    }
}

/// Balises de l'éditeur, dans l'ordre des jetons stockés. `<prénom>`, pas `{{prenom}}`.
const EDITOR_WORDS: &[(&str, &str)] = &[
    ("{{prenom}}", "<prénom>"),
    ("{{family_name}}", "<nom>"),
    ("{{civility}}", "<genre>"),
    ("{{contact}}", "<contact>"),
    ("{{client}}", "<client>"),
    ("{{sujet}}", "<sujet>"),
    ("{{montant}}", "<montant>"),
    ("{{constat}}", "<constat>"),
    ("{{moi}}", "<moi>"),
    ("{{societe}}", "<société>"),
    ("{{signature}}", "<signature>"),
];

const ARTICLES: &[&str] = &[
    "le", "la", "les", "l", "un", "une", "du", "des", "au", "aux",
];

const ORG_WORDS: &[&str] = &[
    "sas",
    "sasu",
    "sarl",
    "eurl",
    "sa",
    "sci",
    "earl",
    "gaec",
    "scea",
    "association",
    "mairie",
    "commune",
    "societe",
    "société",
    "entreprise",
    "atelier",
    "boulangerie",
    "restaurant",
    "hotel",
    "hôtel",
    "garage",
    "ferme",
];

const CIVILITIES: &[&str] = &[
    "m",
    "mr",
    "mme",
    "mlle",
    "monsieur",
    "madame",
    "mademoiselle",
    "dr",
    "docteur",
    "me",
    "maitre",
    "maître",
];

/// Prénom d'une lettre. `contact` est « Qui répond », `client` est « Qui ».
/// Une enseigne ne prête pas son premier mot.
#[must_use]
pub fn given_name(contact: &str, client: &str) -> String {
    let contact = contact.trim();
    let client = client.trim();
    let distinct = !contact.is_empty() && !same_name(contact, client);
    if distinct {
        if looks_like_organization(contact) {
            return String::new();
        }
        return first_given(contact);
    }
    let name = if contact.is_empty() { client } else { contact };
    if name.is_empty() || looks_like_organization(name) {
        String::new()
    } else {
        first_given(name)
    }
}

fn same_name(left: &str, right: &str) -> bool {
    fold_name(left) == fold_name(right)
}

fn fold_name(name: &str) -> String {
    name.trim().to_lowercase().replace(['’', '‘', '´'], "'")
}

fn looks_like_organization(name: &str) -> bool {
    let folded = fold_name(name);
    if folded.is_empty() {
        return false;
    }
    let padded = format!(" {folded} ");
    if padded.contains(" du ")
        || padded.contains(" des ")
        || padded.contains(" de la ")
        || padded.contains(" de l'")
    {
        return true;
    }
    let tokens: Vec<String> = folded.split_whitespace().map(token_key).collect();
    let Some(first) = tokens.first() else {
        return false;
    };
    if ARTICLES.contains(&first.as_str()) || first.starts_with("l'") {
        return true;
    }
    tokens
        .iter()
        .any(|token| ORG_WORDS.contains(&token.as_str()))
}

fn token_key(token: &str) -> String {
    token
        .trim_matches(|c: char| {
            matches!(
                c,
                '.' | ',' | ';' | ':' | '!' | '?' | '«' | '»' | '"' | '(' | ')'
            )
        })
        .replace('.', "")
}

fn first_given(name: &str) -> String {
    let mut parts = name.split_whitespace().filter(|part| !part.is_empty());
    let Some(first) = parts.next() else {
        return String::new();
    };
    if is_civility(first) {
        parts.next().map(clean_token).unwrap_or_default()
    } else {
        clean_token(first)
    }
}

fn is_civility(token: &str) -> bool {
    let key = token_key(&fold_name(token));
    CIVILITIES.contains(&key.as_str())
}

fn clean_token(token: &str) -> String {
    token
        .trim_matches(|c: char| matches!(c, ',' | '.'))
        .to_string()
}

/// Prénom, nom et genre lus dans un texte libre (« Qui répond »).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RespondentParts {
    pub civility: Option<Civility>,
    pub given_name: String,
    pub family_name: String,
}

/// Découpe « Madame Camille Rivière », « Jean-Pierre Martin », un seul mot,
/// ou une enseigne. `M.` / `Mr` / `Mme` deviennent Monsieur ou Madame.
/// « Docteur » est sauté, sans être gardé comme genre.
#[must_use]
pub fn split_respondent(name: &str) -> RespondentParts {
    let name = name.trim();
    if name.is_empty() {
        return RespondentParts {
            civility: None,
            given_name: String::new(),
            family_name: String::new(),
        };
    }
    if looks_like_organization(name) {
        return RespondentParts {
            civility: None,
            given_name: String::new(),
            family_name: name.to_string(),
        };
    }
    let tokens: Vec<&str> = name
        .split_whitespace()
        .filter(|part| !part.is_empty())
        .collect();
    let (civility, rest) = take_civility(&tokens);
    match rest {
        [] => RespondentParts {
            civility,
            given_name: String::new(),
            family_name: String::new(),
        },
        [only] => RespondentParts {
            civility,
            given_name: clean_token(only),
            family_name: String::new(),
        },
        [given, family @ ..] => RespondentParts {
            civility,
            given_name: clean_token(given),
            family_name: family
                .iter()
                .copied()
                .map(clean_token)
                .collect::<Vec<_>>()
                .join(" "),
        },
    }
}

fn take_civility<'a>(tokens: &'a [&'a str]) -> (Option<Civility>, &'a [&'a str]) {
    let Some(first) = tokens.first() else {
        return (None, tokens);
    };
    if let Some(stored) = stored_civility(first) {
        return (Some(stored), &tokens[1..]);
    }
    if is_civility(first) {
        return (None, &tokens[1..]);
    }
    (None, tokens)
}

fn stored_civility(token: &str) -> Option<Civility> {
    match token_key(&fold_name(token)).as_str() {
        "m" | "mr" | "monsieur" => Some(Civility::Monsieur),
        "mme" | "madame" => Some(Civility::Madame),
        _ => None,
    }
}

/// Prénom puis nom, sans le genre. Les bords vides sont omis.
#[must_use]
pub fn compose_respondent_name(given: &str, family: &str) -> String {
    [given.trim(), family.trim()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Ce que l'enregistrement retient. `structured` : les parties font foi, même vides.
/// Sinon le texte libre est découpé.
#[must_use]
pub fn settle_respondent(
    name: &str,
    given: &str,
    family: &str,
    civility: Option<Civility>,
    structured: bool,
) -> RespondentParts {
    let (civility, given_name, family_name) = if structured {
        (
            civility,
            given.trim().to_string(),
            family.trim().to_string(),
        )
    } else {
        let parts = split_respondent(name);
        (parts.civility, parts.given_name, parts.family_name)
    };
    RespondentParts {
        civility,
        given_name,
        family_name,
    }
}

/// Nom affiché : les parties jointes, ou le texte de repli s'il n'y en a pas.
#[must_use]
pub fn settled_display_name(parts: &RespondentParts, fallback: &str) -> String {
    let composed = compose_respondent_name(&parts.given_name, &parts.family_name);
    if composed.is_empty() {
        fallback.trim().to_string()
    } else {
        composed
    }
}

/// Parties montrées tant que les colonnes sont vides et que le contact
/// n'est pas le nom de la fiche.
#[must_use]
pub fn respondent_view(
    contact_name: &str,
    client_name: &str,
    given: &str,
    family: &str,
    civility: Option<Civility>,
) -> RespondentParts {
    let stored_empty = given.trim().is_empty() && family.trim().is_empty() && civility.is_none();
    if stored_empty && !same_name(contact_name, client_name) {
        split_respondent(contact_name)
    } else {
        RespondentParts {
            civility,
            given_name: given.trim().to_string(),
            family_name: family.trim().to_string(),
        }
    }
}

/// Où une lettre part : le standard, ou la personne qui répond.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LetterRecipient {
    #[default]
    Switchboard,
    Correspondent,
}

impl LetterRecipient {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Switchboard => "switchboard",
            Self::Correspondent => "correspondent",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "switchboard" => Some(Self::Switchboard),
            "correspondent" => Some(Self::Correspondent),
            _ => None,
        }
    }
}

/// Adresses connues d'une fiche. Seules les adresses lisibles sont retenues.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FicheChannels {
    pub switchboard_email: Option<String>,
    pub correspondent_email: Option<String>,
    pub correspondent_name: String,
    pub client_name: String,
}

#[must_use]
pub fn fiche_channels(
    client_name: &str,
    switchboard_email: Option<&str>,
    correspondent_name: &str,
    correspondent_email: Option<&str>,
) -> FicheChannels {
    FicheChannels {
        switchboard_email: switchboard_email.and_then(|email| parse_email(email).ok()),
        correspondent_email: correspondent_email.and_then(|email| parse_email(email).ok()),
        correspondent_name: correspondent_name.trim().to_string(),
        client_name: client_name.trim().to_string(),
    }
}

/// Le standard s'il est lisible, sinon l'adresse de la personne.
#[must_use]
pub fn default_letter_address(channels: &FicheChannels) -> Option<&str> {
    channels
        .switchboard_email
        .as_deref()
        .filter(|email| !email.is_empty())
        .or_else(|| {
            channels
                .correspondent_email
                .as_deref()
                .filter(|email| !email.is_empty())
        })
}

#[must_use]
pub fn letter_address(channels: &FicheChannels, choice: LetterRecipient) -> Option<&str> {
    let raw = match choice {
        LetterRecipient::Switchboard => channels.switchboard_email.as_deref(),
        LetterRecipient::Correspondent => channels.correspondent_email.as_deref(),
    };
    raw.map(str::trim).filter(|email| !email.is_empty())
}

#[must_use]
pub fn letter_recipient_name(channels: &FicheChannels, choice: LetterRecipient) -> &str {
    match choice {
        LetterRecipient::Switchboard => channels.client_name.as_str(),
        LetterRecipient::Correspondent => {
            let name = channels.correspondent_name.trim();
            if name.is_empty() {
                channels.client_name.as_str()
            } else {
                name
            }
        }
    }
}

/// Deux adresses identiques restent le standard. Seule l'adresse de la
/// personne, différente, désigne le correspondant.
#[must_use]
pub fn infer_letter_recipient(channels: &FicheChannels, to_address: &str) -> LetterRecipient {
    let to = to_address.trim();
    let switchboard = channels.switchboard_email.as_deref() == Some(to);
    let correspondent = channels.correspondent_email.as_deref() == Some(to);
    if correspondent && !switchboard {
        LetterRecipient::Correspondent
    } else {
        LetterRecipient::Switchboard
    }
}

/// Une phrase, du premier moment au dernier. Le premier est le jour du dossier.
#[must_use]
pub fn chronicle(moments: &[(&str, i64)]) -> String {
    let mut parts = Vec::new();
    for (index, (label, offset)) in moments.iter().enumerate() {
        let label = label.trim();
        let label = if label.is_empty() { "Ce moment" } else { label };
        if index == 0 {
            parts.push(format!("{label} le jour du dossier."));
        } else {
            parts.push(format!("{}, {}.", apres(*offset), lower_first(label)));
        }
    }
    parts.join(" ")
}

fn apres(days: i64) -> String {
    let word = match days {
        0 => return "Le même jour".to_string(),
        1 => return "Un jour ouvré après".to_string(),
        2 => "deux",
        3 => "trois",
        4 => "quatre",
        5 => "cinq",
        6 => "six",
        7 => "sept",
        8 => "huit",
        9 => "neuf",
        10 => "dix",
        11 => "onze",
        12 => "douze",
        13 => "treize",
        14 => "quatorze",
        15 => "quinze",
        16 => "seize",
        n => return format!("{n} jours ouvrés après"),
    };
    format!("{} jours ouvrés après", capitalize_word(word))
}

fn capitalize_word(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

fn lower_first(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_lowercase().collect::<String>() + chars.as_str(),
    }
}

/// Ce que la lettre des phrases montre. Les jetons inconnus restent en accolades.
#[must_use]
pub fn phrase_to_editor(stored: &str) -> String {
    let mut out = stored.to_string();
    for (token, word) in EDITOR_WORDS {
        out = out.replace(token, word);
    }
    out
}

/// Ce que le coffre garde. Une phrase déjà en jetons n'est pas modifiée.
#[must_use]
pub fn phrase_from_editor(edited: &str) -> String {
    let mut out = edited.to_string();
    for (token, word) in EDITOR_WORDS {
        out = out.replace(word, token);
    }
    out
}

/// Ce qui ferme la lettre. Un texte saisi reste tel quel. Vide, c'est
/// « Bien à vous », puis le nom, puis la société, chacun s'il est là.
#[must_use]
pub fn resolve_signature(signature: &str, moi: &str, societe: &str) -> String {
    let signature = signature.trim();
    if !signature.is_empty() {
        return signature.to_string();
    }
    let mut lines = vec!["Bien à vous,".to_string()];
    let moi = moi.trim();
    let societe = societe.trim();
    if !moi.is_empty() {
        lines.push(moi.to_string());
    }
    if !societe.is_empty() {
        lines.push(societe.to_string());
    }
    lines.join("\n")
}

/// Remplace les `{{clés}}` connues. Une clé inconnue reste telle quelle.
/// Sans prénom, « Bonjour , » et « Cher , » se referment.
/// `<Monsieur le Maire/Madame la Maire>` suit le genre. Sans genre, les deux
/// côtés restent, séparés par « / ».
/// `<signature>` est remplacée avant ces formes : son texte est relu.
/// `<constat>` vide : la ligne qui ne contient que lui disparaît.
#[must_use]
pub fn render_template(template: &str, ctx: &TemplateContext) -> String {
    let template = drop_empty_constat_lines(template, &ctx.constat);
    let signature = resolve_signature(&ctx.signature, &ctx.moi, &ctx.societe);
    let text = template
        .replace("{{prenom}}", &ctx.prenom)
        .replace("{{family_name}}", &ctx.family_name)
        .replace("{{civility}}", &ctx.civility)
        .replace("{{contact}}", &ctx.contact)
        .replace("{{client}}", &ctx.client)
        .replace("{{sujet}}", &ctx.sujet)
        .replace("{{montant}}", &ctx.montant)
        .replace("{{constat}}", &ctx.constat)
        .replace("{{moi}}", &ctx.moi)
        .replace("{{societe}}", &ctx.societe)
        .replace("{{facture}}", &ctx.facture)
        .replace("{{echeance}}", &ctx.echeance)
        .replace("{{retard}}", &ctx.retard)
        .replace("{{solde}}", &ctx.solde)
        .replace("{{signature}}", &signature);
    let text = apply_gendered_forms(&text, &ctx.civility);
    let text = if ctx.prenom.trim().is_empty() {
        collapse_empty_greeting(&text)
    } else {
        text
    };
    squeeze_line_spaces(&text)
}

/// `<gauche/droite>` : Monsieur prend la gauche, Madame la droite.
/// Une adresse (`://`) ou plus d'une barre reste telle quelle.
fn apply_gendered_forms(text: &str, civility: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        if let Some(end) = after.find('>')
            && let Some(rendered) = gendered_choice(&after[..end], civility)
        {
            out.push_str(&rendered);
            rest = &after[end + 1..];
            continue;
        }
        out.push('<');
        rest = &rest[start + 1..];
    }
    out.push_str(rest);
    out
}

fn gendered_choice(inside: &str, civility: &str) -> Option<String> {
    if inside.contains("://") || inside.matches('/').count() != 1 {
        return None;
    }
    let (left, right) = inside.split_once('/')?;
    let left = left.trim();
    let right = right.trim();
    if left.is_empty() || right.is_empty() {
        return None;
    }
    match civility.trim() {
        "Monsieur" => Some(left.to_string()),
        "Madame" => Some(right.to_string()),
        _ => Some(format!("{left} / {right}")),
    }
}

/// Une ligne qui ne contient que `{{constat}}` disparaît quand le constat est vide.
/// Les lignes vides qui se suivent alors n'en laissent qu'une.
fn drop_empty_constat_lines(template: &str, constat: &str) -> String {
    if !constat.trim().is_empty() {
        return template.to_string();
    }
    let mut skipped = false;
    let mut lines = Vec::new();
    for line in template.split('\n') {
        if line.trim() == "{{constat}}" {
            skipped = true;
            continue;
        }
        lines.push(line);
    }
    if !skipped {
        return template.to_string();
    }
    let mut collapsed = Vec::new();
    let mut previous_blank = false;
    for line in lines {
        let blank = line.trim().is_empty();
        if blank && previous_blank {
            continue;
        }
        previous_blank = blank;
        collapsed.push(line);
    }
    collapsed.join("\n")
}

fn squeeze_line_spaces(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (index, line) in text.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let mut previous_space = false;
        for ch in line.chars() {
            if ch == ' ' || ch == '\t' {
                if !previous_space {
                    out.push(' ');
                }
                previous_space = true;
            } else {
                previous_space = false;
                out.push(ch);
            }
        }
    }
    out
}

fn collapse_empty_greeting(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while !rest.is_empty() {
        if let Some(greeting) = greeting_at(rest) {
            let after = &rest[greeting.len()..];
            let spaces = after
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .count();
            if spaces > 0 && after.as_bytes().get(spaces) == Some(&b',') {
                out.push_str(greeting);
                out.push(',');
                rest = &after[spaces + 1..];
                continue;
            }
        }
        let Some(ch) = rest.chars().next() else {
            break;
        };
        out.push(ch);
        rest = &rest[ch.len_utf8()..];
    }
    out
}

fn greeting_at(text: &str) -> Option<&'static str> {
    ["Bonjour", "Cher"]
        .into_iter()
        .find(|greeting| text.starts_with(greeting))
}

/// Date en français pour le corps d'un mail (« 5 septembre 2026 »).
#[must_use]
pub fn format_date_fr(date: Date) -> String {
    const MONTHS: [&str; 12] = [
        "janvier",
        "février",
        "mars",
        "avril",
        "mai",
        "juin",
        "juillet",
        "août",
        "septembre",
        "octobre",
        "novembre",
        "décembre",
    ];
    let month = MONTHS[usize::from(u8::from(date.month())) - 1];
    format!("{} {month} {}", date.day(), date.year())
}

/// Presets de snooze, à la Superhuman.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnoozePreset {
    Tomorrow,
    NextWeek,
    NextMonday,
}

/// # Panics
///
/// Jamais : `saturating_add` d'un petit nombre de jours reste dans le calendaire.
#[must_use]
pub fn snooze_date(today: Date, preset: SnoozePreset) -> Date {
    match preset {
        SnoozePreset::Tomorrow => today.saturating_add(Duration::days(1)),
        SnoozePreset::NextWeek => today.saturating_add(Duration::days(7)),
        SnoozePreset::NextMonday => {
            let mut d = today.saturating_add(Duration::days(1));
            while d.weekday() != Weekday::Monday {
                d = d.saturating_add(Duration::days(1));
            }
            d
        }
    }
}

/// `days` hors de 1 à 366 inclus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("Le report se compte de 1 à 366 jours.")]
pub struct SnoozeDaysError;

/// Reporte `today` de `days` jours.
///
/// # Errors
///
/// [`SnoozeDaysError`] si `days` est hors de 1 à 366 inclus.
pub fn snooze_in_days(today: Date, days: i64) -> Result<Date, SnoozeDaysError> {
    if !(1..=366).contains(&days) {
        return Err(SnoozeDaysError);
    }
    Ok(today.saturating_add(Duration::days(days)))
}

/// Un brouillon RFC 5322, prêt à écrire sur disque. Pas encore un envoi.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmlDraft {
    pub filename: String,
    pub rfc5322: String,
}

/// Construit un `.eml` de brouillon (`X-Unsent: 1`) en UTF-8.
#[must_use]
pub fn render_eml(
    from: (&str, &str),
    to: (&str, &str),
    subject: &str,
    body: &str,
    date: OffsetDateTime,
    message_id: &str,
) -> EmlDraft {
    let from = format!("{} <{}>", encode_phrase(from.0), from.1);
    let to = format!("{} <{}>", encode_phrase(to.0), to.1);
    let subject_h = encode_unstructured(subject);
    let date_h = format_rfc5322_date(date);
    let body_crlf = body.replace("\r\n", "\n").replace('\n', "\r\n");
    let rfc5322 = format!(
        "From: {from}\r\nTo: {to}\r\nSubject: {subject_h}\r\nDate: {date_h}\r\n\
         Message-ID: <{message_id}>\r\nMIME-Version: 1.0\r\n\
         Content-Type: text/plain; charset=UTF-8\r\n\
         Content-Transfer-Encoding: 8bit\r\nX-Unsent: 1\r\n\r\n{body_crlf}"
    );
    let filename = format!("{}.eml", slug(subject));
    EmlDraft { filename, rfc5322 }
}

fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
        if out.len() > 48 {
            break;
        }
    }
    let slug = out.trim_end_matches('-');
    if slug.is_empty() {
        "relance".to_string()
    } else {
        slug.to_string()
    }
}

fn encode_phrase(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }
    if s.is_ascii()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b' ' || b == b'-' || b == b'.')
    {
        if s.contains(' ') {
            format!("\"{s}\"")
        } else {
            s.to_string()
        }
    } else {
        rfc2047_encode(s)
    }
}

fn encode_unstructured(s: &str) -> String {
    if s.is_ascii() && !s.bytes().any(|b| b < 32 || b == 127) {
        s.to_string()
    } else {
        rfc2047_encode(s)
    }
}

fn rfc2047_encode(s: &str) -> String {
    let mut encoded = String::from("=?UTF-8?Q?");
    for b in s.as_bytes() {
        match b {
            b' ' => encoded.push('_'),
            b if b.is_ascii_alphanumeric() => encoded.push(char::from(*b)),
            other => {
                encoded.push('=');
                encoded.push_str(&hex_byte(*other));
            }
        }
    }
    encoded.push_str("?=");
    encoded
}

fn hex_byte(b: u8) -> String {
    format!("{b:02X}")
}

fn format_rfc5322_date(date: OffsetDateTime) -> String {
    // RFC 5322 : `Sat, 05 Sep 2026 12:00:00 +0000`
    let weekday = match date.weekday() {
        Weekday::Monday => "Mon",
        Weekday::Tuesday => "Tue",
        Weekday::Wednesday => "Wed",
        Weekday::Thursday => "Thu",
        Weekday::Friday => "Fri",
        Weekday::Saturday => "Sat",
        Weekday::Sunday => "Sun",
    };
    let month = match u8::from(date.month()) {
        1 => "Jan",
        2 => "Feb",
        3 => "Mar",
        4 => "Apr",
        5 => "May",
        6 => "Jun",
        7 => "Jul",
        8 => "Aug",
        9 => "Sep",
        10 => "Oct",
        11 => "Nov",
        _ => "Dec",
    };
    let offset = date.offset();
    format!(
        "{weekday}, {:02} {month} {} {:02}:{:02}:{:02} {offset}",
        date.day(),
        date.year(),
        date.hour(),
        date.minute(),
        date.second(),
    )
}

/// Adresse mail minimale : `local@domaine.tld`, sans espace.
///
/// # Errors
pub fn parse_email(s: &str) -> Result<String, InvalidEmail> {
    let trimmed = s.trim();
    let Some((local, domain)) = trimmed.split_once('@') else {
        return Err(InvalidEmail(trimmed.to_string()));
    };
    if local.is_empty()
        || domain.is_empty()
        || trimmed.contains(' ')
        || !domain.contains('.')
        || domain.starts_with('.')
        || domain.ends_with('.')
    {
        return Err(InvalidEmail(trimmed.to_string()));
    }
    Ok(trimmed.to_string())
}

#[derive(Debug, Error, PartialEq, Eq)]
#[error("adresse email illisible : {0}")]
pub struct InvalidEmail(pub String);

impl fmt::Display for FollowUpKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Civility;
    use crate::domain::ids::OpportunityId;
    use time::{Month, UtcOffset};

    fn date(year: i32, month: Month, day: u8) -> Date {
        Date::from_calendar_date(year, month, day).unwrap()
    }

    fn at(year: i32, month: Month, day: u8) -> OffsetDateTime {
        date(year, month, day)
            .with_hms(12, 0, 0)
            .unwrap()
            .assume_offset(UtcOffset::UTC)
    }

    fn phrase(key: &str, offset_days: i64) -> PhraseStep {
        PhraseStep {
            key: key.to_string(),
            offset_days,
            label: key.to_string(),
            subject: "s".to_string(),
            body: "b".to_string(),
        }
    }

    fn event(fact: FollowUpFact, day: u8, until: Option<Date>) -> FollowUpEvent {
        FollowUpEvent {
            id: FollowUpEventId::new(),
            subject: FollowUpSubject::Opportunity(OpportunityId::new()),
            fact,
            at: at(2026, Month::September, day),
            until,
            rendered_subject: None,
            rendered_body: None,
            retracts: None,
            interaction_id: None,
        }
    }

    #[test]
    fn a_fresh_prospect_is_due_on_its_next_action_at() {
        let today = date(2026, Month::September, 5);
        let anchor = date(2026, Month::September, 8);
        let cursor = derive_cursor(FollowUpKind::Prospect, &[], anchor, today);
        assert_eq!(cursor.position, 0);
        assert_eq!(cursor.step.as_ref().map(|s| s.key.as_str()), Some("hello"));
        assert_eq!(cursor.due_on, Some(anchor));
        assert!(!cursor.drafted);
        assert!(!cursor.exhausted);
    }

    #[test]
    fn marking_sent_advances_a_prospect_by_the_next_gap() {
        let today = date(2026, Month::September, 5);
        let anchor = date(2026, Month::September, 2);
        let sent = event(FollowUpFact::MarkedSent, 5, None);
        let cursor = derive_cursor(FollowUpKind::Prospect, &[sent], anchor, today);
        assert_eq!(cursor.position, 1);
        assert_eq!(cursor.step.as_ref().map(|s| s.key.as_str()), Some("bump"));
        // Samedi 5 septembre 2026, écart 3 : lundi 7, mardi 8, mercredi 9.
        assert_eq!(cursor.due_on, Some(date(2026, Month::September, 9)));
    }

    #[test]
    fn a_friday_send_waits_three_weekdays() {
        let today = date(2026, Month::September, 4);
        let anchor = date(2026, Month::September, 1);
        let sent = event(FollowUpFact::MarkedSent, 4, None);
        let cursor = derive_cursor(FollowUpKind::Prospect, &[sent], anchor, today);
        // Vendredi 4 septembre 2026, écart 3 : mercredi 9.
        assert_eq!(cursor.due_on, Some(date(2026, Month::September, 9)));
    }

    #[test]
    fn a_monday_send_keeps_three_weekdays_inside_the_week() {
        let today = date(2026, Month::September, 7);
        let anchor = date(2026, Month::September, 1);
        let sent = event(FollowUpFact::MarkedSent, 7, None);
        let cursor = derive_cursor(FollowUpKind::Prospect, &[sent], anchor, today);
        // Lundi 7 septembre 2026, écart 3 : jeudi 10.
        assert_eq!(cursor.due_on, Some(date(2026, Month::September, 10)));
    }

    #[test]
    fn a_wednesday_send_skips_the_weekend() {
        let today = date(2026, Month::September, 2);
        let anchor = date(2026, Month::September, 1);
        let sent = event(FollowUpFact::MarkedSent, 2, None);
        let cursor = derive_cursor(FollowUpKind::Prospect, &[sent], anchor, today);
        // Mercredi 2 septembre 2026, écart 3 : lundi 7.
        assert_eq!(cursor.due_on, Some(date(2026, Month::September, 7)));
    }

    #[test]
    fn a_zero_gap_stays_on_the_friday_it_was_sent() {
        let today = date(2026, Month::September, 4);
        let friday = date(2026, Month::September, 4);
        let anchor = date(2026, Month::September, 1);
        let steps = [phrase("hello", 0), phrase("same", 0)];
        let sent = event(FollowUpFact::MarkedSent, 4, None);
        let cursor = derive_cursor_with(FollowUpKind::Prospect, &steps, &[sent], anchor, today);
        assert_eq!(cursor.due_on, Some(friday));
    }

    #[test]
    fn snoozing_does_not_advance_and_overrides_the_due_date() {
        let today = date(2026, Month::September, 5);
        let anchor = date(2026, Month::September, 2);
        let until = date(2026, Month::September, 20);
        let events = vec![
            event(FollowUpFact::MarkedSent, 5, None),
            event(FollowUpFact::Snoozed, 6, Some(until)),
        ];
        let cursor = derive_cursor(FollowUpKind::Prospect, &events, anchor, today);
        assert_eq!(cursor.position, 1);
        assert_eq!(cursor.due_on, Some(until));
    }

    #[test]
    fn skip_advances_like_a_send() {
        let today = date(2026, Month::September, 5);
        let anchor = date(2026, Month::September, 2);
        let skip = event(FollowUpFact::StepSkipped, 5, None);
        let cursor = derive_cursor(FollowUpKind::Prospect, &[skip], anchor, today);
        assert_eq!(cursor.position, 1);
        // Samedi 5 septembre 2026, écart 3 : mercredi 9.
        assert_eq!(cursor.due_on, Some(date(2026, Month::September, 9)));
    }

    #[test]
    fn retracting_a_send_restores_the_previous_step() {
        let today = date(2026, Month::September, 5);
        let anchor = date(2026, Month::September, 2);
        let sent = event(FollowUpFact::MarkedSent, 5, None);
        let retract = FollowUpEvent {
            fact: FollowUpFact::Retracted,
            at: at(2026, Month::September, 5).saturating_add(Duration::seconds(1)),
            retracts: Some(sent.id),
            ..event(FollowUpFact::Retracted, 5, None)
        };
        let cursor = derive_cursor(FollowUpKind::Prospect, &[sent, retract], anchor, today);
        assert_eq!(cursor.position, 0);
        assert_eq!(cursor.due_on, Some(anchor));
    }

    #[test]
    fn four_sends_exhaust_the_prospect_cadence() {
        let today = date(2026, Month::September, 30);
        let anchor = date(2026, Month::September, 1);
        let events: Vec<_> = (1..=4)
            .map(|d| event(FollowUpFact::MarkedSent, d, None))
            .collect();
        let cursor = derive_cursor(FollowUpKind::Prospect, &events, anchor, today);
        assert!(cursor.exhausted);
        assert!(cursor.step.is_none());
        assert_eq!(cursor.due_on, None);
    }

    #[test]
    fn invoice_due_is_absolute_from_the_invoice_due_date() {
        let today = date(2026, Month::September, 20);
        let due = date(2026, Month::September, 1);
        let sent = event(FollowUpFact::MarkedSent, 1, None);
        let cursor = derive_cursor(FollowUpKind::Invoice, &[sent], due, today);
        assert_eq!(cursor.position, 1);
        assert_eq!(cursor.due_on, Some(date(2026, Month::September, 8)));
    }

    #[test]
    fn a_draft_on_the_current_step_is_visible() {
        let today = date(2026, Month::September, 5);
        let anchor = date(2026, Month::September, 5);
        let draft = event(FollowUpFact::DraftPrepared, 5, None);
        let cursor = derive_cursor(FollowUpKind::Prospect, &[draft], anchor, today);
        assert!(cursor.drafted);
        assert_eq!(cursor.position, 0);
    }

    #[test]
    fn editor_words_roundtrip_to_the_stored_tokens() {
        let stored = "Bonjour {{prenom}},\n\n{{sujet}} ({{montant}}).\n{{moi}}\n{{societe}}\n";
        let shown = phrase_to_editor(stored);
        assert!(shown.contains("<prénom>"));
        assert!(shown.contains("<sujet>"));
        assert!(shown.contains("<montant>"));
        assert!(shown.contains("<moi>"));
        assert!(shown.contains("<société>"));
        assert!(!shown.contains("{{"));
        assert!(!shown.contains("« le prénom »"));
        assert_eq!(phrase_from_editor(&shown), stored);
        assert_eq!(phrase_from_editor(stored), stored);
    }

    #[test]
    fn ordinary_words_are_not_turned_into_tokens() {
        let prose = "Je parle du prénom, du sujet, et de la société.";
        assert_eq!(phrase_from_editor(prose), prose);
        assert_eq!(
            phrase_from_editor("Bonjour <prénom>, au <sujet>."),
            "Bonjour {{prenom}}, au {{sujet}}."
        );
    }

    #[test]
    fn an_enseigne_does_not_yield_a_first_word() {
        let pig = TemplateContext::default()
            .with_names("Le porc de Val de la Sensée", "Le porc de Val de la Sensée");
        assert_eq!(pig.prenom, "");
        let valley = TemplateContext::default()
            .with_names("Porc du Val de la Sensée", "Porc du Val de la Sensée");
        assert_eq!(valley.prenom, "");
        let firm = TemplateContext::default().with_names("SASU Atelier Nord", "SASU Atelier Nord");
        assert_eq!(firm.prenom, "");
    }

    #[test]
    fn a_person_name_still_yields_the_given_name() {
        let same = TemplateContext::default().with_names("Camille Rivière", "Camille Rivière");
        assert_eq!(same.prenom, "Camille");
        let replies =
            TemplateContext::default().with_names("Madame Camille Rivière", "Mairie d'Aubigny");
        assert_eq!(replies.prenom, "Camille");
        let jean = TemplateContext::default().with_names("Jean", "Le porc de Val de la Sensée");
        assert_eq!(jean.prenom, "Jean");
        let hyphen = TemplateContext::default().with_names("Jean-Pierre Martin", "Acme");
        assert_eq!(hyphen.prenom, "Jean-Pierre");
    }

    #[test]
    fn constat_is_an_editor_token() {
        let stored = "Voir {{constat}}\n";
        let shown = phrase_to_editor(stored);
        assert!(shown.contains("<constat>"));
        assert!(!shown.contains("{{"));
        assert_eq!(phrase_from_editor(&shown), stored);
    }

    #[test]
    fn an_empty_constat_drops_the_line_that_holds_only_it() {
        let empty = render_template(
            "Bonjour,\n\n{{constat}}\n\nBien à vous,\n",
            &TemplateContext::default(),
        );
        assert_eq!(empty, "Bonjour,\n\nBien à vous,\n");
        let inline = render_template("Voir {{constat}} ici.\n", &TemplateContext::default());
        assert_eq!(inline, "Voir ici.\n");
        let filled = render_template(
            "Bonjour,\n\n{{constat}}\n\nBien à vous,\n",
            &TemplateContext {
                constat: "Le site ne déclare pas les cookies.".into(),
                ..TemplateContext::default()
            },
        );
        assert_eq!(
            filled,
            "Bonjour,\n\nLe site ne déclare pas les cookies.\n\nBien à vous,\n"
        );
    }

    #[test]
    fn an_empty_given_name_closes_the_greeting() {
        let text = render_template(
            "Bonjour {{prenom}},\n\nCher {{prenom}},\n",
            &TemplateContext {
                sujet: "la refonte".into(),
                ..TemplateContext::default()
            },
        );
        assert_eq!(text, "Bonjour,\n\nCher,\n");
    }

    #[test]
    fn the_chronicle_reads_the_series_in_one_sentence() {
        let sentence = chronicle(&[
            ("Premier message", 0),
            ("Petit rappel", 3),
            ("Relance utile", 7),
            ("Dernier mot", 14),
        ]);
        assert_eq!(
            sentence,
            "Premier message le jour du dossier. Trois jours ouvrés après, petit rappel. Sept jours ouvrés après, relance utile. Quatorze jours ouvrés après, dernier mot."
        );
        assert_eq!(chronicle(&[("Seul", 0)]), "Seul le jour du dossier.");
        assert_eq!(
            chronicle(&[("Premier message", 0), ("Suite", 1), ("Bien plus tard", 21)]),
            "Premier message le jour du dossier. Un jour ouvré après, suite. 21 jours ouvrés après, bien plus tard."
        );
    }

    #[test]
    fn a_written_signature_replaces_the_token_and_an_empty_one_keeps_the_formula() {
        let stored = "Au revoir,\n{{signature}}\n";
        let shown = phrase_to_editor(stored);
        assert!(shown.contains("<signature>"));
        assert!(!shown.contains("{{"));
        assert_eq!(phrase_from_editor(&shown), stored);

        let written = render_template(
            stored,
            &TemplateContext {
                signature: "Nicolas\nAtelier".into(),
                moi: "Autre".into(),
                societe: "Autre société".into(),
                ..TemplateContext::default()
            },
        );
        assert_eq!(written, "Au revoir,\nNicolas\nAtelier\n");
        assert!(!written.contains("Autre"));

        let empty = render_template(
            stored,
            &TemplateContext {
                moi: "Nicolas".into(),
                societe: "Lumen".into(),
                ..TemplateContext::default()
            },
        );
        assert_eq!(empty, "Au revoir,\nBien à vous,\nNicolas\nLumen\n");
        assert_eq!(
            render_template("{{signature}}", &TemplateContext::default()),
            "Bien à vous,"
        );
        assert_eq!(
            render_template(
                "{{signature}}",
                &TemplateContext {
                    moi: "Nicolas".into(),
                    ..TemplateContext::default()
                },
            ),
            "Bien à vous,\nNicolas"
        );
    }

    #[test]
    fn the_compiled_letters_close_with_the_signature_token() {
        for step in PROSPECT_CADENCE.iter().chain(INVOICE_CADENCE.iter()) {
            assert!(step.body.contains("{{signature}}\n"), "{}", step.key);
            assert!(!step.body.contains("Bien à vous"), "{}", step.key);
        }
        assert!(NEW_MOMENT_BODY.contains("{{signature}}\n"));
    }

    #[test]
    fn rendering_replaces_known_placeholders() {
        let ctx = TemplateContext {
            prenom: "Marie".into(),
            sujet: "refonte".into(),
            moi: "Nicolas".into(),
            societe: "Lumen".into(),
            ..TemplateContext::default()
        };
        let text = render_template(PROSPECT_CADENCE[0].body, &ctx);
        assert!(text.contains("Bonjour Marie"));
        assert!(text.contains("refonte"));
        assert!(text.contains("Nicolas"));
        assert!(!text.contains("{{"));
    }

    #[test]
    fn first_name_is_the_first_word() {
        let ctx = TemplateContext::default().with_names("Marie Curie", "Acme");
        assert_eq!(ctx.prenom, "Marie");
        assert_eq!(ctx.contact, "Marie Curie");
    }

    #[test]
    fn snooze_next_monday_is_strictly_after_today() {
        let friday = date(2026, Month::September, 4);
        assert_eq!(
            snooze_date(friday, SnoozePreset::NextMonday),
            date(2026, Month::September, 7)
        );
        let monday = date(2026, Month::September, 7);
        assert_eq!(
            snooze_date(monday, SnoozePreset::NextMonday),
            date(2026, Month::September, 14)
        );
    }

    #[test]
    fn snooze_in_days_counts_from_today_and_refuses_outside_one_to_366() {
        // Dates écrites à la main. 2026 et 2027 ne sont pas bissextiles ; 2028 l'est.
        let today = date(2026, Month::September, 4);
        let counted = [
            (1_i64, date(2026, Month::September, 5)),
            (3, date(2026, Month::September, 7)),
            (10, date(2026, Month::September, 14)),
            (365, date(2027, Month::September, 4)),
            (366, date(2027, Month::September, 5)),
        ];
        for (days, expected) in counted {
            assert_eq!(snooze_in_days(today, days), Ok(expected), "{days}");
        }
        assert_eq!(
            snooze_in_days(date(2026, Month::December, 30), 3),
            Ok(date(2027, Month::January, 2))
        );
        assert_eq!(
            snooze_in_days(date(2027, Month::February, 27), 3),
            Ok(date(2027, Month::March, 2))
        );
        assert_eq!(
            snooze_in_days(date(2028, Month::February, 27), 3),
            Ok(date(2028, Month::March, 1))
        );

        let sentence = "Le report se compte de 1 à 366 jours.";
        for days in [0, -1, 367, i64::MIN, i64::MAX] {
            assert_eq!(snooze_in_days(today, days), Err(SnoozeDaysError), "{days}");
            assert_eq!(SnoozeDaysError.to_string(), sentence);
        }
    }

    #[test]
    fn eml_is_an_unsent_utf8_draft() {
        let date = at(2026, Month::September, 5);
        let eml = render_eml(
            ("Société Lumen", "nicolas@lumen.test"),
            ("Marie Curie", "marie@acme.test"),
            "Refonte plateforme",
            "Bonjour Marie,\n\nDispo ?\n",
            date,
            "ff-test",
        );
        assert_eq!(
            std::path::Path::new(&eml.filename)
                .extension()
                .and_then(|e| e.to_str()),
            Some("eml")
        );
        assert!(eml.rfc5322.contains("X-Unsent: 1"));
        assert!(eml.rfc5322.contains("charset=UTF-8"));
        assert!(eml.rfc5322.contains("marie@acme.test"));
        assert!(eml.rfc5322.contains("=?UTF-8?Q?"));
        assert!(eml.rfc5322.contains("\r\n\r\n"));
    }

    #[test]
    fn parse_email_rejects_spaces_and_missing_at() {
        assert!(parse_email("marie@acme.test").is_ok());
        assert!(parse_email("pas une adresse").is_err());
        assert!(parse_email("a@b").is_err());
    }

    #[test]
    fn french_date_is_readable() {
        assert_eq!(
            format_date_fr(date(2026, Month::September, 5)),
            "5 septembre 2026"
        );
    }

    #[test]
    fn a_free_text_name_splits_into_civility_given_and_family() {
        let madame = split_respondent("Madame Camille Rivière");
        assert_eq!(madame.civility, Some(Civility::Madame));
        assert_eq!(madame.given_name, "Camille");
        assert_eq!(madame.family_name, "Rivière");
        assert_eq!(
            settled_display_name(&madame, "Madame Camille Rivière"),
            "Camille Rivière"
        );

        let hyphen = split_respondent("Jean-Pierre Martin");
        assert_eq!(hyphen.civility, None);
        assert_eq!(hyphen.given_name, "Jean-Pierre");
        assert_eq!(hyphen.family_name, "Martin");

        let alone = split_respondent("Camille");
        assert_eq!(alone.given_name, "Camille");
        assert_eq!(alone.family_name, "");

        let mister = split_respondent("M. Camille Rivière");
        assert_eq!(mister.civility, Some(Civility::Monsieur));
        assert_eq!(mister.given_name, "Camille");
        assert_eq!(
            split_respondent("Mr Camille").civility,
            Some(Civility::Monsieur)
        );
        assert_eq!(
            split_respondent("Mme Rivière").civility,
            Some(Civility::Madame)
        );

        let doctor = split_respondent("Docteur Camille Rivière");
        assert_eq!(doctor.civility, None);
        assert_eq!(doctor.given_name, "Camille");
        assert_eq!(doctor.family_name, "Rivière");
    }

    #[test]
    fn an_enseigne_stays_a_family_name_without_a_given_name() {
        for name in ["Mairie de Paris", "Le porc du Val de la Sensée"] {
            let parts = split_respondent(name);
            assert_eq!(parts.civility, None, "{name}");
            assert_eq!(parts.given_name, "", "{name}");
            assert_eq!(parts.family_name, name, "{name}");
            assert_eq!(settled_display_name(&parts, name), name, "{name}");
        }
    }

    #[test]
    fn structured_parts_win_even_when_they_are_empty() {
        let empty = settle_respondent("Mairie de Paris", "", "", None, true);
        assert_eq!(empty.given_name, "");
        assert_eq!(empty.family_name, "");
        assert_eq!(
            settled_display_name(&empty, "Mairie de Paris"),
            "Mairie de Paris"
        );

        let kept = settle_respondent("ignoré", "Camille", "Rivière", Some(Civility::Madame), true);
        assert_eq!(kept.civility, Some(Civility::Madame));
        assert_eq!(settled_display_name(&kept, "ignoré"), "Camille Rivière");
    }

    #[test]
    fn nom_and_genre_tokens_and_a_gendered_formula_follow_the_civility() {
        let stored = "Bonjour {{civility}} {{family_name}},\n<Monsieur le Maire/Madame la Maire>\n";
        let shown = phrase_to_editor(stored);
        assert!(shown.contains("<genre>"));
        assert!(shown.contains("<nom>"));
        assert_eq!(phrase_from_editor(&shown), stored);

        let monsieur = TemplateContext {
            family_name: "Rivière".into(),
            civility: "Monsieur".into(),
            ..TemplateContext::default()
        };
        assert_eq!(
            render_template("{{civility}} {{family_name}}", &monsieur),
            "Monsieur Rivière"
        );
        assert_eq!(
            render_template("<Monsieur le Maire/Madame la Maire>", &monsieur),
            "Monsieur le Maire"
        );
        let madame = TemplateContext {
            civility: "Madame".into(),
            ..TemplateContext::default()
        };
        assert_eq!(
            render_template("<Monsieur le président/Madame la présidente>", &madame),
            "Madame la présidente"
        );
        let unset = TemplateContext::default();
        assert_eq!(
            render_template("<Monsieur le Maire/Madame la Maire>", &unset),
            "Monsieur le Maire / Madame la Maire"
        );
        assert_eq!(
            render_template(
                "Cher {{prenom}} {{family_name}}",
                &TemplateContext {
                    family_name: "Rivière".into(),
                    ..TemplateContext::default()
                }
            ),
            "Cher Rivière"
        );
        assert_eq!(
            render_template("voir <https://atelier.example/maire>", &monsieur),
            "voir <https://atelier.example/maire>"
        );
    }

    #[test]
    fn a_matching_person_address_is_the_correspondent() {
        let both = fiche_channels(
            "Mairie",
            Some("accueil@mairie.example"),
            "Camille Rivière",
            Some("camille@mairie.example"),
        );
        assert_eq!(
            infer_letter_recipient(&both, "camille@mairie.example"),
            LetterRecipient::Correspondent
        );
        assert_eq!(
            infer_letter_recipient(&both, "accueil@mairie.example"),
            LetterRecipient::Switchboard
        );
        assert_eq!(
            default_letter_address(&both),
            Some("accueil@mairie.example")
        );
        let person_only = fiche_channels("Acme", None, "Marie Curie", Some("marie@acme.test"));
        assert_eq!(
            infer_letter_recipient(&person_only, "marie@acme.test"),
            LetterRecipient::Correspondent
        );
        assert_eq!(
            default_letter_address(&person_only),
            Some("marie@acme.test")
        );
    }

    #[test]
    fn the_entered_cycle_is_the_latest_reopening() {
        let first = event(FollowUpFact::CycleOpened, 2, None);
        let later = event(FollowUpFact::CycleOpened, 9, None);
        let later_id = later.id.to_string();
        assert_eq!(entered_cycle_key(&[first, later]), later_id);
        assert_eq!(entered_cycle_key(&[]), "");
    }
}
