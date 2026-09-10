//! `freeflow society` — paysage, se payer, relevé, chapitres. Lectures pures, `today` d'adaptateur.

use clap::Subcommand;
use freeflow_core::app::{ExecutionContext, Executor};
use freeflow_core::clock::today_local;
use freeflow_core::domain::{Money, format_date};
use freeflow_core::fiscal::{FiscalDeadlineKind, VatFilingScheme};
use freeflow_core::society::{
    AmountBasis, AmountStory, BeatKind, BeatWhen, BoxCoverage, BoxRole, ClosingStory,
    DeleteVatCarryIn, DepositPlace, DividendClosed, DividendDoor, Duty, DutyBriefing, DutyFiling,
    Expect, FormBox, IdentityCard, MarkDutyFiled, PayYourself, RecordVatCarryIn, RequestVatRefund,
    RetractDutyFiled, RetractVatRefund, SocietyHome, StatementMove, StatementReading,
    UnknownReason, VatCarryInRecord, VatRefundRecord, VatRefundStatus, WaiverReason, closing_story,
    duty_briefing, pay_yourself, society_duties, society_home, society_identity, statement_moves,
    vat_carry_in,
};
use freeflow_core::store::Store;
use time::Date;

use crate::error::CliError;
use crate::output::{HumanRender, format_json, format_outcome_as, format_value, key_values};
use crate::parsers::{parse_date, parse_money};
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
    let filed = d
        .filed_on
        .map(|on| format!(" · déposé le {}", format_date(on)))
        .unwrap_or_default();
    format!(
        "{} {}{amount}{filed} · {}",
        format_date(d.due_on),
        deadline_occurrence_fr(d.kind, d.vat_scheme, &d.period_key, d.due_on),
        deposit_fr(d.deposit)
    )
}

pub(crate) fn deadline_occurrence_fr(
    kind: FiscalDeadlineKind,
    scheme: VatFilingScheme,
    period_key: &str,
    due_on: Date,
) -> String {
    match kind {
        FiscalDeadlineKind::Ca3 => match scheme {
            VatFilingScheme::Ca3Monthly => period_key
                .split_once('-')
                .and_then(|(_, m)| m.parse::<u8>().ok())
                .map(|m| format!("TVA du mois {}", de_mois(m)))
                .unwrap_or_else(|| deadline_fr(kind, scheme).to_string()),
            _ => deadline_fr(kind, scheme).to_string(),
        },
        FiscalDeadlineKind::IsAcompte => format!(
            "acompte d'impôt sur les sociétés de {}",
            month_fr(u8::from(due_on.month()))
        ),
        _ => deadline_fr(kind, scheme).to_string(),
    }
}

fn de_mois(month: u8) -> String {
    let name = month_fr(month);
    match name.chars().next() {
        Some('a' | 'à' | 'â' | 'e' | 'é' | 'è' | 'ê' | 'i' | 'î' | 'o' | 'ô' | 'u' | 'ù') =>
        {
            format!("d'{name}")
        }
        _ => format!("de {name}"),
    }
}

fn month_fr(month: u8) -> &'static str {
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
    MONTHS
        .get(usize::from(month.saturating_sub(1)))
        .copied()
        .unwrap_or("")
}

