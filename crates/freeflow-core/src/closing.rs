//! Parcours de clôture guidé (lot 34) — la *requête* qui met bout à bout ce que les lots 20 à
//! 33 ont construit : à quel point de la clôture d'un exercice en est-on, qu'est-ce qui la
//! bloque, qu'est-ce qui mérite un coup d'œil avant de figer le résultat, et qu'est-ce qui
//! reste à faire après (approbation, documents, dépôts).
//!
//! Le parcours ne **décide rien** et n'écrit rien : c'est une lecture de l'état du coffre, qui
//! rejoue les gardes de [`crate::fiscal_year::CloseFiscalYear`] (profil, bilan d'ouverture,
//! chevauchement) *avant* qu'elles ne refusent, et qui rappelle les règles que le cœur n'impose
//! pas mais que la loi impose (dotation minimale à la réserve légale, délais d'AG et de dépôt).
//! Les trois façades l'affichent telle quelle ; l'*action* associée à chaque étape est propre à
//! chaque façade (une commande CLI, un outil MCP, un bouton de la fenêtre) et se déduit de la
//! clé stable de l'étape ([`ClosingStepKey`]), jamais d'un texte.
//!
//! Chaque étape porte un statut ([`StepStatus`]) : `blocked` empêche la clôture (le cœur la
//! refuserait, ou le résultat serait faux), `warning` mérite attention mais n'empêche rien,
//! `todo` est l'action attendue maintenant, `later` n'est pas encore atteignable, `done` et
//! `info` ne demandent rien. Le résumé est [`ClosingStage`].

use rusqlite::Connection;
use serde::Serialize;
use serde_json::json;
use time::Date;

use crate::accounting::{AccountingResult, CarryBackBase, carry_back, compute_result};
use crate::app::AppError;
use crate::billing::{aged_balance, compute_totals, list_bank_transactions, list_invoices};
use crate::company::{CompanyProfile, company_profile};
use crate::domain::{FiscalYear, FiscalYearEnd, Money, format_date};
use crate::expenses::expenses_between;
use crate::fiscal::{
    accounts_filing_due_on, approval_meeting_due_on, is_solde_due_on, liasse_due_on,
};
use crate::fiscal_year::{
    FiscalYearRecord, PriorChain, fiscal_year_ending_in, list_fiscal_years, minimum_legal_reserve,
    prior_chain,
};
use crate::ledger::build_ledger;
use crate::opening_balance::opening_balance;

/// Les quatre temps du parcours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClosingPhase {
    /// Vérifier et compléter les faits avant de figer le résultat.
    Prepare,
    /// Le geste de clôture lui-même.
    Close,
    /// Affectation du résultat et approbation des comptes.
    Approve,
    /// Documents, déclarations et dépôts.
    Report,
}

impl ClosingPhase {
    pub const ALL: [Self; 4] = [Self::Prepare, Self::Close, Self::Approve, Self::Report];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Prepare => "prepare",
            Self::Close => "close",
            Self::Approve => "approve",
            Self::Report => "report",
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Prepare => "Préparer",
            Self::Close => "Clore",
            Self::Approve => "Affecter et approuver",
            Self::Report => "Déclarer et déposer",
        }
    }
}

/// Statut d'une étape — voir le commentaire de module pour la sémantique.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Done,
    Todo,
    Warning,
    Blocked,
    Info,
    Later,
}

impl StepStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Todo => "todo",
            Self::Warning => "warning",
            Self::Blocked => "blocked",
            Self::Info => "info",
            Self::Later => "later",
        }
    }
}

/// Identifiant stable d'une étape : c'est sur lui que chaque façade branche son action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClosingStepKey {
    Profile,
    PeriodEnded,
    OpeningBalance,
    PreviousYear,
    Invoices,
    Expenses,
    Bank,
    Result,
    BalanceSheet,
    Close,
    Appropriation,
    Approve,
    Documents,
    CorporateTax,
    Liasse,
    Filing,
}

impl ClosingStepKey {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Profile => "profile",
            Self::PeriodEnded => "period_ended",
            Self::OpeningBalance => "opening_balance",
            Self::PreviousYear => "previous_year",
            Self::Invoices => "invoices",
            Self::Expenses => "expenses",
            Self::Bank => "bank",
            Self::Result => "result",
            Self::BalanceSheet => "balance_sheet",
            Self::Close => "close",
            Self::Appropriation => "appropriation",
            Self::Approve => "approve",
            Self::Documents => "documents",
            Self::CorporateTax => "corporate_tax",
            Self::Liasse => "liasse",
            Self::Filing => "filing",
        }
    }

    #[must_use]
    pub const fn phase(self) -> ClosingPhase {
        match self {
            Self::Profile
            | Self::PeriodEnded
            | Self::OpeningBalance
            | Self::PreviousYear
            | Self::Invoices
            | Self::Expenses
            | Self::Bank
            | Self::Result
            | Self::BalanceSheet => ClosingPhase::Prepare,
            Self::Close => ClosingPhase::Close,
            Self::Appropriation | Self::Approve => ClosingPhase::Approve,
            Self::Documents | Self::CorporateTax | Self::Liasse | Self::Filing => {
                ClosingPhase::Report
            }
        }
    }

    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Profile => "Profil d'entreprise",
            Self::PeriodEnded => "Exercice écoulé",
            Self::OpeningBalance => "Bilan d'ouverture",
            Self::PreviousYear => "Exercice précédent",
            Self::Invoices => "Factures de l'exercice",
            Self::Expenses => "Dépenses et justificatifs",
            Self::Bank => "Rapprochement bancaire",
            Self::Result => "Résultat et IS",
            Self::BalanceSheet => "Bilan dérivé",
            Self::Close => "Clore l'exercice",
            Self::Appropriation => "Affectation du résultat",
            Self::Approve => "Approbation des comptes",
            Self::Documents => "Documents de clôture",
            Self::CorporateTax => "Solde d'IS",
            Self::Liasse => "Liasse fiscale",
            Self::Filing => "Dépôt des comptes au greffe",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClosingStep {
    pub key: ClosingStepKey,
    pub phase: ClosingPhase,
    pub status: StepStatus,
    pub title: &'static str,
    /// Une ou deux phrases : le constat, et ce qu'il implique.
    pub detail: String,
    /// Échéance associée (AG, dépôt, liasse, solde d'IS), quand il y en a une.
    #[serde(with = "crate::domain::serde_date::date::option")]
    pub due_on: Option<Date>,
    /// Montant associé (solde d'IS, dotation minimale), quand il y en a un.
    pub amount: Option<Money>,
}

impl ClosingStep {
    fn new(key: ClosingStepKey, status: StepStatus, detail: impl Into<String>) -> Self {
        Self {
            key,
            phase: key.phase(),
            status,
            title: key.title(),
            detail: detail.into(),
            due_on: None,
            amount: None,
        }
    }

    const fn due(mut self, due_on: Date) -> Self {
        self.due_on = Some(due_on);
        self
    }

    const fn amount(mut self, amount: Money) -> Self {
        self.amount = Some(amount);
        self
    }
}

/// Où en est la clôture, en un mot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClosingStage {
    /// L'exercice court encore.
    NotEnded,
    /// Écoulé, mais au moins un point bloquant.
    Blocked,
    /// Écoulé, rien ne bloque : la clôture est le prochain geste.
    Ready,
    /// Clos, en projet (affectation révisable, approbation attendue).
    Draft,
    /// Clos et approuvé : ne restent que les déclarations et dépôts.
    Approved,
}

impl ClosingStage {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotEnded => "not_ended",
            Self::Blocked => "blocked",
            Self::Ready => "ready",
            Self::Draft => "draft",
            Self::Approved => "approved",
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::NotEnded => "exercice en cours",
            Self::Blocked => "clôture bloquée",
            Self::Ready => "prêt à clore",
            Self::Draft => "clos, en projet",
            Self::Approved => "clos et approuvé",
        }
    }
}

/// Le parcours d'un exercice, tel que le voient les trois façades.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClosingChecklist {
    /// Année civile de la clôture (désignation de l'exercice, comme `year show`).
    pub period: i32,
    pub exercise: FiscalYear,
    #[serde(with = "crate::domain::serde_date::date")]
    pub today: Date,
    pub stage: ClosingStage,
    /// L'exercice clos dans l'application, s'il l'est.
    pub fiscal_year: Option<FiscalYearRecord>,
    /// Le résultat : figé (snapshot) si l'exercice est clos, prévisionnel sinon, `None` sans
    /// profil.
    pub result: Option<AccountingResult>,
    /// Dotation minimale à la réserve légale (art. L232-10), calculable dès que le résultat et la
    /// chaîne le sont.
    pub minimum_legal_reserve: Option<Money>,
    /// Déficit de l'exercice imputable en arrière sur le bénéfice de l'exercice précédent clos
    /// ici, et la créance d'IS qui en naîtrait — `None` si l'option n'est pas ouverte (pas de
    /// déficit, pas d'exercice précédent, pas de bénéfice d'imputation) ou si l'exercice est
    /// déjà clos (la décision est prise).
    pub carry_back_available: Option<(Money, Money)>,
    pub steps: Vec<ClosingStep>,
}

