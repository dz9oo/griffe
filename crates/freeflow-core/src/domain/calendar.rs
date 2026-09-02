//! Jours ouvrés français : week-ends et jours fériés légaux, dont les fêtes mobiles calculées
//! par rapport à Pâques (algorithme de Meeus/Jones/Butcher, calendrier grégorien).

use time::{Date, Duration, Month, Weekday};

/// Dimanche de Pâques pour une année donnée (calendrier grégorien).
///
/// # Panics
///
/// Ne panique jamais en pratique : l'algorithme ne produit que des dates calendaires valides.
#[must_use]
#[allow(clippy::many_single_char_names)] // noms canoniques de l'algorithme de référence.
pub fn easter_sunday(year: i32) -> Date {
    let a = year % 19;
    let b = year / 100;
    let c = year % 100;
    let d = b / 4;
    let e = b % 4;
    let f = (b + 8) / 25;
    let g = (b - f + 1) / 3;
    let h = (19 * a + b - d - g + 15) % 30;
    let i = c / 4;
    let k = c % 4;
    let l = (32 + 2 * e + 2 * i - h - k) % 7;
    let m = (a + 11 * h + 22 * l) / 451;
    let month = (h + l - 7 * m + 114) / 31;
    let day = (h + l - 7 * m + 114) % 31 + 1;

    let month =
        Month::try_from(u8::try_from(month).expect("le mois calculé tient toujours dans 1..=12"))
            .expect("l'algorithme ne produit que mars ou avril");
    let day = u8::try_from(day).expect("le jour calculé tient toujours dans 1..=31");
    Date::from_calendar_date(year, month, day)
        .expect("l'algorithme ne produit que des dates valides")
}

/// Jours fériés légaux français pour une année donnée (fêtes fixes + mobiles).
///
/// # Panics
///
/// Ne panique jamais en pratique, pour les mêmes raisons que [`easter_sunday`].
#[must_use]
pub fn french_public_holidays(year: i32) -> Vec<Date> {
    let easter = easter_sunday(year);
    let fixed = |month: Month, day: u8| {
        Date::from_calendar_date(year, month, day).expect("date fixe toujours valide")
    };
    vec![
        fixed(Month::January, 1),    // Jour de l'An
        easter + Duration::days(1),  // Lundi de Pâques
        fixed(Month::May, 1),        // Fête du Travail
        fixed(Month::May, 8),        // Victoire 1945
        easter + Duration::days(39), // Ascension
        easter + Duration::days(50), // Lundi de Pentecôte
        fixed(Month::July, 14),      // Fête Nationale
        fixed(Month::August, 15),    // Assomption
        fixed(Month::November, 1),   // Toussaint
        fixed(Month::November, 11),  // Armistice
        fixed(Month::December, 25),  // Noël
    ]
}

#[must_use]
pub fn is_french_business_day(date: Date) -> bool {
    !matches!(date.weekday(), Weekday::Saturday | Weekday::Sunday)
        && !french_public_holidays(date.year()).contains(&date)
}

/// Premier jour ouvré français à partir de `date` incluse : `date` elle-même si elle est ouvrée,
/// sinon le jour ouvré suivant. C'est la règle de report des échéances fiscales (une date limite
/// qui tombe un samedi, un dimanche ou un jour férié est reportée au premier jour ouvrable
/// suivant).
///
/// # Panics
///
/// Ne panique jamais en pratique : `Date::next_day` ne peut échouer qu'au-delà de l'an 9999, et il
/// n'existe pas plus de quatre jours non ouvrés consécutifs dans le calendrier français.
#[must_use]
pub fn next_french_business_day_on_or_after(date: Date) -> Date {
    let mut current = date;
    while !is_french_business_day(current) {
        current = current
            .next_day()
            .expect("un jour ouvré existe toujours avant l'an 9999");
    }
    current
}

/// Nombre de jours ouvrés français dans l'intervalle `[start, end]` (bornes inclusives).
///
/// # Panics
///
/// Ne panique jamais en pratique : `Date::next_day` ne peut échouer qu'au-delà de l'an 9999.
#[must_use]
pub fn french_business_days_in(start: Date, end: Date) -> u32 {
    if start > end {
        return 0;
    }
    let mut count = 0u32;
    let mut current = start;
    loop {
        if is_french_business_day(current) {
            count += 1;
        }
        if current == end {
            break;
        }
        current = current
            .next_day()
            .expect("l'intervalle testé ne s'approche jamais de l'an 9999");
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn easter_sunday_matches_known_reference_dates() {
        // Dates de référence largement documentées (Pâques occidentale, calendrier grégorien).
        assert_eq!(
            easter_sunday(2024),
            Date::from_calendar_date(2024, Month::March, 31).unwrap()
        );
        assert_eq!(
            easter_sunday(2025),
            Date::from_calendar_date(2025, Month::April, 20).unwrap()
        );
        assert_eq!(
            easter_sunday(2026),
            Date::from_calendar_date(2026, Month::April, 5).unwrap()
        );
    }

    #[test]
    fn fixed_holidays_are_never_business_days() {
        assert!(!is_french_business_day(
            Date::from_calendar_date(2026, Month::July, 14).unwrap()
        ));
        assert!(!is_french_business_day(
            Date::from_calendar_date(2026, Month::December, 25).unwrap()
        ));
    }

    #[test]
    fn weekends_are_never_business_days() {
        // Samedi 5 septembre 2026 / dimanche 6 septembre 2026.
        assert!(!is_french_business_day(
            Date::from_calendar_date(2026, Month::September, 5).unwrap()
        ));
        assert!(!is_french_business_day(
            Date::from_calendar_date(2026, Month::September, 6).unwrap()
        ));
    }

    #[test]
    fn an_ordinary_weekday_is_a_business_day() {
        // Mardi 1er septembre 2026, aucun jour férié à proximité.
        assert!(is_french_business_day(
            Date::from_calendar_date(2026, Month::September, 1).unwrap()
        ));
    }

    #[test]
    fn business_days_in_a_full_ordinary_week_is_five() {
        let monday = Date::from_calendar_date(2026, Month::August, 31).unwrap();
        let sunday = Date::from_calendar_date(2026, Month::September, 6).unwrap();
        assert_eq!(french_business_days_in(monday, sunday), 5);
    }

    proptest! {
        #[test]
        fn business_days_never_exceeds_calendar_days(offset in 0i64..3650) {
            let start = Date::from_calendar_date(2026, Month::January, 1).unwrap();
            let end = start + Duration::days(offset);
            let calendar_days = offset + 1;
            let business_days = french_business_days_in(start, end);
            prop_assert!(i64::from(business_days) <= calendar_days);
        }

        #[test]
        fn reversed_range_has_zero_business_days(offset in 1i64..3650) {
            let start = Date::from_calendar_date(2026, Month::January, 1).unwrap();
            let end = start + Duration::days(offset);
            prop_assert_eq!(french_business_days_in(end, start), 0);
        }
    }
}
