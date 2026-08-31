//! Calendrier fiscal et social d'une SASU à l'IS — **dates indicatives**, désormais assorties de
//! **montants** là où le domaine peut les calculer (lot 19).
//!
//! **Ce module reste une aide au pilotage, pas une source de vérité fiscale.** Les dates dérivent
//! de la date de clôture d'exercice déclarée dans le profil ([`crate::company::CompanyProfile`]) ;
//! à défaut, l'année civile est supposée. La date exacte de télédéclaration CA3 dépend du dernier
//! chiffre du SIREN (non modélisé) ; les échéances de la liasse, du dépôt des comptes et de l'AG
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
use crate::domain::{FiscalYearEnd, Money, Month, VatRegime};

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
/// alertes qui n'ouvrent pas la base.
#[must_use]
pub fn upcoming_deadlines(today: Date) -> Vec<FiscalDeadline> {
    let mut deadlines = vec![
        bare(FiscalDeadlineKind::Ca3, next_ca3(today)),
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

    // --- TVA (CA3) : montant = TVA du mois précédent (régime mensuel). ---
    if matches!(
        vat_regime,
        None | Some(VatRegime::RealNormalMonthly | VatRegime::RealNormalQuarterly)
    ) {
        let due = next_ca3(today);
        // Le mois déclaré est celui qui précède l'échéance.
        let declared = Month::new(due.year(), u8::from(due.month()))
            .unwrap_or_else(|_| Month::new(due.year(), 1).unwrap())
            .pred();
        let vat = vat_due_for_period(conn, declared.first_day(), declared.last_day())?;
        deadlines.push(FiscalDeadline {
            kind: FiscalDeadlineKind::Ca3,
            due_on: due,
            amount: Some(vat.due),
            note: Some(
                "indicative — date exacte selon le dernier chiffre du SIREN ; TVA du mois \
                 précédent"
                    .to_string(),
            ),
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
    use crate::domain::{Address, FiscalYearEnd, InvoiceLine, Siren, VatRate, VatRegime};
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

    #[test]
    fn the_calendar_prices_the_is_balance_of_the_closed_exercise() {
        let dir = std::env::temp_dir().join(format!(
            "freeflow-calendar-{}-{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ));
        let mut store = Store::create(&dir.join("vault.db"), &Passphrase::from("s3cret")).unwrap();
        let client_id = crate::domain::ClientId::new();
        store
            .connection()
            .execute(
                "INSERT INTO clients (id, name, created_at) \
                 VALUES (?1, 'Argon Digital', '2025-01-01T00:00:00Z')",
                [client_id.to_string()],
            )
            .unwrap();
        let human = ExecutionContext::new(Actor::Human, false);
        Executor::new(&mut store)
            .execute(&calendar_profile(), &human)
            .unwrap();
        // Facture de l'exercice CLOS (2025) : 6 175 € HT → IS 15 % = 926,25 €.
        Executor::new(&mut store)
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
                    issued_on: date(2025, TimeMonth::June, 1),
                    payment_terms_days: 30,
                },
                &human,
            )
            .unwrap();

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
        // Une échéance CA3 chiffrée est présente.
        assert!(
            calendar
                .iter()
                .any(|d| d.kind == FiscalDeadlineKind::Ca3 && d.amount.is_some())
        );
    }
}
