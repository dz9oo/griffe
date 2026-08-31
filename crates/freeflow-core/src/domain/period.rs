//! Périodes calendaires : mois (facturation, CRA) et exercice fiscal.

use std::fmt;
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::Date;
use time::format_description::FormatItem;

static DATE_FORMAT: LazyLock<Vec<FormatItem<'static>>> = LazyLock::new(|| {
    time::format_description::parse_borrowed::<2>("[year]-[month]-[day]")
        .expect("format statique toujours valide")
});

/// Représentation SQL stable (`AAAA-MM-JJ`) d'une date calendaire — utilisée par toutes les
/// colonnes `TEXT` de date du schéma, pour que le tri lexicographique SQL reste chronologique.
///
/// # Panics
///
/// Ne panique jamais en pratique : le format est une constante statique valide.
#[must_use]
pub fn format_date(date: Date) -> String {
    date.format(&DATE_FORMAT)
        .expect("le format de date statique est toujours valide")
}

/// # Errors
///
/// Retourne une erreur si `s` n'est pas au format `AAAA-MM-JJ`.
pub fn parse_date(s: &str) -> Result<Date, time::error::Parse> {
    Date::parse(s, &DATE_FORMAT)
}

#[derive(Debug, Error, PartialEq, Eq)]
#[error("le mois {0} est hors de la plage 1..=12")]
pub struct MonthError(pub u8);

/// Un mois calendaire (année + mois), utilisé pour les périodes de facturation et le CRA.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Month {
    year: i32,
    month: u8,
}

impl Month {
    /// # Errors
    ///
    /// Retourne une erreur si `month` n'est pas compris entre 1 et 12.
    pub const fn new(year: i32, month: u8) -> Result<Self, MonthError> {
        if month == 0 || month > 12 {
            return Err(MonthError(month));
        }
        Ok(Self { year, month })
    }

    #[must_use]
    pub const fn year(self) -> i32 {
        self.year
    }

    #[must_use]
    pub const fn month(self) -> u8 {
        self.month
    }

    #[must_use]
    pub const fn succ(self) -> Self {
        if self.month == 12 {
            Self {
                year: self.year + 1,
                month: 1,
            }
        } else {
            Self {
                year: self.year,
                month: self.month + 1,
            }
        }
    }

    #[must_use]
    pub const fn pred(self) -> Self {
        if self.month == 1 {
            Self {
                year: self.year - 1,
                month: 12,
            }
        } else {
            Self {
                year: self.year,
                month: self.month - 1,
            }
        }
    }

    /// # Panics
    ///
    /// Ne panique jamais en pratique : `month` est garanti dans `1..=12` à la construction.
    #[must_use]
    pub fn first_day(self) -> Date {
        let month =
            time::Month::try_from(self.month).expect("le mois est garanti valide par construction");
        Date::from_calendar_date(self.year, month, 1)
            .expect("le premier jour d'un mois est toujours valide")
    }

    /// Dernier jour du mois (inclusif) : le jour précédant le premier jour du mois suivant.
    ///
    /// # Panics
    ///
    /// Ne panique jamais en pratique, pour la même raison que [`Self::first_day`].
    #[must_use]
    pub fn last_day(self) -> Date {
        self.succ()
            .first_day()
            .previous_day()
            .expect("le premier jour d'un mois a toujours une veille")
    }
}

impl fmt::Display for Month {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}", self.year, self.month)
    }
}

/// Un exercice fiscal (bornes inclusives). Par défaut aligné sur l'année civile, mais le type
/// reste général pour un exercice décalé.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FiscalYear {
    start: Date,
    end: Date,
}

impl FiscalYear {
    #[must_use]
    pub const fn new(start: Date, end: Date) -> Self {
        Self { start, end }
    }

    /// # Panics
    ///
    /// Ne panique jamais en pratique : le 1er janvier et le 31 décembre d'une même année civile
    /// forment toujours une plage de dates valide.
    #[must_use]
    pub fn calendar(year: i32) -> Self {
        let start = Date::from_calendar_date(year, time::Month::January, 1)
            .expect("1er janvier toujours valide");
        let end = Date::from_calendar_date(year, time::Month::December, 31)
            .expect("31 décembre toujours valide");
        Self { start, end }
    }

    #[must_use]
    pub const fn start(self) -> Date {
        self.start
    }

    #[must_use]
    pub const fn end(self) -> Date {
        self.end
    }

