//! `freeflow society` — paysage, se payer, relevé, chapitres. Lectures pures, `today` d'adaptateur.

use clap::Subcommand;
use freeflow_core::clock::today_local;
use freeflow_core::domain::{Money, format_date};
use freeflow_core::fiscal::FiscalDeadlineKind;
use freeflow_core::society::{
    BeatKind, BeatWhen, ClosingStory, DepositPlace, DividendClosed, DividendDoor, Duty,
    IdentityCard, PayYourself, SocietyHome, StatementMove, StatementReading, closing_story,
    pay_yourself, society_duties, society_home, society_identity, statement_moves,
};
use freeflow_core::store::Store;
use time::Date;

use crate::error::CliError;
use crate::output::{HumanRender, format_value, key_values};
use crate::parsers::parse_date;
use crate::table;

impl HumanRender for PayYourself {
    fn render_human(&self) -> String {
        let dividend = match &self.dividend {
            DividendDoor::Open { available } => format!("ouvert · {available}"),
            DividendDoor::Closed {
                reason: DividendClosed::YearNotClosed,
            } => "fermé — l'exercice n'est pas clos".into(),
            DividendDoor::Closed {
                reason: DividendClosed::NoProfit,
            } => "fermé — pas de bénéfice".into(),
        };
        key_values(&[
            ("possible", format!("{} ce mois-ci", self.possible)),
            ("piste gardée", format!("{} mois", self.runway_kept_months)),
            ("brûlage", self.monthly_burn.to_string()),
            ("banque", self.bank.to_string()),
            (
                "salaire",
                format!(
                    "{} nets · société {}",
                    self.salary.net, self.salary.company_cost
                ),
            ),
            ("dividende", dividend),
            (
                "1 000 € de dividendes",
                self.dividend_cost_per_thousand.to_string(),
            ),
            (
                "cette année",
                match self.annual.target {
                    Some(t) => format!("{} versés / {t}", self.annual.paid),
                    None => format!("{} versés", self.annual.paid),
                },
            ),
        ])
    }
}

impl HumanRender for SocietyHome {
    fn render_human(&self) -> String {
        let mut out = String::new();
        if let Some(name) = &self.identity.name {
            out.push_str(name);
            out.push('\n');
        }
        out.push_str(&key_values(&[
            ("banque", self.landscape.bank.to_string()),
            ("possible", format!("{} ce mois-ci", self.pay.possible)),
            (
                "relevé",
                match self.unmatched {
                    0 => "tout est lu".into(),
                    1 => "1 mouvement sans lecture".into(),
                    n => format!("{n} mouvements sans lecture"),
                },
            ),
        ]));
        if let Some(d) = &self.next_duty {
            out.push('\n');
            out.push_str(&duty_line(d));
        }
        out
    }
}

impl HumanRender for Vec<Duty> {
    fn render_human(&self) -> String {
        if self.is_empty() {
            return "Rien à déposer dans l'horizon.".into();
        }
        self.iter().map(duty_line).collect::<Vec<_>>().join("\n")
    }
}

fn duty_line(d: &Duty) -> String {
    let amount = d
        .amount
        .filter(|m| *m != Money::ZERO)
        .map(|m| format!(" · {m}"))
        .unwrap_or_default();
    format!(
        "{} {}{amount} · {}",
        format_date(d.due_on),
        deadline_fr(d.kind),
        deposit_fr(d.deposit)
    )
}

fn deadline_fr(kind: FiscalDeadlineKind) -> &'static str {
    match kind {
        FiscalDeadlineKind::Ca3 => "TVA du trimestre",
        FiscalDeadlineKind::VatInstalment => "acompte de TVA",
        FiscalDeadlineKind::Ca12 => "TVA de l'année",
        FiscalDeadlineKind::IsAcompte => "acompte d'impôt sur les sociétés",
        FiscalDeadlineKind::IsSolde => "solde d'impôt sur les sociétés",
        FiscalDeadlineKind::Cfe => "cotisation foncière",
        FiscalDeadlineKind::Liasse => "liasse fiscale",
        FiscalDeadlineKind::ApprovalMeeting => "approbation des comptes",
        FiscalDeadlineKind::AccountsFiling => "dépôt des comptes",
        FiscalDeadlineKind::Dsn => "déclaration sociale",
        FiscalDeadlineKind::Das2 => "honoraires à déclarer",
        FiscalDeadlineKind::Dividends2777 => "prélèvements sur dividendes",
    }
}

fn deposit_fr(place: DepositPlace) -> &'static str {
    match place {
        DepositPlace::ImpotsGouv => "impots.gouv.fr",
        DepositPlace::Post => "avis par la poste",
        DepositPlace::Greffe => "greffe",
        DepositPlace::NetEntreprises => "net-entreprises",
        DepositPlace::Internal => "chez vous",
    }
}