pub(crate) fn deadline_fr(kind: FiscalDeadlineKind, scheme: VatFilingScheme) -> &'static str {
    match kind {
        FiscalDeadlineKind::Ca3 => match scheme {
            VatFilingScheme::Ca3Monthly => "TVA du mois",
            _ => "TVA du trimestre",
        },
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

impl HumanRender for DutyBriefing {
    fn render_human(&self) -> String {
        let mut lines = vec![
            format!(
                "{} {}",
                format_date(self.due_on),
                deadline_occurrence_fr(self.kind, self.vat_scheme, &self.period_key, self.due_on,)
            ),
            amount_story_fr(&self.amount),
        ];
        if !self.path.is_empty() {
            lines.push(format!("chemin : {}", self.path.join(" → ")));
        }
        if let Some(form) = self.form {
            lines.push(format!("formulaire : {form}"));
        }
        if !matches!(self.amount, AmountStory::NotYourHands { .. }) {
            lines.push(format!("site : {}", self.url));
        }
        let expect = self
            .expect
            .iter()
            .map(expect_fr)
            .collect::<Vec<_>>()
            .join(" ; ");
        if !expect.is_empty() {
            lines.push(format!("à faire : {expect}"));
        }
        if let Some(on) = self.filed_on {
            lines.push(format!("déposé le {}", format_date(on)));
        }
        if !self.boxes.is_empty() {
            lines.push(String::new());
            lines.push("cases :".into());
            for b in &self.boxes {
                lines.push(format!("  {}", box_line(b)));
            }
            if self.coverage == BoxCoverage::Complete {
                lines.push("  le reste, laisse vide".into());
            }
        }
        lines.push("indicatif — l'administration prime".into());
        lines.join("\n")
    }
}

fn box_line(b: &FormBox) -> String {
    let label = box_label(b.form, b.case);
    let amount = b.amount.map_or_else(|| "—".into(), |m| m.to_string());
    let consigne = match b.role {
        BoxRole::Fill => format!("saisir {amount}"),
        BoxRole::SiteComputes => format!("le site calcule, doit faire {amount}"),
        BoxRole::LeaveEmpty => "laisse vide".into(),
        BoxRole::Check => "cocher si besoin".into(),
    };
    format!("{} {} — {label} — {consigne}", b.case, amount)
}

pub(crate) fn box_label(form: &str, case: &str) -> &'static str {
    match (form, case) {
        ("2571", "03") => "montant à payer, impôt sur les sociétés",
        ("2571", "10") => "total à payer",
        ("3310-CA3", "02") => "prestations de services (HT)",
        ("3310-CA3", "08") => "TVA brute 20 %",
        ("3310-CA3", "9B") => "TVA brute 10 %",
        ("3310-CA3", "09") => "TVA brute 5,5 %",
        ("3310-CA3", "19") => "TVA déductible, immobilisations",
        ("3310-CA3", "20") => "TVA déductible, autres biens et services",
        ("3310-CA3", "22") => "crédit reporté",
        ("3310-CA3", "25") => "crédit de cette période",
        ("3310-CA3", "26") => "à vous verser",
        ("3310-CA3", "27") => "crédit de TVA à reporter",
        ("3310-CA3", "28") => "TVA nette due",
        _ => "",
    }
}

fn amount_story_fr(story: &AmountStory) -> String {
    match story {
        AmountStory::Due { amount, basis } => format!("{amount} ({})", amount_basis_fr(*basis)),
        AmountStory::Waiver { reason } => match reason {
            WaiverReason::PriorIsBelowThreshold => {
                "Rien à verser — IS de référence sous 3 000 €".into()
            }
            WaiverReason::FirstExercise => "Rien à verser — premier exercice".into(),
            WaiverReason::VatInstalmentDispensation => {
                "Rien à verser — acompte de TVA dispensé".into()
            }
            WaiverReason::Das2BelowThreshold => "Rien à déclarer — honoraires sous le seuil".into(),
        },
        AmountStory::Unknown { reason } => match reason {
            UnknownReason::NoProfile => "Montant inconnu — profil manquant".into(),
            UnknownReason::PriorIsMissing => {
                "Montant inconnu — IS de l'exercice précédent non repris".into()
            }
            UnknownReason::PriorVatMissing => {
                "Montant inconnu — TVA de l'exercice précédent non reprise".into()
            }
            UnknownReason::PeriodNotInVault => {
                "Montant inconnu — cette période n'est pas dans le coffre".into()
            }
        },
        AmountStory::External => "Le montant est sur l'avis, pas ici".into(),
        AmountStory::NotYourHands { amount } => amount.map_or_else(
            || "Chez l'expert-paie, pas toi sur le site".into(),
            |m| format!("{m} — chez l'expert-paie, pas toi sur le site"),
        ),
        AmountStory::Declaration => "Une déclaration, pas un versement".into(),
    }
}

fn amount_basis_fr(basis: AmountBasis) -> &'static str {
    match basis {
        AmountBasis::QuarterOfPriorIs => "un quart de l'IS de référence",
        AmountBasis::VatForPeriod => "TVA de la période",
        AmountBasis::VatCredit => "l'État vous les doit",
        AmountBasis::VatInstalment => "acompte de TVA",
        AmountBasis::Ca12Net => "TVA de l'année, nette des acomptes",
        AmountBasis::SnapshotIs => "IS de l'exercice clos",
        AmountBasis::FeesBySupplier => "honoraires par bénéficiaire",
        AmountBasis::DividendWithholding => "prélèvements sur dividendes",
    }
}

