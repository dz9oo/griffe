//! Rendu réel des documents de clôture par le vrai binaire `typst` (disponible dans le devShell
//! Nix, comme pour `griffe-invoice`) — pas de mock : on vérifie qu'un PDF sort bien, y
//! compris avec des données hostiles au balisage Typst (guillemets, `#`, crochets) dans le nom
//! de la société.

use griffe_core::accounting::round_to_euro;
use griffe_core::company::CompanyProfile;
use griffe_core::domain::{Address, FiscalYear, FiscalYearId, Money, Siren};
use griffe_core::fiscal_year::FiscalYearRecord;
use griffe_core::ledger::{
    Account, Journal, Ledger, LedgerEntry, LedgerFacts, LedgerLine, LiabilityRubric, OpeningLines,
    accounts,
};
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
    // Sans livre, le snapshot ne remplit plus les cases du compte de résultat.
    assert!(
        export.entries.iter().all(|e| e.form != "2033-B"
            || !matches!(
                e.case,
                "218" | "242" | "244" | "250" | "252" | "254" | "306" | "310" | "230"
            )),
        "le chiffre d'affaires, les charges et l'impôt du snapshot ne sont pas des cases"
    );
    assert!(
        export.discrepancies.is_empty(),
        "sans livre, aucun écart à lister"
    );
    // C1 reste le résultat fiscal du snapshot : 5 375,00 €.
    let c1 = export
        .entries
        .iter()
        .find(|e| e.form == "2065" && e.case == "C1")
        .unwrap();
    // 5 375,00 € de résultat fiscal : les centimes restent, l'euro recopié est 5 375.
    assert_eq!(c1.amount_cents, 537_500);
    assert_eq!(c1.amount_euros, 5_375);
    assert!(
        export.entries.iter().all(|e| e.case != "distributions"),
        "distributions n'est pas une case"
    );
    assert_sasu_nil_forms(&export);
    assert!(export.note.contains("On recopie, rien"));
    assert!(export.note.contains("télétransmis"));
    assert!(export.note.contains("les euros"));
    assert!(export.note.contains("centimes du livre"));
    assert!(export.note.contains("néant"));

    let json = serde_json::to_string_pretty(&export).unwrap();
    assert!(json.contains("2033-B"));
    assert!(json.contains("rien n'est télétransmis") || json.contains("rien n’est télétransmis"));
    assert!(json.contains("2033-E"));
    assert!(json.contains("2033-G"));
    assert!(json.contains("néant"));
    assert!(json.contains("amount_euros"));
    assert!(json.contains("discrepancies"));
    assert_eq!(
        serde_json::to_value(export.mentions).unwrap(),
        serde_json::json!([
            {"form": "2033-E", "text": "néant"},
            {"form": "2033-G", "text": "néant"}
        ])
    );
}

/// Un grand livre minimal : bilan d'ouverture (capital contre banque) et rien d'autre — le
/// rendu ne dépend d'aucune base.
fn ledger(profile: &CompanyProfile) -> Ledger {
    Ledger::build(LedgerFacts {
        profile,
        exercise: FiscalYear::calendar(2026),
        invoices: &[],
        write_offs: &[],
        clients: &[],
        payments: &[],
        expenses: &[],
        posted: &[],
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
        None,
        "sans livre, le chiffre d'affaires du snapshot ne remplit pas la case 218"
    );
    assert_eq!(
        case("2033-B", "210"),
        None,
        "210 est la vente de marchandises"
    );
    assert_eq!(
        case("2033-B", "250"),
        None,
        "sans 641, le brut du profil n'est pas un salaire"
    );
    assert_eq!(
        case("2033-B", "252"),
        None,
        "sans 645, les cotisations du profil ne sont pas une case"
    );
    assert_eq!(
        case("2033-B", "254"),
        None,
        "une dotation à zéro est absente"
    );
    assert_eq!(
        case("2033-B", "306"),
        None,
        "sans 695, l'impôt du snapshot n'est pas la case 306"
    );
    assert_eq!(
        case("2033-B", "310"),
        None,
        "sans livre, le résultat du snapshot n'est pas la case 310"
    );
    assert_eq!(case("2065", "IS"), None, "plus de pseudo-case");
    assert_eq!(case("2065", "NET"), None, "plus de pseudo-case");
    // Résultat fiscal = 5 375 € + 150 € réintégrés. Ce n'est pas un compte du livre.
    // Les centimes du snapshot restent ; l'euro recopié est 5 525.
    let c1 = export
        .entries
        .iter()
        .find(|e| e.form == "2065" && e.case == "C1")
        .unwrap();
    assert_eq!(c1.amount_cents, 552_500);
    assert_eq!(c1.amount_euros, 5_525);
    assert_eq!(case("2065", "C1"), Some(552_500));
    assert_eq!(
        case("2065", "distributions"),
        None,
        "distributions n'est pas une case du 2065"
    );
    let decision = render_appropriation_decision(&named, &year).unwrap();
    if let Some(text) = pdf_text(&decision, "appropriation-dividends") {
        let squeezed: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            squeezed.contains("Distribution de dividendes"),
            "les dividendes restent sur l'affectation : {squeezed}"
        );
        assert!(
            squeezed.contains("1 000,00"),
            "1 000,00 € de dividendes sur l'affectation : {squeezed}"
        );
    }

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

    // Sans associé nommé : pas de 2033-F. Le coût du dirigeant ne devient pas la case 250.
    let anonymous = profile("Argon Digital");
    let export = liasse_export(&anonymous, &year, None);
    assert!(export.capital.is_none());
    assert!(
        export
            .entries
            .iter()
            .all(|e| e.case != "250" && e.case != "252"),
        "nommer ou non l'associé ne poste pas un 641"
    );
}