impl HumanRender for ClosingStory {
    fn render_human(&self) -> String {
        let mut out = String::new();
        if let Some(days) = self.days_left {
            out.push_str(&format!("{days} jours.\n"));
        }
        for beat in &self.beats {
            let when = match beat.when {
                BeatWhen::Done => "fait",
                BeatWhen::Today => "aujourd'hui",
                BeatWhen::Later => beat.on.map_or("ensuite", |_| "plus tard"),
            };
            let on = beat.on.map(format_date).unwrap_or_default();
            let text = match &beat.kind {
                BeatKind::OpeningBalance { recorded: true } => {
                    "Bilan d'ouverture repris".to_string()
                }
                BeatKind::OpeningBalance { recorded: false } => {
                    "Reprendre le bilan d'ouverture".to_string()
                }
                BeatKind::FileStatement { unmatched: 0 } => "Relevé lu".into(),
                BeatKind::FileStatement { unmatched: 1 } => "Ranger un mouvement du relevé".into(),
                BeatKind::FileStatement { unmatched } => {
                    format!("Ranger {unmatched} mouvements du relevé")
                }
                BeatKind::Receipts { missing: true } => "Justificatifs manquants".into(),
                BeatKind::Receipts { missing: false } => "Justificatifs de l'exercice".into(),
                BeatKind::CloseAccounts { ends_on } => {
                    format!(
                        "Arrêter les comptes — pas avant le {}",
                        format_date(*ends_on)
                    )
                }
                BeatKind::ThenApproveAndFile => {
                    "Décider de l'affectation, faire le PV, déposer".into()
                }
            };
            if on.is_empty() {
                out.push_str(&format!("{when:12} {text}\n"));
            } else {
                out.push_str(&format!("{when:12} {text} ({on})\n"));
            }
        }
        out.pop();
        out
    }
}

impl HumanRender for Vec<StatementMove> {
    fn render_human(&self) -> String {
        if self.is_empty() {
            return "Tout est lu.".into();
        }
        let rows: Vec<Vec<String>> = self
            .iter()
            .map(|m| {
                vec![
                    format_date(m.occurred_on),
                    m.amount.to_string(),
                    m.description.clone(),
                    reading_fr(&m.suggested),
                ]
            })
            .collect();
        table::render(&["date", "montant", "libellé", "lecture"], &rows)
    }
}

fn reading_fr(r: &StatementReading) -> String {
    match r {
        StatementReading::Expense => "dépense".into(),
        StatementReading::Debt { label, .. } => format!("dette ({label})"),
        StatementReading::SelfPay => "te payer".into(),
    }
}

impl HumanRender for IdentityCard {
    fn render_human(&self) -> String {
        key_values(&[
            ("nom", self.name.clone().unwrap_or_else(|| "—".into())),
            (
                "forme",
                self.legal_form.clone().unwrap_or_else(|| "—".into()),
            ),
            (
                "capital",
                self.capital.map_or_else(|| "—".into(), |c| c.to_string()),
            ),
            (
                "président",
                self.president_name.clone().unwrap_or_else(|| "—".into()),
            ),
            ("siren", self.siren.clone().unwrap_or_else(|| "—".into())),
            (
                "clôture",
                self.year_end
                    .map(|e| format!("{:02}/{:02}", e.day(), e.month()))
                    .unwrap_or_else(|| "—".into()),
            ),
        ])
    }
}

#[derive(Debug, Subcommand)]
pub enum SocietyCommand {
    /// Identité courte, paysage, conversations, chapitres.
    Show {
        #[arg(long, value_parser = parse_date)]
        today: Option<Date>,
    },
    /// Montant possible ce mois-ci sans casser la piste.
    Pay {
        #[arg(long, value_parser = parse_date)]
        today: Option<Date>,
    },
    /// Ce que tu dois à l'État : dates, montants, où déposer.
    Duties {
        #[arg(long, value_parser = parse_date)]
        today: Option<Date>,
    },
    /// Clore l'exercice, en phrases.
    Closing {
        #[arg(long, value_parser = parse_date)]
        today: Option<Date>,
    },
    /// Le relevé : chaque mouvement, une lecture.
    Statement {
        #[arg(long, value_parser = parse_date)]
        today: Option<Date>,
    },
    /// La carte d'identité de la société.
    Identity,
}

pub fn run(cmd: SocietyCommand, store: &Store, json: bool) -> Result<String, CliError> {
    match cmd {
        SocietyCommand::Show { today } => {
            let today = today.unwrap_or_else(today_local);
            let home = society_home(store.connection(), today)?;
            Ok(format_value(&home, json))
        }
        SocietyCommand::Pay { today } => {
            let today = today.unwrap_or_else(today_local);
            let pay = pay_yourself(store.connection(), today)?;
            Ok(format_value(&pay, json))
        }
        SocietyCommand::Duties { today } => {
            let today = today.unwrap_or_else(today_local);
            let duties = society_duties(store.connection(), today)?;
            Ok(format_value(&duties, json))
        }
        SocietyCommand::Closing { today } => {
            let today = today.unwrap_or_else(today_local);
            let story = closing_story(store.connection(), today)?;
            Ok(format_value(&story, json))
        }
        SocietyCommand::Statement { today } => {
            let today = today.unwrap_or_else(today_local);
            let moves = statement_moves(store.connection(), today)?;
            Ok(format_value(&moves, json))
        }
        SocietyCommand::Identity => {
            let card = society_identity(store.connection())?;
            Ok(format_value(&card, json))
        }
    }
}
