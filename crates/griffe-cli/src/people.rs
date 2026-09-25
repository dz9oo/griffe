//! `freeflow people` — liste et dossier. Lectures pures, `today` d'adaptateur.

use clap::Subcommand;
use griffe_core::clock::today_local;
use griffe_core::domain::format_date;
use griffe_core::people::{
    HistoryKind, OutgoingCadence, PaperKind, PaperStatus, PeopleList, PersonAction, PersonChapter,
    PersonCue, PersonDossier, PersonFigure, PersonRow, people_list, person,
};
use griffe_core::store::Store;
use time::Date;

use crate::error::CliError;
use crate::output::{HumanRender, format_value, key_values};
use crate::parsers::parse_date;
use crate::table;

impl HumanRender for PeopleList {
    fn render_human(&self) -> String {
        if self.is_empty() {
            return "Personne pour l'instant.".into();
        }
        let mut out = String::new();
        out.push_str("En conversation\n");
        out.push_str(&chapter_table(&self.conversations));
        out.push_str("\nEn mission\n");
        out.push_str(&chapter_table(&self.missions));
        out.push_str("\nChez qui ça sort\n");
        out.push_str(&chapter_table(&self.outgoing));
        out
    }
}

fn chapter_table(rows: &[PersonRow]) -> String {
    if rows.is_empty() {
        return "  (aucun)\n".into();
    }
    let table_rows: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            vec![
                r.name.clone(),
                cues_fr(&r.cues),
                figure_fr(r.figure.as_ref()),
            ]
        })
        .collect();
    table::render(&["nom", "en cours", "chiffre"], &table_rows)
}

fn figure_fr(figure: Option<&PersonFigure>) -> String {
    match figure {
        Some(PersonFigure::Money { amount }) => amount.to_string(),
        Some(PersonFigure::Around { amount }) => format!("autour de {amount}"),
        Some(PersonFigure::Spent { amount }) => format!("{amount} versés"),
        Some(PersonFigure::Days { days }) => format!("{days} j"),
        None => String::new(),
    }
}

fn cues_fr(cues: &[PersonCue]) -> String {
    cues.iter().map(cue_fr).collect::<Vec<_>>().join(" · ")
}

fn cue_fr(cue: &PersonCue) -> String {
    match cue {
        PersonCue::QuoteSent { .. } => "estimation envoyée".into(),
        PersonCue::FollowUpDue { today: true, .. } => "à relancer aujourd'hui".into(),
        PersonCue::FollowUpDue { on, .. } => format!("à relancer le {}", format_date(*on)),
        PersonCue::FirstExchange { on } => format!("premier échange le {}", format_date(*on)),
        PersonCue::NothingScheduled => "rien de posé".into(),
        PersonCue::InvoiceOverdue { days } => format!("facture en retard · {days} jours"),
        PersonCue::InvoiceOutstanding => "facture à encaisser".into(),
        PersonCue::NextMilestone { on, label } => {
            format!("{label} le {}", format_date(*on))
        }
        PersonCue::OpeningDebt => "dette reprise au bilan".into(),
        PersonCue::MatchingDebit => "un débit correspond".into(),
        PersonCue::CadenceMonthly => "tous les mois".into(),
        PersonCue::LastNote { on } => format!("dernière note le {}", format_date(*on)),
        PersonCue::QuietSince { on } => format!("plus rien depuis {}", format_date(*on)),
    }
}

impl HumanRender for PersonDossier {
    fn render_human(&self) -> String {
        let mut pairs = vec![
            ("nom", self.name.clone()),
            ("société", self.party.clone()),
            ("chapitre", chapter_fr(self.chapter).into()),
        ];
        if self.not_yet_client {
            pairs.push(("statut", "pas encore cliente".into()));
        }
        if let Some(on) = self.since {
            pairs.push(("depuis", format_date(on)));
        }
        if let Some(amount) = self.current.amount {
            pairs.push(("montant", amount.to_string()));
        }
        let mut out = key_values(&pairs);
        if let Some(project) = &self.project {
            out.push_str(&format!(
                "\nprojet : {} ({})",
                project.name,
                shape_fr(project.shape)
            ));
            if let Some(ms) = &project.next_milestone
                && let Some(on) = ms.due_on
            {
                out.push_str(&format!(" · jalon {} le {}", ms.label, format_date(on)));
            }
        }
        if !self.papers.is_empty() {
            out.push_str("\npapiers :\n");
            for paper in &self.papers {
                out.push_str(&format!("  {} — {}\n", paper_line(paper), paper.amount));
            }
        }
        if !self.history.is_empty() {
            out.push_str("histoire :\n");
            for event in &self.history {
                out.push_str(&format!(
                    "  {} — {}\n",
                    format_date(event.on),
                    history_fr(event)
                ));
            }
        }
        if let Some(outgoing) = &self.outgoing {
            out.push_str(&format!(
                "\nnotes : {} depuis {} · {}\n",
                outgoing.total_paid,
                format_date(outgoing.since),
                cadence_fr(outgoing.cadence)
            ));
            for note in &outgoing.notes {
                let receipt = if note.receipt_filename.is_some() {
                    " · justificatif"
                } else {
                    " · pas de justificatif"
                };
                out.push_str(&format!(
                    "  {} — {} · {}{receipt}\n",
                    format_date(note.on),
                    note.label,
                    note.amount
                ));
            }
        }
        if !self.actions.is_empty() {
            let acts = self
                .actions
                .iter()
                .map(action_fr)
                .collect::<Vec<_>>()
                .join(", ");
            out.push_str(&format!("actions : {acts}"));
        }
        out
    }
}

