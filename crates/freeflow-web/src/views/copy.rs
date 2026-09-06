//! Libellés de la lettre : français parlé, pas les sigles du cœur. Lot 48 : la fenêtre rédige ;
//! les queries du mât / du mois / de se payer arrivent au lot 49.

use freeflow_core::domain::{FiscalYearEnd, format_date_fr};
use freeflow_core::fiscal::FiscalDeadlineKind;
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
pub fn deadline_fr(kind: FiscalDeadlineKind) -> &'static str {
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
        MonthEventKind::StateDuty => "TVA",
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
        assert_eq!(deadline_fr(FiscalDeadlineKind::Ca3), "TVA du trimestre");
        assert_eq!(
            deadline_fr(FiscalDeadlineKind::VatInstalment),
            "acompte de TVA"
        );
        assert!(!deadline_fr(FiscalDeadlineKind::Ca12).contains("CA12"));
    }

    #[test]
    fn september_is_titled_like_the_mockup() {
        assert_eq!(month_title(9), "Septembre.");
    }
}
