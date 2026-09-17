//! Scénario de preuve de bout en bout (lot 35) : **une SASU préexistante clôture seule, deux
//! exercices de suite**, par la CLI — le chemin de code que la fenêtre et le serveur MCP
//! empruntent à l'identique (thèse « Studio »).
//!
//! Le cas est celui de l'analyse du 2 septembre 2026 : Lumen Conseil, SASU au capital de
//! 1 000 €, clôture au **30 septembre**, tenue jusqu'ici par un cabinet ; l'indépendant reprend
//! la main au 1er octobre 2025 avec le bilan du cabinet en poche. Son premier exercice seul
//! n'a **aucun chiffre d'affaires** (année de transition), le second redémarre.
//!
//! Tous les montants attendus ci-dessous ont été posés *à la main* avant d'écrire le test, comme
//! le ferait un expert-comptable sur un coin de table — le test vérifie que l'application
//! retrouve exactement ces chiffres, à chaque étape (parcours, snapshot, bilan 2033-A, FEC,
//! liasse), jamais l'inverse.
//!
//! ## Exercice 2026 (1er octobre 2025 → 30 septembre 2026), sans CA
//!
//! Bilan d'ouverture au 1er octobre 2025 (bilan du cabinet au 30 septembre 2025) :
//!
//! | Compte | Libellé          | Débit    | Crédit   |
//! |--------|------------------|----------|----------|
//! | 101000 | Capital social   |          | 1 000,00 |
//! | 110000 | Report à nouveau |          | 2 400,00 |
//! | 512000 | Banque           | 3 400,00 |          |
//!
//! Dépenses (toutes rapprochées du relevé, toutes avec justificatif à la fin) :
//!
//! | Dépense                          | TTC    | TVA déductible | HT (charge) | Compte |
//! |----------------------------------|--------|----------------|-------------|--------|
//! | Honoraires du cabinet (bilan 2025) | 720,00 | 120,00        | 600,00      | 622600 |
//! | Logiciel de facturation          | 144,00 | 24,00          | 120,00      | 651000 |
//! | Frais de tenue de compte         | 96,00  | 0,00           | 96,00       | 627000 |
//! | **Total**                        | 960,00 | 144,00         | **816,00**  |        |
//!
//! Résultat avant IS = 0 − 816 = **−816,00** ; IS nul ; résultat net −816,00 ; déficit fiscal
//! reportable **816,00** ; report à nouveau après affectation = 2 400 − 816 = **1 584,00** ;
//! dotation minimale à la réserve légale nulle (pas de bénéfice).
//!
//! Bilan au 30 septembre 2026 : actif = banque 3 400 − 960 = 2 440,00 + TVA déductible 144,00
//! = **2 584,00** ; passif = capital 1 000 + report à nouveau 2 400 + résultat −816 =
//! **2 584,00**.
//!
//! Échéances (clôture au 30/09) : approbation dans les six mois (30/03/2027), liasse dans les
//! trois mois (30/12/2026), solde d'IS le 15 du 4e mois (15/01/2027), dépôt au greffe dans le
//! mois de l'approbation.
//!
//! ## Exercice 2027 (1er octobre 2026 → 30 septembre 2027), reprise d'activité
//!
//! | Fait                                     | HT        | TVA      | TTC       |
//! |------------------------------------------|-----------|----------|-----------|
//! | Facture 1, refonte de site (31/03/2027)  | 8 000,00  | 1 600,00 | 9 600,00  | encaissée le 14/04
//! | Facture 2, accompagnement (15/09/2027)   | 4 000,00  | 800,00   | 4 800,00  | non encaissée
//! | Honoraires du cabinet (10/11/2026)       | 700,00    | 140,00   | 840,00    |
//! | Frais de tenue de compte (30/06/2027)    | 96,00     | 0,00     | 96,00     |
//!
//! CA HT 12 000,00 ; charges 796,00 ; résultat avant IS **11 204,00** ; déficit antérieur
//! imputé 816,00 → résultat fiscal **10 388,00** ; IS 15 % = 1 558,20 → **1 558** (arrondi à l'euro, art. 1657 CGI) ; résultat net
//! = 11 204 − 1 558 = **9 646**. Réserve légale minimale : un vingtième de 9 646 =
//! 482,29, plafonné à 10 % du capital (100,00) − 0 déjà doté = **100,00**. Affectation : réserve
//! 100, dividendes 3 000, report à nouveau = 1 584 + 9 646 − 100 − 3 000 = **8 130**.
//!
//! Bilan au 30 septembre 2027 (à-nouveaux dérivés de la clôture 2026) : actif = banque
//! 2 440 + 9 600 − 840 − 96 = 11 104,00 + clients 4 800,00 + TVA déductible 144 + 140 = 284,00
//! = **16 188,00** ; passif = capital 1 000 + report à nouveau 1 584 + résultat 9 646 + TVA
//! collectée 2 400 + IS dû 1 558 = **16 188,00**.

