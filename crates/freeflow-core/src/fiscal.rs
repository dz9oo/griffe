//! Calendrier fiscal et social d'une SASU à l'IS — **dates indicatives**, désormais assorties de
//! **montants** là où le domaine peut les calculer (lot 19).
//!
//! **Ce module reste une aide au pilotage, pas une source de vérité fiscale.** Les dates dérivent
//! de la date de clôture d'exercice déclarée dans le profil ([`crate::company::CompanyProfile`]) ;
//! à défaut, l'année civile est supposée. La date limite de télédéclaration CA3 suit la grille
//! officielle (zone du siège, catégorie de redevable, deux premiers chiffres du SIREN — voir
//! [`crate::domain::Ca3FilingRule`]) et la périodicité du régime de TVA déclaré, avec report au
//! jour ouvré suivant (lot 26) ; un redevable au **réel simplifié** reçoit à la place ses deux
//! acomptes semestriels (formulaire 3514, juillet 55 % / décembre 40 %) et sa CA12 annuelle, tant
//! que ce régime existe — il est supprimé pour les exercices ouverts à compter du 1er janvier
//! 2027, au-delà desquels il bascule en CA3 trimestrielle (lot 27, voir
//! [`SIMPLIFIED_REGIME_REPEAL`]) ; les échéances de la liasse, du dépôt des comptes et de l'AG
//! sont des règles générales approchées. Les montants (TVA à reverser, IS, cotisations DSN) sont
//! calculés à partir des données saisies et restent indicatifs : ils ne remplacent ni la
//! télédéclaration sur impots.gouv.fr, ni le travail de l'expert-comptable.
//!
//! Deux niveaux d'API :
//! - [`upcoming_deadlines`]/[`deadlines_due_within`] : **pures**, dates seules (année civile),
//!   conservées pour le tableau de bord et les alertes qui n'ouvrent pas la base ;
//! - [`fiscal_calendar`] : **requête** qui lit le profil et les données pour produire un
//!   calendrier chiffré, dérivé de l'exercice réel.

use rusqlite::Connection;
use std::fmt::Write as _;
use time::Date;

use crate::accounting::{compute_result, vat_due_for_period};
use crate::app::AppError;
use crate::company::{CompanyProfile, company_profile};
use crate::domain::{
    Ca3FilingRule, FiscalYear, FiscalYearEnd, Money, Month, VatRegime, is_french_business_day,
    next_french_business_day_on_or_after,
};
use crate::opening_balance::opening_balance;

/// Seuil de dispense des acomptes d'IS : aucun acompte n'est dû si l'IS de l'exercice précédent
/// est inférieur à 3 000 €.
const IS_ACOMPTE_DISPENSATION: Money = Money::from_cents(300_000);

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum FiscalDeadlineKind {
    /// Déclaration de TVA (CA3) — périodicité pilotée par le régime de TVA.
    Ca3,
    /// Acompte semestriel de TVA du régime simplifié (formulaire 3514, juillet ou décembre).
    VatInstalment,
    /// Déclaration annuelle de régularisation de TVA du régime simplifié (CA12, ou CA12 E pour un
    /// exercice décalé).
    Ca12,
    /// Acompte trimestriel d'impôt sur les sociétés (formulaire 2571).
    IsAcompte,
    /// Solde d'impôt sur les sociétés (formulaire 2572).
    IsSolde,
    /// Cotisation foncière des entreprises.
    Cfe,
    /// Déclaration de résultats et liasse fiscale (2065 + tableaux 2033).
    Liasse,
    /// Assemblée générale d'approbation des comptes (dans les 6 mois de la clôture).
    ApprovalMeeting,
    /// Dépôt des comptes annuels au greffe.
    AccountsFiling,
    /// Déclaration sociale nominative mensuelle (président rémunéré uniquement).
    Dsn,
    /// Déclaration des honoraires (DAS2, art. 240 CGI) : cumul par bénéficiaire et par année
    /// civile au-delà de 2 400 € (lot 41).
    Das2,
    /// Prélèvement forfaitaire et prélèvements sociaux retenus sur des dividendes (formulaire
    /// 2777), avant le 15 du mois suivant la mise en paiement (lot 41).
    Dividends2777,
}

impl FiscalDeadlineKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ca3 => "ca3",
            Self::VatInstalment => "vat_acompte",
            Self::Ca12 => "ca12",
            Self::IsAcompte => "is_acompte",
            Self::IsSolde => "is_solde",
            Self::Cfe => "cfe",
            Self::Liasse => "liasse",
            Self::ApprovalMeeting => "approval_meeting",
            Self::AccountsFiling => "accounts_filing",
            Self::Dsn => "dsn",
            Self::Das2 => "das2",
            Self::Dividends2777 => "dividends_2777",
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct FiscalDeadline {
    pub kind: FiscalDeadlineKind,
    /// Toujours `>= today` : ce sont les *prochaines* échéances, jamais des échéances passées.
    #[serde(with = "crate::domain::serde_date::date")]
    pub due_on: Date,
    /// Montant estimé quand le domaine peut le calculer (TVA à reverser, IS, cotisations),
    /// `None` pour une échéance purement calendaire (liasse, AG, dépôt, CFE).
    pub amount: Option<Money>,
    /// Précision facultative (« indicatif — dernier chiffre SIREN inconnu », etc.).
    pub note: Option<String>,
}

fn nth_of_month(month: Month, day: u8) -> Date {
    Date::from_calendar_date(
        month.year(),
        time::Month::try_from(month.month()).expect("le mois est garanti valide"),
        day,
    )
    .expect("les jours utilisés ici (1, et 15 à 28) sont valides dans tous les mois")
}

/// Périodicité de la déclaration CA3, pilotée par le régime de TVA du profil.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Ca3Periodicity {
    /// Une CA3 par mois, déclarant le mois précédent.
    Monthly,
    /// Une CA3 par trimestre civil, déposée le mois qui suit le trimestre (janvier, avril,
    /// juillet, octobre).
    Quarterly,
}

impl Ca3Periodicity {
    /// `None` quand le régime ne donne lieu à aucune CA3 *par lui-même* : réel simplifié (acomptes
    /// semestriels et CA12 annuelle, voir [`VatFilingScheme`] qui tient aussi compte de la
    /// suppression de ce régime) ou franchise en base (aucune TVA collectée). Un régime non
    /// renseigné est supposé mensuel — la périodicité la plus exigeante, donc jamais en retard.
    #[must_use]
    pub const fn from_regime(regime: Option<VatRegime>) -> Option<Self> {
        match regime {
            None | Some(VatRegime::RealNormalMonthly) => Some(Self::Monthly),
            Some(VatRegime::RealNormalQuarterly) => Some(Self::Quarterly),
            Some(VatRegime::RealSimplified | VatRegime::Franchise) => None,
        }
    }

    /// Une CA3 est-elle déposée au cours de ce mois ?
    const fn files_in(self, month: Month) -> bool {
        match self {
            Self::Monthly => true,
            Self::Quarterly => matches!(month.month(), 1 | 4 | 7 | 10),
        }
    }

    /// Premier mois de la période que déclare une CA3 déposée en `filing_month`.
    const fn period_start(self, filing_month: Month) -> Month {
        match self {
            Self::Monthly => filing_month.pred(),
            Self::Quarterly => filing_month.pred().pred().pred(),
        }
    }
}

/// Une échéance CA3 datée : la période déclarée et la date limite de dépôt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ca3Filing {
    pub period_start: Month,
    pub period_end: Month,
    /// Le jour de la grille officielle tel quel, avant report.
    pub nominal_due_on: Date,
    /// Date limite effective : `nominal_due_on`, reportée au premier jour ouvré suivant si elle
    /// tombe un samedi, un dimanche ou un jour férié.
    pub due_on: Date,
}

/// Prochaine échéance CA3 à partir de `today` (`due_on >= today`), pour une périodicité et un
/// jour de grille donnés (voir [`Ca3FilingRule`] ; un jour hors de `15..=24` y est ramené).
///
/// # Panics
///
/// Ne panique jamais en pratique : le jour est borné à `15..=24`, valide dans tous les mois, et
/// le mois courant est valide par construction.
#[must_use]
pub fn next_ca3_filing(today: Date, periodicity: Ca3Periodicity, day: u8) -> Ca3Filing {
    next_ca3_filing_from(today, periodicity, day, None)
}

/// Comme [`next_ca3_filing`], mais en ne retenant qu'une CA3 dont la période déclarée commence à
/// `earliest_period` ou après — le cas d'un redevable qui *entre* dans le réel normal : les mois
/// antérieurs relevaient d'un autre régime (CA12 du réel simplifié) et ne se déclarent pas en CA3.
///
/// # Panics
///
/// Ne panique jamais en pratique : mêmes bornes que [`next_ca3_filing`].
#[must_use]
pub fn next_ca3_filing_from(
    today: Date,
    periodicity: Ca3Periodicity,
    day: u8,
    earliest_period: Option<Month>,
) -> Ca3Filing {
    let day = day.clamp(Ca3FilingRule::EARLIEST_DAY, 24);
    let mut filing_month =
        Month::new(today.year(), u8::from(today.month())).expect("mois courant valide");
    // Termine en un nombre borné d'itérations : chaque trimestre contient un mois de dépôt, et
    // `earliest_period` est une borne fixe que `filing_month` finit toujours par dépasser.
    loop {
        if periodicity.files_in(filing_month) {
            let period_start = periodicity.period_start(filing_month);
            let nominal_due_on = nth_of_month(filing_month, day);
            let due_on = next_french_business_day_on_or_after(nominal_due_on);
            if due_on >= today && earliest_period.is_none_or(|earliest| period_start >= earliest) {
                return Ca3Filing {
                    period_start,
                    period_end: filing_month.pred(),
                    nominal_due_on,
                    due_on,
                };
            }
        }
        filing_month = filing_month.succ();
    }
}

// ---------------------------------------------------------------------------------------------
// Régime simplifié d'imposition (RSI) de TVA : acomptes semestriels (3514) et CA12 annuelle.
// ---------------------------------------------------------------------------------------------

/// Premier jour à partir duquel un exercice ne relève plus du régime simplifié de TVA.
///
/// L'art. 38 de la loi n° 2025-127 du 14 février 2025 (loi de finances pour 2025) supprime le
/// régime simplifié d'imposition à compter du 1er janvier 2027 ; pour un exercice qui ne coïncide
/// pas avec l'année civile, il prend fin à compter de l'exercice ouvert après le 31 décembre 2026
/// (rappelé par le rescrit BOI-RES-TVA-000253). Au-delà, l'entreprise relève du réel normal, en
/// déclaration trimestrielle sauf option pour le mensuel — c'est la bascule que
/// [`VatFilingScheme::for_exercise`] applique.
pub const SIMPLIFIED_REGIME_REPEAL: Date =
    match Date::from_calendar_date(2027, time::Month::January, 1) {
        Ok(date) => date,
        Err(_) => panic!("le 1er janvier 2027 est une date valide"),
    };

