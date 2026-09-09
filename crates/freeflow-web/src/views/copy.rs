//! Libellés de la lettre : français parlé, pas les sigles du cœur. Lot 48 : la fenêtre rédige ;
//! les queries du mât / du mois / de se payer arrivent au lot 49.

use freeflow_core::domain::{FiscalYearEnd, format_date_fr};
use freeflow_core::fiscal::{FiscalDeadlineKind, VatFilingScheme};
use time::{Date, Weekday};

#[must_use]
pub fn letter_date(date: Date) -> String {
    let weekday = match date.weekday() {
        Weekday::Monday => "Lundi",
        Weekday::Tuesday => "Mardi",
        Weekday::Wednesday => "Mercredi",
        Weekday::Thursday => "Jeudi",
        Weekday::Friday => "Vendredi",
        Weekday::Saturday => "Samedi",
        Weekday::Sunday => "Dimanche",
    };
    format!("{weekday} {}", format_date_fr(date))
}

#[must_use]
pub fn month_fr(month: u8) -> &'static str {
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

#[must_use]
pub fn year_end_fr(end: FiscalYearEnd) -> String {
    format!("{} {}", end.day(), month_fr(end.month()))
}

#[must_use]
pub fn deadline_fr(kind: FiscalDeadlineKind, scheme: VatFilingScheme) -> &'static str {
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

/// Libellé d'une occurrence, avec la période déclarée : « TVA du mois de septembre ».
#[must_use]
pub fn duty_occurrence_fr(
    kind: FiscalDeadlineKind,
    scheme: VatFilingScheme,
    period_key: &str,
    due_on: Date,
) -> String {
    match kind {
        FiscalDeadlineKind::Ca3 => match scheme {
            VatFilingScheme::Ca3Monthly => parse_year_month(period_key).map_or_else(
                || deadline_fr(kind, scheme).to_string(),
                |(_, month)| format!("TVA du mois {}", de_mois(month)),
            ),
            _ => parse_year_month(period_key).map_or_else(
                || deadline_fr(kind, scheme).to_string(),
                |(year, month)| {
                    let start = month_fr(month);
                    let end_month = month.saturating_add(2);
                    let (end_month, _) = if end_month > 12 {
                        (end_month - 12, year + 1)
                    } else {
                        (end_month, year)
                    };
                    format!("TVA de {start} à {}", month_fr(end_month))
                },
            ),
        },
        FiscalDeadlineKind::VatInstalment => {
            format!("acompte de TVA {}", de_mois(u8::from(due_on.month())))
        }
        FiscalDeadlineKind::Ca12 => format!("TVA de l'année {}", due_on.year()),
        FiscalDeadlineKind::IsAcompte => format!(
            "acompte d'impôt sur les sociétés {}",
            de_mois(u8::from(due_on.month()))
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

fn parse_year_month(key: &str) -> Option<(i32, u8)> {
    let (year, month) = key.split_once('-')?;
    if month.len() != 2 {
        return None;
    }
    Some((year.parse().ok()?, month.parse().ok()?))
}

#[must_use]
pub fn duty_href(kind: FiscalDeadlineKind, period: Option<&str>) -> String {
    let base = format!("/societe/impots/{}", kind.as_str().replace('_', "-"));
    match period {
        Some(p) if !p.is_empty() => format!("{base}/{p}"),
        _ => base,
    }
}

#[must_use]
pub fn is_vat(kind: FiscalDeadlineKind) -> bool {
    matches!(
        kind,
        FiscalDeadlineKind::Ca3 | FiscalDeadlineKind::VatInstalment | FiscalDeadlineKind::Ca12
    )
}

#[must_use]
pub fn month_title(month: u8) -> String {
    let name = month_fr(month);
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => format!("{}{}.", first.to_uppercase(), chars.as_str()),
        None => String::new(),
    }
}

#[must_use]
pub fn event_kind_fr(kind: freeflow_core::day::MonthEventKind) -> &'static str {
    use freeflow_core::day::MonthEventKind;
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

#[must_use]
pub fn gestes_title(n: usize) -> &'static str {
    match n {
        0 => "Rien aujourd'hui.",
        1 => "Un geste.",
        2 => "Deux gestes.",
        3 => "Trois gestes.",
        4 => "Quatre gestes.",
        _ => "Cinq gestes.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fifth_of_september_2026_is_a_saturday() {
        let date = time::macros::date!(2026 - 09 - 05);
        assert_eq!(letter_date(date), "Samedi 5 septembre 2026");
    }

    #[test]
    fn vat_deadlines_are_said_in_french() {
        assert_eq!(
            deadline_fr(FiscalDeadlineKind::Ca3, VatFilingScheme::Ca3Quarterly),
            "TVA du trimestre"
        );
        assert_eq!(
            deadline_fr(
                FiscalDeadlineKind::VatInstalment,
                VatFilingScheme::Simplified
            ),
            "acompte de TVA"
        );
        assert!(
            !deadline_fr(FiscalDeadlineKind::Ca12, VatFilingScheme::Simplified).contains("CA12")
        );
    }

    #[test]
    fn vat_occurrence_names_the_declared_month() {
        assert_eq!(
            duty_occurrence_fr(
                FiscalDeadlineKind::Ca3,
                VatFilingScheme::Ca3Monthly,
                "2026-09",
                time::macros::date!(2026 - 10 - 26)
            ),
            "TVA du mois de septembre"
        );
        assert_eq!(
            duty_occurrence_fr(
                FiscalDeadlineKind::Ca3,
                VatFilingScheme::Ca3Monthly,
                "2026-08",
                time::macros::date!(2026 - 09 - 24)
            ),
            "TVA du mois d'août"
        );
        assert_eq!(
            duty_occurrence_fr(
                FiscalDeadlineKind::Ca3,
                VatFilingScheme::Ca3Quarterly,
                "2026-07",
                time::macros::date!(2026 - 10 - 21)
            ),
            "TVA de juillet à septembre"
        );
        assert_eq!(
            duty_occurrence_fr(
                FiscalDeadlineKind::IsAcompte,
                VatFilingScheme::Ca3Monthly,
                "2026-09-15",
                time::macros::date!(2026 - 09 - 15)
            ),
            "acompte d'impôt sur les sociétés de septembre"
        );
    }

    #[test]
    fn vat_label_follows_the_filing_scheme() {
        assert_eq!(
            deadline_fr(FiscalDeadlineKind::Ca3, VatFilingScheme::Ca3Monthly),
            "TVA du mois"
        );
        assert_eq!(
            deadline_fr(FiscalDeadlineKind::Ca3, VatFilingScheme::Ca3Quarterly),
            "TVA du trimestre"
        );
        assert!(
            !deadline_fr(FiscalDeadlineKind::Ca3, VatFilingScheme::Ca3Monthly)
                .contains("trimestre")
        );
    }

    #[test]
    fn september_is_titled_like_the_mockup() {
        assert_eq!(month_title(9), "Septembre.");
    }

    #[test]
    fn a_state_duty_is_not_always_vat() {
        assert_eq!(
            event_kind_fr(freeflow_core::day::MonthEventKind::StateDuty),
            "État"
        );
    }

    #[test]
    fn an_is_instalment_is_said_like_la_societe() {
        assert_eq!(
            deadline_fr(FiscalDeadlineKind::IsAcompte, VatFilingScheme::Ca3Monthly),
            "acompte d'impôt sur les sociétés"
        );
    }
}