use std::path::Path;

use predicates::prelude::*;

mod common;
use common::{create_client, json_result, provision, temp_db, unlocked};

const SIREN: &str = "901265322";

fn set_profile(db: &Path) {
    unlocked(db)
        .args([
            "company",
            "set-profile",
            "--name",
            "Lumen Conseil",
            "--legal-form",
            "SASU",
            "--siren",
            SIREN,
            "--vat-number",
            "FR33901265322",
            "--street",
            "8 rue des Capucins",
            "--postal-code",
            "69001",
            "--city",
            "Lyon",
            "--country",
            "FR",
            "--share-capital",
            "1000",
            "--rcs-city",
            "Lyon",
            "--president",
            "Nora Lumen",
            "--sole-shareholder",
            "Nora Lumen",
            "--sole-shareholder-address",
            "4 allée des Tilleuls, 69003 Lyon",
            "--share-count",
            "100",
            "--fiscal-year-end",
            "30/09",
            "--vat-regime",
            "real_normal_monthly",
        ])
        .assert()
        .success();
}

/// Un justificatif quelconque : l'archivage hache et copie le contenu, il ne l'interprète pas.
fn receipt(db: &Path, name: &str) -> std::path::PathBuf {
    let path = db.with_file_name(name);
    std::fs::write(&path, format!("%PDF-1.4 justificatif {name}\n")).unwrap();
    path
}

fn import_statement(db: &Path, name: &str, csv: &str) {
    let statement = db.with_file_name(name);
    std::fs::write(&statement, csv).unwrap();
    unlocked(db)
        .args(["bank", "import", "--format", "csv"])
        .arg(&statement)
        .assert()
        .success();
}

