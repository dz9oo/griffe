//! Rendu réel des documents de clôture par le vrai binaire `typst` (disponible dans le devShell
//! Nix, comme pour `griffe-invoice`) — pas de mock : on vérifie qu'un PDF sort bien, y
//! compris avec des données hostiles au balisage Typst (guillemets, `#`, crochets) dans le nom
//! de la société.

use griffe_core::company::CompanyProfile;
use griffe_core::domain::{Address, FiscalYear, FiscalYearId, Money, Siren};
use griffe_core::fiscal_year::FiscalYearRecord;
use griffe_core::ledger::{Ledger, LedgerFacts, LiabilityRubric, OpeningLines};
use griffe_docs::{
    liasse_export, render_appropriation_decision, render_approval_minutes, render_balance_sheet,
    render_efi_notice, render_inventory, render_synthesis,
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
        president_name: None,
        sole_shareholder_name: None,
        sole_shareholder_address: None,
        share_count: None,
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
        depreciation: Money::ZERO,
        result_before_tax: Money::from_cents(537_500),
        corporate_tax: Money::from_cents(80_625),
        net_result: Money::from_cents(456_875),
        legal_reserve: Money::from_cents(5_000),
        dividends: Money::from_cents(100_000),
        retained_earnings: Money::from_cents(351_875),
        approved_on: approved.then(|| date(2027, Month::May, 15)),
        revision: 1,
        created_at: OffsetDateTime::UNIX_EPOCH,
        losses_imputed: Money::ZERO,
        carried_back: Money::ZERO,
        carry_back_credit: Money::ZERO,
        losses_carried_forward: Money::ZERO,
        non_deductible_expenses: Money::ZERO,
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
    let result = year.accounting_result();

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
        .find(|e| e.form == "2033-B" && e.case == "306")
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
        assets: &[],
        bank_transactions: &[],
        opening: Some(OpeningLines::from_opening_balance(
            &griffe_core::domain::OpeningBalance {
                opens_on: date(2026, Month::January, 1),
                source: None,
                lines: vec![
                    "101000:Capital social:C:1000.00".parse().unwrap(),
                    "455000:Compte courant d'associé:C:200.00".parse().unwrap(),
                    "512000:Banque:D:1200.00".parse().unwrap(),
                ],
                tax_losses: Money::ZERO,
            },
        )),
        snapshot: None,
        prior_losses: griffe_core::domain::Money::ZERO,
        appropriations: &[],
    })
    .unwrap()
}