/// Seuil de remboursement d'un crédit de TVA sur la CA12 (formulaire 3519) : 150 € (lot 41).
pub const CA12_REFUND_THRESHOLD: Money = Money::from_cents(15_000);

/// Seuil de la déclaration des honoraires (DAS2) par bénéficiaire et par année civile — 2 400 €
/// depuis les sommes versées en 2024 (art. 240 CGI, BOI-BIC-DECLA-30-70-20 § 140 ; c'était
/// 1 200 € avant). Lot 41.
pub const DAS2_THRESHOLD: Money = Money::from_cents(240_000);

/// Prélèvement forfaitaire non libératoire retenu sur les dividendes (12,8 %), et prélèvements
/// sociaux : 17,2 % jusqu'en 2025, 18,6 % pour les revenus perçus à compter du 1er janvier
/// 2026 (loi de financement de la sécurité sociale pour 2026) — lot 41.
pub const DIVIDEND_INCOME_TAX_BPS: u32 = 1_280;

/// Le taux de prélèvements sociaux applicable à des dividendes mis en paiement à `paid_on`.
#[must_use]
pub fn dividend_social_charges_bps(paid_on: Date) -> u32 {
    if paid_on.year() >= 2026 { 1_860 } else { 1_720 }
}

/// L'échéance de la déclaration des honoraires (DAS2) pour les sommes de l'année civile `year`
/// : avec la déclaration de résultats pour un exercice civil (2e jour ouvré suivant le 1er mai
/// de N+1, BOI-BIC-DECLA-30-70-20 § 400), sinon dans les trois mois de la clôture de l'exercice
/// qui suit la fin de l'année civile (même §).
///
/// # Panics
///
/// Jamais en pratique : le 1er mai et le 31 décembre existent chaque année.
#[must_use]
pub fn das2_due_on(fye: FiscalYearEnd, year: i32) -> Date {
    if fye == FiscalYearEnd::CALENDAR {
        let may_first = Date::from_calendar_date(year + 1, time::Month::May, 1)
            .expect("le 1er mai existe chaque année");
        return second_business_day_after(may_first);
    }
    let dec_31 = Date::from_calendar_date(year, time::Month::December, 31)
        .expect("le 31 décembre existe chaque année");
    let closing = fye.containing(dec_31).end();
    add_months(closing, 3)
}

/// L'échéance du formulaire 2777 pour des dividendes mis en paiement à `paid_on` : le 15 du
/// mois suivant, reporté au jour ouvré.
///
/// # Panics
///
/// Jamais en pratique : le 15 existe dans chaque mois.
#[must_use]
pub fn dividends_2777_due_on(paid_on: Date) -> Date {
    let next = add_months(paid_on, 1);
    let fifteenth =
        Date::from_calendar_date(next.year(), next.month(), 15).expect("le 15 existe chaque mois");
    next_french_business_day_on_or_after(fifteenth)
}

/// Seuil de dispense des acomptes de TVA du régime simplifié : aucun acompte n'est dû si la TVA
/// due au titre de l'exercice précédent est inférieure à 1 000 € (art. 287, 3 du CGI).
pub const VAT_INSTALMENT_DISPENSATION: Money = Money::from_cents(100_000);

/// Un exercice relève-t-il encore du régime simplifié ? Oui s'il s'ouvre avant le
/// [`SIMPLIFIED_REGIME_REPEAL`] — ce qui inclut l'exercice décalé ouvert en 2026 et clos en 2027.
#[must_use]
pub fn simplified_regime_applies_to(exercise: FiscalYear) -> bool {
    exercise.start() < SIMPLIFIED_REGIME_REPEAL
}

/// Les deux acomptes semestriels du régime simplifié (formulaire 3514).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VatInstalment {
    July,
    December,
}

impl VatInstalment {
    pub const ALL: [Self; 2] = [Self::July, Self::December];

    /// L'acompte payable au cours de ce mois calendaire (`1..=12`), s'il y en a un.
    #[must_use]
    pub const fn from_month(month: u8) -> Option<Self> {
        match month {
            7 => Some(Self::July),
            12 => Some(Self::December),
            _ => None,
        }
    }

    #[must_use]
    pub const fn month(self) -> u8 {
        match self {
            Self::July => 7,
            Self::December => 12,
        }
    }

    /// Part de la TVA due au titre de l'exercice précédent : 55 % en juillet, 40 % en décembre
    /// (art. 287, 3 du CGI), en dix-millièmes.
    #[must_use]
    pub const fn rate_bps(self) -> u32 {
        match self {
            Self::July => 5_500,
            Self::December => 4_000,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::July => "juillet",
            Self::December => "décembre",
        }
    }
}

/// Montant d'un acompte semestriel à partir de la TVA due au titre de l'exercice précédent —
/// `None` si l'entreprise en est dispensée (base inférieure à 1 000 €, crédit de TVA compris).
///
/// La base légale est la TVA due *avant déduction de la TVA sur immobilisations* ; le domaine ne
/// distingue pas les immobilisations des autres dépenses, la base retenue est donc la TVA nette
/// de l'exercice — indicatif, comme tout ce module.
#[must_use]
pub fn vat_instalment_amount(
    instalment: VatInstalment,
    previous_exercise_vat: Money,
) -> Option<Money> {
    if previous_exercise_vat < VAT_INSTALMENT_DISPENSATION {
        None
    } else {
        Some(previous_exercise_vat.apply_rate_bps(instalment.rate_bps()))
    }
}

/// Le deuxième jour ouvré *strictement* après `date`.
fn second_business_day_after(date: Date) -> Date {
    let mut candidate = date;
    let mut seen = 0;
    loop {
        candidate = candidate
            .next_day()
            .expect("aucune date du calendrier fiscal n'approche la borne de `time::Date`");
        if is_french_business_day(candidate) {
            seen += 1;
            if seen == 2 {
                return candidate;
            }
        }
    }
}

/// Date limite de dépôt de la CA12 d'un exercice : le deuxième jour ouvré suivant le 1er mai de
/// l'année qui suit une clôture au 31 décembre ; pour un exercice décalé (CA12 E), dans les trois
/// mois suivant la clôture — le même quantième trois mois plus tard, ou le dernier jour du
/// troisième mois quand la clôture tombe elle-même un dernier jour de mois (un 28 février donne
/// le 31 mai, pas le 28), reporté au jour ouvré suivant s'il tombe un samedi, un dimanche ou un
/// jour férié.
///
/// # Panics
///
/// Ne panique jamais en pratique : les dates construites sont le 1er mai d'une année et un
/// jour rogné dans les bornes réelles de son mois, tous deux valides ; seul un exercice clos aux
/// confins de `time::Date` (année 9999) n'aurait pas de lendemain.
#[must_use]
pub fn ca12_due_on(exercise: FiscalYear) -> Date {
    let end = exercise.end();
    if end.month() == time::Month::December && end.day() == 31 {
        let may_first = nth_of_month(
            Month::new(end.year() + 1, 5).expect("mai est un mois valide"),
            1,
        );
        second_business_day_after(may_first)
    } else {
        let end_month =
            Month::new(end.year(), u8::from(end.month())).expect("mois de clôture valide");
        let closes_on_month_end = end == end_month.last_day();
        let mut nominal = add_months(end, 3);
        if closes_on_month_end {
            let target = Month::new(nominal.year(), u8::from(nominal.month()))
                .expect("un mois obtenu par `add_months` est valide");
            nominal = target.last_day();
        }
        next_french_business_day_on_or_after(nominal)
    }
}

/// Ce qu'un redevable déclare en matière de TVA pour un exercice donné — le régime déclaré, vu à
/// travers la suppression du réel simplifié.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VatFilingScheme {
    /// Une CA3 par mois.
    Ca3Monthly,
    /// Une CA3 par trimestre civil — réel normal trimestriel, ou ancien réel simplifié pour un
    /// exercice ouvert à compter du [`SIMPLIFIED_REGIME_REPEAL`].
    Ca3Quarterly,
    /// Réel simplifié encore en vigueur : acomptes 3514 en juillet et décembre, CA12 annuelle.
    Simplified,
    /// Franchise en base : aucune déclaration périodique de TVA.
    NoFiling,
}

impl VatFilingScheme {
    /// Le schéma déclaratif d'un exercice. Un régime non renseigné est supposé mensuel (jamais en
    /// retard) ; le réel simplifié ne vaut que pour un exercice ouvert avant sa suppression.
    #[must_use]
    pub fn for_exercise(regime: Option<VatRegime>, exercise: FiscalYear) -> Self {
        match regime {
            None | Some(VatRegime::RealNormalMonthly) => Self::Ca3Monthly,
            Some(VatRegime::RealNormalQuarterly) => Self::Ca3Quarterly,
            Some(VatRegime::RealSimplified) => {
                if simplified_regime_applies_to(exercise) {
                    Self::Simplified
                } else {
                    Self::Ca3Quarterly
                }
            }
            Some(VatRegime::Franchise) => Self::NoFiling,
        }
    }

    #[must_use]
    pub const fn ca3_periodicity(self) -> Option<Ca3Periodicity> {
        match self {
            Self::Ca3Monthly => Some(Ca3Periodicity::Monthly),
            Self::Ca3Quarterly => Some(Ca3Periodicity::Quarterly),
            Self::Simplified | Self::NoFiling => None,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ca3Monthly => "ca3_monthly",
            Self::Ca3Quarterly => "ca3_quarterly",
            Self::Simplified => "simplified",
            Self::NoFiling => "none",
        }
    }
}

const REPEAL_NOTE: &str = "régime simplifié supprimé pour les exercices ouverts à compter du \
                           1er janvier 2027 (art. 38, loi de finances pour 2025) : CA3 \
                           trimestrielle au-delà";

/// Règle de télédéclaration de TVA dérivée du profil d'entreprise — ce que `company show`
/// affiche à côté du régime déclaré, pour que l'utilisateur voie la grille appliquée sans
/// attendre l'échéance au calendrier.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct VatFilingSummary {
    /// Le régime tel que déclaré dans le profil (`None` : non renseigné, mensuel supposé).
    pub regime: Option<VatRegime>,
    /// Le schéma déclaratif de l'exercice en cours à la date d'observation.
    pub scheme: VatFilingScheme,
    /// La cellule de la grille officielle (zone, catégorie, SIREN, jour du mois) — elle vaut pour
    /// la CA3 comme pour les acomptes 3514, déposés aux mêmes dates limites.
    pub rule: Ca3FilingRule,
    /// Phrase lisible reprenant le tout.
    pub note: String,
}