fn cadence_fr(cadence: OutgoingCadence) -> &'static str {
    match cadence {
        OutgoingCadence::Monthly => "tous les mois",
        OutgoingCadence::Once => "une fois",
        OutgoingCadence::Occasional => "de temps en temps",
    }
}

fn chapter_fr(chapter: PersonChapter) -> &'static str {
    match chapter {
        PersonChapter::Conversation => "en conversation",
        PersonChapter::Mission => "en mission",
        PersonChapter::Outgoing => "chez qui ça sort",
    }
}

fn shape_fr(shape: griffe_core::people::MissionShape) -> &'static str {
    match shape {
        griffe_core::people::MissionShape::Regie => "régie",
        griffe_core::people::MissionShape::Forfait => "forfait",
        griffe_core::people::MissionShape::Recurrent => "récurrent",
    }
}

fn paper_line(paper: &griffe_core::people::Paper) -> String {
    let kind = match paper.kind {
        PaperKind::Quote => "estimation",
        PaperKind::Invoice => "facture",
    };
    let status = match &paper.status {
        PaperStatus::QuoteDraft => "brouillon",
        PaperStatus::QuoteSent => "envoyé",
        PaperStatus::QuoteAccepted => "accepté",
        PaperStatus::QuoteDeclined => "décliné",
        PaperStatus::QuoteExpired => "expiré",
        PaperStatus::InvoiceOutstanding { days_overdue } if *days_overdue > 0 => "échue",
        PaperStatus::InvoiceOutstanding { .. } => "à encaisser",
        PaperStatus::InvoicePaid => "encaissée",
        PaperStatus::InvoiceCredited => "annulée par avoir",
        PaperStatus::InvoiceWrittenOff => "on ne l'attend plus",
    };
    match &paper.number {
        Some(n) => format!("{kind} {n} · {status}"),
        None => format!("{kind} · {status}"),
    }
}

fn history_fr(event: &griffe_core::people::HistoryEvent) -> String {
    match &event.kind {
        HistoryKind::Interaction { interaction } => {
            let kind = match interaction {
                griffe_core::domain::InteractionKind::Call => "téléphone",
                griffe_core::domain::InteractionKind::Email => "e-mail",
                griffe_core::domain::InteractionKind::Meeting => "rencontre physique",
                griffe_core::domain::InteractionKind::Visio => "visioconférence",
                griffe_core::domain::InteractionKind::Note => "note",
            };
            match &event.note {
                Some(n) => format!("{kind}. {n}"),
                None => kind.into(),
            }
        }
        HistoryKind::Letter { subject, body } => {
            format!("lettre — {subject}\n    {body}")
        }
        HistoryKind::QuoteSent { .. } => "Estimation envoyée.".into(),
        HistoryKind::QuoteAccepted => "Estimation acceptée.".into(),
        HistoryKind::InvoiceIssued { number } => format!("Facture {number}."),
    }
}

fn action_fr(action: &PersonAction) -> &'static str {
    match action {
        PersonAction::Write { .. } => "Écrire",
        PersonAction::Fiche { .. } => "La fiche",
        PersonAction::Quote { .. } => "L'estimation",
        PersonAction::LogMeeting { .. } => "Noter une rencontre",
        PersonAction::Snooze { .. } => "Reporter",
        PersonAction::FileStatement => "Ranger le mouvement",
    }
}

#[derive(Debug, Subcommand)]
pub enum PeopleCommand {
    /// Liste unique : en conversation, en mission, chez qui ça sort.
    List {
        #[arg(long, value_parser = parse_date)]
        today: Option<Date>,
    },
    /// Dossier d'une personne (nom, préfixe, UUID).
    Show {
        #[arg(value_name = "RÉF")]
        reference: String,
        #[arg(long, value_parser = parse_date)]
        today: Option<Date>,
    },
}

pub fn run(cmd: PeopleCommand, store: &Store, json: bool) -> Result<String, CliError> {
    match cmd {
        PeopleCommand::List { today } => {
            let today = today.unwrap_or_else(today_local);
            let list = people_list(store.connection(), today)?;
            Ok(format_value(&list, json))
        }
        PeopleCommand::Show { reference, today } => {
            let today = today.unwrap_or_else(today_local);
            let dossier = person(store.connection(), &reference, today)?;
            Ok(format_value(&dossier, json))
        }
    }
}
