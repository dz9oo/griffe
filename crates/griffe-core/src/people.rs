//! Les affaires : une liste, un nom — lectures pures, aucune commande.
//!
//! Lot 50. Le cœur compose des faits typés (chapitres, papiers, historique) ; c'est la fenêtre
//! (et le rendu CLI) qui rédige le français. `today` est un argument d'adaptateur, jamais lu
//! ici. Créer une conversation reste [`crate::prospection::CreateProspect`].

use std::collections::{BTreeMap, HashMap, HashSet};

use rusqlite::Connection;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::Date;
use uuid::Uuid;

use crate::app::AppError;
use crate::billing::{
    aged_balance, invoice_by_id, list_bank_transactions, list_invoices, list_write_offs,
    unmatched_debits,
};
use crate::clients::{client_by_id, list_clients, list_contacts};
use crate::domain::{
    BankTransaction, Client, ClientId, Expense, ExpenseId, ExpensePaidBy, FollowUpFact,
    FollowUpSubject, InteractionId, InteractionKind, Invoice, InvoiceId, LossReason, Milestone,
    Mission, MissionId, MissionKind, Money, Opportunity, OpportunityId, OpportunityStage, Quote,
    QuoteId, QuoteStatus, WriteOffId,
};
use crate::expenses::list_expenses;
use crate::follow_up::{CardStatus, FollowUpCard, events_for, follow_up_board};
use crate::missions::{MissionFilter, list_missions_with, list_time_entries, mission_by_id};
use crate::opening_balance::opening_balance;
use crate::prospection::{
    OpportunityFilter, list_interactions, list_open_opportunities, list_opportunities,
    list_opportunities_with, opportunity_by_id,
};
use crate::quotes::{list_quotes, priced_lines, quote_by_id};
use crate::reference::{RefMatch, resolve_among, resolve_client};

/// # Errors
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PeopleError {
    #[error("personne introuvable : {0}")]
    NotFound(String),
    #[error("« {0} » désigne plusieurs personnes, précisez : {1}")]
    Ambiguous(String, String),
}

impl From<PeopleError> for AppError {
    fn from(e: PeopleError) -> Self {
        Self::Domain(e.to_string())
    }
}

/// Qui est cette personne dans le coffre : une fiche (prospect ou client) ou un fournisseur
/// nommé sur une dépense, sans fiche.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PersonKey {
    Client { id: ClientId },
    Supplier { name: String },
}

impl PersonKey {
    #[must_use]
    pub fn as_ref_str(&self) -> String {
        match self {
            Self::Client { id } => id.to_string(),
            Self::Supplier { name } => name.clone(),
        }
    }
}

/// Les trois chapitres de la liste — un nom, pas un type de document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersonChapter {
    Conversation,
    Mission,
    /// Chez qui l'argent est déjà sorti — une dépense à un nom, pas une dette.
    Outgoing,
    /// Conversation perdue, encore consultable.
    Stopped,
}

/// Forme d'une mission, sans les montants — la fenêtre dit « régie » / « forfait ».
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissionShape {
    Regie,
    Forfait,
    Recurrent,
}

impl MissionShape {
    #[must_use]
    pub const fn from_kind(kind: &MissionKind) -> Self {
        match kind {
            MissionKind::Regie { .. } => Self::Regie,
            MissionKind::Forfait { .. } => Self::Forfait,
            MissionKind::Recurrent { .. } => Self::Recurrent,
        }
    }
}

/// Fait typé d'une ligne de liste. Aucune phrase française.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PersonCue {
    QuoteSent {
        #[serde(with = "crate::domain::serde_date::date")]
        on: Date,
    },
    FollowUpDue {
        #[serde(with = "crate::domain::serde_date::date")]
        on: Date,
        today: bool,
    },
    /// Rien n'a encore été envoyé.
    FirstMessage,
    /// Une lettre classée, ou un e-mail noté, sans suite.
    FirstContact,
    /// Plusieurs échanges, ou une rencontre, sans estimation.
    InExchange,
    /// Montant noté, devis formel non envoyé.
    EstimateNoted,
    /// Brouillon prêt, pas encore classé.
    DraftReady,
    /// Conversation rouverte, rien de nouveau depuis.
    Resumed,
    /// Conversation arrêtée. Le motif est celui posé à la clôture.
    Lost {
        reason: Option<LossReason>,
    },
    InvoiceOverdue {
        days: i64,
    },
    InvoiceOutstanding,
    NextMilestone {
        #[serde(with = "crate::domain::serde_date::date")]
        on: Date,
        label: String,
    },
    OpeningDebt,
    MatchingDebit,
    /// Rythme mensuel des notes (écart médian 25–40 jours, au moins trois notes).
    CadenceMonthly,
    LastNote {
        #[serde(with = "crate::domain::serde_date::date")]
        on: Date,
    },
    QuietSince {
        #[serde(with = "crate::domain::serde_date::date")]
        on: Date,
    },
}

/// Chiffre à droite d'une ligne : un montant, une enveloppe, ce qui a été versé, ou des jours.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PersonFigure {
    /// Devis net, facture à encaisser, ou dette encore ouverte.
    Money {
        amount: Money,
    },
    /// Montant d'opportunité, pas encore un devis.
    Around {
        amount: Money,
    },
    /// Total TTC des notes — l'argent est parti.
    Spent {
        amount: Money,
    },
    Days {
        days: f64,
    },
}

/// Rythme des notes d'une chemise « chez qui ça sort ». La fenêtre rédige ; `Occasional` se tait.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutgoingCadence {
    Monthly,
    Once,
    Occasional,
}

/// Une note : une dépense nommée, avec son justificatif s'il y en a un.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutgoingNote {
    pub expense_id: ExpenseId,
    pub revision: i64,
    #[serde(with = "crate::domain::serde_date::date")]
    pub on: Date,
    pub label: String,
    pub amount: Money,
    pub paid_by: ExpensePaidBy,
    pub receipt_filename: Option<String>,
}

/// Histoire d'une chemise : depuis quand, le rythme, le total, les notes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutgoingChapter {
    #[serde(with = "crate::domain::serde_date::date")]
    pub since: Date,
    pub cadence: OutgoingCadence,
    pub total_paid: Money,
    #[serde(with = "crate::domain::serde_date::date")]
    pub last_on: Date,
    pub owed: Option<Money>,
    pub opening_debt: bool,
    pub matching_debit: bool,
    pub notes: Vec<OutgoingNote>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersonRow {
    pub key: PersonKey,
    pub name: String,
    pub party: String,
    pub chapter: PersonChapter,
    pub figure: Option<PersonFigure>,
    pub cues: Vec<PersonCue>,
    pub client_id: Option<ClientId>,
    pub opportunity_id: Option<OpportunityId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PeopleList {
    pub conversations: Vec<PersonRow>,
    pub missions: Vec<PersonRow>,
    pub outgoing: Vec<PersonRow>,
    /// Hors du compte « X noms » : on les relit, on peut les rouvrir.
    pub stopped: Vec<PersonRow>,
}

impl PeopleList {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.conversations.is_empty()
            && self.missions.is_empty()
            && self.outgoing.is_empty()
            && self.stopped.is_empty()
    }
}