fn cents_line(account: Account, cents: i64) -> LedgerLine {
    LedgerLine {
        account,
        aux: None,
        amount: Money::from_cents(cents),
        line_label: None,
    }
}

fn book(lines: Vec<LedgerLine>) -> Ledger {
    let on = date(2026, Month::June, 30);
    Ledger {
        exercise: FiscalYear::calendar(2026),
        entries: if lines.is_empty() {
            Vec::new()
        } else {
            vec![LedgerEntry {
                journal: Journal::Misc,
                number: 1,
                date: on,
                piece_ref: "OD-TEST".to_string(),
                piece_date: on,
                label: "Écriture de test".to_string(),
                lines,
            }]
        },
    }
}

fn case_amount(export: &griffe_docs::LiasseExport, form: &str, case_id: &str) -> Option<i64> {
    export
        .entries
        .iter()
        .find(|e| e.form == form && e.case == case_id)
        .map(|e| e.amount_cents)
}

fn case_entry<'a>(
    export: &'a griffe_docs::LiasseExport,
    form: &str,
    case_id: &str,
) -> Option<&'a griffe_docs::LiasseEntry> {
    export
        .entries
        .iter()
        .find(|e| e.form == form && e.case == case_id)
}

/// 2033-E et 2033-G, toujours néant : pas de case chiffrée, même si un 641 existe.
fn assert_sasu_nil_forms(export: &griffe_docs::LiasseExport) {
    assert_eq!(export.mentions[0].form, "2033-E");
    assert_eq!(export.mentions[0].text, "néant");
    assert_eq!(export.mentions[1].form, "2033-G");
    assert_eq!(export.mentions[1].text, "néant");
    assert!(
        export
            .entries
            .iter()
            .all(|e| e.form != "2033-E" && e.form != "2033-G"),
        "néant n'est pas une case d'effectif, de valeur ajoutée ou de filiale"
    );
}

#[test]
fn a_profile_gross_does_not_fill_salaries_when_the_book_has_none() {
    let mut named = named_profile();
    named.director_monthly_gross = Some(Money::from_cents(100_000));
    // Snapshot : 800,00 € de chiffre d'affaires, 12 000,00 € de coût dirigeant.
    // Le livre n'a ni 706, ni 641, ni 645.
    let year = FiscalYearRecord {
        revenue_ht: Money::from_cents(80_000),
        director_remuneration: Money::from_cents(1_200_000),
        ..record(true)
    };
    let export = liasse_export(&named, &year, Some(&ledger(&named)));
    assert_eq!(case_amount(&export, "2033-B", "218"), None);
    assert_eq!(case_amount(&export, "2033-B", "250"), None);
    assert_eq!(case_amount(&export, "2033-B", "252"), None);
    assert_eq!(case_amount(&export, "2033-B", "210"), None);
}

#[test]
fn a_credit_of_706_is_case_218_and_the_case_is_absent_without_it() {
    let profile = profile("Argon Digital");
    let year = record(true);
    let with_revenue = book(vec![
        cents_line(accounts::SERVICES, -100_000),
        cents_line(accounts::BANK, 100_000),
    ]);
    let export = liasse_export(&profile, &year, Some(&with_revenue));
    assert_eq!(case_amount(&export, "2033-B", "218"), Some(100_000));
    assert_eq!(case_amount(&export, "2033-B", "210"), None);

    let without = book(vec![]);
    let export = liasse_export(&profile, &year, Some(&without));
    assert_eq!(case_amount(&export, "2033-B", "218"), None);
}