impl ClosingChecklist {
    #[must_use]
    pub fn step(&self, key: ClosingStepKey) -> Option<&ClosingStep> {
        self.steps.iter().find(|s| s.key == key)
    }

    pub fn steps_in(&self, phase: ClosingPhase) -> impl Iterator<Item = &ClosingStep> {
        self.steps.iter().filter(move |s| s.phase == phase)
    }

    pub fn blocking(&self) -> impl Iterator<Item = &ClosingStep> {
        self.steps
            .iter()
            .filter(|s| s.status == StepStatus::Blocked)
    }
}

/// L'exercice désigné par `period`, résolu comme partout ailleurs : l'exercice enregistré s'il
/// existe, sinon dérivé de la clôture du profil (année civile sans profil ou sans date de
/// clôture — le parcours doit pouvoir s'afficher *avant* que le profil existe, pour dire qu'il
/// manque).
fn resolve_exercise(
    record: Option<&FiscalYearRecord>,
    profile: Option<&CompanyProfile>,
    period: i32,
) -> FiscalYear {
    if let Some(record) = record {
        return record.period();
    }
    let fye = profile
        .and_then(|p| p.fiscal_year_end)
        .unwrap_or(FiscalYearEnd::CALENDAR);
    fye.containing(fye.end_in_year(period))
}

fn plural(count: usize, singular: &str, plural: &str) -> String {
    if count == 1 {
        format!("{count} {singular}")
    } else {
        format!("{count} {plural}")
    }
}

/// Tout ce que le parcours lit, chargé une fois.
struct Facts {
    profile: Option<CompanyProfile>,
    record: Option<FiscalYearRecord>,
    exercise: FiscalYear,
    years: Vec<FiscalYearRecord>,
}

impl Facts {
    /// L'exercice clos dans l'application immédiatement avant celui-ci (le plus récent des
    /// exercices clos avant son début) — pas forcément contigu.
    fn previous(&self) -> Option<&FiscalYearRecord> {
        self.years
            .iter()
            .filter(|y| y.ends_on < self.exercise.start())
            .max_by_key(|y| y.ends_on)
    }

    fn fye(&self) -> FiscalYearEnd {
        self.profile
            .as_ref()
            .and_then(|p| p.fiscal_year_end)
            .unwrap_or(FiscalYearEnd::CALENDAR)
    }
}

/// Le parcours de clôture de l'exercice clos dans l'année civile `period`, vu depuis `today`
/// (date fournie par l'adaptateur, jamais lue par le cœur : les échéances et le « l'exercice
/// est-il écoulé » en dépendent).
///
/// # Errors
///
/// Erreur de lecture SQLite. L'absence de profil n'est **pas** une erreur : c'est la première
/// étape bloquante du parcours.
#[allow(clippy::too_many_lines)]
pub fn closing_checklist(
    conn: &Connection,
    period: i32,
    today: Date,
) -> Result<ClosingChecklist, AppError> {
    let profile = company_profile(conn)?;
    let record = fiscal_year_ending_in(conn, period)?;
    let exercise = resolve_exercise(record.as_ref(), profile.as_ref(), period);
    let facts = Facts {
        profile,
        record,
        exercise,
        years: list_fiscal_years(conn)?,
    };

    let mut steps = Vec::with_capacity(16);
    steps.push(profile_step(&facts));
    steps.push(period_ended_step(&facts, today));
    let opening = opening_balance(conn)?.map(|r| r.balance);
    steps.push(opening_balance_step(&facts, opening.as_ref()));
    let chain = prior_chain(conn, exercise.start()).ok();
    steps.push(previous_year_step(&facts, chain.as_ref()));
    steps.push(invoices_step(conn, &facts, today)?);
    steps.push(expenses_step(conn, &facts)?);
    steps.push(bank_step(conn, &facts)?);

    // Résultat : figé si clos, prévisionnel sinon — et ce que la chaîne en fait (réserve
    // minimale, report en arrière possible).
    let result = match (&facts.record, &facts.profile) {
        (Some(record), _) => Some(record.accounting_result()),
        (None, Some(profile)) => Some(compute_result(conn, exercise, profile)?),
        (None, None) => None,
    };
    let minimum_reserve = match (&result, &chain, &facts.profile) {
        (Some(result), Some(chain), Some(profile)) => Some(minimum_legal_reserve(
            result.net_result,
            chain.retained,
            chain.reserve,
            profile.share_capital,
        )),
        _ => None,
    };
    let carry_back_available = match (&facts.record, &result) {
        (None, Some(result)) if !result.deficit().is_zero() => facts
            .previous()
            .filter(|p| p.ends_on.next_day() == Some(exercise.start()))
            .map(|previous| {
                carry_back(
                    result.deficit(),
                    CarryBackBase {
                        taxable_profit: previous.taxable_result(),
                        distributed: previous.dividends,
                    },
                )
            })
            .filter(|cb| !cb.imputed.is_zero())
            .map(|cb| (cb.imputed, cb.credit)),
        _ => None,
    };
    steps.push(result_step(&facts, result.as_ref(), carry_back_available));
    steps.push(balance_sheet_step(conn, &facts)?);

    let blocked_before_close = steps.iter().any(|s| s.status == StepStatus::Blocked);
    steps.push(close_step(&facts, blocked_before_close, minimum_reserve));
    steps.push(appropriation_step(&facts, minimum_reserve));
    steps.push(approve_step(&facts, today));
    steps.push(documents_step(&facts));
    steps.push(corporate_tax_step(&facts, result.as_ref(), today));
    steps.push(liasse_step(&facts, today));
    steps.push(filing_step(&facts, today));

    let stage = match &facts.record {
        Some(record) if record.is_approved() => ClosingStage::Approved,
        Some(_) => ClosingStage::Draft,
        None if today <= exercise.end() => ClosingStage::NotEnded,
        None if blocked_before_close => ClosingStage::Blocked,
        None => ClosingStage::Ready,
    };

    Ok(ClosingChecklist {
        period,
        exercise,
        today,
        stage,
        fiscal_year: facts.record,
        result,
        minimum_legal_reserve: minimum_reserve,
        carry_back_available,
        steps,
    })
}

// ---------------------------------------------------------------------------------------------
// Préparer
// ---------------------------------------------------------------------------------------------

fn profile_step(facts: &Facts) -> ClosingStep {
    let Some(profile) = &facts.profile else {
        return ClosingStep::new(
            ClosingStepKey::Profile,
            StepStatus::Blocked,
            "Aucun profil d'entreprise : la clôture en dépend (résultat, plafond de la réserve \
             légale, calendrier des échéances). Renseignez-le d'abord.",
        );
    };
    let mut missing = Vec::new();
    if profile.fiscal_year_end.is_none() {
        missing.push("date de clôture (année civile supposée)");
    }
    if profile.share_capital.is_none() {
        missing.push("capital social (le plafond de la réserve légale ne peut pas être vérifié)");
    }
    if profile.vat_regime.is_none() {
        missing.push("régime de TVA (mensuel supposé)");
    }
    let director = profile.director_monthly_gross.map_or_else(
        || "président non rémunéré".to_string(),
        |gross| format!("président rémunéré {gross} brut par mois, coût réputé dû"),
    );
    let fye = facts.fye();
    let summary = format!(
        "{} {} — clôture au {:02}/{:02}, {director}.",
        profile.legal_form,
        profile.name,
        fye.day(),
        fye.month(),
    );
    if missing.is_empty() {
        ClosingStep::new(ClosingStepKey::Profile, StepStatus::Done, summary)
    } else {
        ClosingStep::new(
            ClosingStepKey::Profile,
            StepStatus::Warning,
            format!("{summary} Profil incomplet : {}.", missing.join(" ; ")),
        )
    }
}

/// Lot 36 : un exercice clos prématurément dans l'application (avant ce lot, la commande le
/// permettait) reste « non écoulé » jusqu'à sa date — l'étape ne se laisse plus convaincre par
/// la seule existence du snapshot, sinon le parcours cautionnerait un résultat incomplet.
fn period_ended_step(facts: &Facts, today: Date) -> ClosingStep {
    let end = facts.exercise.end();
    if today > end {
        ClosingStep::new(
            ClosingStepKey::PeriodEnded,
            StepStatus::Done,
            format!(
                "Exercice du {} au {} écoulé.",
                format_date(facts.exercise.start()),
                format_date(end)
            ),
        )
    } else {
        ClosingStep::new(
            ClosingStepKey::PeriodEnded,
            StepStatus::Blocked,
            format!(
                "L'exercice court jusqu'au {} : le clore aujourd'hui figerait un résultat \
                 incomplet (les factures et dépenses à venir n'y entreraient pas).",
                format_date(end)
            ),
        )
        .due(end)
    }
}