/// Action que la fenêtre peut proposer, déjà branchée sur un identifiant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PersonAction {
    Write {
        subject: FollowUpSubject,
    },
    /// Identité de la fiche (nom, adresse, contact) — sans pièce commerciale.
    Fiche {
        client_id: ClientId,
    },
    Quote {
        client_id: ClientId,
    },
    LogMeeting {
        opportunity_id: OpportunityId,
    },
    Snooze {
        subject: FollowUpSubject,
    },
    FileStatement,
    /// Le prospect ne donne pas suite.
    Stop {
        opportunity_id: OpportunityId,
    },
    /// La conversation devient une mission au forfait.
    Win {
        opportunity_id: OpportunityId,
    },
    /// Une conversation arrêtée reprend, sur le même dossier.
    Reopen {
        opportunity_id: OpportunityId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaperKind {
    Quote,
    Invoice,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PaperStatus {
    QuoteDraft,
    QuoteSent,
    QuoteAccepted,
    QuoteDeclined,
    QuoteExpired,
    InvoiceOutstanding { days_overdue: i64 },
    InvoicePaid,
    InvoiceCredited,
    InvoiceWrittenOff,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Paper {
    pub kind: PaperKind,
    #[serde(with = "crate::domain::serde_date::date")]
    pub on: Date,
    pub amount: Money,
    pub status: PaperStatus,
    pub quote_id: Option<QuoteId>,
    pub invoice_id: Option<InvoiceId>,
    pub write_off_id: Option<WriteOffId>,
    pub number: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HistoryKind {
    Interaction {
        interaction: InteractionKind,
    },
    /// Double d'une lettre classée (relance envoyée). Le corps est le texte composé ici.
    Letter {
        subject: String,
        body: String,
    },
    QuoteSent {
        version: u32,
    },
    QuoteAccepted,
    InvoiceIssued {
        number: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryEvent {
    #[serde(with = "crate::domain::serde_date::date")]
    pub on: Date,
    pub kind: HistoryKind,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectChapter {
    pub mission_id: MissionId,
    pub name: String,
    pub shape: MissionShape,
    #[serde(with = "crate::domain::serde_date::date")]
    pub started_on: Date,
    #[serde(with = "crate::domain::serde_date::date::option")]
    pub ended_on: Option<Date>,
    pub days_this_month: f64,
    pub days_logged: f64,
    pub next_milestone: Option<Milestone>,
    pub milestones: Vec<Milestone>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CurrentSituation {
    pub opportunity_id: Option<OpportunityId>,
    pub opportunity_name: Option<String>,
    pub amount: Option<Money>,
    pub quote: Option<Paper>,
    pub invoice: Option<Paper>,
    pub follow_up: Option<PersonCue>,
    pub last_interaction: Option<HistoryEvent>,
    pub opening_debt: bool,
    pub matching_debit: bool,
    pub nothing_scheduled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersonDossier {
    pub key: PersonKey,
    pub name: String,
    pub party: String,
    pub contact_name: Option<String>,
    /// Fiche créée comme prospect, sans mission : « pas encore cliente ».
    pub not_yet_client: bool,
    pub chapter: PersonChapter,
    #[serde(with = "crate::domain::serde_date::date::option")]
    pub since: Option<Date>,
    pub current: CurrentSituation,
    pub project: Option<ProjectChapter>,
    pub papers: Vec<Paper>,
    pub history: Vec<HistoryEvent>,
    pub actions: Vec<PersonAction>,
    pub follow_up_subject: Option<FollowUpSubject>,
    pub outgoing: Option<OutgoingChapter>,
    /// Posé seulement tant que la conversation est arrêtée.
    #[serde(default)]
    pub loss_reason: Option<LossReason>,
    /// Travaux notés sur l'estimation. Vide tant que l'historique n'est pas chargé.
    #[serde(default)]
    pub work: Vec<WorkLine>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkLine {
    pub label: String,
    pub amount: Money,
}

/// Liste unique, trois chapitres. Un client en mission n'apparaît pas aussi en conversation.
///
/// # Errors
pub fn people_list(conn: &Connection, today: Date) -> Result<PeopleList, AppError> {
    let snap = Snapshot::load(conn, today)?;
    Ok(snap.list(today))
}

/// Dossier d'une personne. `needle` : nom, préfixe, UUID (fiche, opportunité, mission, devis,
/// facture) ou nom de fournisseur — même doctrine que [`crate::reference`].
///
/// # Errors
pub fn people_dossier(
    conn: &Connection,
    needle: &str,
    today: Date,
) -> Result<PersonDossier, AppError> {
    match resolve_person(conn, needle)? {
        RefMatch::Unique(key) => {
            let snap = Snapshot::load(conn, today)?;
            snap.dossier(&key, today)
                .ok_or_else(|| PeopleError::NotFound(needle.trim().to_string()).into())
        }
        RefMatch::NotFound => Err(PeopleError::NotFound(needle.trim().to_string()).into()),
        RefMatch::Ambiguous(candidates) => {
            let list = candidates
                .iter()
                .map(|(key, label)| format!("{} ({})", label, key.as_ref_str()))
                .collect::<Vec<_>>()
                .join(", ");
            Err(PeopleError::Ambiguous(needle.trim().to_string(), list).into())
        }
    }
}

/// Résout `needle` vers une personne. UUID d'une fiche, d'une opportunité, d'une mission, d'un
/// devis ou d'une facture ; nom / préfixe d'une fiche ou d'un contact ; nom d'un fournisseur.
///
/// # Errors
pub fn resolve_person(conn: &Connection, needle: &str) -> Result<RefMatch<PersonKey>, AppError> {
    let trimmed = needle.trim();
    if trimmed.is_empty() {
        return Ok(RefMatch::NotFound);
    }

    if let Ok(uuid) = Uuid::parse_str(trimmed)
        && let Some(key) = lookup_uuid(conn, uuid)?
    {
        return Ok(RefMatch::Unique(key));
    }

    match resolve_client(conn, trimmed)? {
        RefMatch::Unique(id) => return Ok(RefMatch::Unique(PersonKey::Client { id })),
        RefMatch::Ambiguous(c) => {
            return Ok(RefMatch::Ambiguous(
                c.into_iter()
                    .map(|(id, label)| (PersonKey::Client { id }, label))
                    .collect(),
            ));
        }
        RefMatch::NotFound => {}
    }

    let mut contact_candidates: Vec<(ClientId, String)> = Vec::new();
    for client in list_clients(conn)? {
        for contact in list_contacts(conn, client.id)? {
            if !contact.name.is_empty() {
                contact_candidates.push((client.id, contact.name));
            }
        }
    }
    match resolve_among(trimmed, &contact_candidates) {
        RefMatch::Unique(id) => return Ok(RefMatch::Unique(PersonKey::Client { id })),
        RefMatch::Ambiguous(c) => {
            return Ok(RefMatch::Ambiguous(
                c.into_iter()
                    .map(|(id, label)| (PersonKey::Client { id }, label))
                    .collect(),
            ));
        }
        RefMatch::NotFound => {}
    }

    let suppliers = supplier_names(conn)?;
    match match_labels(trimmed, &suppliers) {
        RefMatch::Unique(name) => Ok(RefMatch::Unique(PersonKey::Supplier { name })),
        RefMatch::Ambiguous(c) => Ok(RefMatch::Ambiguous(
            c.into_iter()
                .map(|(name, label)| (PersonKey::Supplier { name }, label))
                .collect(),
        )),
        RefMatch::NotFound => Ok(RefMatch::NotFound),
    }
}

fn lookup_uuid(conn: &Connection, uuid: Uuid) -> Result<Option<PersonKey>, AppError> {
    let client_id = ClientId::from_uuid(uuid);
    if client_by_id(conn, client_id)?.is_some() {
        return Ok(Some(PersonKey::Client { id: client_id }));
    }
    let opp_id = OpportunityId::from_uuid(uuid);
    if let Some(o) = opportunity_by_id(conn, opp_id)? {
        return Ok(Some(PersonKey::Client { id: o.client_id }));
    }
    let mission_id = MissionId::from_uuid(uuid);
    if let Some(m) = mission_by_id(conn, mission_id)? {
        return Ok(Some(PersonKey::Client { id: m.client_id }));
    }
    let quote_id = QuoteId::from_uuid(uuid);
    if let Some(q) = quote_by_id(conn, quote_id)? {
        return Ok(Some(PersonKey::Client { id: q.client_id }));
    }
    let invoice_id = InvoiceId::from_uuid(uuid);
    if let Some(i) = invoice_by_id(conn, invoice_id)? {
        return Ok(Some(PersonKey::Client { id: i.client_id }));
    }
    Ok(None)
}

fn match_labels(needle: &str, labels: &[String]) -> RefMatch<String> {
    let trimmed = needle.trim();
    let needle_norm = normalize(trimmed);
    let exact: Vec<String> = labels
        .iter()
        .filter(|l| normalize(l) == needle_norm)
        .cloned()
        .collect();
    if exact.len() == 1 {
        return RefMatch::Unique(exact[0].clone());
    }
    if exact.len() > 1 {
        return RefMatch::Ambiguous(exact.into_iter().map(|n| (n.clone(), n)).collect());
    }
    let prefix: Vec<String> = labels
        .iter()
        .filter(|l| normalize(l).starts_with(&needle_norm))
        .cloned()
        .collect();
    match prefix.len() {
        0 => RefMatch::NotFound,
        1 => RefMatch::Unique(prefix[0].clone()),
        _ => RefMatch::Ambiguous(prefix.into_iter().map(|n| (n.clone(), n)).collect()),
    }
}

fn normalize(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| match c {
            'à' | 'â' | 'ä' | 'á' | 'ã' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'î' | 'ï' | 'í' | 'ì' => 'i',
            'ô' | 'ö' | 'ó' | 'ò' | 'õ' => 'o',
            'ù' | 'û' | 'ü' | 'ú' => 'u',
            'ç' => 'c',
            'ñ' => 'n',
            other => other,
        })
        .collect()
}

fn supplier_names(conn: &Connection) -> Result<Vec<String>, AppError> {
    let mut names = std::collections::BTreeSet::new();
    for expense in list_expenses(conn)? {
        if let Some(supplier) = expense.supplier.filter(|s| !s.is_empty()) {
            names.insert(supplier);
        }
    }
    Ok(names.into_iter().collect())
}

fn cadence_of(dates: &[Date]) -> OutgoingCadence {
    if dates.len() <= 1 {
        return OutgoingCadence::Once;
    }
    if dates.len() < 3 {
        return OutgoingCadence::Occasional;
    }
    let mut sorted = dates.to_vec();
    sorted.sort_unstable();
    let mut gaps: Vec<i64> = sorted
        .windows(2)
        .map(|w| (w[1] - w[0]).whole_days())
        .collect();
    gaps.sort_unstable();
    let median = gaps[gaps.len() / 2];
    if (25..=40).contains(&median) {
        OutgoingCadence::Monthly
    } else {
        OutgoingCadence::Occasional
    }
}

const CYCLE_OPENED: &str = "cycle_opened";

fn sent_letters(card: &FollowUpCard) -> u32 {
    u32::try_from(
        card.history
            .iter()
            .filter(|item| item.fact == FollowUpFact::MarkedSent.as_str())
            .count(),
    )
    .unwrap_or(u32::MAX)
}

fn cycle_opened_on(card: &FollowUpCard) -> Option<Date> {
    card.history
        .iter()
        .rev()
        .find(|item| item.fact == CYCLE_OPENED)
        .map(|item| item.on)
}

/// Reprise sans lettre, brouillon ni rencontre plus récente que la frontière.
fn resumed_quiet(card: &FollowUpCard, touch: Option<&Touch>) -> bool {
    let Some(idx) = card
        .history
        .iter()
        .rposition(|item| item.fact == CYCLE_OPENED)
    else {
        return false;
    };
    let on = card.history[idx].on;
    // Les lettres sont datées du `today` du coffre. L'interaction e-mail, elle, prend
    // l'horloge : on ne s'en sert pas pour savoir si quelque chose est plus récent.
    let newer = card.history[idx + 1..].iter().any(|item| {
        item.fact == FollowUpFact::MarkedSent.as_str()
            || item.fact == FollowUpFact::DraftPrepared.as_str()
    });
    if newer {
        return false;
    }
    touch.is_none_or(|t| t.last_meeting.is_none_or(|d| d <= on))
}

fn quote_net(quote: &Quote) -> Money {
    priced_lines(&quote.lines, quote.discount)
        .into_iter()
        .map(|(g, d)| g - d)
        .sum()
}

fn invoice_ttc(invoice: &Invoice) -> Money {
    crate::billing::compute_totals(&invoice.lines).total_ttc
}

struct Snapshot {
    clients: Vec<Client>,
    contacts: HashMap<ClientId, Vec<crate::domain::Contact>>,
    opportunities: Vec<Opportunity>,
    missions: Vec<Mission>,
    quotes: Vec<Quote>,
    invoices: Vec<Invoice>,
    aged: Vec<crate::billing::AgedInvoice>,
    written_off: HashMap<InvoiceId, WriteOffId>,
    expenses: Vec<Expense>,
    unmatched: Vec<BankTransaction>,
    bank: Vec<BankTransaction>,
    opening_class4: Vec<(String, Money)>,
    opening_opens_on: Option<Date>,
    follow_ups: Vec<FollowUpCard>,
    prospect_ids: HashSet<ClientId>,
    touches: HashMap<OpportunityId, Touch>,
    stopped: Vec<Opportunity>,
}

/// Échanges déjà notés sur une opportunité. Les notes internes ne comptent pas.
struct Touch {
    emails: u32,
    meetings: u32,
    last_email: Option<Date>,
    last_meeting: Option<Date>,
}

impl Snapshot {
    fn load(conn: &Connection, today: Date) -> Result<Self, AppError> {
        let clients = list_clients(conn)?;
        let mut contacts = HashMap::new();
        let mut prospect_ids = HashSet::new();
        for client in &clients {
            contacts.insert(client.id, list_contacts(conn, client.id)?);
            if client_is_prospect(conn, client.id)? {
                prospect_ids.insert(client.id);
            }
        }
        let opportunities = list_open_opportunities(conn)?;
        let stopped = list_opportunities_with(
            conn,
            OpportunityFilter {
                include_closed: true,
                include_archived: false,
            },
        )?
        .into_iter()
        .filter(|opp| opp.stage == OpportunityStage::Lost)
        .collect();
        let missions = list_missions_with(conn, MissionFilter::ACTIVE)?;
        let quotes = list_quotes(conn)?;
        let invoices = list_invoices(conn)?;
        let aged = aged_balance(conn, today)?;
        let written_off = list_write_offs(conn)?
            .into_iter()
            .filter(|w| w.retracted_on.is_none())
            .map(|w| (w.invoice_id, w.id))
            .collect();
        let expenses = list_expenses(conn)?;
        let unmatched = unmatched_debits(conn)?;
        let bank = list_bank_transactions(conn)?;
        let (opening_class4, opening_opens_on) = match opening_balance(conn)? {
            Some(record) => {
                let lines = record
                    .balance
                    .lines
                    .iter()
                    .filter(|l| {
                        l.account.starts_with("40")
                            || l.account.starts_with("42")
                            || l.account.starts_with("43")
                            || l.account.starts_with("444")
                            || l.account.starts_with("4455")
                    })
                    .map(|l| (l.label.clone(), l.amount))
                    .collect();
                (lines, Some(record.balance.opens_on))
            }
            None => (Vec::new(), None),
        };
        let follow_ups = follow_up_board(conn, today)?;
        let mut touches = HashMap::new();
        for opp in &opportunities {
            let mut touch = Touch {
                emails: 0,
                meetings: 0,
                last_email: None,
                last_meeting: None,
            };
            for interaction in list_interactions(conn, opp.id)? {
                let on = interaction.occurred_at.date();
                match interaction.kind {
                    InteractionKind::Email => {
                        touch.emails += 1;
                        touch.last_email = Some(touch.last_email.map_or(on, |d| d.max(on)));
                    }
                    InteractionKind::Call | InteractionKind::Meeting | InteractionKind::Visio => {
                        touch.meetings += 1;
                        touch.last_meeting = Some(touch.last_meeting.map_or(on, |d| d.max(on)));
                    }
                    InteractionKind::Note => {}
                }
            }
            touches.insert(opp.id, touch);
        }
        Ok(Self {
            clients,
            contacts,
            opportunities,
            missions,
            quotes,
            invoices,
            aged,
            written_off,
            expenses,
            unmatched,
            bank,
            opening_class4,
            opening_opens_on,
            follow_ups,
            prospect_ids,
            touches,
            stopped,
        })
    }

    fn name_of(&self, id: ClientId) -> String {
        self.clients
            .iter()
            .find(|c| c.id == id)
            .map_or_else(|| id.to_string(), |c| c.name.clone())
    }

    fn contact_name(&self, id: ClientId) -> Option<String> {
        self.contacts
            .get(&id)
            .and_then(|cs| cs.first())
            .map(|c| c.name.clone())
            .filter(|n| !n.is_empty())
    }

    fn display_name(&self, id: ClientId) -> String {
        self.contact_name(id)
            .filter(|n| n != &self.name_of(id))
            .unwrap_or_else(|| self.name_of(id))
    }

    fn list(&self, today: Date) -> PeopleList {
        let in_mission: HashSet<ClientId> = self.missions.iter().map(|m| m.client_id).collect();

        let mut conversations = Vec::new();
        let mut seen_conv: HashSet<ClientId> = HashSet::new();
        for opp in &self.opportunities {
            if in_mission.contains(&opp.client_id) || !seen_conv.insert(opp.client_id) {
                continue;
            }
            conversations.push(self.conversation_row(opp, today));
        }
        conversations.sort_by(|a, b| a.name.cmp(&b.name));

        let mut missions = Vec::new();
        let mut seen_mission: HashSet<ClientId> = HashSet::new();
        for mission in &self.missions {
            if !seen_mission.insert(mission.client_id) {
                continue;
            }
            missions.push(self.mission_row(mission, today));
        }
        missions.sort_by(|a, b| a.name.cmp(&b.name));

        let listed_names: HashSet<String> = conversations
            .iter()
            .chain(missions.iter())
            .map(|r| normalize(&r.party))
            .collect();
        let mut by_name: BTreeMap<String, Vec<&Expense>> = BTreeMap::new();
        for expense in &self.expenses {
            if let Some(supplier) = expense.supplier.as_ref().filter(|s| !s.is_empty()) {
                if listed_names.contains(&normalize(supplier)) {
                    continue;
                }
                by_name.entry(supplier.clone()).or_default().push(expense);
            }
        }
        let mut outgoing: Vec<(bool, Date, PersonRow)> = by_name
            .iter()
            .map(|(name, items)| {
                let facts = self.outgoing_facts(name, items, today);
                let debt = facts.owed.is_some();
                let last_on = facts.last_on;
                (debt, last_on, Self::row_from_outgoing(name, &facts, today))
            })
            .collect();
        outgoing.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| b.1.cmp(&a.1))
                .then_with(|| a.2.name.cmp(&b.2.name))
        });
        let outgoing: Vec<PersonRow> = outgoing.into_iter().map(|(_, _, row)| row).collect();

        let mut stopped: Vec<PersonRow> = self
            .stopped
            .iter()
            .map(|opp| self.stopped_row(opp))
            .collect();
        stopped.sort_by(|a, b| a.name.cmp(&b.name));

        PeopleList {
            conversations,
            missions,
            outgoing,
            stopped,
        }
    }

    fn stopped_row(&self, opp: &Opportunity) -> PersonRow {
        let party = self.name_of(opp.client_id);
        let name = self.display_name(opp.client_id);
        let figure =
            (opp.amount.cents() != 0).then_some(PersonFigure::Around { amount: opp.amount });
        PersonRow {
            key: PersonKey::Client { id: opp.client_id },
            name,
            party,
            chapter: PersonChapter::Stopped,
            figure,
            cues: vec![PersonCue::Lost {
                reason: opp.loss_reason.clone(),
            }],
            client_id: Some(opp.client_id),
            opportunity_id: Some(opp.id),
        }
    }

    fn conversation_row(&self, opp: &Opportunity, today: Date) -> PersonRow {
        let party = self.name_of(opp.client_id);
        let name = self.display_name(opp.client_id);
        let quote = self.latest_quote(opp.client_id);
        let sent = quote.as_ref().filter(|q| q.status == QuoteStatus::Sent);
        let mut cues = Vec::new();
        let mut figure =
            (opp.amount.cents() != 0).then_some(PersonFigure::Around { amount: opp.amount });
        let card = self.follow_up_for_opportunity(opp.id);
        let touch = self.touches.get(&opp.id);
        let letters = card.map_or(0, sent_letters);
        let emails = touch.map_or(letters, |t| t.emails.max(letters));
        let meetings = touch.map_or(0, |t| t.meetings);
        let drafted = card.is_some_and(|c| c.status == CardStatus::Drafted);
        let estimate = opp.amount.cents() != 0;
        let resumed = card.is_some_and(|c| resumed_quiet(c, touch));
        if drafted {
            cues.push(PersonCue::DraftReady);
        } else if let Some(q) = sent {
            cues.push(PersonCue::QuoteSent {
                on: q.created_at.date(),
            });
            figure = Some(PersonFigure::Money {
                amount: quote_net(q),
            });
        } else if resumed {
            cues.push(PersonCue::Resumed);
            if estimate {
                cues.push(PersonCue::EstimateNoted);
            }
        } else if estimate {
            cues.push(PersonCue::EstimateNoted);
        } else if meetings >= 1 || emails >= 2 {
            cues.push(PersonCue::InExchange);
        } else if emails == 1 {
            cues.push(PersonCue::FirstContact);
        } else {
            cues.push(PersonCue::FirstMessage);
        }
        if let Some(on) = card.and_then(|c| c.due_on).or(opp.next_action_at) {
            let stage_says_today = on <= today
                && (cues.contains(&PersonCue::FirstMessage)
                    || cues.contains(&PersonCue::DraftReady));
            let resumed_today = cues.contains(&PersonCue::Resumed)
                && card.is_some_and(|c| cycle_opened_on(c) == Some(today));
            if !stage_says_today && !resumed_today {
                cues.push(PersonCue::FollowUpDue {
                    on,
                    today: on <= today,
                });
            }
        }
        PersonRow {
            key: PersonKey::Client { id: opp.client_id },
            name,
            party,
            chapter: PersonChapter::Conversation,
            figure,
            cues,
            client_id: Some(opp.client_id),
            opportunity_id: Some(opp.id),
        }
    }

    fn mission_row(&self, mission: &Mission, today: Date) -> PersonRow {
        let party = self.name_of(mission.client_id);
        let name = party.clone();
        let mut cues = Vec::new();
        let aged = self
            .aged
            .iter()
            .filter(|a| a.client_id == mission.client_id && a.outstanding.cents() > 0)
            .max_by_key(|a| (a.days_overdue, a.outstanding.cents()));
        let mut figure = None;
        if let Some(a) = aged {
            figure = Some(PersonFigure::Money {
                amount: a.outstanding,
            });
            if a.days_overdue > 0 {
                cues.push(PersonCue::InvoiceOverdue {
                    days: a.days_overdue,
                });
            } else {
                cues.push(PersonCue::InvoiceOutstanding);
            }
        }
        if let Some(ms) = mission
            .milestones
            .iter()
            .filter(|m| m.due_on.is_some_and(|d| d >= today))
            .min_by_key(|m| m.due_on)
            && let Some(on) = ms.due_on
        {
            cues.push(PersonCue::NextMilestone {
                on,
                label: ms.label.clone(),
            });
        }
        if figure.is_none() {
            match &mission.kind {
                MissionKind::Forfait { budget } => {
                    figure = Some(PersonFigure::Money { amount: *budget });
                }
                MissionKind::Regie { .. } | MissionKind::Recurrent { .. } => {}
            }
        }
        PersonRow {
            key: PersonKey::Client {
                id: mission.client_id,
            },
            name,
            party,
            chapter: PersonChapter::Mission,
            figure,
            cues,
            client_id: Some(mission.client_id),
            opportunity_id: mission.opportunity_id,
        }
    }

    fn row_from_outgoing(name: &str, facts: &OutgoingChapter, today: Date) -> PersonRow {
        let mut cues = Vec::new();
        if facts.opening_debt {
            cues.push(PersonCue::OpeningDebt);
        }
        if facts.matching_debit {
            cues.push(PersonCue::MatchingDebit);
        }
        if facts.owed.is_none() {
            match facts.cadence {
                OutgoingCadence::Monthly => {
                    cues.push(PersonCue::CadenceMonthly);
                    cues.push(PersonCue::LastNote { on: facts.last_on });
                }
                OutgoingCadence::Once | OutgoingCadence::Occasional => {
                    if (today - facts.last_on).whole_days() > 90 {
                        cues.push(PersonCue::QuietSince { on: facts.last_on });
                    }
                }
            }
        }
        let figure = Some(match facts.owed {
            Some(amount) => PersonFigure::Money { amount },
            None => PersonFigure::Spent {
                amount: facts.total_paid,
            },
        });
        PersonRow {
            key: PersonKey::Supplier {
                name: name.to_string(),
            },
            name: name.to_string(),
            party: name.to_string(),
            chapter: PersonChapter::Outgoing,
            figure,
            cues,
            client_id: None,
            opportunity_id: None,
        }
    }

    fn outgoing_facts(&self, name: &str, items: &[&Expense], today: Date) -> OutgoingChapter {
        let total_paid: Money = items.iter().map(|e| e.amount).sum();
        let expense_amounts: HashSet<i64> = items.iter().map(|e| e.amount.cents()).collect();
        let opening_lines = self.opening_lines_for(name);
        let opening_amounts: HashSet<i64> = opening_lines.iter().map(|amt| amt.cents()).collect();
        let covered = self.bank.iter().any(|t| {
            t.is_debit()
                && t.is_matched()
                && opening_amounts.contains(&t.amount_cents.saturating_neg())
        });
        let opening_debt = !opening_lines.is_empty() && !covered;
        let matching: Vec<&BankTransaction> = self
            .unmatched
            .iter()
            .filter(|t| t.is_debit() && expense_amounts.contains(&t.amount_cents.saturating_neg()))
            .collect();
        let matching_debit = !matching.is_empty();
        let owed = if matching_debit {
            Some(
                matching
                    .iter()
                    .map(|t| Money::from_cents(t.amount_cents.saturating_neg()))
                    .sum(),
            )
        } else if opening_debt {
            Some(opening_lines.iter().copied().sum())
        } else {
            None
        };
        let mut notes: Vec<OutgoingNote> = items
            .iter()
            .map(|e| OutgoingNote {
                expense_id: e.id,
                revision: e.revision,
                on: e.incurred_on,
                label: e.label.clone(),
                amount: e.amount,
                paid_by: e.paid_by,
                receipt_filename: e.receipt_filename.clone(),
            })
            .collect();
        notes.sort_by(|a, b| b.on.cmp(&a.on).then_with(|| a.label.cmp(&b.label)));
        let last_on = notes.first().map_or_else(
            || items.iter().map(|e| e.incurred_on).max().unwrap_or(today),
            |n| n.on,
        );
        let min_on = items.iter().map(|e| e.incurred_on).min().unwrap_or(today);
        let since = if opening_debt {
            self.opening_opens_on.unwrap_or(min_on)
        } else {
            min_on
        };
        let dates: Vec<Date> = items.iter().map(|e| e.incurred_on).collect();
        OutgoingChapter {
            since,
            cadence: cadence_of(&dates),
            total_paid,
            last_on,
            owed,
            opening_debt,
            matching_debit,
            notes,
        }
    }

    fn opening_lines_for(&self, name: &str) -> Vec<Money> {
        let needle = normalize(name);
        self.opening_class4
            .iter()
            .filter(|(label, _)| {
                let l = normalize(label);
                l == needle || (!needle.is_empty() && (l.contains(&needle) || needle.contains(&l)))
            })
            .map(|(_, amt)| *amt)
            .collect()
    }

    fn latest_quote(&self, client_id: ClientId) -> Option<Quote> {
        self.quotes
            .iter()
            .filter(|q| q.client_id == client_id)
            .max_by_key(|q| (q.created_at, q.version))
            .cloned()
    }

    fn follow_up_for_opportunity(&self, id: OpportunityId) -> Option<&FollowUpCard> {
        self.follow_ups
            .iter()
            .find(|c| c.subject == FollowUpSubject::Opportunity(id))
    }

    fn follow_up_for_invoice(&self, id: InvoiceId) -> Option<&FollowUpCard> {
        self.follow_ups
            .iter()
            .find(|c| c.subject == FollowUpSubject::Invoice(id))
    }

    fn dossier(&self, key: &PersonKey, today: Date) -> Option<PersonDossier> {
        match key {
            PersonKey::Client { id } => self.client_dossier(*id, today),
            PersonKey::Supplier { name } => self.supplier_dossier(name, today),
        }
    }

    #[allow(clippy::too_many_lines)]
    fn client_dossier(&self, id: ClientId, today: Date) -> Option<PersonDossier> {
        let _client = self.clients.iter().find(|c| c.id == id)?;
        let party = self.name_of(id);
        let contact_name = self.contact_name(id);
        let name = self.display_name(id);
        let mission = self.missions.iter().find(|m| m.client_id == id);
        let open = self.opportunities.iter().find(|o| o.client_id == id);
        let stopped_opp = self.stopped.iter().find(|o| o.client_id == id);
        let opp = open.or(stopped_opp);
        let is_stopped = open.is_none() && stopped_opp.is_some();
        let chapter = if mission.is_some() {
            PersonChapter::Mission
        } else if is_stopped {
            PersonChapter::Stopped
        } else {
            PersonChapter::Conversation
        };
        let not_yet_client = mission.is_none() && self.prospect_ids.contains(&id);
        let since = opp
            .map(|o| o.created_at.date())
            .or_else(|| mission.map(|m| m.started_on));

        let papers = self.papers_for(id);
        let quote_paper = papers.iter().find(|p| p.kind == PaperKind::Quote).cloned();
        let invoice_paper = papers
            .iter()
            .find(|p| {
                p.kind == PaperKind::Invoice
                    && matches!(p.status, PaperStatus::InvoiceOutstanding { .. })
            })
            .cloned();

        let follow_subject = invoice_paper
            .as_ref()
            .and_then(|p| p.invoice_id)
            .map(FollowUpSubject::Invoice)
            .or_else(|| opp.map(|o| FollowUpSubject::Opportunity(o.id)));

        let follow_cue = match follow_subject {
            Some(FollowUpSubject::Opportunity(oid)) => {
                self.follow_up_for_opportunity(oid).and_then(|c| {
                    c.due_on.map(|on| PersonCue::FollowUpDue {
                        on,
                        today: on <= today,
                    })
                })
            }
            Some(FollowUpSubject::Invoice(iid)) => self.follow_up_for_invoice(iid).and_then(|c| {
                c.due_on.map(|on| PersonCue::FollowUpDue {
                    on,
                    today: on <= today,
                })
            }),
            None => None,
        };

        let current = CurrentSituation {
            opportunity_id: opp.map(|o| o.id),
            opportunity_name: opp.map(|o| o.name.clone()),
            amount: quote_paper
                .as_ref()
                .map(|p| p.amount)
                .or_else(|| invoice_paper.as_ref().map(|p| p.amount))
                .or_else(|| opp.map(|o| o.amount)),
            quote: quote_paper,
            invoice: invoice_paper.clone(),
            follow_up: follow_cue,
            last_interaction: None,
            opening_debt: false,
            matching_debit: false,
            nothing_scheduled: follow_subject.is_none() && papers.is_empty(),
        };

        let project = mission.map(|m| ProjectChapter {
            mission_id: m.id,
            name: m.name.clone(),
            shape: MissionShape::from_kind(&m.kind),
            started_on: m.started_on,
            ended_on: m.ended_on,
            days_this_month: 0.0,
            days_logged: 0.0,
            next_milestone: m
                .milestones
                .iter()
                .filter(|ms| ms.due_on.is_some_and(|d| d >= today))
                .min_by_key(|ms| ms.due_on)
                .cloned(),
            milestones: m.milestones.clone(),
        });

        let mut actions = Vec::new();
        if is_stopped {
            if let Some(o) = opp {
                actions.push(PersonAction::Reopen {
                    opportunity_id: o.id,
                });
            }
        } else {
            if let Some(subject) = follow_subject {
                actions.push(PersonAction::Write { subject });
            }
            actions.push(PersonAction::Fiche { client_id: id });
            actions.push(PersonAction::Quote { client_id: id });
            if let Some(o) = opp {
                actions.push(PersonAction::LogMeeting {
                    opportunity_id: o.id,
                });
                if mission.is_none() {
                    if o.amount.cents() != 0 {
                        actions.push(PersonAction::Win {
                            opportunity_id: o.id,
                        });
                    }
                    actions.push(PersonAction::Stop {
                        opportunity_id: o.id,
                    });
                }
            }
            if let Some(subject) = follow_subject {
                actions.push(PersonAction::Snooze { subject });
            }
        }

        Some(PersonDossier {
            key: PersonKey::Client { id },
            name,
            party,
            contact_name,
            not_yet_client,
            chapter,
            since,
            current,
            project,
            papers,
            history: Vec::new(),
            actions,
            follow_up_subject: follow_subject,
            outgoing: None,
            loss_reason: stopped_opp.and_then(|o| o.loss_reason.clone()),
            work: Vec::new(),
        })
    }

    fn supplier_dossier(&self, name: &str, today: Date) -> Option<PersonDossier> {
        let items: Vec<&Expense> = self
            .expenses
            .iter()
            .filter(|e| e.supplier.as_deref() == Some(name))
            .collect();
        if items.is_empty() {
            return None;
        }
        let facts = self.outgoing_facts(name, &items, today);
        let mut actions = Vec::new();
        if facts.matching_debit {
            actions.push(PersonAction::FileStatement);
        }
        let amount = match facts.owed {
            Some(owed) => Some(owed),
            None => Some(facts.total_paid),
        };
        Some(PersonDossier {
            key: PersonKey::Supplier {
                name: name.to_string(),
            },
            name: name.to_string(),
            party: name.to_string(),
            contact_name: None,
            not_yet_client: false,
            chapter: PersonChapter::Outgoing,
            since: Some(facts.since),
            current: CurrentSituation {
                opportunity_id: None,
                opportunity_name: None,
                amount,
                quote: None,
                invoice: None,
                follow_up: None,
                last_interaction: None,
                opening_debt: facts.opening_debt,
                matching_debit: facts.matching_debit,
                nothing_scheduled: false,
            },
            project: None,
            papers: Vec::new(),
            history: Vec::new(),
            actions,
            follow_up_subject: None,
            outgoing: Some(facts),
            loss_reason: None,
            work: Vec::new(),
        })
    }

    fn papers_for(&self, client_id: ClientId) -> Vec<Paper> {
        let mut papers = Vec::new();
        let mut seen_roots = HashSet::new();
        for quote in self.quotes.iter().filter(|q| q.client_id == client_id) {
            if !seen_roots.insert(quote.root_id) {
                continue;
            }
            // La version la plus récente de cette lignée.
            let latest = self
                .quotes
                .iter()
                .filter(|q| q.root_id == quote.root_id)
                .max_by_key(|q| q.version)
                .unwrap_or(quote);
            papers.push(Paper {
                kind: PaperKind::Quote,
                on: latest.created_at.date(),
                amount: quote_net(latest),
                status: match latest.status {
                    QuoteStatus::Draft => PaperStatus::QuoteDraft,
                    QuoteStatus::Sent => PaperStatus::QuoteSent,
                    QuoteStatus::Accepted => PaperStatus::QuoteAccepted,
                    QuoteStatus::Declined => PaperStatus::QuoteDeclined,
                    QuoteStatus::Expired => PaperStatus::QuoteExpired,
                },
                quote_id: Some(latest.id),
                invoice_id: None,
                write_off_id: None,
                number: Some(format!("n°{}", latest.version)),
            });
        }
        for invoice in self
            .invoices
            .iter()
            .filter(|i| i.client_id == client_id && i.credited_invoice_id.is_none())
        {
            let aged = self.aged.iter().find(|a| a.invoice_id == invoice.id);
            let credited = self
                .invoices
                .iter()
                .any(|c| c.credited_invoice_id == Some(invoice.id));
            let write_off_id = self.written_off.get(&invoice.id).copied();
            let status = if credited {
                PaperStatus::InvoiceCredited
            } else if write_off_id.is_some() {
                PaperStatus::InvoiceWrittenOff
            } else if let Some(a) = aged {
                PaperStatus::InvoiceOutstanding {
                    days_overdue: a.days_overdue,
                }
            } else {
                PaperStatus::InvoicePaid
            };
            let amount = aged.map_or_else(|| invoice_ttc(invoice), |a| a.outstanding);
            papers.push(Paper {
                kind: PaperKind::Invoice,
                on: invoice.issued_on,
                amount,
                status,
                quote_id: None,
                invoice_id: Some(invoice.id),
                write_off_id,
                number: Some(invoice.number.clone()),
            });
        }
        papers.sort_by_key(|a| std::cmp::Reverse(a.on));
        papers
    }
}

fn client_is_prospect(conn: &Connection, id: ClientId) -> Result<bool, AppError> {
    let flag: Option<i64> = conn
        .query_row(
            "SELECT is_prospect FROM clients WHERE id = ?1",
            [id.to_string()],
            |row| row.get(0),
        )
        .optional()?;
    Ok(flag == Some(1))
}

/// Enrichit le dossier : interactions, jours saisis. Fait dans une seconde passe pour ne pas
/// charger l'historique de tout le monde sur la liste.
///
/// # Errors
pub fn people_dossier_with_history(
    conn: &Connection,
    needle: &str,
    today: Date,
) -> Result<PersonDossier, AppError> {
    let mut dossier = people_dossier(conn, needle, today)?;
    fill_history(conn, &mut dossier, today)?;
    if let Some(id) = dossier.current.opportunity_id {
        dossier.work = crate::prospection::estimation_lines(conn, id)?
            .into_iter()
            .map(|(label, amount)| WorkLine { label, amount })
            .collect();
    }
    Ok(dossier)
}

fn push_filed_letters(
    conn: &Connection,
    subject: FollowUpSubject,
    history: &mut Vec<HistoryEvent>,
    lettered: &mut HashSet<InteractionId>,
) -> Result<(), AppError> {
    let events = events_for(conn, subject)?;
    for event in crate::domain::active_events(&events) {
        if event.fact != FollowUpFact::MarkedSent {
            continue;
        }
        let Some(body) = event.rendered_body.clone() else {
            continue;
        };
        history.push(HistoryEvent {
            on: event.at.date(),
            kind: HistoryKind::Letter {
                subject: event.rendered_subject.clone().unwrap_or_default(),
                body,
            },
            note: None,
        });
        if let Some(id) = event.interaction_id {
            lettered.insert(id);
        }
    }
    Ok(())
}

fn fill_history(
    conn: &Connection,
    dossier: &mut PersonDossier,
    today: Date,
) -> Result<(), AppError> {
    let PersonKey::Client { id } = dossier.key else {
        return Ok(());
    };
    let mut history = Vec::new();
    let opps = list_opportunities(conn)?;
    let mine: Vec<&Opportunity> = opps.iter().filter(|o| o.client_id == id).collect();
    let mut lettered: HashSet<InteractionId> = HashSet::new();
    for opp in &mine {
        push_filed_letters(
            conn,
            FollowUpSubject::Opportunity(opp.id),
            &mut history,
            &mut lettered,
        )?;
    }
    for invoice in list_invoices(conn)? {
        if invoice.client_id == id {
            push_filed_letters(
                conn,
                FollowUpSubject::Invoice(invoice.id),
                &mut history,
                &mut lettered,
            )?;
        }
    }
    let mut last_interaction: Option<HistoryEvent> = None;
    for opp in &mine {
        for interaction in list_interactions(conn, opp.id)? {
            if lettered.contains(&interaction.id) {
                continue;
            }
            let event = HistoryEvent {
                on: interaction.occurred_at.date(),
                kind: HistoryKind::Interaction {
                    interaction: interaction.kind,
                },
                note: Some(interaction.note).filter(|n| !n.is_empty()),
            };
            if last_interaction.as_ref().is_none_or(|e| event.on >= e.on) {
                last_interaction = Some(event.clone());
            }
            history.push(event);
        }
    }
    for paper in &dossier.papers {
        match paper.status {
            PaperStatus::QuoteSent => {
                history.push(HistoryEvent {
                    on: paper.on,
                    kind: HistoryKind::QuoteSent {
                        version: paper
                            .number
                            .as_deref()
                            .and_then(|s| s.trim_start_matches("n°").parse().ok())
                            .unwrap_or(1),
                    },
                    note: None,
                });
            }
            PaperStatus::QuoteAccepted => {
                history.push(HistoryEvent {
                    on: paper.on,
                    kind: HistoryKind::QuoteAccepted,
                    note: None,
                });
            }
            _ => {}
        }
        if paper.kind == PaperKind::Invoice
            && let Some(number) = &paper.number
        {
            history.push(HistoryEvent {
                on: paper.on,
                kind: HistoryKind::InvoiceIssued {
                    number: number.clone(),
                },
                note: None,
            });
        }
    }
    history.sort_by(|a, b| b.on.cmp(&a.on).then_with(|| a.note.cmp(&b.note)));
    dossier.history = history;
    dossier.current.last_interaction = last_interaction;

    if let Some(project) = dossier.project.as_mut() {
        let entries = list_time_entries(conn, project.mission_id)?;
        project.days_logged = entries.iter().map(|e| e.days).sum();
        let month = today.month();
        let year = today.year();
        project.days_this_month = entries
            .iter()
            .filter(|e| e.worked_on.year() == year && e.worked_on.month() == month)
            .map(|e| e.days)
            .sum();
    }
    Ok(())
}

/// `people_dossier` avec l'historique chargé — c'est la lecture que consomment les façades.
///
/// # Errors
pub fn person(conn: &Connection, needle: &str, today: Date) -> Result<PersonDossier, AppError> {
    people_dossier_with_history(conn, needle, today)
}

#[cfg(test)]
mod tests {
    use time::{Duration, Month as TimeMonth, OffsetDateTime};

    use super::*;
    use crate::app::{Actor, ExecutionContext, Executor, Outcome};
    use crate::billing::{
        EmitInvoice, ImportBankTransactions, ParsedTransaction, WriteOffReceivable,
    };
    use crate::clients::CreateClient;
    use crate::domain::{
        InvoiceLine, LineKind, Milestone, MissionKind, OpeningBalanceLine, Probability, VatRate,
    };
    use crate::expenses::RecordExpense;
    use crate::follow_up::SetFollowUpSender;
    use crate::missions::CreateMission;
    use crate::opening_balance::RecordOpeningBalance;
    use crate::prospection::{CreateProspect, LogInteraction};
    use crate::quotes::{CreateQuote, SendQuote};
    use crate::store::Store;
    use crate::store::testing::test_store;

    fn date(year: i32, month: TimeMonth, day: u8) -> Date {
        Date::from_calendar_date(year, month, day).unwrap()
    }

    fn today() -> Date {
        date(2026, TimeMonth::September, 5)
    }

    fn human() -> ExecutionContext {
        ExecutionContext::new(Actor::Human, false)
    }

    fn applied<T: std::fmt::Debug>(outcome: Outcome<T>) -> T {
        match outcome {
            Outcome::Applied(v) => v,
            other => panic!("expected Applied, got {other:?}"),
        }
    }

    fn at(on: Date, hour: u8) -> OffsetDateTime {
        on.with_hms(hour, 0, 0)
            .unwrap()
            .assume_offset(time::UtcOffset::from_hms(2, 0, 0).unwrap())
    }

    #[allow(clippy::too_many_lines)]
    fn seed_people(store: &mut Store) {
        applied(
            Executor::new(store)
                .execute(
                    &RecordOpeningBalance {
                        opens_on: date(2025, TimeMonth::October, 1),
                        source: Some("cabinet".into()),
                        lines: vec![
                            "512000:Banque:D:15000.00"
                                .parse::<OpeningBalanceLine>()
                                .unwrap(),
                            "401000:Cabinet Leroy:C:1200.00"
                                .parse::<OpeningBalanceLine>()
                                .unwrap(),
                            "101000:Capital:C:13800.00"
                                .parse::<OpeningBalanceLine>()
                                .unwrap(),
                        ],
                        tax_losses: Money::ZERO,
                        prior_corporate_tax: None,
                        prior_vat_due: None,
                    },
                    &human(),
                )
                .unwrap(),
        );
        applied(
            Executor::new(store)
                .execute(
                    &ImportBankTransactions {
                        transactions: vec![ParsedTransaction {
                            occurred_on: date(2026, TimeMonth::September, 4),
                            amount_cents: -120_000,
                            description: "VIR LEROY".into(),
                            fitid: None,
                        }],
                    },
                    &human(),
                )
                .unwrap(),
        );
        applied(
            Executor::new(store)
                .execute(
                    &RecordExpense {
                        label: "honoraires clôture".into(),
                        category: crate::domain::ExpenseCategory::Fees,
                        amount: Money::from_cents(120_000),
                        vat_rate: VatRate::Zero,
                        vat_deductible: Money::ZERO,
                        incurred_on: date(2026, TimeMonth::September, 4),
                        receipt_hash: None,
                        receipt_filename: None,
                        bank_transaction_id: None,
                        supplier: Some("Cabinet Leroy".into()),
                        paid_by: crate::domain::ExpensePaidBy::Company,
                    },
                    &human(),
                )
                .unwrap(),
        );

        applied(
            Executor::new(store)
                .execute(
                    &SetFollowUpSender {
                        email: "nicolas@lumen.test".into(),
                        name: Some("Nicolas".into()),
                    },
                    &human(),
                )
                .unwrap(),
        );
        let camille = applied(
            Executor::new(store)
                .execute(
                    &CreateProspect {
                        prospect_name: "Atelier Nord".into(),
                        address: None,
                        representative: Some("Camille Rivière".into()),
                        email: Some("camille@atelier.test".into()),
                        phone: None,
                        name: "accompagnement identité".into(),
                        amount: Money::from_cents(840_000),
                        probability: Probability::new(40).unwrap(),
                        next_action_at: today(),
                        source: None,
                    },
                    &human(),
                )
                .unwrap(),
        );
        let atelier_id = opportunity_by_id(store.connection(), camille)
            .unwrap()
            .unwrap()
            .client_id;
        let quote_id = applied(
            Executor::new(store)
                .execute(
                    &CreateQuote {
                        client_id: atelier_id,
                        opportunity_id: Some(camille),
                        lines: vec![crate::domain::QuoteLine {
                            description: "accompagnement identité".into(),
                            kind: LineKind::Forfait {
                                amount: Money::from_cents(840_000),
                            },
                            vat_rate: VatRate::Standard,
                        }],
                        discount: None,
                        terms: None,
                        valid_until: date(2026, TimeMonth::October, 5),
                    },
                    &human(),
                )
                .unwrap(),
        );
        applied(
            Executor::new(store)
                .execute(&SendQuote { quote_id }, &human())
                .unwrap(),
        );
        applied(
            Executor::new(store)
                .execute(
                    &LogInteraction {
                        opportunity_id: camille,
                        kind: InteractionKind::Call,
                        note: "Budget confirmé, intéressée par le forfait.".into(),
                        occurred_at: Some(at(date(2026, TimeMonth::August, 28), 10)),
                    },
                    &human(),
                )
                .unwrap(),
        );
        applied(
            Executor::new(store)
                .execute(
                    &LogInteraction {
                        opportunity_id: camille,
                        kind: InteractionKind::Note,
                        note: "Premier échange, via Martin Pecheur.".into(),
                        occurred_at: Some(at(date(2026, TimeMonth::August, 12), 16)),
                    },
                    &human(),
                )
                .unwrap(),
        );

        applied(
            Executor::new(store)
                .execute(
                    &CreateProspect {
                        prospect_name: "Martin Pecheur".into(),
                        address: None,
                        representative: None,
                        email: None,
                        phone: None,
                        name: "accompagnement produit".into(),
                        amount: Money::from_cents(1_200_000),
                        probability: Probability::new(20).unwrap(),
                        next_action_at: today() + Duration::days(30),
                        source: None,
                    },
                    &human(),
                )
                .unwrap(),
        );

        let atlas = applied(
            Executor::new(store)
                .execute(
                    &CreateClient {
                        name: "Atlas Digital".into(),
                        siren: None,
                        vat_number: None,
                        address: None,
                    },
                    &human(),
                )
                .unwrap(),
        );
        applied(
            Executor::new(store)
                .execute(
                    &EmitInvoice {
                        client_id: atlas,
                        mission_id: None,
                        lines: vec![InvoiceLine {
                            description: "régie août".into(),
                            quantity: 1.0,
                            unit_price: Money::from_cents(620_000),
                            vat_rate: VatRate::Zero,
                        }],
                        issued_on: date(2026, TimeMonth::August, 15),
                        payment_terms_days: 14,
                    },
                    &human(),
                )
                .unwrap(),
        );
        applied(
            Executor::new(store)
                .execute(
                    &CreateMission {
                        client_id: atlas,
                        quote_id: None,
                        name: "régie Atlas".into(),
                        kind: MissionKind::Regie {
                            daily_rate: Money::from_cents(65_000),
                        },
                        milestones: vec![],
                        started_on: date(2026, TimeMonth::January, 12),
                    },
                    &human(),
                )
                .unwrap(),
        );

        let hume = applied(
            Executor::new(store)
                .execute(
                    &CreateClient {
                        name: "Studio Hume".into(),
                        siren: None,
                        vat_number: None,
                        address: None,
                    },
                    &human(),
                )
                .unwrap(),
        );
        applied(
            Executor::new(store)
                .execute(
                    &CreateMission {
                        client_id: hume,
                        quote_id: None,
                        name: "forfait identité".into(),
                        kind: MissionKind::Forfait {
                            budget: Money::from_cents(960_000),
                        },
                        milestones: vec![Milestone {
                            label: "maquettes".into(),
                            share_bps: 4_000,
                            due_on: Some(date(2026, TimeMonth::September, 22)),
                        }],
                        started_on: date(2026, TimeMonth::June, 1),
                    },
                    &human(),
                )
                .unwrap(),
        );
    }

    #[test]
    fn a_fresh_prospect_is_a_first_message_and_a_filed_letter_is_first_contact() {
        let mut store = test_store("cues-stage");
        let today = today();
        applied(
            Executor::new(&mut store)
                .execute(
                    &SetFollowUpSender {
                        email: "moi@lumen.test".into(),
                        name: Some("Moi".into()),
                    },
                    &human(),
                )
                .unwrap(),
        );
        applied(
            Executor::new(&mut store)
                .execute(
                    &CreateProspect {
                        prospect_name: "Ferme du Nord".into(),
                        address: None,
                        representative: None,
                        email: Some("ferme@nord.test".into()),
                        phone: None,
                        name: "visite".into(),
                        amount: Money::ZERO,
                        probability: Probability::new(10).unwrap(),
                        next_action_at: today,
                        source: None,
                    },
                    &human(),
                )
                .unwrap(),
        );
        let porc = applied(
            Executor::new(&mut store)
                .execute(
                    &CreateProspect {
                        prospect_name: "Le porc du Val".into(),
                        address: None,
                        representative: None,
                        email: Some("porc@val.test".into()),
                        phone: None,
                        name: "charcuterie".into(),
                        amount: Money::ZERO,
                        probability: Probability::new(10).unwrap(),
                        next_action_at: today,
                        source: None,
                    },
                    &human(),
                )
                .unwrap(),
        );
        applied(
            Executor::new(&mut store)
                .execute(
                    &crate::follow_up::MarkFollowUpSent {
                        subject: FollowUpSubject::Opportunity(porc),
                        today,
                        subject_line: Some("Premier contact".into()),
                        body: Some("Bonjour,\n\nJe me permets un premier message.\n".into()),
                    },
                    &human(),
                )
                .unwrap(),
        );

        let list = people_list(store.connection(), today).unwrap();
        let ferme = list
            .conversations
            .iter()
            .find(|r| r.name.contains("Ferme"))
            .expect("Ferme");
        assert!(
            ferme
                .cues
                .iter()
                .any(|c| matches!(c, PersonCue::FirstMessage)),
            "{:?}",
            ferme.cues
        );
        assert!(
            !ferme
                .cues
                .iter()
                .any(|c| matches!(c, PersonCue::FollowUpDue { .. })),
            "le premier message ne se dit pas comme une relance : {:?}",
            ferme.cues
        );
        let porc = list
            .conversations
            .iter()
            .find(|r| r.name.contains("porc"))
            .expect("porc");
        assert!(
            porc.cues
                .iter()
                .any(|c| matches!(c, PersonCue::FirstContact)),
            "{:?}",
            porc.cues
        );
        assert!(
            porc.cues
                .iter()
                .any(|c| matches!(c, PersonCue::FollowUpDue { today: false, .. })),
            "le prochain pas est dans trois jours : {:?}",
            porc.cues
        );
    }

    #[test]
    fn a_lost_conversation_can_be_reopened_with_its_letter_and_its_estimate() {
        let mut store = test_store("reopen");
        let today = today();
        applied(
            Executor::new(&mut store)
                .execute(
                    &SetFollowUpSender {
                        email: "moi@lumen.test".into(),
                        name: Some("Moi".into()),
                    },
                    &human(),
                )
                .unwrap(),
        );
        let id = applied(
            Executor::new(&mut store)
                .execute(
                    &CreateProspect {
                        prospect_name: "Le porc du Val".into(),
                        address: None,
                        representative: None,
                        email: Some("porc@val.test".into()),
                        phone: None,
                        name: "charcuterie".into(),
                        amount: Money::from_cents(180_000),
                        probability: Probability::new(20).unwrap(),
                        next_action_at: today,
                        source: None,
                    },
                    &human(),
                )
                .unwrap(),
        );
        applied(
            Executor::new(&mut store)
                .execute(
                    &crate::follow_up::MarkFollowUpSent {
                        subject: FollowUpSubject::Opportunity(id),
                        today,
                        subject_line: Some("Premier contact".into()),
                        body: Some("Bonjour,\n\nPremier message.\n".into()),
                    },
                    &human(),
                )
                .unwrap(),
        );
        applied(
            Executor::new(&mut store)
                .execute(
                    &crate::prospection::LoseOpportunity {
                        opportunity_id: id,
                        reason: crate::domain::LossReason::NoResponse,
                    },
                    &human(),
                )
                .unwrap(),
        );
        let lost = people_list(store.connection(), today).unwrap();
        assert!(lost.conversations.is_empty());
        assert_eq!(lost.stopped.len(), 1);
        assert!(
            lost.stopped[0]
                .cues
                .iter()
                .any(|c| matches!(c, PersonCue::Lost { .. })),
            "{:?}",
            lost.stopped[0].cues
        );

        applied(
            Executor::new(&mut store)
                .execute(
                    &crate::prospection::ReopenOpportunity {
                        opportunity_id: id,
                        next_action_at: today,
                    },
                    &human(),
                )
                .unwrap(),
        );
        let back = people_list(store.connection(), today).unwrap();
        assert!(back.stopped.is_empty());
        let row = back
            .conversations
            .iter()
            .find(|r| r.name.contains("porc"))
            .expect("porc");
        assert!(
            row.cues.iter().any(|c| matches!(c, PersonCue::Resumed)),
            "{:?}",
            row.cues
        );
        assert!(
            row.cues
                .iter()
                .any(|c| matches!(c, PersonCue::EstimateNoted)),
            "{:?}",
            row.cues
        );
        assert!(
            !row.cues
                .iter()
                .any(|c| matches!(c, PersonCue::FollowUpDue { .. })),
            "le jour de la reprise ne dit pas à reprendre : {:?}",
            row.cues
        );
        let dossier = person(store.connection(), "Le porc du Val", today).unwrap();
        assert!(
            dossier
                .history
                .iter()
                .any(|event| matches!(event.kind, HistoryKind::Letter { .. })),
            "la lettre classée est toujours là"
        );
        applied(
            Executor::new(&mut store)
                .execute(
                    &crate::follow_up::MarkFollowUpSent {
                        subject: FollowUpSubject::Opportunity(id),
                        today,
                        subject_line: Some("Reprise".into()),
                        body: Some("Bonjour,\n\nOn reprend.\n".into()),
                    },
                    &human(),
                )
                .unwrap(),
        );
    }

    #[test]
    fn an_empty_vault_has_three_empty_chapters() {
        let store = test_store("empty");
        let list = people_list(store.connection(), today()).unwrap();
        assert!(list.is_empty());
        assert!(list.conversations.is_empty());
        assert!(list.missions.is_empty());
        assert!(list.outgoing.is_empty());
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn the_list_has_camille_atlas_hume_and_leroy_in_the_right_chapters() {
        let mut store = test_store("list");
        seed_people(&mut store);
        let list = people_list(store.connection(), today()).unwrap();

        assert_eq!(list.conversations.len(), 2, "{:?}", list.conversations);
        let camille = list
            .conversations
            .iter()
            .find(|r| r.name.contains("Camille"))
            .expect("Camille");
        assert_eq!(camille.party, "Atelier Nord");
        assert!(
            camille
                .cues
                .iter()
                .any(|c| matches!(c, PersonCue::QuoteSent { .. })),
            "{:?}",
            camille.cues
        );
        assert!(
            camille
                .cues
                .iter()
                .any(|c| matches!(c, PersonCue::FollowUpDue { today: true, .. })),
            "{:?}",
            camille.cues
        );
        match &camille.figure {
            Some(PersonFigure::Money { amount }) => {
                assert_eq!(*amount, Money::from_cents(840_000));
            }
            other => panic!("{other:?}"),
        }

        let martin = list
            .conversations
            .iter()
            .find(|r| r.name.contains("Martin"))
            .expect("Martin");
        assert!(
            !martin
                .cues
                .iter()
                .any(|c| matches!(c, PersonCue::FollowUpDue { today: true, .. })),
            "Martin n'est pas dû aujourd'hui : {:?}",
            martin.cues
        );
        match &martin.figure {
            Some(PersonFigure::Around { amount }) => {
                assert_eq!(*amount, Money::from_cents(1_200_000));
            }
            other => panic!("enveloppe sans devis, pas un montant nu : {other:?}"),
        }

        assert_eq!(list.missions.len(), 2, "{:?}", list.missions);
        let atlas = list
            .missions
            .iter()
            .find(|r| r.name.contains("Atlas"))
            .expect("Atlas");
        assert!(
            atlas.cues.iter().any(|c| matches!(
                c,
                PersonCue::InvoiceOverdue { days } if *days == 7
            )),
            "{:?}",
            atlas.cues
        );
        match &atlas.figure {
            Some(PersonFigure::Money { amount }) => {
                assert_eq!(*amount, Money::from_cents(620_000));
            }
            other => panic!("{other:?}"),
        }
        assert!(
            !list.conversations.iter().any(|r| r.party.contains("Atlas")),
            "Atlas en mission ne doit pas aussi être en conversation"
        );

        let hume = list
            .missions
            .iter()
            .find(|r| r.name.contains("Hume"))
            .expect("Hume");
        assert!(
            hume.cues.iter().any(|c| matches!(
                c,
                PersonCue::NextMilestone { label, .. } if label == "maquettes"
            )),
            "{:?}",
            hume.cues
        );

        assert_eq!(list.outgoing.len(), 1, "{:?}", list.outgoing);
        let leroy = &list.outgoing[0];
        assert_eq!(leroy.name, "Cabinet Leroy");
        assert_eq!(leroy.chapter, PersonChapter::Outgoing);
        assert!(
            leroy
                .cues
                .iter()
                .any(|c| matches!(c, PersonCue::OpeningDebt)),
            "{:?}",
            leroy.cues
        );
        assert!(
            leroy
                .cues
                .iter()
                .any(|c| matches!(c, PersonCue::MatchingDebit)),
            "{:?}",
            leroy.cues
        );
        match &leroy.figure {
            Some(PersonFigure::Money { amount }) => {
                assert_eq!(*amount, Money::from_cents(120_000));
            }
            other => panic!("dette encore ouverte : {other:?}"),
        }
    }

    #[test]
    fn camille_resolves_by_contact_name_and_her_dossier_has_papers_and_write() {
        let mut store = test_store("dossier");
        seed_people(&mut store);
        match resolve_person(store.connection(), "Camille").unwrap() {
            RefMatch::Unique(PersonKey::Client { .. }) => {}
            other => panic!("{other:?}"),
        }
        let dossier = person(store.connection(), "Camille Rivière", today()).unwrap();
        assert_eq!(dossier.name, "Camille Rivière");
        assert_eq!(dossier.party, "Atelier Nord");
        assert!(dossier.not_yet_client);
        assert_eq!(dossier.chapter, PersonChapter::Conversation);
        assert!(dossier.current.quote.is_some(), "{:?}", dossier.current);
        assert!(
            dossier
                .actions
                .iter()
                .any(|a| matches!(a, PersonAction::Write { .. })),
            "{:?}",
            dossier.actions
        );
        assert!(
            dossier
                .actions
                .iter()
                .any(|a| matches!(a, PersonAction::Fiche { .. })),
            "{:?}",
            dossier.actions
        );
        assert!(
            dossier
                .papers
                .iter()
                .any(|p| p.kind == PaperKind::Quote && p.amount == Money::from_cents(840_000)),
            "{:?}",
            dossier.papers
        );
        assert!(
            dossier.history.iter().any(|h| matches!(
                h.kind,
                HistoryKind::Interaction {
                    interaction: InteractionKind::Call
                }
            )),
            "{:?}",
            dossier.history
        );
        assert!(
            dossier
                .history
                .iter()
                .any(|h| matches!(h.kind, HistoryKind::QuoteSent { .. })),
            "{:?}",
            dossier.history
        );
    }

    #[test]
    fn atlas_dossier_has_the_project_and_the_overdue_invoice() {
        let mut store = test_store("atlas");
        seed_people(&mut store);
        let dossier = person(store.connection(), "Atlas", today()).unwrap();
        assert_eq!(dossier.chapter, PersonChapter::Mission);
        assert!(!dossier.not_yet_client);
        assert!(dossier.project.is_some(), "{:?}", dossier.project);
        assert!(
            dossier.current.invoice.as_ref().is_some_and(|p| matches!(
                p.status,
                PaperStatus::InvoiceOutstanding { days_overdue: 7 }
            )),
            "{:?}",
            dossier.current.invoice
        );
    }

    #[test]
    fn dossier_paper_says_no_longer_expected_not_credited() {
        let mut store = test_store("written-off-paper");
        seed_people(&mut store);
        let before = person(store.connection(), "Atlas", today()).unwrap();
        let paper = before
            .papers
            .iter()
            .find(|p| p.kind == PaperKind::Invoice)
            .expect("facture Atlas");
        let invoice_id = paper.invoice_id.expect("id");
        let ttc = paper.amount;
        applied(
            Executor::new(&mut store)
                .execute(
                    &WriteOffReceivable {
                        invoice_id,
                        written_off_on: today(),
                    },
                    &human(),
                )
                .unwrap(),
        );
        let dossier = person(store.connection(), "Atlas", today()).unwrap();
        let paper = dossier
            .papers
            .iter()
            .find(|p| p.invoice_id == Some(invoice_id))
            .expect("facture Atlas");
        assert_eq!(
            paper.status,
            PaperStatus::InvoiceWrittenOff,
            "une perte n'est ni un avoir ni un encaissement : {:?}",
            paper.status
        );
        assert_eq!(paper.amount, ttc);
        assert!(
            !matches!(
                paper.status,
                PaperStatus::InvoicePaid | PaperStatus::InvoiceCredited
            ),
            "{:?}",
            paper.status
        );
        assert!(
            dossier.current.invoice.is_none(),
            "plus une créance courante : {:?}",
            dossier.current.invoice
        );
    }

    #[test]
    fn an_unknown_name_is_not_found() {
        let store = test_store("missing");
        let err = person(store.connection(), "Personne", today()).unwrap_err();
        assert!(err.to_string().contains("introuvable"), "{err}");
    }

    #[test]
    fn a_mission_uuid_opens_the_client_dossier() {
        let mut store = test_store("uuid");
        seed_people(&mut store);
        let mission = list_missions_with(store.connection(), MissionFilter::ACTIVE)
            .unwrap()
            .into_iter()
            .find(|m| m.name.contains("forfait"))
            .unwrap();
        let dossier = person(store.connection(), &mission.id.to_string(), today()).unwrap();
        assert!(dossier.party.contains("Hume"), "{}", dossier.party);
        assert_eq!(
            dossier
                .project
                .as_ref()
                .unwrap()
                .next_milestone
                .as_ref()
                .unwrap()
                .label,
            "maquettes"
        );
    }

    fn record_named_expense(
        store: &mut Store,
        label: &str,
        supplier: &str,
        cents: i64,
        on: Date,
        paid_by: crate::domain::ExpensePaidBy,
        receipt_filename: Option<&str>,
    ) {
        applied(
            Executor::new(store)
                .execute(
                    &RecordExpense {
                        label: label.into(),
                        category: crate::domain::ExpenseCategory::Software,
                        amount: Money::from_cents(cents),
                        vat_rate: VatRate::Zero,
                        vat_deductible: Money::ZERO,
                        incurred_on: on,
                        receipt_hash: receipt_filename.map(|_| "abc".into()),
                        receipt_filename: receipt_filename.map(str::to_string),
                        bank_transaction_id: None,
                        supplier: Some(supplier.into()),
                        paid_by,
                    },
                    &human(),
                )
                .unwrap(),
        );
    }

    #[test]
    fn a_paid_vendor_is_spent_not_owed_and_lists_its_notes() {
        let mut store = test_store("tiime");
        record_named_expense(
            &mut store,
            "Abonnement mars",
            "Tiime",
            2_000,
            date(2026, TimeMonth::January, 1),
            crate::domain::ExpensePaidBy::Company,
            Some("facture-mars.pdf"),
        );
        record_named_expense(
            &mut store,
            "Abonnement février",
            "Tiime",
            2_000,
            date(2026, TimeMonth::February, 1),
            crate::domain::ExpensePaidBy::Company,
            None,
        );
        record_named_expense(
            &mut store,
            "Abonnement mars bis",
            "Tiime",
            2_000,
            date(2026, TimeMonth::March, 1),
            crate::domain::ExpensePaidBy::Associate,
            None,
        );

        let list = people_list(store.connection(), today()).unwrap();
        assert_eq!(list.outgoing.len(), 1, "{:?}", list.outgoing);
        let tiime = &list.outgoing[0];
        assert_eq!(tiime.name, "Tiime");
        match &tiime.figure {
            Some(PersonFigure::Spent { amount }) => {
                assert_eq!(*amount, Money::from_cents(6_000));
            }
            other => panic!("payé, pas une dette : {other:?}"),
        }
        assert!(
            tiime
                .cues
                .iter()
                .any(|c| matches!(c, PersonCue::CadenceMonthly)),
            "{:?}",
            tiime.cues
        );
        assert!(
            tiime.cues.iter().any(|c| matches!(
                c,
                PersonCue::LastNote { on } if *on == date(2026, TimeMonth::March, 1)
            )),
            "{:?}",
            tiime.cues
        );
        assert!(
            !tiime
                .cues
                .iter()
                .any(|c| matches!(c, PersonCue::OpeningDebt | PersonCue::MatchingDebit)),
            "{:?}",
            tiime.cues
        );

        let dossier = person(store.connection(), "Tiime", today()).unwrap();
        assert_eq!(dossier.chapter, PersonChapter::Outgoing);
        let outgoing = dossier.outgoing.as_ref().expect("chemise");
        assert_eq!(outgoing.total_paid, Money::from_cents(6_000));
        assert_eq!(outgoing.cadence, OutgoingCadence::Monthly);
        assert!(outgoing.owed.is_none());
        assert_eq!(outgoing.notes.len(), 3);
        assert_eq!(outgoing.notes[0].label, "Abonnement mars bis");
        assert_eq!(
            outgoing.notes[0].paid_by,
            crate::domain::ExpensePaidBy::Associate
        );
        assert_eq!(
            outgoing.notes[2].receipt_filename.as_deref(),
            Some("facture-mars.pdf")
        );
        assert_eq!(outgoing.since, date(2026, TimeMonth::January, 1));
    }

    #[test]
    fn a_quiet_vendor_has_quiet_since_not_a_debt() {
        let mut store = test_store("numbr");
        record_named_expense(
            &mut store,
            "Honoraires 2025",
            "Numbr Hauts de France",
            180_000,
            date(2026, TimeMonth::January, 12),
            crate::domain::ExpensePaidBy::Company,
            None,
        );
        let list = people_list(store.connection(), today()).unwrap();
        let numbr = list
            .outgoing
            .iter()
            .find(|r| r.name.contains("Numbr"))
            .expect("Numbr");
        match &numbr.figure {
            Some(PersonFigure::Spent { amount }) => {
                assert_eq!(*amount, Money::from_cents(180_000));
            }
            other => panic!("{other:?}"),
        }
        assert!(
            numbr.cues.iter().any(|c| matches!(
                c,
                PersonCue::QuietSince { on } if *on == date(2026, TimeMonth::January, 12)
            )),
            "{:?}",
            numbr.cues
        );
    }

    #[test]
    fn opening_debt_clears_once_the_debit_is_filed() {
        let mut store = test_store("leroy-settled");
        seed_people(&mut store);
        let before = people_list(store.connection(), today()).unwrap();
        assert!(
            before.outgoing[0]
                .cues
                .iter()
                .any(|c| matches!(c, PersonCue::OpeningDebt)),
            "{:?}",
            before.outgoing[0].cues
        );

        let tx = crate::billing::list_bank_transactions(store.connection()).unwrap()[0].id;
        applied(
            Executor::new(&mut store)
                .execute(
                    &crate::billing::SettleBankTransaction {
                        transaction_id: tx,
                        account: "401000".parse().unwrap(),
                        label: None,
                    },
                    &human(),
                )
                .unwrap(),
        );

        let after = people_list(store.connection(), today()).unwrap();
        let leroy = after
            .outgoing
            .iter()
            .find(|r| r.name.contains("Leroy"))
            .expect("Leroy reste");
        assert!(
            !leroy
                .cues
                .iter()
                .any(|c| matches!(c, PersonCue::OpeningDebt | PersonCue::MatchingDebit)),
            "la dette ne survit pas au rangement : {:?}",
            leroy.cues
        );
        match &leroy.figure {
            Some(PersonFigure::Spent { amount }) => {
                assert_eq!(*amount, Money::from_cents(120_000));
            }
            other => panic!("après rangement, versé : {other:?}"),
        }
    }

    #[test]
    fn outgoing_puts_an_open_debt_ahead_of_a_quiet_vendor() {
        let mut store = test_store("order");
        seed_people(&mut store);
        record_named_expense(
            &mut store,
            "Abonnement",
            "Tiime",
            2_000,
            date(2026, TimeMonth::March, 1),
            crate::domain::ExpensePaidBy::Company,
            None,
        );
        let list = people_list(store.connection(), today()).unwrap();
        let names: Vec<&str> = list.outgoing.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["Cabinet Leroy", "Tiime"], "{names:?}");
    }

    #[test]
    fn leroy_dossier_carries_the_note_and_the_opening_debt() {
        let mut store = test_store("leroy-dossier");
        seed_people(&mut store);
        let dossier = person(store.connection(), "Cabinet Leroy", today()).unwrap();
        assert_eq!(dossier.chapter, PersonChapter::Outgoing);
        assert!(dossier.current.opening_debt);
        assert!(dossier.current.matching_debit);
        let outgoing = dossier.outgoing.as_ref().expect("chemise");
        assert_eq!(outgoing.owed, Some(Money::from_cents(120_000)));
        assert_eq!(outgoing.notes.len(), 1);
        assert_eq!(outgoing.notes[0].label, "honoraires clôture");
        assert!(
            dossier
                .actions
                .iter()
                .any(|a| matches!(a, PersonAction::FileStatement)),
            "{:?}",
            dossier.actions
        );
    }
}
