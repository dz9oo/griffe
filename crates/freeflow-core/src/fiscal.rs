//! Échéances fiscales d'une SASU/EURL à l'IS, exercice aligné sur l'année civile — CA3 (TVA),
//! acomptes d'IS, CFE.
//!
//! **Ce module calcule des dates indicatives, pas une source de vérité fiscale.** Le régime de
//! TVA réel (mensuel, trimestriel, ou simplifié avec acomptes) dépend de choix déclarés par
//! l'entreprise, et la date exacte de télédéclaration dépend du dernier chiffre du SIREN — deux
//! informations que ce domaine ne porte pas. Le calcul CA3 ci-dessous suppose le cas le plus
//! courant (régime réel normal, déclaration mensuelle) avec une échéance approximée à 15 jours
//! après la fin du mois : à vérifier sur impots.gouv.fr, jamais à prendre pour une date légale
//! certaine. Les acomptes d'IS (15 mars/juin/septembre/décembre) et la CFE (15 décembre) sont en
//! revanche des dates fixées par le calendrier fiscal pour un exercice aligné sur l'année civile
//! — celles-là sont réellement stables.

use time::Date;

use crate::domain::Month;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FiscalDeadlineKind {
    /// Déclaration de TVA (CA3) du mois précédent — approximative, voir la documentation du
    /// module.
    Ca3,
    /// Acompte d'impôt sur les sociétés.
    IsAcompte,
    /// Cotisation foncière des entreprises.
    Cfe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FiscalDeadline {
    pub kind: FiscalDeadlineKind,
    /// Toujours `>= today` : ce sont les *prochaines* échéances, jamais des échéances passées.
    pub due_on: Date,
}

fn nth_of_month(month: Month, day: u8) -> Date {
    Date::from_calendar_date(
        month.year(),
        time::Month::try_from(month.month()).expect("le mois est garanti valide"),
        day,
    )
    .expect("les jours utilisés ici (15) sont valides dans tous les mois")
}

/// Prochaine échéance CA3 à partir de `today` : le 15 du mois courant (qui déclare l'activité du
/// mois précédent) si cette date n'est pas encore passée, sinon le 15 du mois suivant.
fn next_ca3(today: Date) -> Date {
    let this_month =
        Month::new(today.year(), u8::from(today.month())).expect("mois courant valide");
    let candidate = nth_of_month(this_month, 15);
    if today <= candidate {
        candidate
    } else {
        nth_of_month(this_month.succ(), 15)
    }
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

/// Les trois prochaines échéances (une par type), triées par date.
#[must_use]
pub fn upcoming_deadlines(today: Date) -> Vec<FiscalDeadline> {
    let mut deadlines = vec![
        FiscalDeadline {
            kind: FiscalDeadlineKind::Ca3,
            due_on: next_ca3(today),
        },
        FiscalDeadline {
            kind: FiscalDeadlineKind::IsAcompte,
            due_on: next_is_acompte(today),
        },
        FiscalDeadline {
            kind: FiscalDeadlineKind::Cfe,
            due_on: next_cfe(today),
        },
    ];
    deadlines.sort_by_key(|d| d.due_on);
    deadlines
}

/// Échéances dans les `within_days` prochains jours (relance) — la CLI/GUI l'utilise pour
/// alerter avant J plutôt qu'après.
#[must_use]
pub fn deadlines_due_within(today: Date, within_days: i64) -> Vec<FiscalDeadline> {
    upcoming_deadlines(today)
        .into_iter()
        .filter(|d| (d.due_on - today).whole_days() <= within_days)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::Month as TimeMonth;

    fn date(year: i32, month: TimeMonth, day: u8) -> Date {
        Date::from_calendar_date(year, month, day).unwrap()
    }

    #[test]
    fn ca3_rolls_over_to_the_following_month_once_the_15th_has_passed() {
        assert_eq!(
            next_ca3(date(2026, TimeMonth::September, 1)),
            date(2026, TimeMonth::September, 15),
            "avant le 15, l'échéance du mois courant (déclarant août) est encore à venir"
        );
        assert_eq!(
            next_ca3(date(2026, TimeMonth::September, 16)),
            date(2026, TimeMonth::October, 15),
            "après le 15, on saute à l'échéance du mois suivant"
        );
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
    fn upcoming_deadlines_are_sorted_chronologically() {
        let deadlines = upcoming_deadlines(date(2026, TimeMonth::September, 1));
        for pair in deadlines.windows(2) {
            assert!(pair[0].due_on <= pair[1].due_on);
        }
    }

    #[test]
    fn deadlines_due_within_excludes_far_future_ones() {
        // Depuis le 1er septembre 2026 : CA3 et acompte d'IS tombent tous deux le 15 sept.
        // (14j, inclus), la CFE le 15 déc. (loin, exclue) — un filtre à 20 jours ne garde que
        // les deux premières.
        let today = date(2026, TimeMonth::September, 1);
        let soon = deadlines_due_within(today, 20);
        let kinds: Vec<_> = soon.iter().map(|d| d.kind).collect();
        assert_eq!(
            kinds,
            vec![FiscalDeadlineKind::Ca3, FiscalDeadlineKind::IsAcompte]
        );
    }
}