/// Dérive la règle de télédéclaration de TVA du profil, pour l'exercice en cours à `today`.
#[must_use]
pub fn vat_filing_summary(profile: &CompanyProfile, today: Date) -> VatFilingSummary {
    use std::fmt::Write as _;
    let fye = profile.fiscal_year_end.unwrap_or(FiscalYearEnd::CALENDAR);
    let current = fye.current(today);
    let rule = Ca3FilingRule::derive(
        &profile.legal_form,
        &profile.name,
        profile.siren,
        &profile.address.postal_code,
    );
    let scheme = VatFilingScheme::for_exercise(profile.vat_regime, current);
    let grid = format!(
        "le {} du mois ({}, {}, SIREN {:02}…), reporté au jour ouvré suivant si samedi, \
         dimanche ou férié",
        rule.day,
        rule.category.as_str(),
        rule.zone.as_str(),
        rule.siren_leading_pair
    );
    // Écrire dans une `String` est infaillible : le `Result` de `write!` est ignoré à dessein.
    let mut note = match scheme {
        VatFilingScheme::Ca3Monthly => format!("CA3 mensuelle, {grid}"),
        VatFilingScheme::Ca3Quarterly => {
            format!("CA3 trimestrielle (janvier, avril, juillet, octobre), {grid}")
        }
        VatFilingScheme::Simplified => {
            let ca12 = if fye.month() == 12 {
                "le 2e jour ouvré suivant le 1er mai"
            } else {
                "dans les 3 mois suivant la clôture (CA12 E)"
            };
            format!(
                "réel simplifié : acomptes 3514 en juillet (55 %) et décembre (40 %) de la TVA de \
                 l'exercice précédent, {grid} ; CA12 {ca12}"
            )
        }
        VatFilingScheme::NoFiling => "franchise en base : aucune déclaration de TVA".to_string(),
    };
    if profile.vat_regime.is_none() {
        note.push_str(" ; régime de TVA non renseigné, mensuel supposé");
    }
    if profile.vat_regime == Some(VatRegime::RealSimplified) {
        let _ = write!(note, " ; {REPEAL_NOTE}");
    }
    VatFilingSummary {
        regime: profile.vat_regime,
        scheme,
        rule,
        note,
    }
}

/// Le profil d'entreprise et la règle de TVA qui en découle, tels que `company show` les affiche
/// (CLI, MCP et ressource `freeflow://company` partagent cette vue). Les champs du profil restent
/// au premier niveau du JSON : le contrat antérieur n'est qu'enrichi d'une clé `vat_filing`.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct CompanyProfileWithVatFiling {
    #[serde(flatten)]
    pub profile: CompanyProfile,
    pub vat_filing: VatFilingSummary,
}

/// Lit le profil et y adjoint la règle de télédéclaration dérivée pour l'exercice en cours à
/// `today` ; `None` si aucun profil n'est défini.
///
/// # Errors
///
/// Erreur de lecture SQLite.
pub fn company_profile_with_vat_filing(
    conn: &Connection,
    today: Date,
) -> Result<Option<CompanyProfileWithVatFiling>, AppError> {
    Ok(
        company_profile(conn)?.map(|profile| CompanyProfileWithVatFiling {
            vat_filing: vat_filing_summary(&profile, today),
            profile,
        }),
    )
}

/// Note lisible d'une échéance CA3 : période déclarée, règle de date appliquée et report éventuel.
fn ca3_note(
    filing: &Ca3Filing,
    periodicity: Ca3Periodicity,
    rule: Option<Ca3FilingRule>,
    regime_unknown: bool,
) -> String {
    use std::fmt::Write as _;
    let mut note = match periodicity {
        Ca3Periodicity::Monthly => format!("CA3 mensuelle, TVA de {}", filing.period_start),
        Ca3Periodicity::Quarterly => format!(
            "CA3 trimestrielle, TVA de {} à {}",
            filing.period_start, filing.period_end
        ),
    };
    // Écrire dans une `String` est infaillible : le `Result` de `write!` est ignoré à dessein.
    match rule {
        Some(rule) => {
            let _ = write!(
                note,
                " ; le {} du mois ({}, {}, SIREN {:02}…)",
                rule.day,
                rule.category.as_str(),
                rule.zone.as_str(),
                rule.siren_leading_pair
            );
        }
        None => note.push_str(
            " ; le 15 par défaut, profil d'entreprise non renseigné (borne basse de la grille \
             officielle)",
        ),
    }
    if filing.due_on != filing.nominal_due_on {
        let _ = write!(
            note,
            ", reportée du {} au jour ouvré suivant",
            filing.nominal_due_on
        );
    }
    if regime_unknown {
        note.push_str(" ; régime de TVA non renseigné, mensuel supposé");
    }
    note
}

const IS_ACOMPTE_MONTHS: [u8; 4] = [3, 6, 9, 12];

fn next_is_acompte(today: Date) -> Date {
    let year = today.year();
    for month in IS_ACOMPTE_MONTHS {
        let candidate = nth_of_month(Month::new(year, month).unwrap(), 15);
        if today <= candidate {
            return candidate;
        }
    }
    nth_of_month(Month::new(year + 1, 3).unwrap(), 15)
}

fn next_cfe(today: Date) -> Date {
    let candidate = nth_of_month(Month::new(today.year(), 12).unwrap(), 15);
    if today <= candidate {
        candidate
    } else {
        nth_of_month(Month::new(today.year() + 1, 12).unwrap(), 15)
    }
}

fn bare(kind: FiscalDeadlineKind, due_on: Date) -> FiscalDeadline {
    FiscalDeadline {
        kind,
        due_on,
        amount: None,
        note: None,
    }
}

/// Les trois prochaines échéances calendaires de base (CA3, acompte d'IS, CFE), triées par date —
/// **fonction pure**, année civile, sans montants. Conservée pour le tableau de bord et les
/// alertes qui n'ouvrent pas la base. Sans profil, la CA3 est supposée mensuelle et datée au 15
/// (borne basse de la grille officielle), reportée au jour ouvré suivant.
#[must_use]
pub fn upcoming_deadlines(today: Date) -> Vec<FiscalDeadline> {
    let ca3 = next_ca3_filing(today, Ca3Periodicity::Monthly, Ca3FilingRule::EARLIEST_DAY);
    let mut deadlines = vec![
        bare(FiscalDeadlineKind::Ca3, ca3.due_on),
        bare(FiscalDeadlineKind::IsAcompte, next_is_acompte(today)),
        bare(FiscalDeadlineKind::Cfe, next_cfe(today)),
    ];
    deadlines.sort_by_key(|d| d.due_on);
    deadlines
}

/// Échéances (pures) dans les `within_days` prochains jours — la CLI/GUI l'utilise pour alerter
/// avant J plutôt qu'après.
#[must_use]
pub fn deadlines_due_within(today: Date, within_days: i64) -> Vec<FiscalDeadline> {
    upcoming_deadlines(today)
        .into_iter()
        .filter(|d| (d.due_on - today).whole_days() <= within_days)
        .collect()
}

/// Solde d'IS : pour un exercice clos au 31/12, échéance fixée au 15 mai N+1 ; pour un exercice
/// décalé, le 15 du 4e mois suivant la clôture (règle générale approchée). Partagée par le
/// calendrier et le parcours de clôture ([`crate::closing`]).
///
/// # Panics
///
/// Ne panique jamais : les mois calculés restent dans `1..=12` et le 15 existe dans tous les
/// mois.
#[must_use]
pub fn is_solde_due_on(fye: FiscalYearEnd, end: Date) -> Date {
    if fye.month() == 12 {
        nth_of_month(Month::new(end.year() + 1, 5).unwrap(), 15)
    } else {
        let mut month = Month::new(end.year(), end.month().into()).unwrap();
        for _ in 0..4 {
            month = month.succ();
        }
        nth_of_month(month, 15)
    }
}

/// Liasse fiscale (2065 + tableaux 2033) : pour un exercice civil, le deuxième jour ouvré
/// suivant le 1er mai est approché au 15 mai N+1 ; pour un exercice décalé, dans les trois mois
/// suivant la clôture. Indicatif — la date exacte est fixée chaque année par l'administration.
///
/// # Panics
///
/// Ne panique jamais : les mois calculés restent dans `1..=12` et les jours sont rognés au
/// dernier jour réel du mois.
#[must_use]
pub fn liasse_due_on(fye: FiscalYearEnd, end: Date) -> Date {
    if fye.month() == 12 {
        nth_of_month(Month::new(end.year() + 1, 5).unwrap(), 15)
    } else {
        add_months(end, 3)
    }
}

/// Assemblée d'approbation des comptes : dans les six mois de la clôture (art. L225-100 du
/// Code de commerce, applicable à la SAS par renvoi ; décision de l'associé unique en SASU).
#[must_use]
pub fn approval_meeting_due_on(end: Date) -> Date {
    add_months(end, 6)
}

/// Dépôt des comptes annuels au greffe : dans le mois suivant l'approbation (art. L232-23 du
/// Code de commerce ; deux mois par voie électronique — la borne la plus courte est retenue).
#[must_use]
pub fn accounts_filing_due_on(approved_on: Date) -> Date {
    add_months(approved_on, 1)
}

/// Ajoute `months` mois à une date, en ramenant le jour au dernier jour réel du mois cible.
pub(crate) fn add_months(date: Date, months: u32) -> Date {
    let mut month = Month::new(date.year(), u8::from(date.month())).unwrap();
    for _ in 0..months {
        month = month.succ();
    }
    let day = date.day().min(month.last_day().day());
    nth_of_month(month, day)
}

/// Fenêtre d'observation des échéances du réel simplifié (paramètres de
/// [`push_simplified_vat_deadlines`], regroupés pour rester lisibles).
#[derive(Debug, Clone, Copy)]
struct SimplifiedVatWindow {
    today: Date,
    horizon: Date,
    fye: FiscalYearEnd,
    previous: FiscalYear,
    current: FiscalYear,
    /// Jour de la grille officielle (celui de la CA3, qui vaut aussi pour le 3514).
    day: u8,
}