fn expect_fr(expect: &Expect) -> &'static str {
    match expect {
        Expect::NeedsProfessionalSpace => "espace professionnel requis",
        Expect::CheckPrefill => "vérifier le montant prérempli",
        Expect::FileEvenIfZero => "déposer même à zéro",
        Expect::ModulateDown => "on peut baisser si l'impôt attendu est inférieur",
        Expect::PayElectronically => "télérèglement",
        Expect::SeeEfiNotice => "recopier la notice EFI",
        Expect::NoticeInSpace => "l'avis est dans l'espace professionnel",
        Expect::PayrollExpertDoesIt => "l'expert-paie dépose",
        Expect::GuichetUnique => "guichet unique INPI",
        Expect::ConfidentialityOption => "option de confidentialité des comptes",
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
    /// Lettre d'une démarche hors de l'app (chemin, montant, ce que l'écran demandera).
    Duty {
        /// Nature (`is_acompte`, `ca3`, `cfe`, …).
        kind: FiscalDeadlineKind,
        /// Période (`AAAA-MM` ou `AAAA-MM-JJ`). Défaut : la prochaine non déposée.
        #[arg(long)]
        period: Option<String>,
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
    /// Marquer une démarche comme déposée (fait humain, pas une preuve DGFiP).
    Filed {
        /// Nature (`is_acompte`, `ca3`, …).
        kind: FiscalDeadlineKind,
        /// Période (`AAAA-MM` ou `AAAA-MM-JJ`). Défaut : l'occurrence courante.
        #[arg(long)]
        period: Option<String>,
        #[arg(long, value_parser = parse_date)]
        today: Option<Date>,
    },
    /// Retirer le marquage « déposé ».
    Unfiled {
        kind: FiscalDeadlineKind,
        #[arg(long)]
        period: Option<String>,
        #[arg(long, value_parser = parse_date)]
        today: Option<Date>,
    },
    /// Crédit de TVA à reporter (case 27 de la dernière CA3 déjà déposée).
    #[command(subcommand)]
    VatCredit(VatCreditCommand),
    /// Demander le versement d'un crédit de TVA (case 26), ou l'annuler.
    #[command(subcommand)]
    VatRefund(VatRefundCommand),
}

#[derive(Debug, Subcommand)]
pub enum VatCreditCommand {
    /// Afficher le crédit repris, s'il est enregistré.
    Show,
    /// Enregistrer le crédit repris — une seule fois ; ensuite il est figé.
    Set {
        /// Période de la dernière CA3 déjà déposée (`AAAA-MM`).
        #[arg(long)]
        after: String,
        /// Case 27, en euros.
        #[arg(long, value_parser = parse_money)]
        amount: Money,
        /// Provenance libre (espace impôts, CA3 d'août…).
        #[arg(long)]
        source: Option<String>,
    },
    /// Refusé : le crédit repris est figé.
    Rm,
}

#[derive(Debug, Subcommand)]
pub enum VatRefundCommand {
    /// Demander le versement (tout le crédit, ou --amount une partie).
    Request {
        /// Période CA3 `AAAA-MM`.
        period: String,
        #[arg(long, value_parser = parse_money)]
        amount: Option<Money>,
        #[arg(long, value_parser = parse_date)]
        today: Option<Date>,
    },
    /// Annuler la demande (si la CA3 n'est pas encore marquée déposée).
    Retract {
        period: String,
        #[arg(long, value_parser = parse_date)]
        today: Option<Date>,
    },
}

impl HumanRender for DutyFiling {
    fn render_human(&self) -> String {
        format!(
            "{} {} déposé le {}",
            format_date(self.due_on),
            deadline_fr(self.kind, VatFilingScheme::Ca3Monthly),
            format_date(self.filed_on)
        )
    }
}

pub fn run(
    cmd: SocietyCommand,
    store: &mut Store,
    ctx: &ExecutionContext,
    json: bool,
) -> Result<String, CliError> {
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
        SocietyCommand::Duty {
            kind,
            period,
            today,
        } => {
            let today = today.unwrap_or_else(today_local);
            let briefing = duty_briefing(store.connection(), kind, today, period.as_deref())?;
            Ok(format_value(&briefing, json))
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
        SocietyCommand::Filed {
            kind,
            period,
            today,
        } => {
            let today = today.unwrap_or_else(today_local);
            let briefing = duty_briefing(store.connection(), kind, today, period.as_deref())?;
            let outcome = Executor::new(store).execute(
                &MarkDutyFiled {
                    kind,
                    period_key: briefing.period_key,
                    due_on: briefing.due_on,
                    filed_on: today,
                },
                ctx,
            )?;
            Ok(format_outcome_as(&outcome, json, |f| {
                format!("déposé le {}", format_date(f.filed_on))
            }))
        }
        SocietyCommand::Unfiled {
            kind,
            period,
            today,
        } => {
            let today = today.unwrap_or_else(today_local);
            let briefing = duty_briefing(store.connection(), kind, today, period.as_deref())?;
            let outcome = Executor::new(store).execute(
                &RetractDutyFiled {
                    kind,
                    period_key: briefing.period_key,
                },
                ctx,
            )?;
            Ok(format_outcome_as(&outcome, json, |_| "dépôt retiré".into()))
        }
        SocietyCommand::VatCredit(cmd) => run_vat_credit(cmd, store, ctx, json),
        SocietyCommand::VatRefund(cmd) => run_vat_refund(cmd, store, ctx, json),
    }
}

impl HumanRender for VatCarryInRecord {
    fn render_human(&self) -> String {
        let mut pairs = vec![
            ("après", self.after_period.clone()),
            ("crédit à reporter", self.credit.to_string()),
            ("révision", self.revision.to_string()),
        ];
        if let Some(source) = &self.source {
            pairs.push(("provenance", source.clone()));
        }
        key_values(&pairs)
    }
}

impl HumanRender for VatRefundRecord {
    fn render_human(&self) -> String {
        key_values(&[
            ("période", self.period_key.clone()),
            ("versement", self.amount.to_string()),
            ("demandé le", format_date(self.requested_on)),
        ])
    }
}

fn run_vat_credit(
    cmd: VatCreditCommand,
    store: &mut Store,
    ctx: &ExecutionContext,
    json: bool,
) -> Result<String, CliError> {
    match cmd {
        VatCreditCommand::Show => match vat_carry_in(store.connection())? {
            Some(record) => Ok(format_value(&record, json)),
            None if json => Ok(format_json(&serde_json::Value::Null)),
            None => Ok(
                "aucun crédit de TVA repris — `freeflow society vat-credit set --after AAAA-MM --amount …`"
                    .into(),
            ),
        },
        VatCreditCommand::Set {
            after,
            amount,
            source,
        } => {
            let outcome = Executor::new(store).execute(
                &RecordVatCarryIn {
                    after_period: after,
                    credit: amount,
                    source,
                },
                ctx,
            )?;
            Ok(format_outcome_as(&outcome, json, |revision| {
                format!("crédit de TVA repris (révision {revision})")
            }))
        }
        VatCreditCommand::Rm => {
            let Some(existing) = vat_carry_in(store.connection())? else {
                return Err(CliError::Domain("aucun crédit de TVA repris".into()));
            };
            let outcome = Executor::new(store).execute(
                &DeleteVatCarryIn {
                    revision: existing.revision,
                },
                ctx,
            )?;
            Ok(format_outcome_as(&outcome, json, |()| {
                "crédit de TVA repris oublié".into()
            }))
        }
    }
}

fn run_vat_refund(
    cmd: VatRefundCommand,
    store: &mut Store,
    ctx: &ExecutionContext,
    json: bool,
) -> Result<String, CliError> {
    match cmd {
        VatRefundCommand::Request {
            period,
            amount,
            today,
        } => {
            let today = today.unwrap_or_else(today_local);
            let briefing = duty_briefing(
                store.connection(),
                FiscalDeadlineKind::Ca3,
                today,
                Some(&period),
            )?;
            let amount = match amount {
                Some(amount) => amount,
                None => match briefing.vat_refund {
                    Some(VatRefundStatus::Offered { credit, .. }) => credit,
                    _ => {
                        return Err(CliError::Domain("pas de crédit à récupérer".into()));
                    }
                },
            };
            let outcome = Executor::new(store).execute(
                &RequestVatRefund {
                    period_key: briefing.period_key,
                    amount,
                    requested_on: today,
                },
                ctx,
            )?;
            Ok(format_outcome_as(&outcome, json, |r| {
                format!("versement demandé ({})", r.amount)
            }))
        }
        VatRefundCommand::Retract { period, today } => {
            let today = today.unwrap_or_else(today_local);
            let briefing = duty_briefing(
                store.connection(),
                FiscalDeadlineKind::Ca3,
                today,
                Some(&period),
            )?;
            let outcome = Executor::new(store).execute(
                &RetractVatRefund {
                    period_key: briefing.period_key,
                },
                ctx,
            )?;
            Ok(format_outcome_as(&outcome, json, |()| {
                "demande de versement retirée".into()
            }))
        }
    }
}