#[test]
fn the_case_keeps_the_book_and_the_snapshot_gap_stays_in_the_list() {
    let profile = profile("Argon Digital");
    // Snapshot 500,00 € de chiffre d'affaires. Livre 100,00 € au 706.
    let year = FiscalYearRecord {
        revenue_ht: Money::from_cents(50_000),
        expenses: Money::ZERO,
        director_remuneration: Money::ZERO,
        depreciation: Money::ZERO,
        result_before_tax: Money::from_cents(50_000),
        corporate_tax: Money::ZERO,
        net_result: Money::from_cents(50_000),
        legal_reserve: Money::ZERO,
        dividends: Money::ZERO,
        retained_earnings: Money::from_cents(50_000),
        non_deductible_expenses: Money::ZERO,
        ..record(false)
    };
    let ledger = book(vec![
        cents_line(accounts::SERVICES, -10_000),
        cents_line(accounts::BANK, 10_000),
    ]);
    let export = liasse_export(&profile, &year, Some(&ledger));
    assert_eq!(case_amount(&export, "2033-B", "218"), Some(10_000));
    let gap = export
        .discrepancies
        .iter()
        .find(|gap| gap.subject == "chiffre d'affaires")
        .expect("l'écart de chiffre d'affaires est dans la liste");
    assert_eq!(gap.snapshot_cents, 50_000);
    assert_eq!(gap.ledger_cents, 10_000);
    let gap_json = serde_json::to_value(gap).unwrap();
    assert!(gap_json.get("amount_euros").is_none());
    assert_eq!(gap_json["snapshot_cents"], 50_000);
    assert_eq!(gap_json["ledger_cents"], 10_000);
    assert!(
        export
            .entries
            .iter()
            .all(|e| e.amount_cents != 50_000 || e.case == "C1"),
        "l'écart n'est pas recopié dans la case de production"
    );

    let notice = render_efi_notice(&export).unwrap();
    assert_is_pdf(&notice, "notice EFI avec écart");
    if let Some(text) = pdf_text(&notice, "efi-gap") {
        let squeezed: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            squeezed.contains("chiffre d'affaires"),
            "la notice qui liste les cases montre l'écart : {squeezed}"
        );
        assert!(squeezed.contains("500,00"), "{squeezed}");
        assert!(squeezed.contains("100,00"), "{squeezed}");
    }
}

#[test]
fn income_statement_cases_read_the_book_accounts() {
    let profile = profile("Argon Digital");
    // Le snapshot dit autre chose. Les cases suivent le livre.
    // 706 crédit 1 000,00 ; 758 crédit 3,26 ; 622600 débit 200,00 ; 635 débit 50,00 ;
    // 641 débit 300,00 ; 645 débit 80,00 ; 681 débit 40,00 ; 695 débit 30,00.
    // Produits 1 003,26 − charges 700,00 = résultat 303,26.
    let year = FiscalYearRecord {
        revenue_ht: Money::from_cents(1),
        expenses: Money::from_cents(999_999),
        director_remuneration: Money::from_cents(1),
        depreciation: Money::from_cents(1),
        result_before_tax: Money::from_cents(1),
        corporate_tax: Money::from_cents(1),
        net_result: Money::from_cents(1),
        ..record(false)
    };
    let ledger = book(vec![
        cents_line(accounts::SERVICES, -100_000),
        cents_line(accounts::SUNDRY_INCOME, -326),
        cents_line(accounts::FEES, 20_000),
        cents_line(accounts::TAXES, 5_000),
        cents_line(accounts::DIRECTOR_PAY, 30_000),
        cents_line(accounts::SOCIAL_CHARGES, 8_000),
        cents_line(accounts::DEPRECIATION, 4_000),
        cents_line(accounts::CORPORATE_TAX, 3_000),
        cents_line(accounts::BANK, 30_326),
    ]);
    let export = liasse_export(&profile, &year, Some(&ledger));
    assert_eq!(case_amount(&export, "2033-B", "218"), Some(100_000));
    assert_eq!(case_amount(&export, "2033-B", "230"), Some(326));
    assert_eq!(case_amount(&export, "2033-B", "242"), Some(20_000));
    assert_eq!(case_amount(&export, "2033-B", "244"), Some(5_000));
    assert_eq!(case_amount(&export, "2033-B", "250"), Some(30_000));
    // Un 641 au livre ne remplit pas le 2033-E : pas d'effectif, pas de CVAE.
    assert_sasu_nil_forms(&export);
    assert_eq!(case_amount(&export, "2033-B", "252"), Some(8_000));
    assert_eq!(case_amount(&export, "2033-B", "254"), Some(4_000));
    assert_eq!(case_amount(&export, "2033-B", "306"), Some(3_000));
    assert_eq!(case_amount(&export, "2033-B", "310"), Some(30_326));
    assert_eq!(case_amount(&export, "2033-B", "210"), None);
}