fn opening_balance_step(
    facts: &Facts,
    opening: Option<&crate::domain::OpeningBalance>,
) -> ClosingStep {
    let start = facts.exercise.start();
    if let Some(previous) = facts.previous() {
        return ClosingStep::new(
            ClosingStepKey::OpeningBalance,
            StepStatus::Info,
            format!(
                "Sans objet pour cet exercice : les à-nouveaux dérivent de l'exercice clos \
                 précédent ({} → {}).",
                format_date(previous.starts_on),
                format_date(previous.ends_on)
            ),
        );
    }
    match opening {
        None => ClosingStep::new(
            ClosingStepKey::OpeningBalance,
            StepStatus::Warning,
            "Aucun bilan d'ouverture : la chaîne part de zéro (report à nouveau, réserve \
             légale et déficits antérieurs nuls, aucun à-nouveau au grand livre). Si la société \
             existait avant FreeFlow, reprenez le dernier bilan de l'expert-comptable avant de \
             clore — il sera figé ensuite.",
        ),
        Some(o) if o.opens_on == start => {
            let equity = o.equity();
            ClosingStep::new(
                ClosingStepKey::OpeningBalance,
                StepStatus::Done,
                format!(
                    "Repris au {} ({}) : capital {}, report à nouveau {}, réserve légale {}, \
                     déficits fiscaux reportables {}.",
                    format_date(o.opens_on),
                    plural(o.lines.len(), "compte", "comptes"),
                    equity.share_capital,
                    equity.retained_earnings,
                    equity.legal_reserve,
                    o.tax_losses
                ),
            )
        }
        Some(o) => ClosingStep::new(
            ClosingStepKey::OpeningBalance,
            StepStatus::Blocked,
            format!(
                "Le bilan d'ouverture est daté du {} alors que l'exercice commence le {} : la \
                 clôture sera refusée. Corrigez la période de l'exercice, ou la date du bilan.",
                format_date(o.opens_on),
                format_date(start)
            ),
        ),
    }
}

fn previous_year_step(facts: &Facts, chain: Option<&PriorChain>) -> ClosingStep {
    let start = facts.exercise.start();
    let Some(previous) = facts.previous() else {
        return ClosingStep::new(
            ClosingStepKey::PreviousYear,
            StepStatus::Info,
            "Aucun exercice antérieur clos dans l'application : celui-ci est le premier maillon \
             de la chaîne (report à nouveau, réserve légale, déficits).",
        );
    };
    let span = format!(
        "{} → {}",
        format_date(previous.starts_on),
        format_date(previous.ends_on)
    );
    if previous.ends_on.next_day() != Some(start) {
        return ClosingStep::new(
            ClosingStepKey::PreviousYear,
            StepStatus::Warning,
            format!(
                "L'exercice clos précédent ({span}) ne se termine pas la veille du {} : trou \
                 dans la chaîne, aucun à-nouveau dérivé pour cet exercice et report en arrière \
                 impossible. Le report à nouveau et les déficits, eux, sont bien hérités.",
                format_date(start)
            ),
        );
    }
    // La réserve légale *cumulée* vient de la chaîne (bilan d'ouverture + toutes les dotations),
    // pas de la seule dotation du dernier exercice — lot 36, relevé par l'audit.
    let reserve = chain.map_or(previous.legal_reserve, |c| c.reserve);
    let inherited = format!(
        "report à nouveau {}, réserve légale cumulée {}, déficits reportables {}",
        previous.retained_earnings, reserve, previous.losses_carried_forward
    );
    if previous.is_approved() {
        ClosingStep::new(
            ClosingStepKey::PreviousYear,
            StepStatus::Done,
            format!("Exercice précédent ({span}) approuvé — hérité : {inherited}."),
        )
    } else {
        ClosingStep::new(
            ClosingStepKey::PreviousYear,
            StepStatus::Warning,
            format!(
                "L'exercice précédent ({span}) est encore un projet : approuvez-le, ou \
                 corrigez-le maintenant — une fois celui-ci clos, il ne sera plus ni modifiable \
                 ni supprimable. Hérité : {inherited}."
            ),
        )
    }
}

fn invoices_step(conn: &Connection, facts: &Facts, today: Date) -> Result<ClosingStep, AppError> {
    let exercise = facts.exercise;
    let invoices: Vec<_> = list_invoices(conn)?
        .into_iter()
        .filter(|inv| exercise.contains(inv.issued_on))
        .collect();
    if invoices.is_empty() {
        return Ok(ClosingStep::new(
            ClosingStepKey::Invoices,
            StepStatus::Info,
            "Aucune facture émise sur l'exercice : chiffre d'affaires nul.",
        ));
    }
    let revenue_ht: Money = invoices
        .iter()
        .map(|inv| compute_totals(&inv.lines).subtotal_ht)
        .sum();
    let credit_notes = invoices
        .iter()
        .filter(|inv| inv.credited_invoice_id.is_some())
        .count();
    let aged = aged_balance(conn, today)?;
    let outstanding: Vec<_> = aged
        .iter()
        .filter(|a| invoices.iter().any(|inv| inv.id == a.invoice_id))
        .filter(|a| a.outstanding > Money::ZERO)
        .collect();
    let credit_notes = if credit_notes == 0 {
        String::new()
    } else {
        format!(" (dont {})", plural(credit_notes, "avoir", "avoirs"))
    };
    let summary = format!(
        "{}{credit_notes} sur l'exercice, CA HT {revenue_ht}",
        plural(invoices.len(), "facture émise", "factures émises"),
    );
    if outstanding.is_empty() {
        return Ok(ClosingStep::new(
            ClosingStepKey::Invoices,
            StepStatus::Done,
            format!("{summary} ; toutes encaissées."),
        ));
    }
    let due: Money = outstanding.iter().map(|a| a.outstanding).sum();
    let overdue = outstanding.iter().filter(|a| a.days_overdue > 0).count();
    let overdue = if overdue == 0 {
        String::new()
    } else {
        format!(", dont {overdue} en retard")
    };
    Ok(ClosingStep::new(
        ClosingStepKey::Invoices,
        StepStatus::Warning,
        format!(
            "{summary} ; {} pour {due}{overdue}. Sans effet sur le résultat (IS et TVA sont \
             calculés sur les factures émises), mais un encaissement oublié fausse le bilan \
             clients et le compte banque — relancez ou enregistrez les paiements avant de clore.",
            plural(outstanding.len(), "non encaissée", "non encaissées"),
        ),
    )
    .amount(due))
}

fn expenses_step(conn: &Connection, facts: &Facts) -> Result<ClosingStep, AppError> {
    let expenses = expenses_between(conn, facts.exercise.start(), facts.exercise.end())?;
    if expenses.is_empty() {
        return Ok(ClosingStep::new(
            ClosingStepKey::Expenses,
            StepStatus::Info,
            "Aucune dépense sur l'exercice.",
        ));
    }
    let total: Money = expenses.iter().map(|e| e.amount).sum();
    let vat: Money = expenses.iter().map(|e| e.vat_deductible).sum();
    let without_receipt = expenses.iter().filter(|e| e.receipt_hash.is_none()).count();
    let summary = format!(
        "{} sur l'exercice, {total} TTC, TVA déductible {vat}",
        plural(expenses.len(), "dépense", "dépenses")
    );
    if without_receipt == 0 {
        Ok(ClosingStep::new(
            ClosingStepKey::Expenses,
            StepStatus::Done,
            format!("{summary} ; toutes avec justificatif archivé."),
        ))
    } else {
        Ok(ClosingStep::new(
            ClosingStepKey::Expenses,
            StepStatus::Warning,
            format!(
                "{summary} ; {without_receipt} sans justificatif archivé — une charge sans pièce \
                 n'est pas déductible en cas de contrôle, et sa TVA ne l'est pas non plus. \
                 Joignez les pièces avant de clore : une dépense d'un exercice clos ne se \
                 modifie plus."
            ),
        ))
    }
}

fn bank_step(conn: &Connection, facts: &Facts) -> Result<ClosingStep, AppError> {
    let exercise = facts.exercise;
    let transactions: Vec<_> = list_bank_transactions(conn)?
        .into_iter()
        .filter(|t| exercise.contains(t.occurred_on))
        .collect();
    if transactions.is_empty() {
        return Ok(ClosingStep::new(
            ClosingStepKey::Bank,
            StepStatus::Info,
            "Aucun mouvement de relevé importé sur la période : le compte 512 dérivé suit les \
             dates des faits (encaissements, dépenses), pas celles du relevé. Importez le relevé \
             (bank import) pour rapprocher.",
        ));
    }
    let unmatched: Vec<_> = transactions.iter().filter(|t| !t.is_matched()).collect();
    if unmatched.is_empty() {
        return Ok(ClosingStep::new(
            ClosingStepKey::Bank,
            StepStatus::Done,
            format!(
                "{} sur l'exercice, tous rapprochés (encaissements et dépenses).",
                plural(
                    transactions.len(),
                    "mouvement du relevé",
                    "mouvements du relevé"
                )
            ),
        ));
    }
    let debits = unmatched.iter().filter(|t| t.is_debit()).count();
    let credits = unmatched.len() - debits;
    Ok(ClosingStep::new(
        ClosingStepKey::Bank,
        StepStatus::Warning,
        format!(
            "{} non rapproché(s) sur {} ({debits} débit(s), {credits} crédit(s)) : un crédit non \
             rapproché est un encaissement non enregistré, un débit une dépense manquante ou non \
             appariée — le compte 512 dérivé ne collera pas au relevé.",
            unmatched.len(),
            transactions.len()
        ),
    ))
}

