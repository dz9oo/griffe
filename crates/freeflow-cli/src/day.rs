//! `freeflow day` — mât, gestes, mois. Lectures pures, `today` d'adaptateur.

use clap::Subcommand;
use freeflow_core::clock::today_local;
use freeflow_core::day::{
    DayGesture, GestureSource, GestureVerb, Mast, MastSignal, MonthEventKind, MonthView,
    day_gestures, day_mast, day_month,
};
use freeflow_core::domain::{Month, format_date};
use freeflow_core::store::Store;
use time::Date;

use crate::error::CliError;
use crate::output::{HumanRender, format_value, key_values};
use crate::parsers::{parse_date, parse_month};
use crate::table;

impl HumanRender for Mast {
    fn render_human(&self) -> String {
        let piste = self
            .runway_months
            .map_or_else(|| "—".to_string(), |m| format!("{m} mois"));
        let encaisser = self.receivables.first().map_or_else(
            || "rien".to_string(),
            |r| format!("{} chez {}", r.amount, r.party),
        );
        let mut pairs = vec![
            ("banque", self.bank.to_string()),
            ("piste", piste),
            ("à encaisser", encaisser),
            ("conversations", self.open_conversations.to_string()),
        ];
        if !self.signals.is_empty() {
            let signals = self
                .signals
                .iter()
                .map(signal_label)
                .collect::<Vec<_>>()
                .join(" · ");
            pairs.push(("signaux", signals));
        }
        key_values(&pairs)
    }
}

fn signal_label(signal: &MastSignal) -> String {
    match signal {
        MastSignal::RunwayAlarm { months } => format!("piste sous 2 mois ({months})"),
        MastSignal::RunwayUnbacked { months } => {
            format!("piste de {months} mois, rien derrière")
        }
        MastSignal::PipelineEmpty => "aucune conversation ouverte".into(),
        MastSignal::PipelineThin { party, .. } => {
            format!("une seule conversation ({party})")
        }
        MastSignal::ReceivableStale { party, days } => {
            format!("{party} : {days} jours")
        }
    }
}

impl HumanRender for Vec<DayGesture> {
    fn render_human(&self) -> String {
        if self.is_empty() {
            return "Rien aujourd'hui.".into();
        }
        let rows: Vec<Vec<String>> = self
            .iter()
            .enumerate()
            .map(|(i, g)| {
                vec![
                    (i + 1).to_string(),
                    verb_fr(g.verb).into(),
                    gesture_detail(g),
                ]
            })
            .collect();
        table::render(&["#", "verbe", "détail"], &rows)
    }
}

fn verb_fr(verb: GestureVerb) -> &'static str {
    match verb {
        GestureVerb::Setup => "configurer",
        GestureVerb::Write => "écrire",
        GestureVerb::Remind => "relancer",
        GestureVerb::FileStatement => "ranger le relevé",
        GestureVerb::KnowVat => "savoir pour la TVA",
        GestureVerb::KnowDuty => "savoir pour l'État",
    }
}

fn gesture_detail(g: &DayGesture) -> String {
    match &g.source {
        GestureSource::Setup { step } => step.text().to_string(),
        GestureSource::FollowUp {
            contact_name,
            party,
            title,
            ..
        } => {
            let who = contact_name.as_deref().unwrap_or(party.as_str());
            format!("{who} — {title}")
        }
        GestureSource::BankStatement { unmatched } => {
            if *unmatched == 1 {
                "1 mouvement sans lecture".into()
            } else {
                format!("{unmatched} mouvements sans lecture")
            }
        }
        GestureSource::StateDuty { due_on, amount, .. } => {
            let amt = amount
                .filter(|m| *m != freeflow_core::domain::Money::ZERO)
                .map(|m| format!("{m} · "))
                .unwrap_or_default();
            format!("{amt}avant le {}", format_date(*due_on))
        }
    }
}

impl HumanRender for MonthView {
    fn render_human(&self) -> String {
        let mut out = format!("Mois {}\n", self.month);
        if self.tracks.is_empty() {
            out.push_str("Aucune mission ce mois-ci.\n");
        } else {
            let rows: Vec<Vec<String>> = self
                .tracks
                .iter()
                .map(|t| {
                    let end = t
                        .ended_on
                        .map_or_else(|| "en cours".to_string(), format_date);
                    vec![t.client_name.clone(), t.name.clone(), end]
                })
                .collect();
            out.push_str(&table::render(&["client", "mission", "fin"], &rows));
            out.push('\n');
        }
        let from_today: Vec<_> = self
            .events
            .iter()
            .filter(|e| e.on >= self.month.first_day())
            .collect();
        if from_today.is_empty() {
            out.push_str("Aucun événement.");
        } else {
            let rows: Vec<Vec<String>> = from_today
                .iter()
                .map(|e| {
                    vec![
                        format_date(e.on),
                        event_kind_fr(e.kind).into(),
                        e.party.clone().unwrap_or_default(),
                        e.title.clone(),
                    ]
                })
                .collect();
            out.push_str(&table::render(&["date", "nature", "qui", "titre"], &rows));
        }
        if let Some(on) = self.empty_after {
            out.push_str(&format!(
                "\nAprès le {}, plus aucune mission sur le papier.",
                format_date(on)
            ));
            if self.next_month_empty {
                out.push_str(" Le mois suivant est vide.");
            }
        }
        out
    }
}

fn event_kind_fr(kind: MonthEventKind) -> &'static str {
    match kind {
        MonthEventKind::FollowUp => "relance",
        MonthEventKind::InvoiceDue => "échéance",
        MonthEventKind::Meeting => "rencontre",
        MonthEventKind::Milestone => "jalon",
        MonthEventKind::MissionEnd => "fin de mission",
        MonthEventKind::StateDuty => "État",
        MonthEventKind::YearEnd => "exercice",
    }
}

#[derive(Debug, Subcommand)]
pub enum DayCommand {
    /// Banque, piste en mois, à encaisser, signaux.
    Mast {
        #[arg(long, value_parser = parse_date)]
        today: Option<Date>,
    },
    /// Gestes du jour : relances, relevé, obligation d'État.
    Gestures {
        #[arg(long, value_parser = parse_date)]
        today: Option<Date>,
    },
    /// Grille du mois : événements et bandes de missions.
    Month {
        /// Mois `AAAA-MM` (défaut : le mois de `today`).
        #[arg(long, value_parser = parse_month)]
        month: Option<Month>,
        #[arg(long, value_parser = parse_date)]
        today: Option<Date>,
    },
}

pub fn run(cmd: DayCommand, store: &Store, json: bool) -> Result<String, CliError> {
    match cmd {
        DayCommand::Mast { today } => {
            let today = today.unwrap_or_else(today_local);
            let mast = day_mast(store.connection(), today)?;
            Ok(format_value(&mast, json))
        }
        DayCommand::Gestures { today } => {
            let today = today.unwrap_or_else(today_local);
            let gestes = day_gestures(store.connection(), today)?;
            Ok(format_value(&gestes, json))
        }
        DayCommand::Month { month, today } => {
            let today = today.unwrap_or_else(today_local);
            let month = month.unwrap_or_else(|| {
                Month::new(today.year(), u8::from(today.month()))
                    .expect("le mois courant est toujours valide")
            });
            let view = day_month(store.connection(), month, today)?;
            Ok(format_value(&view, json))
        }
    }
}