#[test]
fn a_zero_taxable_result_omits_case_c1() {
    let profile = profile("Argon Digital");
    let year = FiscalYearRecord {
        revenue_ht: Money::ZERO,
        expenses: Money::ZERO,
        director_remuneration: Money::ZERO,
        depreciation: Money::ZERO,
        result_before_tax: Money::ZERO,
        corporate_tax: Money::ZERO,
        net_result: Money::ZERO,
        legal_reserve: Money::ZERO,
        dividends: Money::ZERO,
        retained_earnings: Money::ZERO,
        non_deductible_expenses: Money::ZERO,
        ..record(false)
    };
    let export = liasse_export(&profile, &year, None);
    assert_eq!(case_amount(&export, "2065", "C1"), None);
}

/// 250 centimes de 758. `round_to_euro` (art. 1657 CGI) vaut 300 centimes,
/// soit 3 €. La case garde `amount_cents` 250 et porte `amount_euros` 3.
/// Pas de 706 : la case 218 est absente. Un brut au profil ne crée pas
/// les cases 250 ni 252.
#[test]
fn sundry_income_of_250_cents_copies_three_euros_on_case_230() {
    assert_eq!(
        round_to_euro(Money::from_cents(250)),
        Money::from_cents(300),
        "l'euro arrondi est 3"
    );

    let mut named = named_profile();
    named.director_monthly_gross = Some(Money::from_cents(200_000));
    let year = FiscalYearRecord {
        revenue_ht: Money::from_cents(50_000),
        director_remuneration: Money::from_cents(2_400_000),
        ..record(true)
    };
    let ledger = book(vec![
        cents_line(accounts::SUNDRY_INCOME, -250),
        cents_line(accounts::BANK, 250),
    ]);
    let export = liasse_export(&named, &year, Some(&ledger));
    assert_eq!(case_amount(&export, "2033-B", "218"), None);
    assert_eq!(case_amount(&export, "2033-B", "250"), None);
    assert_eq!(case_amount(&export, "2033-B", "252"), None);
    let case_230 = case_entry(&export, "2033-B", "230").expect("case 230");
    assert_eq!(case_230.amount_cents, 250);
    assert_eq!(case_230.amount_euros, 3);
    assert!(
        export.entries.iter().all(|e| e.case != "210"),
        "pas de vente de marchandises"
    );
    assert_sasu_nil_forms(&export);

    let json = serde_json::to_value(&export).unwrap();
    let copied = json["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["form"] == "2033-B" && e["case"] == "230")
        .unwrap();
    assert_eq!(copied["amount_cents"], 250);
    assert_eq!(copied["amount_euros"], 3);

    let notice = render_efi_notice(&export).unwrap();
    assert_is_pdf(&notice, "notice EFI case 230");
    if let Some(text) = pdf_text(&notice, "efi-230") {
        let squeezed: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            squeezed.contains("saisir 3 € en case 230 du 2033-B"),
            "la notice montre l'euro à recopier : {squeezed}"
        );
        assert!(squeezed.contains("2033-E"), "{squeezed}");
        assert!(squeezed.contains("2033-G"), "{squeezed}");
        assert!(squeezed.contains("néant"), "{squeezed}");
        assert!(
            squeezed.contains("2,50"),
            "l'écart avec le livre reste en centimes : {squeezed}"
        );
    }
}

/// 49 centimes de 758 : l'euro recopié est 0, la case 230 est omise.
/// 50 centimes : la case reste, `amount_cents` 50, `amount_euros` 1
/// (art. 1657 CGI, 0,50 € compté pour 1).
#[test]
fn forty_nine_cents_of_758_omit_case_230_and_fifty_cents_copy_one_euro() {
    assert_eq!(round_to_euro(Money::from_cents(49)), Money::from_cents(0));
    assert_eq!(round_to_euro(Money::from_cents(50)), Money::from_cents(100));

    let profile = profile("Argon Digital");
    let year = record(false);
    let below = book(vec![
        cents_line(accounts::SUNDRY_INCOME, -49),
        cents_line(accounts::BANK, 49),
    ]);
    let export = liasse_export(&profile, &year, Some(&below));
    assert_eq!(case_entry(&export, "2033-B", "230"), None);

    let half = book(vec![
        cents_line(accounts::SUNDRY_INCOME, -50),
        cents_line(accounts::BANK, 50),
    ]);
    let export = liasse_export(&profile, &year, Some(&half));
    let case_230 = case_entry(&export, "2033-B", "230").expect("50 centimes donnent la case 230");
    assert_eq!(case_230.amount_cents, 50);
    assert_eq!(case_230.amount_euros, 1);
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