fn result_step(
    facts: &Facts,
    result: Option<&AccountingResult>,
    carry_back_available: Option<(Money, Money)>,
) -> ClosingStep {
    use std::fmt::Write as _;
    let Some(result) = result else {
        return ClosingStep::new(
            ClosingStepKey::Result,
            StepStatus::Later,
            "Calculable dès que le profil d'entreprise est renseigné.",
        );
    };
    let figures = format!(
        "CA HT {}, charges externes {}, rémunération du dirigeant {}, résultat avant IS {}, \
         déficits antérieurs imputés {}, résultat fiscal {}, IS {}, résultat net {}",
        result.revenue_ht,
        result.expenses,
        result.director_remuneration,
        result.result_before_tax,
        result.losses_imputed,
        result.taxable_result,
        result.corporate_tax,
        result.net_result
    );
    let mut detail = if facts.record.is_some() {
        format!("Figé à la clôture : {figures}.")
    } else {
        format!("Prévision sur les faits saisis : {figures}.")
    };
    if !result.deficit().is_zero() && facts.record.is_none() {
        let _ = write!(
            detail,
            " Déficit fiscal de {} : reportable en avant sur les bénéfices suivants",
            result.deficit()
        );
        match carry_back_available {
            Some((imputed, credit)) => {
                let _ = write!(
                    detail,
                    ", ou reportable en arrière à hauteur de {imputed} sur le bénéfice de \
                     l'exercice précédent (créance d'IS de {credit}, option à prendre à la \
                     clôture)."
                );
            }
            None => detail.push_str(
                " (pas de report en arrière possible : aucun exercice précédent clos ici avec \
                 un bénéfice d'imputation).",
            ),
        }
    }
    ClosingStep::new(ClosingStepKey::Result, StepStatus::Info, detail).amount(result.corporate_tax)
}

fn balance_sheet_step(conn: &Connection, facts: &Facts) -> Result<ClosingStep, AppError> {
    if facts.profile.is_none() {
        return Ok(ClosingStep::new(
            ClosingStepKey::BalanceSheet,
            StepStatus::Later,
            "Dérivable dès que le profil d'entreprise est renseigné.",
        ));
    }
    let sheet = build_ledger(conn, facts.exercise)?.balance_sheet();
    if sheet.is_balanced() {
        Ok(ClosingStep::new(
            ClosingStepKey::BalanceSheet,
            StepStatus::Done,
            format!(
                "Bilan dérivé équilibré : total actif net {} = total passif {} (présentation \
                 2033-A, à relire avant de clore).",
                sheet.total_assets_net, sheet.total_liabilities
            ),
        ))
    } else {
        Ok(ClosingStep::new(
            ClosingStepKey::BalanceSheet,
            StepStatus::Blocked,
            format!(
                "Bilan dérivé déséquilibré : actif net {}, passif {} — une anomalie du grand \
                 livre dérivé, à signaler ; ne clôturez pas sur ces chiffres.",
                sheet.total_assets_net, sheet.total_liabilities
            ),
        ))
    }
}

// ---------------------------------------------------------------------------------------------
// Clore
// ---------------------------------------------------------------------------------------------

fn close_step(facts: &Facts, blocked: bool, minimum_reserve: Option<Money>) -> ClosingStep {
    if let Some(record) = &facts.record {
        let status = if record.is_approved() {
            "approuvé"
        } else {
            "en projet"
        };
        return ClosingStep::new(
            ClosingStepKey::Close,
            StepStatus::Done,
            format!(
                "Clos le {} ({status}) : résultat net {}.",
                format_date(record.created_at.date()),
                record.net_result
            ),
        );
    }
    if blocked {
        return ClosingStep::new(
            ClosingStepKey::Close,
            StepStatus::Blocked,
            "Corrigez d'abord les points bloquants ci-dessus.",
        );
    }
    let mut step = ClosingStep::new(
        ClosingStepKey::Close,
        StepStatus::Todo,
        "Rien ne bloque : la clôture fige le résultat et enregistre l'affectation en projet, \
         révisable jusqu'à l'approbation.",
    );
    match minimum_reserve {
        Some(minimum) if minimum.is_zero() => step.detail.push_str(
            " Aucune dotation à la réserve légale n'est obligatoire cette année (pas de \
             bénéfice à mettre en réserve, ou réserve déjà au dixième du capital).",
        ),
        Some(minimum) => {
            use std::fmt::Write as _;
            let _ = write!(
                step.detail,
                " Dotation minimale à la réserve légale : {minimum} (art. L232-10 du Code de \
                 commerce — un vingtième du bénéfice diminué des pertes antérieures, jusqu'à \
                 10 % du capital)."
            );
            step = step.amount(minimum);
        }
        None => {}
    }
    step
}

// ---------------------------------------------------------------------------------------------
// Affecter et approuver
// ---------------------------------------------------------------------------------------------

fn appropriation_step(facts: &Facts, minimum_reserve: Option<Money>) -> ClosingStep {
    let Some(record) = &facts.record else {
        return ClosingStep::new(
            ClosingStepKey::Appropriation,
            StepStatus::Later,
            "Se décide à la clôture (réserve légale, dividendes), puis se révise tant que \
             l'exercice est un projet.",
        );
    };
    let summary = format!(
        "réserve légale {}, dividendes {}, report à nouveau {}",
        record.legal_reserve, record.dividends, record.retained_earnings
    );
    match minimum_reserve {
        Some(minimum) if record.legal_reserve < minimum => ClosingStep::new(
            ClosingStepKey::Appropriation,
            StepStatus::Warning,
            format!(
                "Affectation : {summary}. Dotation à la réserve légale insuffisante : {} pour un \
                 minimum de {minimum} (art. L232-10 du Code de commerce, un vingtième du \
                 bénéfice diminué des pertes antérieures) — une délibération contraire est \
                 nulle. {}",
                record.legal_reserve,
                if record.is_approved() {
                    "L'exercice étant approuvé, la correction passe par une nouvelle décision."
                } else {
                    "Révisez l'affectation avant d'approuver."
                }
            ),
        )
        .amount(minimum),
        _ => ClosingStep::new(
            ClosingStepKey::Appropriation,
            StepStatus::Done,
            format!("Affectation : {summary}."),
        ),
    }
}

fn approve_step(facts: &Facts, today: Date) -> ClosingStep {
    let due = approval_meeting_due_on(facts.exercise.end());
    let Some(record) = &facts.record else {
        return ClosingStep::new(
            ClosingStepKey::Approve,
            StepStatus::Later,
            format!(
                "Après la clôture : décision de l'associé unique approuvant les comptes, dans \
                 les six mois de la clôture (avant le {}).",
                format_date(due)
            ),
        )
        .due(due);
    };
    if let Some(approved_on) = record.approved_on {
        return ClosingStep::new(
            ClosingStepKey::Approve,
            StepStatus::Done,
            format!(
                "Comptes approuvés le {} : l'exercice est immuable.",
                format_date(approved_on)
            ),
        )
        // L'échéance reste la date légale, même dépassée : c'est elle que le lecteur compare à
        // la date d'approbation (lot 36).
        .due(due);
    }
    if today > due {
        ClosingStep::new(
            ClosingStepKey::Approve,
            StepStatus::Warning,
            format!(
                "Délai d'approbation dépassé (six mois après la clôture, soit le {}) : approuvez \
                 les comptes sans attendre — le dépôt au greffe en dépend.",
                format_date(due)
            ),
        )
        .due(due)
    } else {
        ClosingStep::new(
            ClosingStepKey::Approve,
            StepStatus::Todo,
            format!(
                "À approuver par décision de l'associé unique avant le {} (six mois après la \
                 clôture) ; la date d'approbation scelle l'exercice.",
                format_date(due)
            ),
        )
        .due(due)
    }
}

// ---------------------------------------------------------------------------------------------
// Déclarer et déposer
// ---------------------------------------------------------------------------------------------

fn documents_step(facts: &Facts) -> ClosingStep {
    match &facts.record {
        None => ClosingStep::new(
            ClosingStepKey::Documents,
            StepStatus::Later,
            "Après la clôture : PV d'approbation, décision d'affectation, compte de résultat, \
             bilan et balance, liasse (JSON) et FEC — générés depuis le snapshot figé.",
        ),
        Some(record) if record.is_approved() => ClosingStep::new(
            ClosingStepKey::Documents,
            StepStatus::Todo,
            "À générer, relire et conserver : PV d'approbation (daté de la décision), décision \
             d'affectation, compte de résultat, bilan et balance, liasse (JSON pour \
             l'expert-comptable) et FEC.",
        ),
        Some(_) => ClosingStep::new(
            ClosingStepKey::Documents,
            StepStatus::Todo,
            "Disponibles en projet : compte de résultat, bilan et balance, liasse et FEC ; le PV \
             d'approbation reste un projet daté du jour tant que l'exercice n'est pas approuvé.",
        ),
    }
}