/// Acomptes 3514 (juillet, décembre) et CA12 des exercices encore placés sous le régime simplifié,
/// dans `[today, horizon]`. La base d'un acompte est la TVA nette de l'exercice qui précède celui
/// au cours duquel il est payé ; la CA12 d'un exercice régularise sa TVA nette des deux acomptes
/// versés à son titre.
#[allow(clippy::too_many_lines)]
fn push_simplified_vat_deadlines(
    conn: &Connection,
    deadlines: &mut Vec<FiscalDeadline>,
    window: SimplifiedVatWindow,
) -> Result<(), AppError> {
    let SimplifiedVatWindow {
        today,
        horizon,
        fye,
        previous,
        current,
        day,
    } = window;
    let day = day.clamp(Ca3FilingRule::EARLIEST_DAY, 24);
    // Lot 40 : un exercice antérieur au bilan d'ouverture n'est pas suivi ici — sa TVA due est
    // celle reprise au bilan, ou inconnue.
    let reprise = opening_balance(conn)?;
    let vat_of = |exercise: FiscalYear| -> Result<Option<Money>, AppError> {
        if let Some(o) = &reprise
            && exercise.start() < o.balance.opens_on
        {
            return Ok(o.prior_vat_due);
        }
        Ok(Some(
            vat_due_for_period(conn, exercise.start(), exercise.end())?.due,
        ))
    };

    // --- Acomptes : chaque juillet et décembre de l'horizon tombant dans un exercice RSI. ---
    let mut month = Month::new(today.year(), u8::from(today.month())).expect("mois courant valide");
    while month.first_day() <= horizon {
        if let Some(instalment) = VatInstalment::from_month(month.month()) {
            let nominal = nth_of_month(month, day);
            let due_on = next_french_business_day_on_or_after(nominal);
            let exercise = fye.containing(nominal);
            if due_on >= today && due_on <= horizon && simplified_regime_applies_to(exercise) {
                let base_exercise = fye.previous(exercise);
                let known_base = vat_of(base_exercise)?;
                let base = known_base.unwrap_or(Money::ZERO);
                let amount = known_base.and_then(|b| vat_instalment_amount(instalment, b));
                // Écrire dans une `String` est infaillible : le `Result` de `write!` est ignoré.
                let mut note = match (known_base, amount) {
                    (None, _) => format!(
                        "acompte de TVA de {} (3514) : base inconnue — l'exercice {} – {} n'est \
                         pas suivi ici et le bilan d'ouverture ne reprend pas sa TVA due (year \
                         opening set --prior-vat) ; vérifiez sur votre dernière CA12",
                        instalment.as_str(),
                        base_exercise.start(),
                        base_exercise.end(),
                    ),
                    (Some(_), Some(_)) => format!(
                        "acompte de TVA de {} (3514) : {} % de la TVA due au titre de l'exercice \
                         {} – {} ({base}, hors TVA sur immobilisations non distinguée) ; le {day} \
                         du mois (grille CA3)",
                        instalment.as_str(),
                        instalment.rate_bps() / 100,
                        base_exercise.start(),
                        base_exercise.end(),
                    ),
                    (Some(_), None) => format!(
                        "acompte de TVA de {} (3514) : dispense, TVA due au titre de l'exercice \
                         {} – {} ({base}) inférieure à 1 000 €",
                        instalment.as_str(),
                        base_exercise.start(),
                        base_exercise.end(),
                    ),
                };
                if due_on != nominal {
                    let _ = write!(note, ", reporté du {nominal} au jour ouvré suivant");
                }
                deadlines.push(FiscalDeadline {
                    kind: FiscalDeadlineKind::VatInstalment,
                    due_on,
                    amount,
                    note: Some(note),
                });
            }
        }
        month = month.succ();
    }

    // --- CA12 : régularisation annuelle de l'exercice clos et de l'exercice en cours (l'échéance
    // d'un exercice ultérieur tombe toujours au-delà d'un horizon de 12 mois). ---
    for exercise in [previous, current] {
        if !simplified_regime_applies_to(exercise) {
            continue;
        }
        let due_on = ca12_due_on(exercise);
        if due_on < today || due_on > horizon {
            continue;
        }
        // Lot 41 : le crédit de TVA repris au bilan d'ouverture (445670) vient en déduction de
        // la première CA12 — sans lui, la régularisation surestimait la TVA à payer.
        let opening_credit = reprise
            .as_ref()
            .filter(|o| o.balance.opens_on == exercise.start())
            .map_or(Money::ZERO, |o| {
                o.balance
                    .lines
                    .iter()
                    .filter(|l| l.account.starts_with("44567"))
                    .map(crate::domain::OpeningBalanceLine::signed)
                    .sum()
            });
        let vat = vat_of(exercise)?.map(|v| v - opening_credit);
        let base = vat_of(fye.previous(exercise))?;
        let instalments: Option<Money> = base.map(|b| {
            VatInstalment::ALL
                .iter()
                .filter_map(|i| vat_instalment_amount(*i, b))
                .sum()
        });
        let amount = match (vat, instalments) {
            (Some(v), Some(i)) => Some(v - i),
            _ => None,
        };
        let unknown = |m: Option<Money>| {
            m.map_or_else(
                || "inconnue : exercice non suivi ici, voir le bilan d'ouverture".to_string(),
                |m| m.to_string(),
            )
        };
        let mut note = format!(
            "CA12 : TVA due au titre de l'exercice {} – {} ({}{}) − acomptes de juillet et \
             décembre ({}) ; {}",
            exercise.start(),
            exercise.end(),
            unknown(vat),
            if opening_credit.is_zero() {
                String::new()
            } else {
                format!(", crédit de TVA repris à l'ouverture déduit : {opening_credit}")
            },
            unknown(instalments),
            if exercise.end().month() == time::Month::December && exercise.end().day() == 31 {
                "le 2e jour ouvré suivant le 1er mai"
            } else {
                "dans les 3 mois suivant la clôture (CA12 E)"
            },
        );
        if exercise.end() >= today {
            note.push_str(" ; exercice en cours, montant partiel");
        }
        let successor = fye.containing(add_months(exercise.end(), 1));
        if !simplified_regime_applies_to(successor) {
            let _ = write!(note, " ; dernière CA12 — {REPEAL_NOTE}");
        }
        if amount.is_some_and(|a| a < -CA12_REFUND_THRESHOLD) {
            let _ = write!(
                note,
                " ; crédit supérieur à 150 € : remboursement possible sur demande (formulaire \
                 3519) plutôt que report"
            );
        }
        deadlines.push(FiscalDeadline {
            kind: FiscalDeadlineKind::Ca12,
            due_on,
            amount,
            note: Some(note),
        });
    }
    Ok(())
}

