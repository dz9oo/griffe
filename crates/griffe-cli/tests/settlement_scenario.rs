//! Scénario de preuve du lot 37 : le cas de l'expert-comptable de l'audit du 2 septembre 2026,
//! rejoué par la CLI — une SASU reprise d'un cabinet dont le bilan d'ouverture porte des dettes
//! **payées pendant l'exercice**, sans aucune facture émise.
//!
//! Tous les montants attendus ont été posés à la main dans le rapport d'audit
//! (`docs/audits/2026-09-02-cloturer-seul/rapport-expert-comptable.md`) avant ce test :
//!
//! - bilan d'ouverture au 1er octobre 2025 : 101000 C 1 000 ; 106100 C 100 ; 110000 C 6 350 ;
//!   401000 C 600 (facture du cabinet de septembre 2025) ; 444000 C 1 200 (solde d'IS 2025) ;
//!   455000 C 500 ; 445670 D 210 ; 512000 D 9 540 — équilibré à 9 750 ;
//! - relevé du 1er octobre 2025 au 30 septembre 2026, 17 lignes : 12 × −12,50 de frais de
//!   compte ; −600 (cabinet, **dette reprise**) ; −227 (CFE) ; −1 200 (solde d'IS, **dette
//!   reprise**) ; −900 (honoraires 2026 : 750 HT + 150 TVA) ; −119,88 (OVH : 99,90 + 19,98) ;
//!   solde bancaire réel au 30 septembre 2026 : **6 343,12 €** ;
//! - charges de l'exercice 750 + 150 + 99,90 + 227 = **1 226,90 €**, résultat −1 226,90, IS 0,
//!   déficit reportable 1 226,90, report à nouveau après affectation 6 350 − 1 226,90 =
//!   **5 123,10 €**, TVA déductible 169,98 ; **401 et 444 soldés à zéro** ; banque 6 343,12 ;
//! - 2033-B : autres charges externes 999,90 (case 242), impôts et taxes 227 (case 244).
//!
//! Avant ce lot, les deux débits de dettes reprises n'avaient aucune lecture juste : en dépense
//! (résultat −2 926,90, dettes toujours au passif) ou ignorés (banque surestimée de 1 800 €).

use std::path::Path;

use predicates::prelude::*;

mod common;
use common::{json_result, provision, temp_db, unlocked};

fn set_profile(db: &Path) {
    unlocked(db)
        .args([
            "company",
            "set-profile",
            "--name",
            "Nova Dev",
            "--legal-form",
            "SASU",
            "--siren",
            "889112348",
            "--vat-number",
            "FR16889112348",
            "--street",
            "3 allée des Tanneurs",
            "--postal-code",
            "44000",
            "--city",
            "Nantes",
            "--country",
            "FR",
            "--share-capital",
            "1000",
            "--rcs-city",
            "Nantes",
            "--fiscal-year-end",
            "30/09",
            "--vat-regime",
            "real_simplified",
        ])
        .assert()
        .success();
}

fn json_out(db: &Path, args: &[&str]) -> serde_json::Value {
    let out = unlocked(db)
        .arg("--json")
        .args(args)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    json_result(&out)
}