    #[must_use]
    pub fn contains(self, date: Date) -> bool {
        date >= self.start && date <= self.end
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FiscalYearEndError {
    #[error("le mois de clôture {0} est hors de la plage 1..=12")]
    Month(u8),
    #[error("le jour de clôture {0} est hors de la plage 1..=31")]
    Day(u8),
}

/// Date de clôture d'exercice **récurrente** (mois + jour), telle que déclarée dans les statuts —
/// typiquement le 31/12, mais un exercice décalé (30/06, etc.) est courant. C'est de cette date
/// que dérivent toutes les échéances fiscales : acomptes et solde d'IS, liasse, AG d'approbation,
/// dépôt des comptes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FiscalYearEnd {
    month: u8,
    day: u8,
}

impl FiscalYearEnd {
    /// # Errors
    ///
    /// [`FiscalYearEndError`] si `month` n'est pas dans `1..=12` ou `day` dans `1..=31`. Un jour
    /// trop grand pour le mois visé (ex. 31 pour un mois de 30 jours, ou 29/02 une année non
    /// bissextile) n'est **pas** une erreur : il est rogné au dernier jour réel du mois lors de la
    /// résolution d'un exercice concret — c'est la convention comptable habituelle.
    pub const fn new(month: u8, day: u8) -> Result<Self, FiscalYearEndError> {
        if month == 0 || month > 12 {
            return Err(FiscalYearEndError::Month(month));
        }
        if day == 0 || day > 31 {
            return Err(FiscalYearEndError::Day(day));
        }
        Ok(Self { month, day })
    }

    /// Clôture au 31 décembre — l'exercice aligné sur l'année civile, cas par défaut.
    pub const CALENDAR: Self = Self { month: 12, day: 31 };

    #[must_use]
    pub const fn month(self) -> u8 {
        self.month
    }

    #[must_use]
    pub const fn day(self) -> u8 {
        self.day
    }

    /// Date de clôture concrète dans l'année civile `year`, le jour rogné au dernier jour réel du
    /// mois si besoin (29/02 → 28/02 hors année bissextile, 31 → 30 pour un mois de 30 jours).
    ///
    /// # Panics
    ///
    /// Ne panique jamais : `month` est validé `1..=12` à la construction, et le jour est rogné
    /// dans les bornes réelles du mois avant construction de la `Date`.
    #[must_use]
    pub fn end_in_year(self, year: i32) -> Date {
        let month = Month::new(year, self.month).expect("mois validé à la construction");
        let last_day = month.last_day().day();
        let day = self.day.min(last_day);
        let tm = time::Month::try_from(self.month).expect("mois validé à la construction");
        Date::from_calendar_date(year, tm, day).expect("jour rogné dans les bornes réelles du mois")
    }

    /// L'exercice (bornes inclusives) qui contient `date`, dérivé de cette clôture récurrente.
    ///
    /// # Panics
    ///
    /// Ne panique jamais : une date de clôture concrète (jour rogné dans les bornes du mois) a
    /// toujours un lendemain calendaire.
    #[must_use]
    pub fn containing(self, date: Date) -> FiscalYear {
        let this_year_end = self.end_in_year(date.year());
        let end = if date <= this_year_end {
            this_year_end
        } else {
            self.end_in_year(date.year() + 1)
        };
        let previous_end = self.end_in_year(end.year() - 1);
        let start = previous_end
            .next_day()
            .expect("une date de clôture a toujours un lendemain");
        FiscalYear::new(start, end)
    }

    /// L'exercice en cours à la date `today`.
    #[must_use]
    pub fn current(self, today: Date) -> FiscalYear {
        self.containing(today)
    }

    /// L'exercice qui précède immédiatement `fiscal_year`.
    ///
    /// # Panics
    ///
    /// Ne panique jamais : un début d'exercice est une date réelle, qui a toujours une veille.
    #[must_use]
    pub fn previous(self, fiscal_year: FiscalYear) -> FiscalYear {
        let day_before_start = fiscal_year
            .start()
            .previous_day()
            .expect("un début d'exercice a toujours une veille");
        self.containing(day_before_start)
    }
}

/// Régime de TVA déclaré par l'entreprise : il pilote la périodicité de la déclaration CA3 (et
/// donc l'échéancier de TVA du calendrier fiscal).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VatRegime {
    /// Réel normal, déclaration mensuelle (CA3 chaque mois).
    RealNormalMonthly,
    /// Réel normal, déclaration trimestrielle (autorisée si TVA annuelle < 4 000 €).
    RealNormalQuarterly,
    /// Réel simplifié : deux acomptes semestriels + régularisation annuelle (CA12).
    RealSimplified,
    /// Franchise en base : pas de TVA collectée ni de déclaration périodique.
    Franchise,
}

