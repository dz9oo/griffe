//! Relances : cadence, journal de faits, rendu de brouillon `.eml`. Aucune IO.

use std::collections::HashSet;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::{Date, Duration, OffsetDateTime, Weekday};

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

/// Premier message dès la date déjà posée, puis +3 / +7 / +14 depuis le dernier geste.
pub const PROSPECT_CADENCE: [CadenceStep; 4] = [
    CadenceStep {
        key: "hello",
        offset_days: 0,
        label: "Premier message",
        subject: "{{sujet}}",
        body: "Bonjour {{prenom}},\n\nJe me permets de revenir vers vous au sujet de {{sujet}}.\n\nAuriez-vous un créneau cette semaine pour en parler ?\n\nBien à vous,\n{{moi}}\n{{societe}}\n",
    },
    CadenceStep {
        key: "bump",
        offset_days: 3,
        label: "Petit rappel",
        subject: "{{sujet}} — je me permets un rappel",
        body: "Bonjour {{prenom}},\n\nJe me permets un court rappel au sujet de {{sujet}}.\n\nDites-moi simplement si le moment n'est pas le bon.\n\nBien à vous,\n{{moi}}\n",
    },
    CadenceStep {
        key: "value",
        offset_days: 7,
        label: "Relance utile",
        subject: "{{sujet}}",
        body: "Bonjour {{prenom}},\n\nJe reviens vers vous concernant {{sujet}} ({{montant}}).\n\nJe reste disponible pour un échange de quinze minutes, à votre convenance.\n\nBien à vous,\n{{moi}}\n{{societe}}\n",
    },
    CadenceStep {
        key: "close",
        offset_days: 14,
        label: "Dernier mot",
        subject: "{{sujet}} — je clos le dossier de mon côté ?",
        body: "Bonjour {{prenom}},\n\nJe n'ai pas eu de retour au sujet de {{sujet}}.\n\nSi le timing n'est pas le bon, dites-le-moi et je referme le dossier sans insister.\n\nBien à vous,\n{{moi}}\n",
    },
];

/// J+0 (échéance), J+7, J+15, J+30 — depuis la date d'échéance de la facture.
pub const INVOICE_CADENCE: [CadenceStep; 4] = [
    CadenceStep {
        key: "due",
        offset_days: 0,
        label: "Échéance",
        subject: "Facture {{facture}} — échéance le {{echeance}}",
        body: "Bonjour {{prenom}},\n\nLa facture {{facture}} ({{solde}}) arrive à échéance le {{echeance}}.\n\nMerci de me confirmer la mise en paiement, ou de me signaler si un document manque.\n\nBien à vous,\n{{moi}}\n{{societe}}\n",
    },
    CadenceStep {
        key: "gentle",
        offset_days: 7,
        label: "Premier rappel",
        subject: "Facture {{facture}} — {{solde}} en attente",
        body: "Bonjour {{prenom}},\n\nLa facture {{facture}} ({{solde}}), échue le {{echeance}}, ne m'est pas encore parvenue.\n\nPouvez-vous me confirmer la date de virement ?\n\nBien à vous,\n{{moi}}\n{{societe}}\n",
    },
    CadenceStep {
        key: "firm",
        offset_days: 15,
        label: "Deuxième rappel",
        subject: "Relance — facture {{facture}} ({{solde}})",
        body: "Bonjour {{prenom}},\n\nSauf erreur de ma part, la facture {{facture}} ({{solde}}) reste due depuis le {{echeance}} (J+{{retard}}).\n\nMerci de régulariser dès que possible, ou de me dire si un point bloque.\n\nBien à vous,\n{{moi}}\n{{societe}}\n",
    },
    CadenceStep {
        key: "last",
        offset_days: 30,
        label: "Dernière relance",
        subject: "Dernière relance — facture {{facture}}",
        body: "Bonjour {{prenom}},\n\nLa facture {{facture}} ({{solde}}) est échue depuis le {{echeance}}.\n\nSans règlement ou échange de votre part, je considérerai ce dossier comme à traiter en priorité.\n\nBien à vous,\n{{moi}}\n{{societe}}\n",
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
        Some(
            adv.at
                .date()
                .saturating_add(Duration::days(cadence[position].offset_days)),
        )
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

/// Contexte de substitution des modèles — toutes les clés `{{…}}` connues.
#[derive(Debug, Clone, Default)]
pub struct TemplateContext {
    pub prenom: String,
    pub contact: String,
    pub client: String,
    pub sujet: String,
    pub montant: String,
    pub moi: String,
    pub societe: String,
    pub facture: String,
    pub echeance: String,
    pub retard: String,
    pub solde: String,
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
    ("{{contact}}", "<contact>"),
    ("{{client}}", "<client>"),
    ("{{sujet}}", "<sujet>"),
    ("{{montant}}", "<montant>"),
    ("{{moi}}", "<moi>"),
    ("{{societe}}", "<société>"),
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
        1 => return "Un jour après".to_string(),
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
        n => return format!("{n} jours après"),
    };
    format!("{} jours après", capitalize_word(word))
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

/// Remplace les `{{clés}}` connues. Une clé inconnue reste telle quelle.
/// Sans prénom, « Bonjour , » et « Cher , » se referment.
#[must_use]
pub fn render_template(template: &str, ctx: &TemplateContext) -> String {
    let text = template
        .replace("{{prenom}}", &ctx.prenom)
        .replace("{{contact}}", &ctx.contact)
        .replace("{{client}}", &ctx.client)
        .replace("{{sujet}}", &ctx.sujet)
        .replace("{{montant}}", &ctx.montant)
        .replace("{{moi}}", &ctx.moi)
        .replace("{{societe}}", &ctx.societe)
        .replace("{{facture}}", &ctx.facture)
        .replace("{{echeance}}", &ctx.echeance)
        .replace("{{retard}}", &ctx.retard)
        .replace("{{solde}}", &ctx.solde);
    if ctx.prenom.trim().is_empty() {
        collapse_empty_greeting(&text)
    } else {
        text
    }
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
        assert_eq!(cursor.due_on, Some(date(2026, Month::September, 8)));
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
        assert_eq!(cursor.due_on, Some(date(2026, Month::September, 8)));
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
            "Premier message le jour du dossier. Trois jours après, petit rappel. Sept jours après, relance utile. Quatorze jours après, dernier mot."
        );
        assert_eq!(chronicle(&[("Seul", 0)]), "Seul le jour du dossier.");
        assert_eq!(
            chronicle(&[("Premier message", 0), ("Suite", 1), ("Bien plus tard", 21)]),
            "Premier message le jour du dossier. Un jour après, suite. 21 jours après, bien plus tard."
        );
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
    fn the_entered_cycle_is_the_latest_reopening() {
        let first = event(FollowUpFact::CycleOpened, 2, None);
        let later = event(FollowUpFact::CycleOpened, 9, None);
        let later_id = later.id.to_string();
        assert_eq!(entered_cycle_key(&[first, later]), later_id);
        assert_eq!(entered_cycle_key(&[]), "");
    }
}
