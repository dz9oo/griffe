//! Calendrier fiscal et social d'une SASU à l'IS — **dates indicatives**, désormais assorties de
//! **montants** là où le domaine peut les calculer (lot 19).
//!
//! **Ce module reste une aide au pilotage, pas une source de vérité fiscale.** Les dates dérivent
//! de la date de clôture d'exercice déclarée dans le profil ([`crate::company::CompanyProfile`]) ;
//! à défaut, l'année civile est supposée. La date limite de télédéclaration CA3 suit la grille
//! officielle (zone du siège, catégorie de redevable, deux premiers chiffres du SIREN — voir
//! [`crate::domain::Ca3FilingRule`]) et la périodicité du régime de TVA déclaré, avec report au
//! jour ouvré suivant (lot 26) ; les échéances de la liasse, du dépôt des comptes et de l'AG
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
use time::Date;

use crate::accounting::{compute_result, vat_due_for_period};
use crate::app::AppError;
use crate::company::company_profile;
use crate::domain::{
    Ca3FilingRule, FiscalYearEnd, Money, Month, VatRegime, next_french_business_day_on_or_after,
};

/// Seuil de dispense des acomptes d'IS : aucun acompte n'est dû si l'IS de l'exercice précédent
/// est inférieur à 3 000 €.
const IS_ACOMPTE_DISPENSATION: Money = Money::from_cents(300_000);

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum FiscalDeadlineKind {
    /// Déclaration de TVA (CA3) — périodicité pilotée par le régime de TVA.
    Ca3,
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
}