fn unmatched_transaction(db: &Path, description: &str) -> String {
    json_out(db, &["bank", "list", "--unmatched"])
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["description"] == description)
        .unwrap_or_else(|| panic!("transaction « {description} » introuvable ou déjà rapprochée"))
        ["id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn step<'a>(checklist: &'a serde_json::Value, key: &str) -> &'a serde_json::Value {
    checklist["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["key"] == key)
        .unwrap_or_else(|| panic!("étape {key} absente"))
}

fn account_balance(balance: &serde_json::Value, account: &str) -> i64 {
    balance["trial_balance"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["account"] == account)
        .map_or(0, |r| r["balance_cents"].as_i64().unwrap())
}

fn record_from_debit(db: &Path, tx: &str, label: &str, category: &str, rate: &str, vat: &str) {
    unlocked(db)
        .args([
            "expense",
            "record",
            "--transaction",
            tx,
            "--label",
            label,
            "--category",
            category,
            "--vat-rate",
            rate,
            "--vat-deductible",
            vat,
        ])
        .assert()
        .success();
}

#[test]
#[allow(clippy::too_many_lines)]
fn reprised_debts_paid_from_the_statement_are_settled_not_expensed() {
    let db = temp_db("settlement-scenario");
    provision(&db);
    set_profile(&db);

    // Le bilan du cabinet, recopié.
    unlocked(&db)
        .args([
            "year",
            "opening",
            "set",
            "--opens-on",
            "2025-10-01",
            "--source",
            "bilan au 30/09/2025, cabinet",
            "--line",
            "101000:Capital social:C:1000.00",
            "--line",
            "106100:Réserve légale:C:100.00",
            "--line",
            "110000:Report à nouveau:C:6350.00",
            "--line",
            "401000:Fournisseurs (honoraires cabinet, facture 09/2025):C:600.00",
            "--line",
            "444000:État - IS à payer (solde IS 2025):C:1200.00",
            "--line",
            "455000:Compte courant d'associé:C:500.00",
            "--line",
            "445670:Crédit de TVA:D:210.00",
            "--line",
            "512000:Banque:D:9540.00",
        ])
        .assert()
        .success();

    // Le relevé de l'exercice, 17 lignes.
    let mut csv = String::from("date;description;montant\n");
    for month in [
        "2025-10-31",
        "2025-11-30",
        "2025-12-31",
        "2026-01-31",
        "2026-02-28",
        "2026-03-31",
        "2026-04-30",
        "2026-05-31",
        "2026-06-30",
        "2026-07-31",
        "2026-08-31",
        "2026-09-30",
    ] {
        csv.push_str(&format!("{month};FRAIS TENUE DE COMPTE;-12.50\n"));
    }
    csv.push_str("2025-10-15;VIR CABINET COMPTA FACT 09/2025;-600.00\n");
    csv.push_str("2025-12-15;PRLV DGFIP CFE 2025;-227.00\n");
    csv.push_str("2026-01-15;PRLV DGFIP SOLDE IS 2025;-1200.00\n");
    csv.push_str("2026-03-10;VIR CABINET COMPTA HONORAIRES 2026;-900.00\n");
    csv.push_str("2026-02-05;CB OVH;-119.88\n");
    let statement = db.with_file_name("releve.csv");
    std::fs::write(&statement, csv).unwrap();
    unlocked(&db)
        .args(["bank", "import", "--format", "csv"])
        .arg(&statement)
        .assert()
        .success()
        .stdout(predicate::str::contains("17 nouvelle(s) transaction(s)"));

    // Avant tout rapprochement, le parcours propose le règlement des deux dettes reprises —
    // et non la dépense — et liste les comptes de tiers repris encore ouverts.
    let before = json_out(&db, &["year", "checklist", "2026", "--today", "2026-10-02"]);
    let bank = step(&before, "bank");
    assert_eq!(bank["status"], "warning");
    let detail = bank["detail"].as_str().unwrap();
    assert!(
        detail.contains(
            "Le débit de 600,00\u{a0}€ du 2025-10-15 (VIR CABINET COMPTA FACT 09/2025) \
             correspond au solde repris 401000"
        ),
        "{detail}"
    );
    assert!(
        detail.contains("Le débit de 1\u{202f}200,00\u{a0}€ du 2026-01-15"),
        "{detail}"
    );
    assert!(detail.contains("encore ouvertes au 2026-09-30"), "{detail}");

    // Les quinze charges, créées depuis le relevé.
    for _ in 0..12 {
        let tx = unmatched_transaction(&db, "FRAIS TENUE DE COMPTE");
        record_from_debit(
            &db,
            &tx,
            "Frais de tenue de compte",
            "bank_charges",
            "zero",
            "0",
        );
    }
    let cfe = unmatched_transaction(&db, "PRLV DGFIP CFE 2025");
    record_from_debit(&db, &cfe, "CFE 2025", "taxes", "zero", "0");
    let fees = unmatched_transaction(&db, "VIR CABINET COMPTA HONORAIRES 2026");
    record_from_debit(
        &db,
        &fees,
        "Honoraires cabinet 2026",
        "fees",
        "standard",
        "150",
    );
    let ovh = unmatched_transaction(&db, "CB OVH");
    // 150 € de TVA sur 119,88 € : refusé, la TVA est bornée par le taux (lot 37).
    unlocked(&db)
        .args([
            "expense",
            "record",
            "--transaction",
            &ovh,
            "--label",
            "OVH",
            "--category",
            "software",
            "--vat-rate",
            "standard",
            "--vat-deductible",
            "150",
        ])
        .assert()
        .failure()
        .code(4)
        .stderr(predicate::str::contains("au plus 19,98"));
    record_from_debit(&db, &ovh, "OVH", "software", "standard", "19.98");

    // Les deux dettes reprises : un règlement, pas une dépense. Un agent le propose, un humain
    // le confirme ; l'humain règle directement.
    let cabinet = unmatched_transaction(&db, "VIR CABINET COMPTA FACT 09/2025");
    let pending = json_out(
        &db,
        &[
            "--actor",
            "agent:audit",
            "bank",
            "settle",
            &cabinet,
            "--account",
            "401000",
        ],
    );
    assert_eq!(pending["status"], "pending_confirmation");
    let pending_id = pending["pending_action_id"].as_str().unwrap();
    unlocked(&db)
        .args(["confirm", pending_id])
        .assert()
        .success()
        .stdout(predicate::str::starts_with("✓"));
    let is_balance = unmatched_transaction(&db, "PRLV DGFIP SOLDE IS 2025");
    unlocked(&db)
        .args(["bank", "settle", &is_balance, "--account", "444000"])
        .assert()
        .success()
        .stdout(predicate::str::contains("réglée sur le compte 444000"));
    // Un compte de gestion n'est pas un compte de règlement.
    unlocked(&db)
        .args(["bank", "settle", &is_balance, "--account", "622600"])
        .assert()
        .failure();
    unlocked(&db)
        .args(["bank", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("réglée (401000 Fournisseurs)"))
        .stdout(predicate::str::contains(
            "réglée (444000 État — impôts sur les bénéfices)",
        ));

    // Le parcours : tout est rapproché, plus aucun compte de tiers repris ouvert.
    let ready = json_out(&db, &["year", "checklist", "2026", "--today", "2026-10-02"]);
    let bank = step(&ready, "bank");
    assert_eq!(bank["status"], "done", "{}", bank["detail"]);
    assert!(
        bank["detail"]
            .as_str()
            .unwrap()
            .contains("2 règlement(s) de compte de bilan")
    );
    assert_eq!(ready["stage"], "ready");

    // Le bilan dérivé au 30 septembre 2026 : 401 et 444 à zéro, banque = relevé.
    let balance = json_out(&db, &["year", "balance", "2026"]);
    assert_eq!(account_balance(&balance, "401000"), 0);
    assert_eq!(account_balance(&balance, "444000"), 0);
    assert_eq!(account_balance(&balance, "512000"), 634_312);
    assert_eq!(account_balance(&balance, "455000"), -50_000);
    assert_eq!(account_balance(&balance, "445670"), 21_000);
    assert_eq!(account_balance(&balance, "445660"), 16_998);
    assert_eq!(account_balance(&balance, "635000"), 22_700);
    assert_eq!(account_balance(&balance, "622600"), 75_000);
    assert_eq!(account_balance(&balance, "627000"), 15_000);
    assert_eq!(account_balance(&balance, "651000"), 9_990);
    let liabilities = balance["balance_sheet"]["liabilities"].as_array().unwrap();
    let suppliers = liabilities
        .iter()
        .find(|l| l["case"] == "166")
        .map_or(0, |l| l["amount_cents"].as_i64().unwrap());
    assert_eq!(suppliers, 0, "plus de dette fantôme au passif");
    assert_eq!(
        balance["balance_sheet"]["total_liabilities_cents"], 672_310,
        "{balance}"
    );

    // Clôture : résultat −1 226,90, IS nul, report à nouveau 5 123,10.
    unlocked(&db)
        .args(["year", "close", "--period", "2026", "--today", "2026-10-02"])
        .assert()
        .success();
    let closed = json_out(&db, &["year", "show", "2026"]);
    assert_eq!(closed["expenses_cents"], 122_690);
    assert_eq!(closed["result_before_tax_cents"], -122_690);
    assert_eq!(closed["corporate_tax_cents"], 0);
    assert_eq!(closed["losses_carried_forward_cents"], 122_690);
    assert_eq!(closed["retained_earnings_cents"], 512_310);

    // Liasse : impôts et taxes en 244, le reste des charges externes en 242.
    let liasse_path = db.with_file_name("liasse-2026.json");
    unlocked(&db)
        .args(["year", "render", "2026", "liasse", "--out"])
        .arg(&liasse_path)
        .assert()
        .success();
    let liasse: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&liasse_path).unwrap()).unwrap();
    let case = |form: &str, case: &str| {
        liasse["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["form"] == form && e["case"] == case)
            .map(|e| e["amount_cents"].as_i64().unwrap())
    };
    assert_eq!(case("2033-B", "244"), Some(22_700));
    assert_eq!(case("2033-B", "242"), Some(99_990));
    assert_eq!(case("2033-A", "166"), None, "case à zéro omise");

    // FEC (colonnes : JournalCode 0, CompteNum 4, CompteLib 5, PieceRef 8) : deux écritures de
    // règlement `BQ-<uuid>`, chaque compte avec un seul libellé, une pièce par dépense.
    let fec_path = db.with_file_name("fec.txt");
    unlocked(&db)
        .args(["fec", "export", "2026", "--out"])
        .arg(&fec_path)
        .assert()
        .success();
    let fec = std::fs::read_to_string(&fec_path).unwrap();
    let rows: Vec<Vec<&str>> = fec
        .lines()
        .skip(1)
        .map(|l| l.split('|').collect())
        .collect();
    let settlements: std::collections::BTreeSet<&str> = rows
        .iter()
        .filter(|r| r[8].starts_with("BQ-"))
        .map(|r| r[8])
        .collect();
    assert_eq!(settlements.len(), 2, "{fec}");
    let mut labels_by_account: std::collections::BTreeMap<&str, std::collections::BTreeSet<&str>> =
        std::collections::BTreeMap::new();
    for r in &rows {
        labels_by_account.entry(r[4]).or_default().insert(r[5]);
    }
    for (account, labels) in &labels_by_account {
        assert_eq!(labels.len(), 1, "{account} : {labels:?}");
    }
    assert_eq!(
        labels_by_account["401000"].iter().next(),
        Some(&"Fournisseurs")
    );
    let expense_pieces: std::collections::BTreeSet<&str> = rows
        .iter()
        .filter(|r| r[8].starts_with("DEP-") && r[0] == "AC")
        .map(|r| r[8])
        .collect();
    assert_eq!(expense_pieces.len(), 15, "une pièce distincte par dépense");

    // Défaire un règlement rend le débit « à rapprocher ».
    unlocked(&db)
        .args(["bank", "unsettle", &is_balance])
        .assert()
        .success()
        .stdout(predicate::str::contains("à rapprocher"));
    assert_eq!(
        unmatched_transaction(&db, "PRLV DGFIP SOLDE IS 2025"),
        is_balance
    );
}