fn corporate_tax_step(
    facts: &Facts,
    result: Option<&AccountingResult>,
    today: Date,
) -> ClosingStep {
    let due = is_solde_due_on(facts.fye(), facts.exercise.end());
    let tax = result.map(|r| r.corporate_tax);
    match (&facts.record, tax) {
        (None, _) => ClosingStep::new(
            ClosingStepKey::CorporateTax,
            StepStatus::Later,
            format!(
                "Après la clôture : solde d'IS (relevé 2572) à verser avant le {}{}.",
                format_date(due),
                tax.map_or_else(String::new, |t| format!(" — estimation {t}"))
            ),
        )
        .due(due),
        (Some(_), Some(tax)) if tax.is_zero() => ClosingStep::new(
            ClosingStepKey::CorporateTax,
            StepStatus::Info,
            "Aucun IS dû (résultat fiscal nul ou déficitaire) : pas de solde à verser.",
        ),
        (Some(_), tax) => {
            let amount = tax.unwrap_or(Money::ZERO);
            let status = if today > due {
                StepStatus::Warning
            } else {
                StepStatus::Todo
            };
            ClosingStep::new(
                ClosingStepKey::CorporateTax,
                status,
                format!(
                    "Solde d'IS de {amount} à verser avant le {} (relevé 2572, formulaire de \
                     télépaiement) — les acomptes déjà versés ne sont pas suivis ici, déduisez-les.{}",
                    format_date(due),
                    if today > due { " Échéance dépassée." } else { "" }
                ),
            )
            .due(due)
            .amount(amount)
        }
    }
}

fn liasse_step(facts: &Facts, today: Date) -> ClosingStep {
    let due = liasse_due_on(facts.fye(), facts.exercise.end());
    if facts.record.is_none() {
        return ClosingStep::new(
            ClosingStepKey::Liasse,
            StepStatus::Later,
            format!(
                "Après la clôture : déclaration de résultats 2065 et tableaux 2033 à \
                 télétransmettre (EDI-TDFC) avant le {} (indicatif).",
                format_date(due)
            ),
        )
        .due(due);
    }
    let late = today > due;
    ClosingStep::new(
        ClosingStepKey::Liasse,
        if late {
            StepStatus::Warning
        } else {
            StepStatus::Todo
        },
        format!(
            "Déclaration de résultats 2065 et tableaux 2033 à télétransmettre (EDI-TDFC, via \
             l'expert-comptable ou un partenaire EDI) avant le {} (indicatif) ; l'export liasse \
             (JSON) en pré-remplit les cases.{}",
            format_date(due),
            if late { " Échéance dépassée." } else { "" }
        ),
    )
    .due(due)
}

