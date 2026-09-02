//! Rendu réel des documents de clôture par le vrai binaire `typst` (disponible dans le devShell
//! Nix, comme pour `freeflow-invoice`) — pas de mock : on vérifie qu'un PDF sort bien, y
//! compris avec des données hostiles au balisage Typst (guillemets, `#`, crochets) dans le nom
//! de la société.

use freeflow_core::accounting::AccountingResult;
use freeflow_core::company::CompanyProfile;
use freeflow_core::domain::{Address, FiscalYear, FiscalYearId, Money, Siren};
use freeflow_core::fiscal_year::FiscalYearRecord;
use freeflow_core::ledger::{Ledger, LedgerFacts, LiabilityRubric, OpeningLines};
use freeflow_docs::{
    liasse_export, render_appropriation_decision, render_approval_minutes, render_balance_sheet,
    render_synthesis,
};
use time::{Date, Month, OffsetDateTime};

fn date(y: i32, m: Month, d: u8) -> Date {
    Date::from_calendar_date(y, m, d).unwrap()
}

fn profile(name: &str) -> CompanyProfile {
    CompanyProfile {
        name: name.to_string(),
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
        fiscal_year_end: None,
        vat_regime: None,
        director_monthly_gross: None,
        director_charge_ratio_bps: None,
    }
}

fn record(approved: bool) -> FiscalYearRecord {
    FiscalYearRecord {
        id: FiscalYearId::new(),
        starts_on: date(2026, Month::January, 1),
        ends_on: date(2026, Month::December, 31),
        revenue_ht: Money::from_cents(617_500),
        expenses: Money::from_cents(80_000),
        director_remuneration: Money::ZERO,
        result_before_tax: Money::from_cents(537_500),
        corporate_tax: Money::from_cents(80_625),
        net_result: Money::from_cents(456_875),
        legal_reserve: Money::from_cents(5_000),
        dividends: Money::from_cents(100_000),
        retained_earnings: Money::from_cents(351_875),
        approved_on: approved.then(|| date(2027, Month::May, 15)),
        revision: 1,
        created_at: OffsetDateTime::UNIX_EPOCH,
    }
}

fn assert_is_pdf(bytes: &[u8], label: &str) {
    assert!(
        bytes.starts_with(b"%PDF-"),
        "{label} ne commence pas par un en-tête PDF"
    );
    assert!(bytes.len() > 1_000, "{label} suspicieusement petit");
}

#[test]
fn all_three_documents_render_to_pdf() {
    let profile = profile("Argon Digital");
    let year = record(true);
    let result = AccountingResult {
        period: FiscalYear::new(year.starts_on, year.ends_on),
        revenue_ht: year.revenue_ht,
        expenses: year.expenses,
        director_remuneration: year.director_remuneration,
        result_before_tax: year.result_before_tax,
        corporate_tax: year.corporate_tax,
        net_result: year.net_result,
    };

    let minutes = render_approval_minutes(&profile, &year, date(2027, Month::May, 20)).unwrap();
    assert_is_pdf(&minutes, "PV d'approbation");

    let decision = render_appropriation_decision(&profile, &year).unwrap();
    assert_is_pdf(&decision, "décision d'affectation");

    let prior = FiscalYearRecord {
        starts_on: date(2025, Month::January, 1),
        ends_on: date(2025, Month::December, 31),
        ..record(true)
    };
    let with_prior = render_synthesis(&profile, &result, Some(&prior)).unwrap();
    assert_is_pdf(&with_prior, "synthèse avec N-1");

    let without_prior = render_synthesis(&profile, &result, None).unwrap();
    assert_is_pdf(&without_prior, "synthèse sans N-1");
}

#[test]
fn hostile_company_name_does_not_break_typst_markup() {
    // Le nom contient tout ce qui casserait une interpolation naïve dans du balisage Typst.
    let profile = profile("ACME \"#emph[pwn]\" \\ [x] & Co");
    let year = record(false);
    let minutes = render_approval_minutes(&profile, &year, date(2027, Month::May, 20)).unwrap();
    assert_is_pdf(&minutes, "PV avec nom hostile");
}

#[test]
fn liasse_export_serializes_with_the_expected_cases() {
    let profile = profile("Argon Digital");
    let year = record(true);
    let export = liasse_export(&profile, &year, None);

    assert!(export.approved);
    assert_eq!(export.siren, "552100554");
    let is_entry = export
        .entries
        .iter()
        .find(|e| e.form == "2065" && e.case == "IS")
        .unwrap();
    assert_eq!(is_entry.amount_cents, 80_625);

    let json = serde_json::to_string_pretty(&export).unwrap();
    assert!(json.contains("2033-B"));
    assert!(json.contains("EDI-TDFC"));
}

/// Un grand livre minimal : bilan d'ouverture (capital contre banque) et rien d'autre — le
/// rendu ne dépend d'aucune base.
fn ledger(profile: &CompanyProfile) -> Ledger {
    Ledger::build(LedgerFacts {
        profile,
        exercise: FiscalYear::calendar(2026),
        invoices: &[],
        clients: &[],
        payments: &[],
        expenses: &[],
        opening: Some(OpeningLines::from_opening_balance(
            &freeflow_core::domain::OpeningBalance {
                opens_on: date(2026, Month::January, 1),
                source: None,
                lines: vec![
                    "101000:Capital social:C:1000.00".parse().unwrap(),
                    "455000:Compte courant d'associé:C:200.00".parse().unwrap(),
                    "512000:Banque:D:1200.00".parse().unwrap(),
                ],
            },
        )),
        snapshot: None,
        appropriations: &[],
    })
}

#[test]
fn the_balance_sheet_renders_to_pdf_and_feeds_the_2033a_cases_of_the_liasse() {
    let profile = profile("Argon \"#quote\" Digital");
    let ledger = ledger(&profile);
    let sheet = ledger.balance_sheet();
    let pdf = render_balance_sheet(&profile, &sheet, &ledger.trial_balance()).unwrap();
    assert_is_pdf(&pdf, "bilan");

    let export = liasse_export(&profile, &record(false), Some(&sheet));
    let case = |c: &str| {
        export
            .entries
            .iter()
            .find(|e| e.form == "2033-A" && e.case == c)
            .map(|e| e.amount_cents)
    };
    assert_eq!(case("084"), Some(120_000), "disponibilités");
    assert_eq!(case("120"), Some(100_000), "capital");
    assert_eq!(case("172"), Some(20_000), "autres dettes");
    assert_eq!(
        case("169"),
        Some(20_000),
        "dont comptes courants d'associés"
    );
    assert_eq!(case("112"), Some(120_000));
    assert_eq!(case("180"), Some(120_000));
    assert_eq!(case("136"), None, "résultat nul : case omise");
    assert_eq!(sheet.liability(LiabilityRubric::Result), Money::ZERO);
    assert!(export.note.contains("2033-A"));
}