/// Identifiant d'une transaction importée non rapprochée, par libellé du relevé.
fn unmatched_transaction(db: &Path, description: &str) -> String {
    let out = unlocked(db)
        .args(["--json", "bank", "list", "--unmatched"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    json_result(&out)
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

fn checklist(db: &Path, period: &str, today: &str) -> serde_json::Value {
    let out = unlocked(db)
        .args(["--json", "year", "checklist", period, "--today", today])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    json_result(&out)
}

fn step<'a>(checklist: &'a serde_json::Value, key: &str) -> &'a serde_json::Value {
    checklist["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["key"] == key)
        .unwrap_or_else(|| panic!("étape {key} absente"))
}

fn year_show(db: &Path, period: &str) -> serde_json::Value {
    let out = unlocked(db)
        .args(["--json", "year", "show", period])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    json_result(&out)
}

fn balance(db: &Path, period: &str) -> serde_json::Value {
    let out = unlocked(db)
        .args(["--json", "year", "balance", period])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    json_result(&out)
}

fn account_balance(balance: &serde_json::Value, account: &str) -> i64 {
    balance["trial_balance"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["account"] == account)
        .map_or(0, |r| r["balance_cents"].as_i64().unwrap())
}

fn liability(balance: &serde_json::Value, case: &str) -> i64 {
    balance["balance_sheet"]["liabilities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["case"] == case)
        .map_or(0, |l| l["amount_cents"].as_i64().unwrap())
}

/// Rend les cinq documents de clôture et le FEC dans un répertoire, et renvoie la liasse lue.
fn render_everything(db: &Path, period: &str) -> serde_json::Value {
    let dir = db.with_file_name(format!("cloture-{period}"));
    std::fs::create_dir_all(&dir).unwrap();
    for doc in [
        "minutes",
        "appropriation",
        "synthesis",
        "balance-sheet",
        "liasse",
    ] {
        let ext = if doc == "liasse" { "json" } else { "pdf" };
        let out = dir.join(format!("{doc}.{ext}"));
        unlocked(db)
            .args(["year", "render", period, doc, "--out"])
            .arg(&out)
            .assert()
            .success();
        let bytes = std::fs::read(&out).unwrap();
        if ext == "pdf" {
            assert!(bytes.starts_with(b"%PDF-"), "{doc} n'est pas un PDF");
        }
    }
    unlocked(db)
        .args(["fec", "export", period, "--out"])
        .arg(&dir)
        .assert()
        .success();
    serde_json::from_slice(&std::fs::read(dir.join("liasse.json")).unwrap()).unwrap()
}

/// Le texte d'un PDF via `pdftotext` (poppler), ou `None` si l'outil manque sur la machine —
/// le scénario dit alors sur stderr ce qu'il n'a pas pu vérifier.
fn pdf_text(pdf: &Path) -> Option<String> {
    match std::process::Command::new("pdftotext")
        .arg("-layout")
        .arg(pdf)
        .arg("-")
        .output()
    {
        Ok(output) => {
            assert!(
                output.status.success(),
                "pdftotext a échoué sur {}",
                pdf.display()
            );
            Some(String::from_utf8_lossy(&output.stdout).replace('\u{a0}', " "))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!(
                "pdftotext absent : contenu de {} non vérifié",
                pdf.display()
            );
            None
        }
        Err(e) => panic!("pdftotext : {e}"),
    }
}

fn liasse_case(liasse: &serde_json::Value, form: &str, case: &str) -> Option<i64> {
    liasse["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["form"] == form && e["case"] == case)
        .map(|e| e["amount_cents"].as_i64().unwrap())
}

#[test]
fn a_preexisting_sasu_closes_two_exercises_alone_from_the_cli() {
    let db = temp_db("closing-scenario");
    provision(&db);

    // ---------------------------------------------------------------------------------------
    // Avant tout : sans profil, le parcours dit clairement ce qui manque, et ne calcule rien
    // (sans date de clôture connue, il suppose l'année civile : vu de janvier 2027).
    // ---------------------------------------------------------------------------------------
    let bare = checklist(&db, "2026", "2027-01-05");
    assert_eq!(bare["stage"], "blocked");
    assert_eq!(step(&bare, "profile")["status"], "blocked");
    assert!(bare["result"].is_null());

    set_profile(&db);

    // ---------------------------------------------------------------------------------------
    // Reprise du bilan du cabinet au 1er octobre 2025.
    // ---------------------------------------------------------------------------------------
    unlocked(&db)
        .args([
            "year",
            "opening",
            "set",
            "--opens-on",
            "2025-10-01",
            "--source",
            "bilan au 30/09/2025, cabinet Fiducia",
            "--line",
            "101000:Capital social:C:1000.00",
            "--line",
            "110000:Report à nouveau:C:2400.00",
            "--line",
            "512000:Banque:D:3400.00",
        ])
        .assert()
        .success();

    // Le parcours, vu le 5 octobre 2026 : l'exercice est écoulé, rien ne bloque, mais il n'y a
    // encore ni dépense ni relevé — le résultat prévisionnel est nul.
    let empty = checklist(&db, "2026", "2026-10-05");
    assert_eq!(empty["starts_on"], "2025-10-01");
    assert_eq!(empty["ends_on"], "2026-09-30");
    assert_eq!(empty["stage"], "ready");
    assert_eq!(step(&empty, "opening_balance")["status"], "done");
    assert_eq!(step(&empty, "invoices")["status"], "info");
    assert_eq!(step(&empty, "expenses")["status"], "info");
    assert_eq!(step(&empty, "bank")["status"], "info");
    assert_eq!(empty["result"]["result_before_tax_cents"], 0);

    // ---------------------------------------------------------------------------------------
    // Les dépenses de l'exercice, depuis le relevé bancaire importé.
    // ---------------------------------------------------------------------------------------
    import_statement(
        &db,
        "releve-2025-2026.csv",
        "date;description;montant\n\
         2025-11-05;PRLV CABINET FIDUCIA;-720.00\n\
         2026-02-12;CB LOGICIEL FACTURE;-144.00\n\
         2026-06-30;FRAIS TENUE DE COMPTE;-96.00\n",
    );
    // 1. Les honoraires, créés *depuis* le débit : montant et date repris du relevé.
    let fees_tx = unmatched_transaction(&db, "PRLV CABINET FIDUCIA");
    unlocked(&db)
        .args([
            "expense",
            "record",
            "--label",
            "Honoraires cabinet Fiducia — bilan 2025",
            "--category",
            "fees",
            "--supplier",
            "Cabinet Fiducia",
            "--vat-rate",
            "standard",
            "--vat-deductible",
            "120",
            "--transaction",
            &fees_tx,
            "--receipt",
        ])
        .arg(receipt(&db, "facture-fiducia-2025.pdf"))
        .assert()
        .success();
    // 2. Le logiciel, saisi à sa date de facture (10/02) puis rapproché du débit du 12/02.
    unlocked(&db)
        .args([
            "expense",
            "record",
            "--label",
            "Logiciel de facturation (abonnement annuel)",
            "--category",
            "software",
            "--amount",
            "144",
            "--vat-rate",
            "standard",
            "--vat-deductible",
            "24",
            "--incurred-on",
            "2026-02-10",
            "--receipt",
        ])
        .arg(receipt(&db, "facture-logiciel.pdf"))
        .assert()
        .success();
    let software_tx = unmatched_transaction(&db, "CB LOGICIEL FACTURE");
    unlocked(&db)
        .args([
            "expense",
            "reconcile",
            "logiciel",
            "--transaction",
            &software_tx,
        ])
        .assert()
        .success();
    // 3. Les frais bancaires, sans justificatif pour l'instant.
    let charges_tx = unmatched_transaction(&db, "FRAIS TENUE DE COMPTE");
    unlocked(&db)
        .args([
            "expense",
            "record",
            "--label",
            "Frais de tenue de compte",
            "--category",
            "bank_charges",
            "--vat-rate",
            "zero",
            "--vat-deductible",
            "0",
            "--transaction",
            &charges_tx,
        ])
        .assert()
        .success();

    // Le parcours signale la pièce manquante — et rien d'autre : tout est rapproché, le
    // résultat est celui calculé à la main, le déficit est annoncé reportable en avant.
    let missing_receipt = checklist(&db, "2026", "2026-10-05");
    assert_eq!(missing_receipt["stage"], "ready");
    let expenses = step(&missing_receipt, "expenses");
    assert_eq!(expenses["status"], "warning");
    assert!(
        expenses["detail"]
            .as_str()
            .unwrap()
            .contains("1 sans justificatif"),
        "{expenses}"
    );
    assert_eq!(step(&missing_receipt, "bank")["status"], "done");
    assert_eq!(
        missing_receipt["result"]["result_before_tax_cents"],
        -81_600
    );
    assert_eq!(missing_receipt["result"]["corporate_tax_cents"], 0);
    assert!(
        step(&missing_receipt, "result")["detail"]
            .as_str()
            .unwrap()
            .contains("Déficit fiscal de 816,00\u{a0}€ : reportable en avant"),
        "{}",
        step(&missing_receipt, "result")
    );
    assert!(missing_receipt["carry_back_available_cents"].is_null());
    unlocked(&db)
        .args(["year", "checklist", "2026", "--today", "2026-10-05"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "freeflow expense edit <RÉF> --receipt <fichier>",
        ));

    // On joint la pièce : l'étape passe au vert.
    unlocked(&db)
        .args(["expense", "edit", "frais de tenue", "--receipt"])
        .arg(receipt(&db, "releve-frais-juin.pdf"))
        .assert()
        .success();
    let ready = checklist(&db, "2026", "2026-10-05");
    assert_eq!(ready["stage"], "ready");
    assert_eq!(ready["blocked"], 0);
    for key in [
        "profile",
        "period_ended",
        "opening_balance",
        "expenses",
        "bank",
        "balance_sheet",
    ] {
        assert_eq!(step(&ready, key)["status"], "done", "étape {key}");
    }
    assert_eq!(step(&ready, "previous_year")["status"], "info");
    assert_eq!(step(&ready, "close")["status"], "todo");
    assert!(step(&ready, "close")["amount_cents"].is_null());
    assert_eq!(step(&ready, "approve")["due_on"], "2027-03-30");
    assert_eq!(step(&ready, "liasse")["due_on"], "2026-12-30");
    assert_eq!(step(&ready, "corporate_tax")["due_on"], "2027-01-15");
    // Lot 41 : la TVA de l'exercice (aucune facture : crédit, CA3 « néant » à déposer quand
    // même) et la DAS2 (720 € au cabinet : sous le seuil de 2 400 €, rien à déclarer).
    // Bilan d'ouverture sans crédit repris (case 22) : avertissement — les CA3 partiraient
    // de zéro (`vat_carry_in`).
    assert_eq!(step(&ready, "vat")["status"], "warning");
    assert!(
        step(&ready, "vat")["detail"]
            .as_str()
            .unwrap()
            .contains("néant"),
        "{}",
        step(&ready, "vat")["detail"]
    );
    assert_eq!(step(&ready, "das2")["status"], "info");
    assert!(
        step(&ready, "das2")["detail"]
            .as_str()
            .unwrap()
            .contains("rien à déclarer"),
        "{}",
        step(&ready, "das2")["detail"]
    );
    assert_eq!(ready["result"]["revenue_ht_cents"], 0);
    assert_eq!(ready["result"]["expenses_cents"], 81_600);
    assert_eq!(ready["result"]["net_result_cents"], -81_600);

    // Le bilan dérivé *avant* de clore : c'est celui calculé à la main.
    let before = balance(&db, "2026");
    assert_eq!(before["balance_sheet"]["balanced"], true);
    assert_eq!(before["balance_sheet"]["total_assets_net_cents"], 258_400);
    assert_eq!(account_balance(&before, "512000"), 244_000);
    assert_eq!(account_balance(&before, "445660"), 14_400);
    assert_eq!(account_balance(&before, "401000"), 0);
    assert_eq!(account_balance(&before, "622600"), 60_000);
    assert_eq!(account_balance(&before, "627000"), 9_600);

    // ---------------------------------------------------------------------------------------
    // Clôture, puis approbation dans les délais.
    // ---------------------------------------------------------------------------------------
    unlocked(&db)
        .args(["year", "close", "--period", "2026", "--today", "2026-10-05"])
        .assert()
        .success();
    let closed = year_show(&db, "2026");
    assert_eq!(closed["starts_on"], "2025-10-01");
    assert_eq!(closed["ends_on"], "2026-09-30");
    assert_eq!(closed["result_before_tax_cents"], -81_600);
    assert_eq!(closed["taxable_result_cents"], -81_600);
    assert_eq!(closed["corporate_tax_cents"], 0);
    assert_eq!(closed["net_result_cents"], -81_600);
    assert_eq!(closed["losses_carried_forward_cents"], 81_600);
    assert_eq!(closed["legal_reserve_cents"], 0);
    assert_eq!(closed["retained_earnings_cents"], 158_400);

    let draft = checklist(&db, "2026", "2026-10-06");
    assert_eq!(draft["stage"], "draft");
    assert_eq!(step(&draft, "appropriation")["status"], "done");
    assert_eq!(step(&draft, "approve")["status"], "todo");
    assert_eq!(step(&draft, "corporate_tax")["status"], "info");

    unlocked(&db)
        .args([
            "year",
            "approve",
            "2026",
            "--approved-on",
            "2026-12-15",
            "--today",
            "2026-12-15",
        ])
        .assert()
        .success();
    let approved = checklist(&db, "2026", "2026-12-20");
    assert_eq!(approved["stage"], "approved");
    assert_eq!(step(&approved, "approve")["status"], "done");
    assert_eq!(
        step(&approved, "documents")["status"],
        "done",
        "l'approbation fige les originaux (PV, FEC, liasse…) : l'étape Documents dit vrai"
    );
    assert_eq!(step(&approved, "liasse")["status"], "todo");
    assert_eq!(step(&approved, "filing")["status"], "todo");
    assert_eq!(step(&approved, "filing")["due_on"], "2027-01-15");

    // Documents et déclarations : les cinq documents se rendent, le FEC porte les à-nouveaux et
    // le décaissement daté du relevé, la liasse porte le déficit dans ses cases de suivi.
    let liasse = render_everything(&db, "2026");
    assert_eq!(liasse["approved"], true);
    assert_eq!(liasse_case(&liasse, "2033-A", "180"), Some(258_400));
    assert_eq!(liasse_case(&liasse, "2033-B", "372"), Some(81_600));
    assert_eq!(liasse_case(&liasse, "2033-D", "860"), Some(81_600));
    assert_eq!(liasse_case(&liasse, "2033-D", "870"), Some(81_600));
    assert_eq!(liasse_case(&liasse, "2065", "C1"), Some(-81_600));
    // Lot 41 : 218 (prestations, ici aucune), 306 (IS, nul), 2033-F depuis le profil, et un PV
    // nominatif qui porte l'associée unique, le 223 quater et le registre des décisions.
    assert_eq!(liasse_case(&liasse, "2033-B", "218"), Some(0));
    assert_eq!(liasse_case(&liasse, "2033-B", "306"), Some(0));
    assert_eq!(liasse_case(&liasse, "2065", "IS"), None);
    assert_eq!(liasse["capital"]["form"], "2033-F");
    assert_eq!(liasse["capital"]["shares_held_by_individuals"], 100);
    assert_eq!(liasse["capital"]["holders"][0]["name"], "Nora Lumen");
    assert_eq!(liasse["capital"]["holders"][0]["percent_bps"], 10_000);
    if let Some(text) = pdf_text(&db.with_file_name("cloture-2026").join("minutes.pdf")) {
        let squeezed: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
        for expected in [
            "Nora Lumen",
            "223 quater",
            "registre des décisions",
            "L227-9 al. 3",
            "perte de 816,00 €",
        ] {
            assert!(
                squeezed.contains(expected),
                "« {expected} » absent du PV : {squeezed}"
            );
        }
    }
    let fec = std::fs::read_to_string(
        db.with_file_name("cloture-2026")
            .join(format!("{SIREN}FEC20260930.txt")),
    )
    .unwrap();
    assert!(
        fec.contains("AN|À-nouveaux|1|20251001|101000|Capital social|"),
        "{fec}"
    );
    assert!(
        fec.contains("|20260212|512000|") && fec.contains("|20260210|651000|"),
        "le logiciel doit être engagé le 10/02 et décaissé le 12/02 :\n{fec}"
    );
    assert!(!fec.contains("695000"), "aucun IS sur un déficit :\n{fec}");

    // Une dépense de l'exercice clos ne bouge plus ; le bilan d'ouverture non plus.
    unlocked(&db)
        .args(["expense", "edit", "logiciel", "--amount", "150"])
        .assert()
        .failure();
    unlocked(&db)
        .args(["year", "opening", "rm"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("déjà clos"));

    // ---------------------------------------------------------------------------------------
    // Exercice 2027 : l'activité reprend.
    // ---------------------------------------------------------------------------------------
    let client_id = create_client(&db, "Atelier Verdier");
    import_statement(
        &db,
        "releve-2026-2027.csv",
        "date;description;montant\n\
         2026-11-10;PRLV CABINET FIDUCIA;-840.00\n\
         2027-04-14;VIR ATELIER VERDIER;9600.00\n\
         2027-06-30;FRAIS TENUE DE COMPTE;-96.00\n",
    );
    let fees_tx = unmatched_transaction(&db, "PRLV CABINET FIDUCIA");
    unlocked(&db)
        .args([
            "expense",
            "record",
            "--label",
            "Honoraires cabinet Fiducia — relecture bilan 2026",
            "--category",
            "fees",
            "--supplier",
            "Cabinet Fiducia",
            "--vat-rate",
            "standard",
            "--vat-deductible",
            "140",
            "--transaction",
            &fees_tx,
            "--receipt",
        ])
        .arg(receipt(&db, "facture-fiducia-2026.pdf"))
        .assert()
        .success();
    let charges_tx = unmatched_transaction(&db, "FRAIS TENUE DE COMPTE");
    unlocked(&db)
        .args([
            "expense",
            "record",
            "--label",
            "Frais de tenue de compte 2027",
            "--category",
            "bank_charges",
            "--vat-rate",
            "zero",
            "--vat-deductible",
            "0",
            "--transaction",
            &charges_tx,
            "--receipt",
        ])
        .arg(receipt(&db, "releve-frais-juin-2027.pdf"))
        .assert()
        .success();

    let emit = |label: &str, quantity: &str, issued_on: &str| -> String {
        let lines = format!(
            r#"[{{"description":"{label}","quantity":{quantity},"unit_price":800000,"vat_rate":"Standard"}}]"#
        );
        let out = unlocked(&db)
            .args([
                "--json",
                "invoice",
                "emit",
                "--client",
                &client_id,
                "--lines",
                &lines,
                "--issued-on",
                issued_on,
            ])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        json_result(&out)["result"]["id"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let first_invoice = emit("Refonte du site — forfait", "1", "2027-03-31");
    let payment_tx = unmatched_transaction(&db, "VIR ATELIER VERDIER");
    unlocked(&db)
        .args([
            "bank",
            "reconcile",
            "--transaction",
            &payment_tx,
            "--invoice",
            &first_invoice,
        ])
        .assert()
        .success();
    let _second_invoice = emit("Accompagnement — 5 jours", "5", "2027-09-15");
    // Oups : 5 × 8 000 € n'est pas ce qu'on voulait. Une facture émise ne se modifie pas : on
    // l'annule par un avoir et on la refait — c'est le régime d'immuabilité, vécu de l'intérieur.
    unlocked(&db)
        .args([
            "invoice",
            "credit-note",
            "--id",
            &_second_invoice,
            "--issued-on",
            "2027-09-15",
        ])
        .assert()
        .success();
    let lines = r#"[{"description":"Accompagnement — 5 jours","quantity":5,"unit_price":80000,"vat_rate":"Standard"}]"#;
    unlocked(&db)
        .args([
            "invoice",
            "emit",
            "--client",
            &client_id,
            "--lines",
            lines,
            "--issued-on",
            "2027-09-15",
        ])
        .assert()
        .success();

    // Le parcours, vu le 10 octobre 2027 : la chaîne est héritée de 2026, la facture de
    // septembre reste à encaisser (sans effet sur le résultat), le déficit de 2026 s'impute, et
    // la dotation minimale à la réserve légale est chiffrée.
    let ready = checklist(&db, "2027", "2027-10-10");
    assert_eq!(ready["stage"], "ready");
    let previous = step(&ready, "previous_year");
    assert_eq!(previous["status"], "done");
    assert!(
        previous["detail"].as_str().unwrap().contains(
            "report à nouveau 1\u{202f}584,00\u{a0}€, réserve légale cumulée 0,00\u{a0}€, \
             déficits reportables 816,00\u{a0}€"
        ),
        "{previous}"
    );
    assert_eq!(step(&ready, "opening_balance")["status"], "info");
    let invoices = step(&ready, "invoices");
    assert_eq!(invoices["status"], "warning");
    assert_eq!(invoices["amount_cents"], 480_000);
    assert!(
        invoices["detail"]
            .as_str()
            .unwrap()
            .contains("4 factures émises (dont 1 avoir)"),
        "{invoices}"
    );
    assert_eq!(step(&ready, "expenses")["status"], "done");
    assert_eq!(step(&ready, "bank")["status"], "done");
    assert_eq!(ready["result"]["revenue_ht_cents"], 1_200_000);
    assert_eq!(ready["result"]["expenses_cents"], 79_600);
    assert_eq!(ready["result"]["result_before_tax_cents"], 1_120_400);
    assert_eq!(ready["result"]["prior_losses_available_cents"], 81_600);
    assert_eq!(ready["result"]["losses_imputed_cents"], 81_600);
    assert_eq!(ready["result"]["taxable_result_cents"], 1_038_800);
    assert_eq!(ready["result"]["corporate_tax_cents"], 155_800);
    assert_eq!(ready["result"]["net_result_cents"], 964_600);
    assert_eq!(ready["minimum_legal_reserve_cents"], 10_000);
    assert_eq!(step(&ready, "close")["amount_cents"], 10_000);
    unlocked(&db)
        .args(["year", "checklist", "2027", "--today", "2027-10-10"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "freeflow year close --period 2027 --legal-reserve 100.00",
        ));

    // Clôture avec l'affectation décidée : réserve légale minimale, 3 000 € de dividendes.
    unlocked(&db)
        .args([
            "year",
            "close",
            "--period",
            "2027",
            "--legal-reserve",
            "100",
            "--dividends",
            "3000",
            "--today",
            "2027-10-10",
        ])
        .assert()
        .success();
    let closed = year_show(&db, "2027");
    assert_eq!(closed["revenue_ht_cents"], 1_200_000);
    assert_eq!(closed["losses_imputed_cents"], 81_600);
    assert_eq!(closed["taxable_result_cents"], 1_038_800);
    assert_eq!(closed["corporate_tax_cents"], 155_800);
    assert_eq!(closed["net_result_cents"], 964_600);
    assert_eq!(closed["legal_reserve_cents"], 10_000);
    assert_eq!(closed["dividends_cents"], 300_000);
    assert_eq!(closed["retained_earnings_cents"], 813_000);
    assert_eq!(closed["losses_carried_forward_cents"], 0);

    // Le bilan au 30 septembre 2027 s'ouvre sur la clôture 2026 et retombe sur le calcul manuel.
    let sheet = balance(&db, "2027");
    assert_eq!(sheet["balance_sheet"]["balanced"], true);
    assert_eq!(sheet["balance_sheet"]["total_assets_net_cents"], 1_618_800);
    assert_eq!(sheet["balance_sheet"]["total_liabilities_cents"], 1_618_800);
    assert_eq!(sheet["balance_sheet"]["result_cents"], 964_600);
    assert_eq!(account_balance(&sheet, "512000"), 1_110_400);
    assert_eq!(account_balance(&sheet, "411000"), 480_000);
    assert_eq!(account_balance(&sheet, "445660"), 28_400);
    assert_eq!(account_balance(&sheet, "445710"), -240_000);
    assert_eq!(account_balance(&sheet, "444000"), -155_800);
    assert_eq!(account_balance(&sheet, "706000"), -1_200_000);
    assert_eq!(account_balance(&sheet, "101000"), -100_000);
    assert_eq!(
        account_balance(&sheet, "110000") + account_balance(&sheet, "119000"),
        -158_400,
        "report à nouveau net après affectation de la perte 2026"
    );
    assert_eq!(liability(&sheet, "120"), 100_000);
    assert_eq!(liability(&sheet, "134"), 158_400);
    assert_eq!(liability(&sheet, "136"), 964_600);

    unlocked(&db)
        .args([
            "year",
            "approve",
            "2027",
            "--approved-on",
            "2027-12-10",
            "--today",
            "2027-12-10",
        ])
        .assert()
        .success();
    let approved = checklist(&db, "2027", "2027-12-15");
    assert_eq!(approved["stage"], "approved");
    assert_eq!(step(&approved, "appropriation")["status"], "done");
    let tax = step(&approved, "corporate_tax");
    assert_eq!(tax["status"], "todo");
    assert_eq!(tax["amount_cents"], 155_800);
    assert_eq!(tax["due_on"], "2028-01-15");
    assert_eq!(step(&approved, "liasse")["due_on"], "2027-12-30");
    assert_eq!(step(&approved, "filing")["due_on"], "2028-01-10");

    let liasse = render_everything(&db, "2027");
    assert_eq!(liasse_case(&liasse, "2065", "C1"), Some(1_038_800));
    assert_eq!(liasse_case(&liasse, "2033-B", "218"), Some(1_200_000));
    assert_eq!(liasse_case(&liasse, "2033-B", "306"), Some(155_800));
    assert_eq!(liasse_case(&liasse, "2065", "distributions"), Some(300_000));
    assert_eq!(liasse_case(&liasse, "2033-B", "360"), Some(81_600));
    assert_eq!(liasse_case(&liasse, "2033-D", "982"), Some(81_600));
    assert_eq!(liasse_case(&liasse, "2033-D", "983"), Some(81_600));
    assert_eq!(
        liasse_case(&liasse, "2033-D", "870"),
        None,
        "plus rien à reporter"
    );
    assert_eq!(liasse_case(&liasse, "2033-A", "180"), Some(1_618_800));
    let fec = std::fs::read_to_string(
        db.with_file_name("cloture-2027")
            .join(format!("{SIREN}FEC20270930.txt")),
    )
    .unwrap();
    assert!(
        fec.contains("AN|À-nouveaux|1|20261001|"),
        "à-nouveaux dérivés de la clôture 2026 :\n{fec}"
    );
    assert!(
        fec.contains("|20261215|129000|") || fec.contains("|20261215|119000|"),
        "affectation de la perte 2026 datée de l'approbation :\n{fec}"
    );
    assert!(fec.contains("|695000|"), "IS de clôture :\n{fec}");

    // L'exercice suivant démarre sur la chaîne à jour : c'est la preuve que rien ne se perd.
    let next = checklist(&db, "2028", "2027-12-15");
    assert_eq!(next["stage"], "not_ended");
    let previous = step(&next, "previous_year");
    assert_eq!(previous["status"], "done");
    assert!(
        previous["detail"].as_str().unwrap().contains(
            "report à nouveau 8\u{202f}130,00\u{a0}€, réserve légale cumulée 100,00\u{a0}€, \
             déficits reportables 0,00\u{a0}€"
        ),
        "{previous}"
    );
    unlocked(&db)
        .args(["audit", "verify-chain"])
        .assert()
        .success();

    // Et les mots du parcours sont expliqués, sans jargon, au même endroit.
    unlocked(&db)
        .args(["year", "checklist", "2028", "--today", "2027-12-15"])
        .assert()
        .success()
        .stdout(predicate::str::contains("freeflow year glossary"));
    unlocked(&db)
        .args(["year", "glossary"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Report à nouveau"))
        .stdout(predicate::str::contains("Dépôt des comptes au greffe"));
    let glossary = json_result(
        &unlocked(&db)
            .args(["--json", "year", "glossary"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone(),
    );
    assert!(glossary.as_array().unwrap().len() >= 15);
}