/// Calendrier fiscal et social chiffré sur les 12 prochains mois à partir de `today`, dérivé de la
/// date de clôture d'exercice du profil (année civile à défaut) et enrichi des montants
/// calculables. Chaque échéance retournée a `due_on >= today`.
///
/// # Errors
///
/// Erreur de lecture SQLite, ou de calcul du résultat/de la TVA.
///
/// # Panics
///
/// Ne panique jamais : tous les `unwrap`/`expect` internes portent sur des mois `1..=12` et des
/// jours (15, ou rognés au dernier jour réel du mois) valides par construction.
#[allow(clippy::too_many_lines)]
pub fn fiscal_calendar(conn: &Connection, today: Date) -> Result<Vec<FiscalDeadline>, AppError> {
    let profile = company_profile(conn)?;
    let fye = profile
        .as_ref()
        .and_then(|p| p.fiscal_year_end)
        .unwrap_or(FiscalYearEnd::CALENDAR);
    let vat_regime = profile.as_ref().and_then(|p| p.vat_regime);

    let current = fye.current(today);
    let previous = fye.previous(current);
    let horizon = add_months(today, 12);

    let mut deadlines: Vec<FiscalDeadline> = Vec::new();

    // --- TVA : jour de la grille officielle dérivé du profil (zone du siège, forme juridique,
    // deux premiers chiffres du SIREN), report au jour ouvré — la même cellule vaut pour la CA3
    // et pour les acomptes 3514 du réel simplifié. ---
    let rule = profile
        .as_ref()
        .map(|p| Ca3FilingRule::derive(&p.legal_form, &p.name, p.siren, &p.address.postal_code));
    let day = rule.map_or(Ca3FilingRule::EARLIEST_DAY, |r| r.day);

    // Réel simplifié : acomptes et CA12 des exercices encore sous ce régime.
    if vat_regime == Some(VatRegime::RealSimplified) {
        push_simplified_vat_deadlines(
            conn,
            &mut deadlines,
            SimplifiedVatWindow {
                today,
                horizon,
                fye,
                previous,
                current,
                day,
            },
        )?;
    }

    // CA3 : dès maintenant au réel normal ; pour un ancien réel simplifié, seulement à partir du
    // premier exercice ouvert après la suppression du régime (ses mois antérieurs relèvent de la
    // CA12), s'il commence dans l'horizon.
    let scheme = VatFilingScheme::for_exercise(vat_regime, current);
    let ca3 = match scheme.ca3_periodicity() {
        // Lot 41 : quand l'exercice précédent était encore au réel simplifié (sa CA12 E couvre
        // tout jusqu'à sa clôture), la première CA3 ne déclare rien d'antérieur au début de
        // l'exercice courant — sinon le trimestre qui chevauche la clôture décalée serait
        // déclaré deux fois (constat de l'audit sur une clôture au 30/09).
        Some(periodicity) => Some((
            periodicity,
            simplified_regime_applies_to(previous).then(|| {
                let start = current.start();
                Month::new(start.year(), u8::from(start.month()))
                    .expect("un début d'exercice a un mois valide")
            }),
        )),
        None if scheme == VatFilingScheme::Simplified => {
            let next = fye.containing(add_months(current.end(), 1));
            (!simplified_regime_applies_to(next)).then(|| {
                let start = next.start();
                let earliest = Month::new(start.year(), u8::from(start.month()))
                    .expect("un début d'exercice a un mois valide");
                (Ca3Periodicity::Quarterly, Some(earliest))
            })
        }
        None => None,
    };
    if let Some((periodicity, earliest_period)) = ca3 {
        let filing = next_ca3_filing_from(today, periodicity, day, earliest_period);
        if filing.due_on <= horizon {
            let vat = vat_due_for_period(
                conn,
                filing.period_start.first_day(),
                filing.period_end.last_day(),
            )?;
            let mut note = ca3_note(&filing, periodicity, rule, vat_regime.is_none());
            if vat_regime == Some(VatRegime::RealSimplified) {
                note.push_str(" ; ");
                note.push_str(REPEAL_NOTE);
            }
            deadlines.push(FiscalDeadline {
                kind: FiscalDeadlineKind::Ca3,
                due_on: filing.due_on,
                amount: Some(vat.due),
                note: Some(note),
            });
        }
    }

    // --- IS : solde de l'exercice clos + acomptes de l'exercice en cours, montants tirés du
    // résultat de l'exercice précédent (base des acomptes). ---
    // Lot 40 : si l'exercice précédent n'est pas suivi ici (il précède le bilan d'ouverture),
    // sa base d'acomptes est celle reprise au bilan — ou inconnue, et on le dit.
    let reprise = opening_balance(conn)?.filter(|o| previous.start() < o.balance.opens_on);
    let previous_result = match &reprise {
        Some(_) => None,
        None => profile
            .as_ref()
            .map(|p| compute_result(conn, previous, p))
            .transpose()?,
    };
    let reference_unknown = reprise
        .as_ref()
        .is_some_and(|o| o.prior_corporate_tax.is_none());
    let previous_is = match &reprise {
        Some(o) => o.prior_corporate_tax,
        None => previous_result.map(|r| r.corporate_tax),
    };

    let solde_due = is_solde_due_on(fye, previous.end());
    if solde_due >= today && solde_due <= horizon {
        deadlines.push(FiscalDeadline {
            kind: FiscalDeadlineKind::IsSolde,
            due_on: solde_due,
            amount: previous_is,
            note: Some("solde d'IS de l'exercice clos (indicatif)".to_string()),
        });
    }

    let acompte_due = next_is_acompte(today);
    if acompte_due <= horizon {
        // Un acompte vaut le quart de l'IS de l'exercice de référence, sauf dispense (< 3 000 €).
        let amount = previous_is.and_then(|is| {
            if is < IS_ACOMPTE_DISPENSATION {
                None
            } else {
                Some(
                    is.split_equally(4)
                        .into_iter()
                        .next()
                        .unwrap_or(Money::ZERO),
                )
            }
        });
        let note = if reference_unknown {
            "IS de référence inconnu : l'exercice précédent n'est pas suivi ici et le bilan \
             d'ouverture ne le reprend pas (year opening set --prior-is) — vérifiez sur votre \
             dernier relevé 2572"
                .to_string()
        } else if previous_is.is_some_and(|is| is < IS_ACOMPTE_DISPENSATION) {
            "dispense d'acompte (IS de référence < 3 000 €)".to_string()
        } else {
            "acompte = 1/4 de l'IS de référence (indicatif)".to_string()
        };
        deadlines.push(FiscalDeadline {
            kind: FiscalDeadlineKind::IsAcompte,
            due_on: acompte_due,
            amount,
            note: Some(note),
        });
    }

    // --- CFE : date fixe, montant non modélisé. ---
    let cfe = next_cfe(today);
    if cfe <= horizon {
        deadlines.push(FiscalDeadline {
            kind: FiscalDeadlineKind::Cfe,
            due_on: cfe,
            amount: None,
            note: Some("montant établi par l'avis de CFE, non calculé ici".to_string()),
        });
    }

    // --- Liasse fiscale : pour un exercice civil, ~mi-mai N+1 (approché au 15). ---
    let liasse_due = liasse_due_on(fye, previous.end());
    if liasse_due >= today && liasse_due <= horizon {
        deadlines.push(FiscalDeadline {
            kind: FiscalDeadlineKind::Liasse,
            due_on: liasse_due,
            amount: None,
            note: Some("2065 + tableaux 2033, télétransmission EDI-TDFC (indicatif)".to_string()),
        });
    }

    // --- AG d'approbation (clôture + 6 mois) et dépôt au greffe (AG + 1 mois). ---
    let approval_due = approval_meeting_due_on(previous.end());
    if approval_due >= today && approval_due <= horizon {
        deadlines.push(bare(FiscalDeadlineKind::ApprovalMeeting, approval_due));
    }
    let filing_due = accounts_filing_due_on(approval_due);
    if filing_due >= today && filing_due <= horizon {
        deadlines.push(bare(FiscalDeadlineKind::AccountsFiling, filing_due));
    }

    // --- DSN mensuelle : uniquement si le président est rémunéré ; montant = coût employeur du
    // mois (brut + cotisations patronales estimées). ---
    let director = profile
        .as_ref()
        .and_then(|p| p.director_monthly_gross.map(|g| (p, g)));
    if let Some((profile, gross)) = director {
        // Lot 41 : le montant de la DSN est ce que la société *verse aux organismes*, les
        // cotisations estimées (coût − brut), pas le coût total incluant le salaire.
        let employer = profile
            .director_charge_ratio_bps
            .map_or(Money::ZERO, |bps| gross.apply_rate_bps(bps));
        let monthly_cost = employer;
        // La DSN d'un mois se dépose le 5 ou le 15 du mois suivant ; on retient le 15 (indicatif)
        // du mois courant si à venir, sinon du mois suivant.
        let this_month = Month::new(today.year(), u8::from(today.month())).unwrap();
        let candidate = nth_of_month(this_month, 15);
        let dsn_due = if today <= candidate {
            candidate
        } else {
            nth_of_month(this_month.succ(), 15)
        };
        deadlines.push(FiscalDeadline {
            kind: FiscalDeadlineKind::Dsn,
            due_on: dsn_due,
            amount: Some(monthly_cost),
            note: Some(
                "cotisations sociales mensuelles du dirigeant (estimation : charges patronales et \
                 salariales selon le ratio du profil, hors salaire net)"
                    .to_string(),
            ),
        });
    }

    // --- DAS2 (lot 41) : les honoraires de chaque année civile dont l'échéance tombe dans
    // l'horizon, par bénéficiaire, au-delà du seuil. ---
    for year in [today.year() - 1, today.year()] {
        let due_on = das2_due_on(fye, year);
        if due_on < today || due_on > horizon {
            continue;
        }
        let fees = crate::expenses::fees_by_supplier(conn, year)?;
        let total: Money = fees.iter().map(|(_, m)| *m).sum();
        if total.is_zero() {
            continue;
        }
        let above: Vec<String> = fees
            .iter()
            .filter(|(s, m)| s.is_some() && *m >= DAS2_THRESHOLD)
            .map(|(s, m)| format!("{} ({m})", s.as_deref().unwrap_or_default()))
            .collect();
        let unnamed: Money = fees
            .iter()
            .filter(|(s, _)| s.is_none())
            .map(|(_, m)| *m)
            .sum();
        let mut note = if above.is_empty() {
            format!(
                "DAS2 {year} : aucun bénéficiaire d'honoraires au-delà de {DAS2_THRESHOLD} sur \
                 l'année civile (cumul par bénéficiaire, art. 240 CGI) — rien à déclarer"
            )
        } else {
            format!(
                "DAS2 {year} : honoraires à déclarer par bénéficiaire (plus de {DAS2_THRESHOLD} \
                 sur l'année civile, art. 240 CGI) : {}",
                above.join(", ")
            )
        };
        if !unnamed.is_zero() {
            let _ = write!(
                note,
                " ; {unnamed} d'honoraires sans bénéficiaire renseigné — nommez-le sur chaque \
                 dépense (expense edit --supplier) pour que le cumul soit juste"
            );
        }
        deadlines.push(FiscalDeadline {
            kind: FiscalDeadlineKind::Das2,
            due_on,
            amount: Some(if above.is_empty() {
                Money::ZERO
            } else {
                fees.iter()
                    .filter(|(s, m)| s.is_some() && *m >= DAS2_THRESHOLD)
                    .map(|(_, m)| *m)
                    .sum()
            }),
            note: Some(note),
        });
    }

    // --- 2777 (lot 41) : dès qu'une affectation approuvée distribue des dividendes, le
    // prélèvement retenu à la source se déclare et se paie avant le 15 du mois suivant la mise
    // en paiement (réputée à la date d'approbation). ---
    for record in crate::fiscal_year::list_fiscal_years(conn)? {
        let (Some(approved_on), false) = (record.approved_on, record.dividends.is_zero()) else {
            continue;
        };
        let due_on = dividends_2777_due_on(approved_on);
        if due_on < today || due_on > horizon {
            continue;
        }
        let social_bps = dividend_social_charges_bps(approved_on);
        let income_tax = record.dividends.apply_rate_bps(DIVIDEND_INCOME_TAX_BPS);
        let social = record.dividends.apply_rate_bps(social_bps);
        deadlines.push(FiscalDeadline {
            kind: FiscalDeadlineKind::Dividends2777,
            due_on,
            amount: Some(income_tax + social),
            note: Some(format!(
                "dividendes de {} mis en paiement le {} (exercice clos le {}) : la société \
                 retient et verse le prélèvement forfaitaire non libératoire de 12,8 % \
                 ({income_tax}) et les prélèvements sociaux de {},{} % ({social}) — formulaire \
                 2777, avant le 15 du mois suivant",
                record.dividends,
                approved_on,
                record.ends_on,
                social_bps / 100,
                social_bps % 100 / 10,
            )),
        });
    }

    deadlines.retain(|d| d.due_on >= today);
    deadlines.sort_by(|a, b| {
        a.due_on
            .cmp(&b.due_on)
            .then(a.kind.as_str().cmp(b.kind.as_str()))
    });
    Ok(deadlines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::Month as TimeMonth;

    fn date(year: i32, month: TimeMonth, day: u8) -> Date {
        Date::from_calendar_date(year, month, day).unwrap()
    }

    fn month(year: i32, month: u8) -> Month {
        Month::new(year, month).unwrap()
    }

    #[test]
    fn ca3_rolls_over_to_the_following_month_once_the_15th_has_passed() {
        let sept = next_ca3_filing(
            date(2026, TimeMonth::September, 1),
            Ca3Periodicity::Monthly,
            15,
        );
        assert_eq!(
            sept.due_on,
            date(2026, TimeMonth::September, 15),
            "avant le 15, l'échéance du mois courant (déclarant août) est encore à venir"
        );
        assert_eq!(
            (sept.period_start, sept.period_end),
            (month(2026, 8), month(2026, 8))
        );
        let oct = next_ca3_filing(
            date(2026, TimeMonth::September, 16),
            Ca3Periodicity::Monthly,
            15,
        );
        assert_eq!(
            oct.due_on,
            date(2026, TimeMonth::October, 15),
            "après le 15, on saute à l'échéance du mois suivant"
        );
        assert_eq!(
            (oct.period_start, oct.period_end),
            (month(2026, 9), month(2026, 9))
        );
    }

    #[test]
    fn a_ca3_falling_on_a_sunday_before_whit_monday_moves_to_the_tuesday() {
        // 24 mai 2026 : dimanche ; lundi 25 mai 2026 : lundi de Pentecôte → mardi 26.
        let filing = next_ca3_filing(date(2026, TimeMonth::May, 1), Ca3Periodicity::Monthly, 24);
        assert_eq!(filing.nominal_due_on, date(2026, TimeMonth::May, 24));
        assert_eq!(filing.due_on, date(2026, TimeMonth::May, 26));
        // Le 25 mai, l'échéance reportée est encore à venir : on ne saute pas au mois suivant.
        let still = next_ca3_filing(date(2026, TimeMonth::May, 25), Ca3Periodicity::Monthly, 24);
        assert_eq!(still.due_on, date(2026, TimeMonth::May, 26));
    }

    #[test]
    fn a_quarterly_ca3_is_filed_the_month_after_the_quarter() {
        let q3 = next_ca3_filing(
            date(2026, TimeMonth::September, 1),
            Ca3Periodicity::Quarterly,
            21,
        );
        assert_eq!(q3.due_on, date(2026, TimeMonth::October, 21));
        assert_eq!(
            (q3.period_start, q3.period_end),
            (month(2026, 7), month(2026, 9))
        );
        let q4 = next_ca3_filing(
            date(2026, TimeMonth::October, 22),
            Ca3Periodicity::Quarterly,
            21,
        );
        assert_eq!(q4.due_on, date(2027, TimeMonth::January, 21));
        assert_eq!(
            (q4.period_start, q4.period_end),
            (month(2026, 10), month(2026, 12))
        );
    }

    #[test]
    fn only_the_real_normal_regimes_file_a_ca3() {
        assert_eq!(
            Ca3Periodicity::from_regime(None),
            Some(Ca3Periodicity::Monthly)
        );
        assert_eq!(
            Ca3Periodicity::from_regime(Some(VatRegime::RealNormalQuarterly)),
            Some(Ca3Periodicity::Quarterly)
        );
        assert_eq!(
            Ca3Periodicity::from_regime(Some(VatRegime::RealSimplified)),
            None
        );
        assert_eq!(
            Ca3Periodicity::from_regime(Some(VatRegime::Franchise)),
            None
        );
    }

    use proptest::prelude::*;

    proptest! {
        #[test]
        fn the_next_ca3_is_a_business_day_on_or_after_today_and_after_its_period(
            days in 0i64..(365 * 20),
            quarterly in any::<bool>(),
            day in 15u8..=24,
        ) {
            let today = date(2020, TimeMonth::January, 1) + time::Duration::days(days);
            let periodicity = if quarterly { Ca3Periodicity::Quarterly } else { Ca3Periodicity::Monthly };
            let filing = next_ca3_filing(today, periodicity, day);
            prop_assert!(filing.due_on >= today);
            prop_assert!(filing.due_on >= filing.nominal_due_on);
            prop_assert!(crate::domain::is_french_business_day(filing.due_on));
            prop_assert_eq!(filing.nominal_due_on.day(), day);
            prop_assert!(filing.period_end.last_day() < filing.nominal_due_on);
            let span = if quarterly { 3 } else { 1 };
            let mut m = filing.period_start;
            for _ in 1..span { m = m.succ(); }
            prop_assert_eq!(m, filing.period_end);
            if quarterly {
                prop_assert!(matches!(filing.period_start.month(), 1 | 4 | 7 | 10));
            }
        }
    }

    #[test]
    fn is_acompte_wraps_around_to_march_of_next_year() {
        assert_eq!(
            next_is_acompte(date(2026, TimeMonth::December, 16)),
            date(2027, TimeMonth::March, 15)
        );
        assert_eq!(
            next_is_acompte(date(2026, TimeMonth::January, 1)),
            date(2026, TimeMonth::March, 15)
        );
    }

    #[test]
    fn cfe_is_once_a_year_on_december_15() {
        assert_eq!(
            next_cfe(date(2026, TimeMonth::June, 1)),
            date(2026, TimeMonth::December, 15)
        );
        assert_eq!(
            next_cfe(date(2026, TimeMonth::December, 16)),
            date(2027, TimeMonth::December, 15)
        );
    }

    #[test]
    fn is_solde_for_a_calendar_year_falls_on_may_15_next_year() {
        let fye = FiscalYearEnd::CALENDAR;
        assert_eq!(
            is_solde_due_on(fye, date(2025, TimeMonth::December, 31)),
            date(2026, TimeMonth::May, 15)
        );
    }

    #[test]
    fn is_solde_for_an_offset_year_is_the_15th_of_the_fourth_month_after_close() {
        let fye = FiscalYearEnd::new(6, 30).unwrap();
        assert_eq!(
            is_solde_due_on(fye, date(2026, TimeMonth::June, 30)),
            date(2026, TimeMonth::October, 15)
        );
    }

    #[test]
    fn upcoming_deadlines_are_sorted_chronologically() {
        let deadlines = upcoming_deadlines(date(2026, TimeMonth::September, 1));
        for pair in deadlines.windows(2) {
            assert!(pair[0].due_on <= pair[1].due_on);
        }
    }

    #[test]
    fn deadlines_due_within_excludes_far_future_ones() {
        let today = date(2026, TimeMonth::September, 1);
        let soon = deadlines_due_within(today, 20);
        let kinds: Vec<_> = soon.iter().map(|d| d.kind).collect();
        assert_eq!(
            kinds,
            vec![FiscalDeadlineKind::Ca3, FiscalDeadlineKind::IsAcompte]
        );
    }

    // --- Intégration : le calendrier chiffre le solde d'IS de l'exercice clos. ---

    use crate::app::{Actor, ExecutionContext, Executor};
    use crate::billing::EmitInvoice;
    use crate::company::SetCompanyProfile;
    use crate::domain::{Address, ClientId, FiscalYearEnd, InvoiceLine, Siren, VatRate, VatRegime};
    use crate::store::{Passphrase, Store};

    fn calendar_profile() -> SetCompanyProfile {
        SetCompanyProfile {
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
            share_capital: Some(Money::from_cents(100_000)),
            rcs_city: Some("Paris".to_string()),
            iban: None,
            fiscal_year_end: Some(FiscalYearEnd::CALENDAR),
            vat_regime: Some(VatRegime::RealNormalMonthly),
            director_monthly_gross: None,
            director_charge_ratio_bps: None,
            president_name: None,
            sole_shareholder_name: None,
            sole_shareholder_address: None,
            share_count: None,
        }
    }

    fn fresh_store(tag: &str) -> (Store, ClientId) {
        let dir = std::env::temp_dir().join(format!(
            "freeflow-{tag}-{}-{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ));
        let store = Store::create(&dir.join("vault.db"), &Passphrase::from("s3cret")).unwrap();
        let client_id = ClientId::new();
        store
            .connection()
            .execute(
                "INSERT INTO clients (id, name, created_at) \
                 VALUES (?1, 'Argon Digital', '2025-01-01T00:00:00Z')",
                [client_id.to_string()],
            )
            .unwrap();
        (store, client_id)
    }

    fn emit(store: &mut Store, client_id: ClientId, ht_cents: i64, quantity: f64, issued_on: Date) {
        Executor::new(store)
            .execute(
                &EmitInvoice {
                    client_id,
                    mission_id: None,
                    lines: vec![InvoiceLine {
                        description: "Prestation".to_string(),
                        quantity,
                        unit_price: Money::from_cents(ht_cents),
                        vat_rate: VatRate::Standard,
                    }],
                    issued_on,
                    payment_terms_days: 30,
                },
                &ExecutionContext::new(Actor::Human, false),
            )
            .unwrap();
    }

    #[test]
    fn the_calendar_prices_the_is_balance_of_the_closed_exercise() {
        let (mut store, client_id) = fresh_store("calendar");
        let human = ExecutionContext::new(Actor::Human, false);
        Executor::new(&mut store)
            .execute(&calendar_profile(), &human)
            .unwrap();
        // Facture de l'exercice CLOS (2025) : 6 175 € HT → IS 15 % = 926,25 € → 926 €.
        emit(
            &mut store,
            client_id,
            65_000,
            9.5,
            date(2025, TimeMonth::June, 1),
        );

        // Au 1er avril 2026, l'exercice en cours est 2026, l'exercice clos 2025.
        let calendar =
            fiscal_calendar(store.connection(), date(2026, TimeMonth::April, 1)).unwrap();

        // Trié chronologiquement.
        for pair in calendar.windows(2) {
            assert!(pair[0].due_on <= pair[1].due_on);
        }
        // Le solde d'IS de 2025 tombe le 15 mai 2026, chiffré à l'IS calculé.
        let solde = calendar
            .iter()
            .find(|d| d.kind == FiscalDeadlineKind::IsSolde)
            .expect("le solde d'IS de l'exercice clos doit figurer au calendrier");
        assert_eq!(solde.due_on, date(2026, TimeMonth::May, 15));
        assert_eq!(solde.amount, Some(Money::from_cents(92_600)));
        // La CA3 d'une SASU parisienne au SIREN 55… tombe le 23 (grille officielle), pas le 15.
        let ca3 = calendar
            .iter()
            .find(|d| d.kind == FiscalDeadlineKind::Ca3)
            .expect("une échéance CA3 chiffrée doit figurer au calendrier");
        assert_eq!(ca3.due_on, date(2026, TimeMonth::April, 23));
        assert!(ca3.amount.is_some());
        let note = ca3.note.as_deref().unwrap();
        assert!(note.contains("le 23 du mois"), "{note}");
        assert!(note.contains("SA/SAS"), "{note}");
        assert!(note.contains("TVA de 2026-03"), "{note}");
    }

    #[test]
    fn a_quarterly_filer_outside_paris_gets_the_21st_and_the_whole_quarters_vat() {
        let (mut store, client_id) = fresh_store("quarterly");
        let mut profile = calendar_profile();
        profile.legal_form = "EURL".to_string();
        profile.address.postal_code = "69001".to_string();
        profile.address.city = "Lyon".to_string();
        profile.vat_regime = Some(VatRegime::RealNormalQuarterly);
        Executor::new(&mut store)
            .execute(&profile, &ExecutionContext::new(Actor::Human, false))
            .unwrap();
        // Juin : hors trimestre déclaré. Juillet et août : dans le T3 → 2 × 200 € de TVA.
        emit(
            &mut store,
            client_id,
            100_000,
            1.0,
            date(2026, TimeMonth::June, 30),
        );
        emit(
            &mut store,
            client_id,
            100_000,
            1.0,
            date(2026, TimeMonth::July, 15),
        );
        emit(
            &mut store,
            client_id,
            100_000,
            1.0,
            date(2026, TimeMonth::August, 15),
        );

        let calendar =
            fiscal_calendar(store.connection(), date(2026, TimeMonth::September, 1)).unwrap();
        let ca3 = calendar
            .iter()
            .find(|d| d.kind == FiscalDeadlineKind::Ca3)
            .unwrap();
        assert_eq!(ca3.due_on, date(2026, TimeMonth::October, 21));
        assert_eq!(ca3.amount, Some(Money::from_cents(40_000)));
        let note = ca3.note.as_deref().unwrap();
        assert!(note.contains("TVA de 2026-07 à 2026-09"), "{note}");
        assert!(
            note.contains("société hors SA, siège hors Paris/92/93/94"),
            "{note}"
        );
    }

    // --- Réel simplifié (lot 27) : acomptes 3514, CA12, suppression du régime en 2027. ---

    fn exercise(start: Date, end: Date) -> FiscalYear {
        FiscalYear::new(start, end)
    }

    #[test]
    fn the_ca12_of_a_calendar_year_is_due_the_second_business_day_after_may_1st() {
        // 2027 : 1er mai un samedi ; lundi 3 (1er jour ouvré), mardi 4 (2e).
        assert_eq!(
            ca12_due_on(FiscalYear::calendar(2026)),
            date(2027, TimeMonth::May, 4)
        );
        // 2026 : 1er mai un vendredi (férié) ; lundi 4, mardi 5.
        assert_eq!(
            ca12_due_on(FiscalYear::calendar(2025)),
            date(2026, TimeMonth::May, 5)
        );
    }

    #[test]
    fn the_ca12e_of_an_offset_year_is_due_three_months_after_close_on_a_business_day() {
        // Clôture au 30 juin 2026 → 30 septembre 2026, un mercredi.
        assert_eq!(
            ca12_due_on(exercise(
                date(2025, TimeMonth::July, 1),
                date(2026, TimeMonth::June, 30)
            )),
            date(2026, TimeMonth::September, 30)
        );
        // Clôture au 28 février 2026 → 31 mai 2026, un dimanche → lundi 1er juin.
        assert_eq!(
            ca12_due_on(exercise(
                date(2025, TimeMonth::March, 1),
                date(2026, TimeMonth::February, 28)
            )),
            date(2026, TimeMonth::June, 1)
        );
    }

    #[test]
    fn the_simplified_regime_ends_with_exercises_opened_from_2027() {
        assert!(simplified_regime_applies_to(FiscalYear::calendar(2026)));
        assert!(
            simplified_regime_applies_to(exercise(
                date(2026, TimeMonth::July, 1),
                date(2027, TimeMonth::June, 30)
            )),
            "un exercice décalé ouvert en 2026 reste au réel simplifié jusqu'à sa clôture"
        );
        assert!(!simplified_regime_applies_to(FiscalYear::calendar(2027)));
        let simplified = Some(VatRegime::RealSimplified);
        assert_eq!(
            VatFilingScheme::for_exercise(simplified, FiscalYear::calendar(2026)),
            VatFilingScheme::Simplified
        );
        assert_eq!(
            VatFilingScheme::for_exercise(simplified, FiscalYear::calendar(2027)),
            VatFilingScheme::Ca3Quarterly
        );
        assert_eq!(
            VatFilingScheme::for_exercise(Some(VatRegime::Franchise), FiscalYear::calendar(2027)),
            VatFilingScheme::NoFiling
        );
        assert_eq!(
            VatFilingScheme::for_exercise(None, FiscalYear::calendar(2027)),
            VatFilingScheme::Ca3Monthly
        );
    }

    #[test]
    fn vat_instalments_are_55_and_40_percent_unless_the_base_is_under_1000_euros() {
        let base = Money::from_cents(400_000);
        assert_eq!(
            vat_instalment_amount(VatInstalment::July, base),
            Some(Money::from_cents(220_000))
        );
        assert_eq!(
            vat_instalment_amount(VatInstalment::December, base),
            Some(Money::from_cents(160_000))
        );
        assert_eq!(
            vat_instalment_amount(VatInstalment::July, Money::from_cents(99_999)),
            None
        );
        assert_eq!(
            vat_instalment_amount(VatInstalment::December, Money::from_cents(-50_000)),
            None,
            "un crédit de TVA dispense aussi"
        );
    }

    #[test]
    fn a_ca3_bounded_by_an_earliest_period_skips_the_quarters_before_it() {
        // Au 5 janvier 2027, la CA3 de janvier déclarerait le T4 2026 — qui relevait de la CA12
        // si le réel normal ne commence qu'avec l'exercice 2027.
        let filing = next_ca3_filing_from(
            date(2027, TimeMonth::January, 5),
            Ca3Periodicity::Quarterly,
            23,
            Some(month(2027, 1)),
        );
        assert_eq!(filing.period_start, month(2027, 1));
        assert_eq!(filing.period_end, month(2027, 3));
        assert_eq!(filing.due_on, date(2027, TimeMonth::April, 23));
    }

    proptest! {
        #[test]
        fn a_ca12_is_always_due_on_a_business_day_after_the_exercise_closes(
            year in 2020i32..2040,
            end_month in 1u8..=12,
        ) {
            let fye = FiscalYearEnd::new(end_month, 31).unwrap();
            let exercise = fye.containing(date(year, TimeMonth::June, 15));
            let due = ca12_due_on(exercise);
            prop_assert!(due > exercise.end());
            prop_assert!(crate::domain::is_french_business_day(due));
            // Au plus 3 mois + quelques jours de report après la clôture (5 mois pour l'exercice
            // civil, dont la CA12 attend mai).
            let max_lag = if end_month == 12 { 130 } else { 100 };
            prop_assert!((due - exercise.end()).whole_days() <= max_lag);
        }
    }

    #[test]
    fn the_vat_filing_summary_reflects_the_grid_and_the_repeal() {
        let set = calendar_profile();
        let mut profile = CompanyProfile {
            name: set.name,
            legal_form: set.legal_form,
            siren: set.siren,
            vat_number: None,
            address: set.address,
            share_capital: set.share_capital,
            rcs_city: set.rcs_city,
            iban: None,
            fiscal_year_end: set.fiscal_year_end,
            vat_regime: set.vat_regime,
            director_monthly_gross: None,
            director_charge_ratio_bps: None,
            president_name: None,
            sole_shareholder_name: None,
            sole_shareholder_address: None,
            share_count: None,
        };
        let today = date(2026, TimeMonth::September, 2);

        let monthly = vat_filing_summary(&profile, today);
        assert_eq!(monthly.scheme, VatFilingScheme::Ca3Monthly);
        assert_eq!(monthly.rule.day, 23);
        assert!(
            monthly.note.contains("CA3 mensuelle, le 23 du mois"),
            "{}",
            monthly.note
        );
        assert!(!monthly.note.contains("supprimé"), "{}", monthly.note);

        profile.vat_regime = Some(VatRegime::RealSimplified);
        let simplified = vat_filing_summary(&profile, today);
        assert_eq!(simplified.scheme, VatFilingScheme::Simplified);
        assert!(
            simplified.note.contains("acomptes 3514"),
            "{}",
            simplified.note
        );
        assert!(
            simplified.note.contains("2e jour ouvré suivant le 1er mai"),
            "{}",
            simplified.note
        );
        assert!(
            simplified.note.contains("1er janvier 2027"),
            "{}",
            simplified.note
        );

        let after = vat_filing_summary(&profile, date(2027, TimeMonth::January, 5));
        assert_eq!(after.scheme, VatFilingScheme::Ca3Quarterly);
        assert!(
            after.note.starts_with("CA3 trimestrielle"),
            "{}",
            after.note
        );

        profile.vat_regime = None;
        let unknown = vat_filing_summary(&profile, today);
        assert_eq!(unknown.scheme, VatFilingScheme::Ca3Monthly);
        assert!(unknown.note.contains("mensuel supposé"), "{}", unknown.note);
    }

    #[test]
    fn a_simplified_filer_gets_its_3514_instalment_the_ca12_and_then_quarterly_ca3s() {
        let (mut store, client_id) = fresh_store("simplified");
        let mut profile = calendar_profile();
        profile.vat_regime = Some(VatRegime::RealSimplified);
        Executor::new(&mut store)
            .execute(&profile, &ExecutionContext::new(Actor::Human, false))
            .unwrap();
        // 2025 : 20 000 € HT → 4 000 € de TVA, base des acomptes 2026.
        emit(
            &mut store,
            client_id,
            2_000_000,
            1.0,
            date(2025, TimeMonth::March, 1),
        );
        // 2026 : 5 000 € HT → 1 000 € de TVA, régularisée par la CA12 de mai 2027.
        emit(
            &mut store,
            client_id,
            500_000,
            1.0,
            date(2026, TimeMonth::February, 1),
        );

        let calendar =
            fiscal_calendar(store.connection(), date(2026, TimeMonth::September, 2)).unwrap();
        for pair in calendar.windows(2) {
            assert!(pair[0].due_on <= pair[1].due_on);
        }

        // Un seul acompte dans l'horizon : décembre 2026. Juillet 2027 n'existe plus, l'exercice
        // 2027 n'étant plus au réel simplifié.
        let instalments: Vec<_> = calendar
            .iter()
            .filter(|d| d.kind == FiscalDeadlineKind::VatInstalment)
            .collect();
        assert_eq!(instalments.len(), 1, "{instalments:?}");
        let december = instalments[0];
        assert_eq!(december.due_on, date(2026, TimeMonth::December, 23));
        assert_eq!(december.amount, Some(Money::from_cents(160_000)));
        let note = december.note.as_deref().unwrap();
        assert!(note.contains("40 %"), "{note}");
        assert!(note.contains("2025-01-01 – 2025-12-31"), "{note}");

        // CA12 de 2026 : 1 000 € − (2 200 + 1 600) = crédit de 2 800 €, le 4 mai 2027.
        let ca12 = calendar
            .iter()
            .find(|d| d.kind == FiscalDeadlineKind::Ca12)
            .expect("la CA12 de l'exercice en cours doit figurer au calendrier");
        assert_eq!(ca12.due_on, date(2027, TimeMonth::May, 4));
        assert_eq!(ca12.amount, Some(Money::from_cents(-280_000)));
        let note = ca12.note.as_deref().unwrap();
        assert!(note.contains("exercice en cours"), "{note}");
        assert!(note.contains("dernière CA12"), "{note}");

        // Première CA3 trimestrielle du réel normal : T1 2027, déposée le 23 avril 2027 — jamais
        // une CA3 sur une période encore couverte par la CA12.
        let quarterly: Vec<_> = calendar
            .iter()
            .filter(|d| d.kind == FiscalDeadlineKind::Ca3)
            .collect();
        assert_eq!(quarterly.len(), 1, "{quarterly:?}");
        assert_eq!(quarterly[0].due_on, date(2027, TimeMonth::April, 23));
        let note = quarterly[0].note.as_deref().unwrap();
        assert!(note.contains("TVA de 2027-01 à 2027-03"), "{note}");
        assert!(note.contains("supprimé"), "{note}");
    }

    #[test]
    fn a_simplified_filer_with_little_vat_is_dispensed_from_instalments() {
        let (mut store, client_id) = fresh_store("dispensed");
        let mut profile = calendar_profile();
        profile.vat_regime = Some(VatRegime::RealSimplified);
        Executor::new(&mut store)
            .execute(&profile, &ExecutionContext::new(Actor::Human, false))
            .unwrap();
        // 2025 : 500 € HT → 100 € de TVA, sous le seuil de 1 000 €.
        emit(
            &mut store,
            client_id,
            50_000,
            1.0,
            date(2025, TimeMonth::March, 1),
        );
        emit(
            &mut store,
            client_id,
            500_000,
            1.0,
            date(2026, TimeMonth::February, 1),
        );
        let calendar =
            fiscal_calendar(store.connection(), date(2026, TimeMonth::September, 2)).unwrap();
        let december = calendar
            .iter()
            .find(|d| d.kind == FiscalDeadlineKind::VatInstalment)
            .unwrap();
        assert_eq!(december.amount, None);
        assert!(
            december.note.as_deref().unwrap().contains("dispense"),
            "{:?}",
            december.note
        );
        // Sans acompte versé, la CA12 régularise toute la TVA de l'exercice.
        let ca12 = calendar
            .iter()
            .find(|d| d.kind == FiscalDeadlineKind::Ca12)
            .unwrap();
        assert_eq!(ca12.amount, Some(Money::from_cents(100_000)));
    }

    #[test]
    fn an_offset_simplified_exercise_opened_in_2026_still_files_a_ca12e_and_no_ca3_yet() {
        let (mut store, client_id) = fresh_store("offset-simplified");
        let mut profile = calendar_profile();
        profile.vat_regime = Some(VatRegime::RealSimplified);
        profile.fiscal_year_end = Some(FiscalYearEnd::new(6, 30).unwrap());
        Executor::new(&mut store)
            .execute(&profile, &ExecutionContext::new(Actor::Human, false))
            .unwrap();
        // Exercice clos 2025-07 → 2026-06 : 10 000 € HT → 2 000 € de TVA.
        emit(
            &mut store,
            client_id,
            1_000_000,
            1.0,
            date(2026, TimeMonth::January, 15),
        );

        let calendar =
            fiscal_calendar(store.connection(), date(2026, TimeMonth::September, 2)).unwrap();
        // CA12 E de l'exercice clos au 30 juin 2026 : le 30 septembre 2026, 2 000 € moins les
        // acomptes (base = exercice 2024-25, vide → dispense).
        let ca12 = calendar
            .iter()
            .find(|d| d.kind == FiscalDeadlineKind::Ca12)
            .expect("la CA12 E de l'exercice clos doit figurer au calendrier");
        assert_eq!(ca12.due_on, date(2026, TimeMonth::September, 30));
        assert_eq!(ca12.amount, Some(Money::from_cents(200_000)));
        let note = ca12.note.as_deref().unwrap();
        assert!(note.contains("CA12 E"), "{note}");
        assert!(!note.contains("dernière CA12"), "{note}");
        // Acompte de décembre 2026 : l'exercice 2026-27, ouvert avant 2027, reste au réel
        // simplifié ; base = exercice 2025-26 (2 000 €) → 40 % = 800 €.
        let december = calendar
            .iter()
            .find(|d| d.kind == FiscalDeadlineKind::VatInstalment)
            .unwrap();
        assert_eq!(december.due_on, date(2026, TimeMonth::December, 23));
        assert_eq!(december.amount, Some(Money::from_cents(80_000)));
        // Aucune CA3 : le réel normal ne commence qu'avec l'exercice ouvert le 1er juillet 2027,
        // dont la première CA3 (octobre 2027) dépasse l'horizon.
        assert!(calendar.iter().all(|d| d.kind != FiscalDeadlineKind::Ca3));
    }

    // --- Lot 41 : DAS2, 2777, première CA3 après un exercice au réel simplifié. ---

    #[test]
    fn the_das2_is_due_with_the_tax_return_or_three_months_after_an_offset_close() {
        // Exercice civil : sommes de 2025, déclarées avec la liasse, le 2e jour ouvré suivant
        // le 1er mai 2026 (vendredi férié) → mardi 5 mai.
        assert_eq!(
            das2_due_on(FiscalYearEnd::CALENDAR, 2025),
            date(2026, TimeMonth::May, 5)
        );
        // Clôture au 30 septembre : les sommes de 2025 se déclarent dans les trois mois de la
        // clôture de l'exercice qui suit le 31 décembre 2025 (30 septembre 2026) → 30 décembre.
        assert_eq!(
            das2_due_on(FiscalYearEnd::new(9, 30).unwrap(), 2025),
            date(2026, TimeMonth::December, 30)
        );
    }

    #[test]
    fn the_2777_is_due_on_the_15th_of_the_month_after_payment_moved_to_a_business_day() {
        assert_eq!(
            dividends_2777_due_on(date(2026, TimeMonth::June, 10)),
            date(2026, TimeMonth::July, 15)
        );
        // 15 août 2026 : samedi et férié → lundi 17.
        assert_eq!(
            dividends_2777_due_on(date(2026, TimeMonth::July, 20)),
            date(2026, TimeMonth::August, 17)
        );
        assert_eq!(
            dividend_social_charges_bps(date(2025, TimeMonth::December, 31)),
            1_720
        );
        assert_eq!(
            dividend_social_charges_bps(date(2026, TimeMonth::January, 1)),
            1_860
        );
    }

    fn fees(store: &mut Store, supplier: Option<&str>, cents: i64, on: Date) {
        Executor::new(store)
            .execute(
                &crate::expenses::RecordExpense {
                    label: "Honoraires".to_string(),
                    category: crate::domain::ExpenseCategory::Fees,
                    amount: Money::from_cents(cents),
                    vat_rate: VatRate::Standard,
                    vat_deductible: Money::ZERO,
                    incurred_on: on,
                    receipt_hash: None,
                    receipt_filename: None,
                    supplier: supplier.map(str::to_string),
                    bank_transaction_id: None,
                },
                &ExecutionContext::new(Actor::Human, false),
            )
            .unwrap();
    }

    #[test]
    fn the_das2_cumulates_fees_per_beneficiary_and_per_calendar_year() {
        let (mut store, _) = fresh_store("das2");
        let human = ExecutionContext::new(Actor::Human, false);
        Executor::new(&mut store)
            .execute(&calendar_profile(), &human)
            .unwrap();
        // 600 € en octobre 2025 + 900 € en mars 2026 au même cabinet : 1 500 € sur deux années
        // civiles, jamais 2 400 € sur une seule — rien à déclarer.
        fees(
            &mut store,
            Some("Cabinet Lumen"),
            60_000,
            date(2025, TimeMonth::October, 3),
        );
        fees(
            &mut store,
            Some("Cabinet Lumen"),
            90_000,
            date(2026, TimeMonth::March, 3),
        );
        let calendar =
            fiscal_calendar(store.connection(), date(2026, TimeMonth::April, 1)).unwrap();
        let das2: Vec<_> = calendar
            .iter()
            .filter(|d| d.kind == FiscalDeadlineKind::Das2)
            .collect();
        assert_eq!(das2.len(), 1, "{das2:?}");
        assert_eq!(das2[0].due_on, date(2026, TimeMonth::May, 5));
        assert_eq!(das2[0].amount, Some(Money::ZERO));
        assert!(das2[0].note.as_deref().unwrap().contains("rien à déclarer"));

        // 1 300 € deux fois de plus en 2026 : 3 500 € au même bénéficiaire → à déclarer le
        // 4 mai 2027 (1er mai 2027 un samedi : lundi 3 puis mardi 4), et 500 € sans
        // bénéficiaire signalés.
        fees(
            &mut store,
            Some("Cabinet Lumen"),
            130_000,
            date(2026, TimeMonth::June, 1),
        );
        fees(
            &mut store,
            Some("Cabinet Lumen"),
            130_000,
            date(2026, TimeMonth::September, 1),
        );
        fees(&mut store, None, 50_000, date(2026, TimeMonth::November, 1));
        let calendar =
            fiscal_calendar(store.connection(), date(2027, TimeMonth::January, 10)).unwrap();
        let das2 = calendar
            .iter()
            .find(|d| d.kind == FiscalDeadlineKind::Das2)
            .expect("la DAS2 2026 doit figurer au calendrier");
        assert_eq!(das2.due_on, date(2027, TimeMonth::May, 4));
        assert_eq!(das2.amount, Some(Money::from_cents(350_000)));
        let note = das2.note.as_deref().unwrap();
        assert!(
            note.contains(&format!("Cabinet Lumen ({})", Money::from_cents(350_000))),
            "{note}"
        );
        assert!(
            note.contains(&format!(
                "{} d'honoraires sans bénéficiaire",
                Money::from_cents(50_000)
            )),
            "{note}"
        );
    }

    #[test]
    fn approved_dividends_open_a_2777_deadline_priced_at_the_flat_tax() {
        let (mut store, client_id) = fresh_store("2777");
        let human = ExecutionContext::new(Actor::Human, false);
        Executor::new(&mut store)
            .execute(&calendar_profile(), &human)
            .unwrap();
        emit(
            &mut store,
            client_id,
            1_000_000,
            1.0,
            date(2025, TimeMonth::June, 1),
        );
        let crate::app::Outcome::Applied(id) = Executor::new(&mut store)
            .execute(
                &crate::fiscal_year::CloseFiscalYear {
                    starts_on: date(2025, TimeMonth::January, 1),
                    ends_on: date(2025, TimeMonth::December, 31),
                    legal_reserve: Money::from_cents(10_000),
                    dividends: Money::from_cents(400_000),
                    carry_back: false,
                    today: None,
                    non_deductible_expenses: Money::ZERO,
                },
                &human,
            )
            .unwrap()
        else {
            panic!("expected Applied")
        };
        Executor::new(&mut store)
            .execute(
                &crate::fiscal_year::ApproveFiscalYear {
                    id,
                    revision: 1,
                    approved_on: date(2026, TimeMonth::June, 10),
                    today: None,
                },
                &human,
            )
            .unwrap();
        let calendar =
            fiscal_calendar(store.connection(), date(2026, TimeMonth::June, 15)).unwrap();
        let d = calendar
            .iter()
            .find(|d| d.kind == FiscalDeadlineKind::Dividends2777)
            .expect("les dividendes approuvés ouvrent une échéance 2777");
        assert_eq!(d.due_on, date(2026, TimeMonth::July, 15));
        // 4 000 € × (12,8 % + 18,6 %) = 512 € + 744 € = 1 256 €.
        assert_eq!(d.amount, Some(Money::from_cents(125_600)));
        let note = d.note.as_deref().unwrap();
        assert!(note.contains("18,6 %"), "{note}");
        assert!(
            note.contains(&Money::from_cents(51_200).to_string()),
            "{note}"
        );
    }

    #[test]
    fn the_first_quarterly_ca3_after_a_simplified_exercise_starts_with_the_new_exercise() {
        // Réel simplifié, clôture au 30 juin : l'exercice 2026-27 (ouvert avant 2027) reste
        // au RSI et se solde par une CA12 E ; l'exercice ouvert le 1er juillet 2027 passe en
        // CA3 trimestrielle — la première déclare le T3 2027 (dépôt en octobre), jamais le
        // T2 2027 déjà couvert par la CA12 E.
        let (mut store, client_id) = fresh_store("first-ca3");
        let mut profile = calendar_profile();
        profile.vat_regime = Some(VatRegime::RealSimplified);
        profile.fiscal_year_end = Some(FiscalYearEnd::new(6, 30).unwrap());
        Executor::new(&mut store)
            .execute(&profile, &ExecutionContext::new(Actor::Human, false))
            .unwrap();
        emit(
            &mut store,
            client_id,
            1_000_000,
            1.0,
            date(2027, TimeMonth::May, 15),
        );
        let calendar = fiscal_calendar(store.connection(), date(2027, TimeMonth::July, 5)).unwrap();
        let ca12 = calendar
            .iter()
            .find(|d| d.kind == FiscalDeadlineKind::Ca12)
            .expect("CA12 E de l'exercice clos au 30 juin 2027");
        assert_eq!(ca12.due_on, date(2027, TimeMonth::September, 30));
        let ca3 = calendar
            .iter()
            .find(|d| d.kind == FiscalDeadlineKind::Ca3)
            .expect("première CA3 trimestrielle de l'exercice 2027-28");
        // SASU parisienne au SIREN 55… : le 24 (grille SA/SAS), dimanche → lundi 25.
        assert_eq!(ca3.due_on, date(2027, TimeMonth::October, 25));
        let note = ca3.note.as_deref().unwrap();
        assert!(note.contains("2027-07"), "{note}");
        assert!(!note.contains("2027-04"), "{note}");
    }
}