impl VatRegime {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RealNormalMonthly => "real_normal_monthly",
            Self::RealNormalQuarterly => "real_normal_quarterly",
            Self::RealSimplified => "real_simplified",
            Self::Franchise => "franchise",
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
#[error("régime de TVA inconnu : {0}")]
pub struct UnknownVatRegime(pub String);

impl std::str::FromStr for VatRegime {
    type Err = UnknownVatRegime;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "real_normal_monthly" => Ok(Self::RealNormalMonthly),
            "real_normal_quarterly" => Ok(Self::RealNormalQuarterly),
            "real_simplified" => Ok(Self::RealSimplified),
            "franchise" => Ok(Self::Franchise),
            other => Err(UnknownVatRegime(other.to_string())),
        }
    }
}

impl fmt::Display for VatRegime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_date_round_trips_through_parse_date() {
        let date = Date::from_calendar_date(2026, time::Month::September, 2).unwrap();
        assert_eq!(format_date(date), "2026-09-02");
        assert_eq!(parse_date("2026-09-02").unwrap(), date);
    }

    #[test]
    fn month_out_of_range_is_rejected() {
        assert_eq!(Month::new(2026, 0), Err(MonthError(0)));
        assert_eq!(Month::new(2026, 13), Err(MonthError(13)));
    }

    #[test]
    fn succ_rolls_over_to_next_year() {
        let december = Month::new(2026, 12).unwrap();
        assert_eq!(december.succ(), Month::new(2027, 1).unwrap());
    }

    #[test]
    fn first_and_last_day_bound_the_month_correctly() {
        let september = Month::new(2026, 9).unwrap();
        assert_eq!(
            september.first_day(),
            Date::from_calendar_date(2026, time::Month::September, 1).unwrap()
        );
        assert_eq!(
            september.last_day(),
            Date::from_calendar_date(2026, time::Month::September, 30).unwrap()
        );

        let february_leap = Month::new(2024, 2).unwrap();
        assert_eq!(
            february_leap.last_day(),
            Date::from_calendar_date(2024, time::Month::February, 29).unwrap()
        );
    }

    #[test]
    fn pred_rolls_back_to_previous_year() {
        let january = Month::new(2027, 1).unwrap();
        assert_eq!(january.pred(), Month::new(2026, 12).unwrap());
    }

    #[test]
    fn calendar_fiscal_year_contains_its_bounds() {
        let fy = FiscalYear::calendar(2026);
        assert!(fy.contains(fy.start()));
        assert!(fy.contains(fy.end()));
        assert!(!fy.contains(fy.end().next_day().unwrap()));
    }

    fn date(y: i32, m: time::Month, d: u8) -> Date {
        Date::from_calendar_date(y, m, d).unwrap()
    }

    #[test]
    fn calendar_year_end_derives_the_civil_year_exercise() {
        let end = FiscalYearEnd::CALENDAR;
        let fy = end.containing(date(2026, time::Month::May, 1));
        assert_eq!(fy.start(), date(2026, time::Month::January, 1));
        assert_eq!(fy.end(), date(2026, time::Month::December, 31));
    }

    #[test]
    fn offset_year_end_spans_two_civil_years() {
        let end = FiscalYearEnd::new(6, 30).unwrap();
        // Une date avant la clôture appartient à l'exercice qui se termine cette année-là.
        let spring = end.containing(date(2026, time::Month::May, 1));
        assert_eq!(spring.start(), date(2025, time::Month::July, 1));
        assert_eq!(spring.end(), date(2026, time::Month::June, 30));
        // Une date après la clôture bascule sur l'exercice suivant.
        let autumn = end.containing(date(2026, time::Month::August, 1));
        assert_eq!(autumn.start(), date(2026, time::Month::July, 1));
        assert_eq!(autumn.end(), date(2027, time::Month::June, 30));
    }

    #[test]
    fn february_29_end_is_clamped_on_non_leap_years() {
        let end = FiscalYearEnd::new(2, 29).unwrap();
        assert_eq!(end.end_in_year(2025), date(2025, time::Month::February, 28));
        assert_eq!(end.end_in_year(2024), date(2024, time::Month::February, 29));
    }

    #[test]
    fn previous_exercise_ends_the_day_before_this_one_starts() {
        let end = FiscalYearEnd::CALENDAR;
        let this = end.containing(date(2026, time::Month::May, 1));
        let prev = end.previous(this);
        assert_eq!(prev.end(), date(2025, time::Month::December, 31));
        assert_eq!(prev.end().next_day().unwrap(), this.start());
    }

    #[test]
    fn vat_regime_round_trips_through_str() {
        for regime in [
            VatRegime::RealNormalMonthly,
            VatRegime::RealNormalQuarterly,
            VatRegime::RealSimplified,
            VatRegime::Franchise,
        ] {
            assert_eq!(regime.as_str().parse::<VatRegime>().unwrap(), regime);
        }
        assert!("bogus".parse::<VatRegime>().is_err());
    }
}