impl FiscalDeadlineKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ca3 => "ca3",
            Self::IsAcompte => "is_acompte",
            Self::IsSolde => "is_solde",
            Self::Cfe => "cfe",
            Self::Liasse => "liasse",
            Self::ApprovalMeeting => "approval_meeting",
            Self::AccountsFiling => "accounts_filing",
            Self::Dsn => "dsn",
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct FiscalDeadline {
    pub kind: FiscalDeadlineKind,
    /// Toujours `>= today` : ce sont les *prochaines* échéances, jamais des échéances passées.
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
    .expect("les jours utilisés ici (15 à 28) sont valides dans tous les mois")
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
    /// `None` quand le régime ne donne lieu à aucune CA3 : réel simplifié (CA12 annuelle et
    /// acomptes semestriels, non modélisés) ou franchise en base (aucune TVA collectée). Un régime
    /// non renseigné est supposé mensuel — la périodicité la plus exigeante, donc jamais en retard.
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
    let day = day.clamp(Ca3FilingRule::EARLIEST_DAY, 24);
    let mut filing_month =
        Month::new(today.year(), u8::from(today.month())).expect("mois courant valide");
    // Termine en au plus quatre itérations : chaque trimestre contient un mois de dépôt.
    loop {
        if periodicity.files_in(filing_month) {
            let nominal_due_on = nth_of_month(filing_month, day);
            let due_on = next_french_business_day_on_or_after(nominal_due_on);
            if due_on >= today {
                return Ca3Filing {
                    period_start: periodicity.period_start(filing_month),
                    period_end: filing_month.pred(),
                    nominal_due_on,
                    due_on,
                };
            }
        }
        filing_month = filing_month.succ();
    }
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
/// décalé, le 15 du 4e mois suivant la clôture (règle générale approchée).
fn is_solde_due(fye: FiscalYearEnd, end: Date) -> Date {
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

/// Ajoute `months` mois à une date, en ramenant le jour au dernier jour réel du mois cible.
fn add_months(date: Date, months: u32) -> Date {
    let mut month = Month::new(date.year(), u8::from(date.month())).unwrap();
    for _ in 0..months {
        month = month.succ();
    }
    let day = date.day().min(month.last_day().day());
    nth_of_month(month, day)
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

    // --- TVA (CA3) : périodicité du régime, jour de la grille officielle dérivé du profil
    // (zone du siège, forme juridique, deux premiers chiffres du SIREN), report au jour ouvré. ---
    if let Some(periodicity) = Ca3Periodicity::from_regime(vat_regime) {
        let rule = profile.as_ref().map(|p| {
            Ca3FilingRule::derive(&p.legal_form, &p.name, p.siren, &p.address.postal_code)
        });
        let day = rule.map_or(Ca3FilingRule::EARLIEST_DAY, |r| r.day);
        let filing = next_ca3_filing(today, periodicity, day);
        let vat = vat_due_for_period(
            conn,
            filing.period_start.first_day(),
            filing.period_end.last_day(),
        )?;
        deadlines.push(FiscalDeadline {
            kind: FiscalDeadlineKind::Ca3,
            due_on: filing.due_on,
            amount: Some(vat.due),
            note: Some(ca3_note(&filing, periodicity, rule, vat_regime.is_none())),
        });
    }

    // --- IS : solde de l'exercice clos + acomptes de l'exercice en cours, montants tirés du
    // résultat de l'exercice précédent (base des acomptes). ---
    let previous_result = profile
        .as_ref()
        .map(|p| compute_result(conn, previous, p))
        .transpose()?;
    let previous_is = previous_result.map(|r| r.corporate_tax);

    let solde_due = is_solde_due(fye, previous.end());
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
        let note = if previous_is.is_some_and(|is| is < IS_ACOMPTE_DISPENSATION) {
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
    let liasse_due = if fye.month() == 12 {
        nth_of_month(Month::new(previous.end().year() + 1, 5).unwrap(), 15)
    } else {
        add_months(previous.end(), 3)
    };
    if liasse_due >= today && liasse_due <= horizon {
        deadlines.push(FiscalDeadline {
            kind: FiscalDeadlineKind::Liasse,
            due_on: liasse_due,
            amount: None,
            note: Some("2065 + tableaux 2033, télétransmission EDI-TDFC (indicatif)".to_string()),
        });
    }

    // --- AG d'approbation (clôture + 6 mois) et dépôt au greffe (AG + 1 mois). ---
    let approval_due = add_months(previous.end(), 6);
    if approval_due >= today && approval_due <= horizon {
        deadlines.push(bare(FiscalDeadlineKind::ApprovalMeeting, approval_due));
    }
    let filing_due = add_months(approval_due, 1);
    if filing_due >= today && filing_due <= horizon {
        deadlines.push(bare(FiscalDeadlineKind::AccountsFiling, filing_due));
    }

    // --- DSN mensuelle : uniquement si le président est rémunéré ; montant = coût employeur du
    // mois (brut + cotisations patronales estimées). ---
    let director = profile
        .as_ref()
        .and_then(|p| p.director_monthly_gross.map(|g| (p, g)));
    if let Some((profile, gross)) = director {
        let employer = profile
            .director_charge_ratio_bps
            .map_or(Money::ZERO, |bps| gross.apply_rate_bps(bps));
        let monthly_cost = gross + employer;
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
            note: Some("cotisations sociales mensuelles du dirigeant (estimation)".to_string()),
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
            is_solde_due(fye, date(2025, TimeMonth::December, 31)),
            date(2026, TimeMonth::May, 15)
        );
    }

    #[test]
    fn is_solde_for_an_offset_year_is_the_15th_of_the_fourth_month_after_close() {
        let fye = FiscalYearEnd::new(6, 30).unwrap();
        assert_eq!(
            is_solde_due(fye, date(2026, TimeMonth::June, 30)),
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
        // Facture de l'exercice CLOS (2025) : 6 175 € HT → IS 15 % = 926,25 €.
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
        assert_eq!(solde.amount, Some(Money::from_cents(92_625)));
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

    #[test]
    fn a_simplified_regime_files_no_ca3() {
        let (mut store, _) = fresh_store("simplified");
        let mut profile = calendar_profile();
        profile.vat_regime = Some(VatRegime::RealSimplified);
        Executor::new(&mut store)
            .execute(&profile, &ExecutionContext::new(Actor::Human, false))
            .unwrap();
        let calendar =
            fiscal_calendar(store.connection(), date(2026, TimeMonth::September, 1)).unwrap();
        assert!(calendar.iter().all(|d| d.kind != FiscalDeadlineKind::Ca3));
    }
}
