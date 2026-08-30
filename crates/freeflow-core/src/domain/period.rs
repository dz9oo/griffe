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
}