#[test]
fn the_balance_sheet_renders_to_pdf_and_feeds_the_2033a_cases_of_the_liasse() {
    let profile = profile("Argon \"#quote\" Digital");
    let ledger = ledger(&profile);
    let sheet = ledger.balance_sheet();
    let pdf = render_balance_sheet(&profile, &sheet, &ledger.trial_balance()).unwrap();
    assert_is_pdf(&pdf, "bilan");
    let inventory = render_inventory(&profile, &ledger.trial_balance()).unwrap();
    assert_is_pdf(&inventory, "inventaire");
    let notice =
        render_efi_notice(&liasse_export(&profile, &record(false), Some(&ledger))).unwrap();
    assert_is_pdf(&notice, "notice EFI");

    let export = liasse_export(&profile, &record(false), Some(&ledger));
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

// --- Lot 41 : cases vérifiées de la 2033-B, composition du capital, PV nominatif -------------

/// Le texte d'un PDF via `pdftotext` (poppler), ou `None` si l'outil manque sur la machine —
/// le test dit alors ce qu'il n'a pas pu vérifier plutôt que d'échouer sur l'environnement.
fn pdf_text(pdf: &[u8], tag: &str) -> Option<String> {
    let dir = std::env::temp_dir().join(format!("griffe-docs-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("doc.pdf");
    std::fs::write(&path, pdf).unwrap();
    let output = match std::process::Command::new("pdftotext")
        .arg("-layout")
        .arg(&path)
        .arg("-")
        .output()
    {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("pdftotext absent : contenu du PDF non vérifié ({tag})");
            return None;
        }
        Err(e) => panic!("pdftotext : {e}"),
    };
    assert!(output.status.success(), "pdftotext a échoué sur {tag}");
    Some(String::from_utf8_lossy(&output.stdout).replace('\u{a0}', " "))
}

fn named_profile() -> CompanyProfile {
    let mut p = profile("Lumen Conseil");
    p.director_monthly_gross = Some(Money::from_cents(300_000));
    p.director_charge_ratio_bps = Some(2_000);
    p.president_name = Some("Nora Lumen".to_string());
    p.sole_shareholder_name = Some("Nora Lumen".to_string());
    p.sole_shareholder_address = Some("4 allée des Tilleuls, 69003 Lyon".to_string());
    p.share_count = Some(100);
    p
}

#[test]
fn the_liasse_uses_the_verified_2033b_lines_and_describes_the_capital() {
    let named = named_profile();
    // Coût employeur figé de 43 200 € = 36 000 € brut (profil) + 7 200 € de cotisations.
    let year = FiscalYearRecord {
        director_remuneration: Money::from_cents(4_320_000),
        depreciation: Money::ZERO,
        non_deductible_expenses: Money::from_cents(15_000),
        ..record(true)
    };
    let export = liasse_export(&named, &year, None);
    let case = |form: &str, c: &str| {
        export
            .entries
            .iter()
            .find(|e| e.form == form && e.case == c)
            .map(|e| e.amount_cents)
    };
    assert_eq!(
        case("2033-B", "218"),
        Some(617_500),
        "production vendue de services"
    );
    assert_eq!(
        case("2033-B", "210"),
        None,
        "210 est la vente de marchandises"
    );
    assert_eq!(case("2033-B", "250"), Some(3_600_000), "salaires bruts");
    assert_eq!(case("2033-B", "252"), Some(720_000), "charges sociales");
    assert_eq!(case("2033-B", "254"), Some(0), "pas de dotation sur ce jeu");
    assert_eq!(case("2033-B", "306"), Some(80_625), "IS");
    assert_eq!(
        case("2033-B", "310"),
        Some(456_875),
        "résultat comptable après IS"
    );
    assert_eq!(case("2065", "IS"), None, "plus de pseudo-case");
    assert_eq!(case("2065", "NET"), None, "plus de pseudo-case");
    // Résultat fiscal = 5 375 € + 150 € réintégrés.
    assert_eq!(case("2065", "C1"), Some(552_500));
    assert_eq!(case("2065", "distributions"), Some(100_000));

    let capital = export
        .capital
        .as_ref()
        .expect("2033-F dès que l'associé est nommé");
    assert_eq!(capital.form, "2033-F");
    assert_eq!(capital.legal_entity_shareholders, 0);
    assert_eq!(capital.individual_shareholders, 1);
    assert_eq!(capital.shares_held_by_individuals, Some(100));
    assert_eq!(capital.holders.len(), 1);
    assert_eq!(capital.holders[0].name, "Nora Lumen");
    assert_eq!(capital.holders[0].percent_bps, 10_000);
    assert_eq!(
        capital.holders[0].address.as_deref(),
        Some("4 allée des Tilleuls, 69003 Lyon")
    );

    // Sans associé nommé : pas de 2033-F, et tout le coût du dirigeant en 250.
    let anonymous = profile("Argon Digital");
    let export = liasse_export(&anonymous, &year, None);
    assert!(export.capital.is_none());
    let c250 = export
        .entries
        .iter()
        .find(|e| e.form == "2033-B" && e.case == "250")
        .unwrap();
    assert_eq!(c250.amount_cents, 4_320_000);
    let c252 = export
        .entries
        .iter()
        .find(|e| e.form == "2033-B" && e.case == "252")
        .unwrap();
    assert_eq!(
        c252.amount_cents, 0,
        "présente à zéro comme les autres cases fixes"
    );
}

#[test]
fn the_minutes_name_the_shareholder_and_carry_the_legal_mentions() {
    let named = named_profile();
    let year = FiscalYearRecord {
        non_deductible_expenses: Money::from_cents(15_000),
        ..record(true)
    };
    let pdf = render_approval_minutes(&named, &year, date(2027, Month::June, 1)).unwrap();
    assert_is_pdf(&pdf, "PV nominatif");
    if let Some(text) = pdf_text(&pdf, "minutes") {
        let squeezed: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
        for expected in [
            "Nora Lumen",
            "4 allée des Tilleuls",
            "223 quater",
            "150,00 €",
            "L227-10",
            "L232-1 IV",
            "registre des décisions",
            "L227-9 al. 3",
            "bénéfice net de 4 568,75 €",
            "Dotation à la réserve légale",
        ] {
            assert!(
                squeezed.contains(expected),
                "« {expected} » absent de :\n{squeezed}"
            );
        }
        assert!(
            !squeezed.contains("________"),
            "aucun blanc quand le profil est complet"
        );
    }

    // Une perte : pas de tableau d'affectation, la perte va au report à nouveau ; et sans
    // nom au profil, des blancs à compléter.
    let loss = FiscalYearRecord {
        result_before_tax: Money::from_cents(-81_600),
        corporate_tax: Money::ZERO,
        net_result: Money::from_cents(-81_600),
        legal_reserve: Money::ZERO,
        dividends: Money::ZERO,
        retained_earnings: Money::from_cents(-81_600),
        non_deductible_expenses: Money::ZERO,
        ..record(false)
    };
    let pdf = render_approval_minutes(&profile("Argon Digital"), &loss, date(2027, Month::June, 1))
        .unwrap();
    if let Some(text) = pdf_text(&pdf, "minutes-loss") {
        let squeezed: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(squeezed.contains("perte de 816,00 €"), "{squeezed}");
        assert!(
            squeezed.contains("en totalité au compte « report à nouveau »"),
            "{squeezed}"
        );
        assert!(
            !squeezed.contains("Distribution de dividendes"),
            "{squeezed}"
        );
        assert!(squeezed.contains("aucune dépense ni charge"), "{squeezed}");
        assert!(squeezed.contains("________"), "{squeezed}");
        assert!(!squeezed.contains("L227-9 al. 3"), "{squeezed}");
    }
    let decision = render_appropriation_decision(&profile("Argon Digital"), &loss).unwrap();
    if let Some(text) = pdf_text(&decision, "appropriation-loss") {
        assert!(!text.contains("Total distribuable"), "{text}");
        assert!(text.contains("Solde à reporter"), "{text}");
    }
}