fn filing_step(facts: &Facts, today: Date) -> ClosingStep {
    match &facts.record {
        Some(record) if record.is_approved() => {
            let approved_on = record.approved_on.unwrap_or(record.ends_on);
            let due = accounts_filing_due_on(approved_on);
            let late = today > due;
            ClosingStep::new(
                ClosingStepKey::Filing,
                if late {
                    StepStatus::Warning
                } else {
                    StepStatus::Todo
                },
                format!(
                    "Comptes annuels, décision d'affectation et PV à déposer au greffe dans le \
                     mois suivant l'approbation, avant le {} (deux mois par voie électronique).{}",
                    format_date(due),
                    if late { " Échéance dépassée." } else { "" }
                ),
            )
            .due(due)
        }
        _ => {
            let due = accounts_filing_due_on(approval_meeting_due_on(facts.exercise.end()));
            ClosingStep::new(
                ClosingStepKey::Filing,
                StepStatus::Later,
                format!(
                    "Après l'approbation : dépôt des comptes au greffe dans le mois suivant \
                     l'assemblée (au plus tard le {} si l'assemblée se tient au dernier jour du \
                     délai).",
                    format_date(due)
                ),
            )
            .due(due)
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Vue JSON partagée par les façades
// ---------------------------------------------------------------------------------------------

fn result_json(r: &AccountingResult) -> serde_json::Value {
    json!({
        "revenue_ht_cents": r.revenue_ht.cents(),
        "expenses_cents": r.expenses.cents(),
        "director_remuneration_cents": r.director_remuneration.cents(),
        "result_before_tax_cents": r.result_before_tax.cents(),
        "prior_losses_available_cents": r.prior_losses_available.cents(),
        "losses_imputed_cents": r.losses_imputed.cents(),
        "taxable_result_cents": r.taxable_result.cents(),
        "corporate_tax_cents": r.corporate_tax.cents(),
        "carried_back_cents": r.carried_back.cents(),
        "carry_back_credit_cents": r.carry_back_credit.cents(),
        "net_result_cents": r.net_result.cents(),
    })
}

/// Une entrée du lexique de la clôture : le mot tel qu'il apparaît dans le parcours et les
/// documents, et son explication avec les mots de tous les jours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct GlossaryEntry {
    pub term: &'static str,
    pub meaning: &'static str,
}

/// Lexique de la clôture (lot 35) — écrit pour un indépendant sans notion de comptabilité, et
/// partagé tel quel par les trois façades (`freeflow year glossary`, volet « lexique » du
/// panneau « parcours » de la fenêtre, ressource MCP `freeflow://closing-glossary`) : le
/// vocabulaire est expliqué une fois, dans le cœur, jamais paraphrasé par une façade. Dans
/// l'ordre où l'on rencontre les mots en clôturant.
pub const GLOSSARY: &[GlossaryEntry] = &[
    GlossaryEntry {
        term: "Exercice",
        meaning: "La période d'un an sur laquelle on fait les comptes (du 1er janvier au \
                  31 décembre, ou décalée — du 1er octobre au 30 septembre par exemple). \
                  FreeFlow désigne un exercice par l'année civile de sa fin : « 2026 » est \
                  l'exercice qui se termine en 2026.",
    },
    GlossaryEntry {
        term: "Bilan",
        meaning: "La photo, au dernier jour de l'exercice, de ce que la société possède (sa \
                  banque, ce que ses clients lui doivent, la TVA à récupérer — l'actif) et de \
                  ce qu'elle doit (TVA à reverser, impôt, et ce qu'elle doit à son associé : \
                  capital, bénéfices passés — le passif). Les deux totaux sont toujours égaux.",
    },
    GlossaryEntry {
        term: "Bilan d'ouverture",
        meaning: "La photo au premier jour du premier exercice suivi ici, recopiée compte par \
                  compte depuis le dernier bilan de l'expert-comptable. C'est le point de \
                  départ de tout : sans lui, FreeFlow part de zéro, comme si la société \
                  venait d'être créée.",
    },
    GlossaryEntry {
        term: "À-nouveaux",
        meaning: "Les soldes de départ d'un exercice : le bilan d'ouverture pour le premier, \
                  le bilan de clôture de l'exercice précédent pour les suivants. FreeFlow les \
                  enchaîne tout seul.",
    },
    GlossaryEntry {
        term: "Grand livre, balance",
        meaning: "Le grand livre est la liste de toutes les écritures comptables, compte par \
                  compte ; la balance en est le résumé (un solde par compte). FreeFlow les \
                  dérive de ce qu'il connaît (factures, encaissements, dépenses, relevé) : il \
                  ne tient pas de comptabilité, il la déduit de vos faits.",
    },
    GlossaryEntry {
        term: "Rapprochement bancaire",
        meaning: "Faire coïncider chaque ligne du relevé bancaire importé avec une facture \
                  encaissée (un crédit) ou une dépense (un débit). C'est ce qui rend le \
                  compte « banque » du bilan égal à votre vrai solde.",
    },
    GlossaryEntry {
        term: "Justificatif",
        meaning: "La pièce (facture, ticket, relevé) qui prouve une dépense. Sans elle, la \
                  dépense et sa TVA ne sont pas déductibles en cas de contrôle. FreeFlow \
                  l'archive à côté du coffre et en garde l'empreinte.",
    },
    GlossaryEntry {
        term: "Résultat",
        meaning: "Ce que l'exercice a rapporté : les ventes hors taxes moins les dépenses hors \
                  taxes (et la rémunération du président s'il y en a une). Positif, c'est un \
                  bénéfice ; négatif, une perte.",
    },
    GlossaryEntry {
        term: "Résultat fiscal, déficit reportable",
        meaning: "Le résultat sur lequel l'impôt se calcule. Une perte (un « déficit ») n'est \
                  pas perdue : elle se garde en réserve et vient réduire les bénéfices des \
                  exercices suivants avant impôt. FreeFlow tient ce compte tout seul.",
    },
    GlossaryEntry {
        term: "IS (impôt sur les sociétés)",
        meaning: "L'impôt de la société sur son résultat fiscal : 15 % jusqu'à 42 500 €, \
                  25 % au-delà. Nul quand l'exercice est en perte. Le solde se paie le 15 du \
                  quatrième mois suivant la clôture (relevé 2572, sur impots.gouv.fr).",
    },
    GlossaryEntry {
        term: "Report en arrière",
        meaning: "Une option, à la clôture d'un exercice en perte : au lieu de garder la perte \
                  pour plus tard, la déduire du bénéfice de l'exercice précédent — l'État doit \
                  alors à la société une partie de l'impôt déjà payé (une « créance »). \
                  Proposée seulement quand elle est possible.",
    },
    GlossaryEntry {
        term: "Report à nouveau",
        meaning: "Le cumul des bénéfices et des pertes passés que vous n'avez pas distribués. \
                  Il augmente d'un bénéfice, diminue d'une perte et des dividendes versés.",
    },
    GlossaryEntry {
        term: "Réserve légale",
        meaning: "Une part du bénéfice que la loi oblige à garder dans la société : au moins \
                  un vingtième (5 %) du bénéfice chaque année, jusqu'à ce que la réserve \
                  atteigne un dixième du capital. FreeFlow calcule ce minimum et vous le \
                  propose.",
    },
    GlossaryEntry {
        term: "Affectation du résultat",
        meaning: "Décider ce que devient le bénéfice : la réserve légale d'abord, puis des \
                  dividendes et/ou le report à nouveau. Une perte, elle, vient en moins du \
                  report à nouveau. Se décide à la clôture, se révise jusqu'à l'approbation.",
    },
    GlossaryEntry {
        term: "Approbation des comptes",
        meaning: "En SASU, la décision écrite de l'associé unique qui arrête les comptes de \
                  l'exercice, à prendre dans les six mois de la clôture. FreeFlow en rédige le \
                  procès-verbal (PV). Une fois approuvé, l'exercice ne se modifie plus.",
    },
    GlossaryEntry {
        term: "Liasse fiscale",
        meaning: "La déclaration annuelle de résultat : le formulaire 2065 et les tableaux \
                  2033 (bilan, compte de résultat, suivi des déficits). À télétransmettre sur \
                  impots.gouv.fr en EDI (via un expert-comptable ou un partenaire EDI) dans \
                  les trois mois de la clôture. FreeFlow en fournit les chiffres, case par case.",
    },
    GlossaryEntry {
        term: "FEC",
        meaning: "Le Fichier des Écritures Comptables, que l'administration peut demander en \
                  cas de contrôle : toutes les écritures de l'exercice dans un format imposé. \
                  FreeFlow l'exporte depuis son grand livre dérivé.",
    },
    GlossaryEntry {
        term: "Dépôt des comptes au greffe",
        meaning: "Rendre publics le bilan, le compte de résultat et la décision d'affectation, \
                  sur le guichet unique des formalités d'entreprises, dans le mois qui suit \
                  l'approbation (deux mois par voie électronique).",
    },
];

/// Le lexique en JSON (`[{ "term", "meaning" }]`), partagé par `year glossary --json` et la
/// ressource MCP.
#[must_use]
pub fn glossary_json() -> serde_json::Value {
    json!(GLOSSARY)
}

/// La vue JSON du parcours — la même pour `year checklist --json`, l'outil MCP et la ressource.
/// Montants en centimes (`_cents`), dates en `AAAA-MM-JJ`, clés et statuts en `snake_case`.
#[must_use]
pub fn checklist_json(checklist: &ClosingChecklist) -> serde_json::Value {
    json!({
        "period": checklist.period,
        "starts_on": format_date(checklist.exercise.start()),
        "ends_on": format_date(checklist.exercise.end()),
        "today": format_date(checklist.today),
        "stage": checklist.stage.as_str(),
        "stage_label": checklist.stage.label(),
        "fiscal_year_id": checklist.fiscal_year.as_ref().map(|r| r.id.to_string()),
        "approved_on": checklist
            .fiscal_year
            .as_ref()
            .and_then(|r| r.approved_on)
            .map(format_date),
        "result": checklist.result.as_ref().map(result_json),
        "minimum_legal_reserve_cents": checklist.minimum_legal_reserve.map(Money::cents),
        "carry_back_available_cents": checklist.carry_back_available.map(|(m, _)| m.cents()),
        "carry_back_credit_cents": checklist.carry_back_available.map(|(_, c)| c.cents()),
        "blocked": checklist.blocking().count(),
        "steps": checklist
            .steps
            .iter()
            .map(|s| json!({
                "key": s.key.as_str(),
                "phase": s.phase.as_str(),
                "status": s.status.as_str(),
                "title": s.title,
                "detail": s.detail,
                "due_on": s.due_on.map(format_date),
                "amount_cents": s.amount.map(Money::cents),
            }))
            .collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Actor, ExecutionContext, Executor, Outcome};
    use crate::billing::{EmitInvoice, ImportBankTransactions, ParsedTransaction, RecordPayment};
    use crate::company::SetCompanyProfile;
    use crate::domain::{
        Address, ClientId, ExpenseCategory, InvoiceLine, OpeningBalanceLine, PaymentMethod, Siren,
        VatRate, VatRegime,
    };
    use crate::expenses::RecordExpense;
    use crate::fiscal_year::{ApproveFiscalYear, CloseFiscalYear};
    use crate::opening_balance::RecordOpeningBalance;
    use crate::store::{Passphrase, Store};
    use time::Month as TimeMonth;

    #[test]
    fn the_glossary_explains_every_step_title_without_repeating_itself() {
        // Lot 35 : chaque entrée est unique, non vide, et le lexique couvre le vocabulaire des
        // titres d'étapes qu'un non-comptable rencontre en premier.
        let mut seen = std::collections::HashSet::new();
        for entry in GLOSSARY {
            assert!(
                !entry.term.is_empty() && entry.meaning.len() > 40,
                "{entry:?}"
            );
            assert!(seen.insert(entry.term), "terme en double : {}", entry.term);
        }
        for word in [
            "Bilan d'ouverture",
            "Rapprochement bancaire",
            "Affectation du résultat",
            "Approbation des comptes",
            "Liasse fiscale",
            "Dépôt des comptes au greffe",
        ] {
            assert!(
                GLOSSARY.iter().any(|e| e.term == word),
                "{word} manque au lexique"
            );
        }
        assert_eq!(glossary_json().as_array().unwrap().len(), GLOSSARY.len());
    }

    fn test_store(label: &str) -> Store {
        let dir = std::env::temp_dir().join(format!(
            "freeflow-closing-test-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ));
        Store::create(&dir.join("vault.db"), &Passphrase::from("s3cret")).unwrap()
    }

    fn human() -> ExecutionContext {
        ExecutionContext::new(Actor::Human, false)
    }

    fn date(y: i32, m: TimeMonth, d: u8) -> Date {
        Date::from_calendar_date(y, m, d).unwrap()
    }

    fn set_profile(store: &mut Store, capital_cents: Option<i64>) {
        let cmd = SetCompanyProfile {
            name: "Argon Digital".to_string(),
            legal_form: "SASU".to_string(),
            siren: Siren::parse("552100554").unwrap(),
            vat_number: None,
            address: Address {
                street: "12 rue de la Paix".to_string(),
                postal_code: "75002".to_string(),
                city: "Paris".to_string(),
                country: "FR".to_string(),
            },
            share_capital: capital_cents.map(Money::from_cents),
            rcs_city: Some("Paris".to_string()),
            iban: None,
            fiscal_year_end: Some(FiscalYearEnd::CALENDAR),
            vat_regime: Some(VatRegime::RealNormalMonthly),
            director_monthly_gross: None,
            director_charge_ratio_bps: None,
        };
        Executor::new(store).execute(&cmd, &human()).unwrap();
    }

    fn client(store: &Store) -> ClientId {
        let client_id = ClientId::new();
        store
            .connection()
            .execute(
                "INSERT INTO clients (id, name, created_at) \
                 VALUES (?1, 'Client Test', '2026-01-01T00:00:00Z')",
                [client_id.to_string()],
            )
            .unwrap();
        client_id
    }

    /// Une facture de 6 175 € HT (7 410 € TTC) le 30 septembre `year`, et une dépense de
    /// 1 000 € TTC (TVA déductible 200 €) le 5 octobre, sans justificatif : résultat avant IS
    /// 5 375 €, IS 806,25 €, net 4 568,75 € (chiffres du test d'intégration d'`accounting.rs`).
    fn seed_activity(store: &mut Store, year: i32) -> crate::domain::InvoiceId {
        let client_id = client(store);
        let Outcome::Applied(emitted) = Executor::new(store)
            .execute(
                &EmitInvoice {
                    client_id,
                    mission_id: None,
                    lines: vec![InvoiceLine {
                        description: "Prestation".to_string(),
                        quantity: 9.5,
                        unit_price: Money::from_cents(65_000),
                        vat_rate: VatRate::Standard,
                    }],
                    issued_on: date(year, TimeMonth::September, 30),
                    payment_terms_days: 30,
                },
                &human(),
            )
            .unwrap()
        else {
            panic!("expected Applied")
        };
        let invoice_id = emitted.id;
        Executor::new(store)
            .execute(
                &RecordExpense {
                    label: "Matériel".to_string(),
                    category: ExpenseCategory::Equipment,
                    amount: Money::from_cents(100_000),
                    vat_rate: VatRate::Standard,
                    vat_deductible: Money::from_cents(20_000),
                    incurred_on: date(year, TimeMonth::October, 5),
                    receipt_hash: None,
                    receipt_filename: None,
                    bank_transaction_id: None,
                },
                &human(),
            )
            .unwrap();
        invoice_id
    }

    fn close(store: &mut Store, year: i32, legal_reserve: i64) -> crate::domain::FiscalYearId {
        let cmd = CloseFiscalYear {
            starts_on: date(year, TimeMonth::January, 1),
            ends_on: date(year, TimeMonth::December, 31),
            legal_reserve: Money::from_cents(legal_reserve),
            dividends: Money::ZERO,
            carry_back: false,
            today: None,
        };
        let Outcome::Applied(id) = Executor::new(store).execute(&cmd, &human()).unwrap() else {
            panic!("expected Applied")
        };
        id
    }

    fn status_of(checklist: &ClosingChecklist, key: ClosingStepKey) -> StepStatus {
        checklist.step(key).unwrap().status
    }

    // -- Règle pure : dotation minimale à la réserve légale --------------------------------

    #[test]
    fn minimum_legal_reserve_is_a_twentieth_of_the_profit_net_of_prior_losses() {
        let capital = Some(Money::from_cents(1_000_000));
        // 5 % de 4 568,75 € = 228,44 € (arrondi au centime, pair).
        assert_eq!(
            minimum_legal_reserve(
                Money::from_cents(456_875),
                Money::ZERO,
                Money::ZERO,
                capital
            ),
            Money::from_cents(22_844)
        );
        // Un report à nouveau débiteur réduit la base ; un report créditeur ne l'augmente pas.
        assert_eq!(
            minimum_legal_reserve(
                Money::from_cents(456_875),
                Money::from_cents(-56_875),
                Money::ZERO,
                capital
            ),
            Money::from_cents(20_000)
        );
        assert_eq!(
            minimum_legal_reserve(
                Money::from_cents(456_875),
                Money::from_cents(900_000),
                Money::ZERO,
                capital
            ),
            Money::from_cents(22_844)
        );
        // Perte : rien à doter.
        assert_eq!(
            minimum_legal_reserve(Money::from_cents(-100), Money::ZERO, Money::ZERO, capital),
            Money::ZERO
        );
    }

    #[test]
    fn minimum_legal_reserve_stops_at_a_tenth_of_the_share_capital() {
        let capital = Some(Money::from_cents(1_000_000));
        // Plafond 1 000 € ; 950 € déjà dotés : il ne reste que 50 € à doter, pas 228,44 €.
        assert_eq!(
            minimum_legal_reserve(
                Money::from_cents(456_875),
                Money::ZERO,
                Money::from_cents(95_000),
                capital
            ),
            Money::from_cents(5_000)
        );
        // Plafond atteint : le prélèvement cesse d'être obligatoire.
        assert_eq!(
            minimum_legal_reserve(
                Money::from_cents(456_875),
                Money::ZERO,
                Money::from_cents(100_000),
                capital
            ),
            Money::ZERO
        );
        // Sans capital connu : le vingtième plein, faute de pouvoir plafonner.
        assert_eq!(
            minimum_legal_reserve(Money::from_cents(456_875), Money::ZERO, Money::ZERO, None),
            Money::from_cents(22_844)
        );
    }

    // -- Parcours sur base ----------------------------------------------------------------------

    #[test]
    fn without_a_profile_the_journey_is_blocked_on_its_first_step() {
        let store = test_store("no-profile");
        let checklist =
            closing_checklist(store.connection(), 2026, date(2027, TimeMonth::March, 1)).unwrap();
        assert_eq!(checklist.stage, ClosingStage::Blocked);
        assert_eq!(checklist.exercise, FiscalYear::calendar(2026));
        assert_eq!(
            status_of(&checklist, ClosingStepKey::Profile),
            StepStatus::Blocked
        );
        assert_eq!(
            status_of(&checklist, ClosingStepKey::Result),
            StepStatus::Later
        );
        assert_eq!(
            status_of(&checklist, ClosingStepKey::BalanceSheet),
            StepStatus::Later
        );
        assert_eq!(
            status_of(&checklist, ClosingStepKey::Close),
            StepStatus::Blocked
        );
        assert!(checklist.result.is_none());
        assert_eq!(checklist.blocking().count(), 2, "profil + clôture");
        // Toutes les étapes sont là, dans l'ordre des phases.
        let phases: Vec<_> = checklist.steps.iter().map(|s| s.phase).collect();
        let mut sorted = phases.clone();
        sorted.sort_by_key(|p| ClosingPhase::ALL.iter().position(|q| q == p));
        assert_eq!(phases, sorted);
        assert_eq!(checklist.steps.len(), 16);
    }

    #[test]
    fn a_running_exercise_is_not_ended_and_cannot_be_closed_yet() {
        let mut store = test_store("running");
        set_profile(&mut store, Some(1_000_000));
        seed_activity(&mut store, 2026);
        let checklist = closing_checklist(
            store.connection(),
            2026,
            date(2026, TimeMonth::November, 15),
        )
        .unwrap();
        assert_eq!(checklist.stage, ClosingStage::NotEnded);
        let ended = checklist.step(ClosingStepKey::PeriodEnded).unwrap();
        assert_eq!(ended.status, StepStatus::Blocked);
        assert_eq!(ended.due_on, Some(date(2026, TimeMonth::December, 31)));
        // Le résultat prévisionnel est quand même montré.
        let result = checklist.result.unwrap();
        assert_eq!(result.result_before_tax, Money::from_cents(537_500));
        assert_eq!(
            status_of(&checklist, ClosingStepKey::Result),
            StepStatus::Info
        );
        assert!(
            checklist
                .step(ClosingStepKey::Result)
                .unwrap()
                .detail
                .starts_with("Prévision")
        );
    }

    #[test]
    fn a_first_exercise_without_opening_balance_is_ready_with_warnings() {
        let mut store = test_store("ready");
        set_profile(&mut store, Some(1_000_000));
        seed_activity(&mut store, 2026);
        let today = date(2027, TimeMonth::January, 10);
        let checklist = closing_checklist(store.connection(), 2026, today).unwrap();
        assert_eq!(checklist.stage, ClosingStage::Ready);
        assert_eq!(
            status_of(&checklist, ClosingStepKey::Profile),
            StepStatus::Done
        );
        assert_eq!(
            status_of(&checklist, ClosingStepKey::OpeningBalance),
            StepStatus::Warning,
            "chaîne partant de zéro : à signaler, pas à bloquer"
        );
        assert_eq!(
            status_of(&checklist, ClosingStepKey::PreviousYear),
            StepStatus::Info
        );
        // Facture non encaissée (7 410 € TTC, échue le 30 octobre) et dépense sans justificatif.
        let invoices = checklist.step(ClosingStepKey::Invoices).unwrap();
        assert_eq!(invoices.status, StepStatus::Warning);
        assert_eq!(invoices.amount, Some(Money::from_cents(741_000)));
        assert!(
            invoices.detail.contains("1 en retard"),
            "{}",
            invoices.detail
        );
        assert_eq!(
            status_of(&checklist, ClosingStepKey::Expenses),
            StepStatus::Warning
        );
        assert_eq!(
            status_of(&checklist, ClosingStepKey::Bank),
            StepStatus::Info
        );
        assert_eq!(
            status_of(&checklist, ClosingStepKey::BalanceSheet),
            StepStatus::Done
        );
        // Prêt à clore, avec la dotation minimale (5 % de 4 568,75 € = 228,44 €).
        let close = checklist.step(ClosingStepKey::Close).unwrap();
        assert_eq!(close.status, StepStatus::Todo);
        assert_eq!(close.amount, Some(Money::from_cents(22_844)));
        assert_eq!(
            checklist.minimum_legal_reserve,
            Some(Money::from_cents(22_844))
        );
        assert!(
            checklist.carry_back_available.is_none(),
            "bénéfice : pas de déficit"
        );
        // L'aval n'est pas encore atteignable, mais ses échéances sont déjà connues.
        for key in [
            ClosingStepKey::Appropriation,
            ClosingStepKey::Approve,
            ClosingStepKey::Documents,
            ClosingStepKey::CorporateTax,
            ClosingStepKey::Liasse,
            ClosingStepKey::Filing,
        ] {
            assert_eq!(status_of(&checklist, key), StepStatus::Later, "{key:?}");
        }
        assert_eq!(
            checklist.step(ClosingStepKey::Approve).unwrap().due_on,
            Some(date(2027, TimeMonth::June, 30))
        );
        assert_eq!(
            checklist.step(ClosingStepKey::CorporateTax).unwrap().due_on,
            Some(date(2027, TimeMonth::May, 15))
        );
        assert_eq!(
            checklist.step(ClosingStepKey::Liasse).unwrap().due_on,
            Some(date(2027, TimeMonth::May, 15))
        );
    }

    #[test]
    fn a_mismatched_opening_balance_blocks_exactly_where_the_close_would_refuse() {
        let mut store = test_store("mismatch");
        set_profile(&mut store, Some(1_000_000));
        seed_activity(&mut store, 2026);
        Executor::new(&mut store)
            .execute(
                &RecordOpeningBalance {
                    opens_on: date(2026, TimeMonth::February, 1),
                    source: None,
                    lines: vec![
                        "101000:Capital social:C:1000.00"
                            .parse::<OpeningBalanceLine>()
                            .unwrap(),
                        "512000:Banque:D:1000.00"
                            .parse::<OpeningBalanceLine>()
                            .unwrap(),
                    ],
                    tax_losses: Money::ZERO,
                },
                &human(),
            )
            .unwrap();
        let checklist =
            closing_checklist(store.connection(), 2026, date(2027, TimeMonth::January, 10))
                .unwrap();
        assert_eq!(checklist.stage, ClosingStage::Blocked);
        assert_eq!(
            status_of(&checklist, ClosingStepKey::OpeningBalance),
            StepStatus::Blocked
        );
        assert_eq!(
            status_of(&checklist, ClosingStepKey::Close),
            StepStatus::Blocked
        );
        // Sans chaîne lisible, pas de dotation minimale proposée — mais le résultat, lui, se
        // calcule (lecture tolérante).
        assert!(checklist.minimum_legal_reserve.is_none());
        assert!(checklist.result.is_some());
        // La commande refuse bien au même endroit.
        let refused = Executor::new(&mut store).execute(
            &CloseFiscalYear {
                starts_on: date(2026, TimeMonth::January, 1),
                ends_on: date(2026, TimeMonth::December, 31),
                legal_reserve: Money::ZERO,
                dividends: Money::ZERO,
                carry_back: false,
                today: None,
            },
            &human(),
        );
        assert!(
            refused
                .unwrap_err()
                .to_string()
                .contains("bilan d'ouverture est daté du 2026-02-01")
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn the_journey_follows_the_exercise_from_draft_to_approved() {
        let mut store = test_store("lifecycle");
        set_profile(&mut store, Some(1_000_000));
        let invoice_id = seed_activity(&mut store, 2026);
        // Encaissement rapproché d'un relevé où reste un débit non apparié.
        Executor::new(&mut store)
            .execute(
                &ImportBankTransactions {
                    transactions: vec![
                        ParsedTransaction {
                            occurred_on: date(2026, TimeMonth::October, 20),
                            amount_cents: 741_000,
                            description: "VIR CLIENT TEST".to_string(),
                        },
                        ParsedTransaction {
                            occurred_on: date(2026, TimeMonth::October, 6),
                            amount_cents: -100_000,
                            description: "CB MATERIEL".to_string(),
                        },
                    ],
                },
                &human(),
            )
            .unwrap();
        Executor::new(&mut store)
            .execute(
                &RecordPayment {
                    invoice_id,
                    amount: Money::from_cents(741_000),
                    received_on: date(2026, TimeMonth::October, 20),
                    method: PaymentMethod::BankTransfer,
                },
                &human(),
            )
            .unwrap();

        let before =
            closing_checklist(store.connection(), 2026, date(2027, TimeMonth::January, 10))
                .unwrap();
        assert_eq!(before.stage, ClosingStage::Ready);
        assert_eq!(
            status_of(&before, ClosingStepKey::Invoices),
            StepStatus::Done
        );
        let bank = before.step(ClosingStepKey::Bank).unwrap();
        assert_eq!(bank.status, StepStatus::Warning);
        assert!(
            bank.detail.starts_with("2 non rapproché(s) sur 2"),
            "{}",
            bank.detail
        );

        // Clôture avec une dotation trop faible : le parcours le dit, le cœur l'accepte.
        let id = close(&mut store, 2026, 10_000);
        let draft =
            closing_checklist(store.connection(), 2026, date(2027, TimeMonth::March, 1)).unwrap();
        assert_eq!(draft.stage, ClosingStage::Draft);
        assert_eq!(draft.fiscal_year.as_ref().map(|r| r.id), Some(id));
        assert_eq!(status_of(&draft, ClosingStepKey::Close), StepStatus::Done);
        assert!(
            draft
                .step(ClosingStepKey::Result)
                .unwrap()
                .detail
                .starts_with("Figé à la clôture")
        );
        let appropriation = draft.step(ClosingStepKey::Appropriation).unwrap();
        assert_eq!(appropriation.status, StepStatus::Warning);
        assert_eq!(appropriation.amount, Some(Money::from_cents(22_844)));
        assert!(
            appropriation.detail.contains("Révisez l'affectation"),
            "{}",
            appropriation.detail
        );
        let approve = draft.step(ClosingStepKey::Approve).unwrap();
        assert_eq!(approve.status, StepStatus::Todo);
        assert_eq!(approve.due_on, Some(date(2027, TimeMonth::June, 30)));
        let tax = draft.step(ClosingStepKey::CorporateTax).unwrap();
        assert_eq!(tax.status, StepStatus::Todo);
        assert_eq!(tax.amount, Some(Money::from_cents(80_625)));
        assert_eq!(status_of(&draft, ClosingStepKey::Liasse), StepStatus::Todo);
        assert_eq!(status_of(&draft, ClosingStepKey::Filing), StepStatus::Later);
        assert_eq!(
            status_of(&draft, ClosingStepKey::Documents),
            StepStatus::Todo
        );
        // Les étapes de préparation restent lisibles après coup (l'exercice est écoulé).
        assert_eq!(
            status_of(&draft, ClosingStepKey::PeriodEnded),
            StepStatus::Done
        );

        // En retard sur l'AG : avertissement, pas simple « à faire ».
        let late =
            closing_checklist(store.connection(), 2026, date(2027, TimeMonth::August, 1)).unwrap();
        assert_eq!(
            status_of(&late, ClosingStepKey::Approve),
            StepStatus::Warning
        );
        assert_eq!(
            status_of(&late, ClosingStepKey::CorporateTax),
            StepStatus::Warning
        );

        // Approbation : ne restent que les dépôts.
        let record = crate::fiscal_year::fiscal_year_by_id(store.connection(), id)
            .unwrap()
            .unwrap();
        Executor::new(&mut store)
            .execute(
                &ApproveFiscalYear {
                    id,
                    revision: record.revision,
                    approved_on: date(2027, TimeMonth::April, 15),
                    today: None,
                },
                &human(),
            )
            .unwrap();
        let approved =
            closing_checklist(store.connection(), 2026, date(2027, TimeMonth::April, 20)).unwrap();
        assert_eq!(approved.stage, ClosingStage::Approved);
        assert_eq!(
            status_of(&approved, ClosingStepKey::Approve),
            StepStatus::Done
        );
        let filing = approved.step(ClosingStepKey::Filing).unwrap();
        assert_eq!(filing.status, StepStatus::Todo);
        assert_eq!(filing.due_on, Some(date(2027, TimeMonth::May, 15)));
        assert_eq!(
            status_of(&approved, ClosingStepKey::Appropriation),
            StepStatus::Warning,
            "la dotation insuffisante reste signalée sur un exercice approuvé"
        );

        // L'exercice suivant hérite : bilan d'ouverture sans objet, précédent approuvé.
        let next =
            closing_checklist(store.connection(), 2027, date(2028, TimeMonth::January, 5)).unwrap();
        assert_eq!(
            status_of(&next, ClosingStepKey::OpeningBalance),
            StepStatus::Info
        );
        assert_eq!(
            status_of(&next, ClosingStepKey::PreviousYear),
            StepStatus::Done
        );
        assert_eq!(next.stage, ClosingStage::Ready);

        // La vue JSON partagée.
        let json = checklist_json(&approved);
        assert_eq!(json["stage"], "approved");
        assert_eq!(json["fiscal_year_id"], id.to_string());
        assert_eq!(json["approved_on"], "2027-04-15");
        assert_eq!(json["result"]["corporate_tax_cents"], 80_625);
        assert_eq!(json["minimum_legal_reserve_cents"], 22_844);
        assert_eq!(json["steps"].as_array().unwrap().len(), 16);
        assert_eq!(json["steps"][0]["key"], "profile");
        assert_eq!(json["steps"][0]["status"], "done");
    }

    #[test]
    fn a_deficit_after_a_profitable_year_offers_the_carry_back_and_warns_on_a_draft_predecessor() {
        let mut store = test_store("carry-back");
        set_profile(&mut store, Some(1_000_000));
        seed_activity(&mut store, 2026);
        close(&mut store, 2026, 22_844);
        // 2027 : une seule dépense de 800 € HT, aucun CA.
        Executor::new(&mut store)
            .execute(
                &RecordExpense {
                    label: "Honoraires".to_string(),
                    category: ExpenseCategory::Fees,
                    amount: Money::from_cents(96_000),
                    vat_rate: VatRate::Standard,
                    vat_deductible: Money::from_cents(16_000),
                    incurred_on: date(2027, TimeMonth::March, 3),
                    receipt_hash: Some("abc".to_string()),
                    receipt_filename: Some("honoraires.pdf".to_string()),
                    bank_transaction_id: None,
                },
                &human(),
            )
            .unwrap();
        let checklist =
            closing_checklist(store.connection(), 2027, date(2028, TimeMonth::January, 10))
                .unwrap();
        assert_eq!(checklist.stage, ClosingStage::Ready);
        assert_eq!(
            status_of(&checklist, ClosingStepKey::PreviousYear),
            StepStatus::Warning,
            "l'exercice 2026 est encore un projet : clore 2027 le figerait"
        );
        assert_eq!(
            status_of(&checklist, ClosingStepKey::Expenses),
            StepStatus::Done
        );
        assert_eq!(
            status_of(&checklist, ClosingStepKey::Invoices),
            StepStatus::Info
        );
        let result = checklist.result.unwrap();
        assert_eq!(result.deficit(), Money::from_cents(80_000));
        // 800 € imputables sur les 5 375 € de bénéfice fiscal 2026 (au taux réduit) : créance
        // de 15 % × 800 = 120 €.
        assert_eq!(
            checklist.carry_back_available,
            Some((Money::from_cents(80_000), Money::from_cents(12_000)))
        );
        let detail = &checklist.step(ClosingStepKey::Result).unwrap().detail;
        assert!(detail.contains("reportable en arrière"), "{detail}");
        assert_eq!(
            checklist.minimum_legal_reserve,
            Some(Money::ZERO),
            "pas de dotation sur une perte"
        );
    }
}
