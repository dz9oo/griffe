//! Coffre mock Lumen Conseil — SASU fictionnelle pour captures marketing.
//!
//! Toutes les mutations passent par la CLI (`Command`). Les montants attendus du mât et des
//! listes sont posés à la main après une première lecture `--json` du moteur.

mod common;
use std::path::{Path, PathBuf};

use common::{json_result, passphrase_file, temp_db, unlocked};
use griffe_core::store::{Passphrase, Store};

const TODAY: &str = "2026-09-05";
const SIREN: &str = "901265322";
const OPENING: &str = include_str!("../../../scripts/lumen/opening.txt");
const QONTO: &str = include_str!("../../../scripts/lumen/qonto-septembre.csv");

fn vault_path() -> PathBuf {
    std::env::var_os("GRIFFE_LUMEN_OUT").map_or_else(|| temp_db("lumen-demo"), PathBuf::from)
}

fn passphrase() -> String {
    std::env::var("GRIFFE_LUMEN_PASSPHRASE_FILE").map_or_else(
        |_| "s3cret".into(),
        |p| std::fs::read_to_string(p).unwrap().trim().to_string(),
    )
}

fn provision_lumen(db: &Path) {
    let pass = passphrase();
    let _ = passphrase_file(db, &pass);
    if db.exists() {
        panic!(
            "un coffre existe déjà à {} — seed.sh le retire avant",
            db.display()
        );
    }
    drop(Store::create(db, &Passphrase::from(pass.as_str())).unwrap());
}

fn json(db: &Path, args: &[&str]) -> serde_json::Value {
    let mut cmd = unlocked(db);
    cmd.arg("--json");
    cmd.args(args);
    json_result(&cmd.assert().success().get_output().stdout.clone())
}

fn result_id(db: &Path, args: &[&str]) -> String {
    let v = json(db, args);
    if let Some(s) = v["result"].as_str() {
        return s.to_string();
    }
    if let Some(s) = v["result"]["id"].as_str() {
        return s.to_string();
    }
    panic!("pas d'identifiant dans {v}");
}

fn tx_id(db: &Path, needle: &str) -> String {
    json(db, &["bank", "list", "--unmatched"])
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["description"].as_str().unwrap().contains(needle))
        .unwrap_or_else(|| panic!("transaction « {needle} » introuvable"))["id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn expense_from_tx(
    db: &Path,
    label: &str,
    category: &str,
    supplier: Option<&str>,
    vat_rate: &str,
    vat_deductible: &str,
    needle: &str,
) {
    let id = tx_id(db, needle);
    let mut cmd = unlocked(db);
    cmd.args([
        "expense",
        "record",
        "--label",
        label,
        "--category",
        category,
        "--vat-rate",
        vat_rate,
        "--vat-deductible",
        vat_deductible,
        "--transaction",
        &id,
    ]);
    if let Some(supplier) = supplier {
        cmd.args(["--supplier", supplier]);
    }
    cmd.assert().success();
}

fn emit_atlas(db: &Path, client: &str, mission: &str, issued_on: &str, days: &str) -> String {
    let lines = format!(
        r#"[{{"description":"Régie produit","quantity":{days},"unit_price":65000,"vat_rate":"Standard"}}]"#
    );
    result_id(
        db,
        &[
            "invoice",
            "emit",
            "--client",
            client,
            "--mission",
            mission,
            "--issued-on",
            issued_on,
            "--payment-terms-days",
            "30",
            "--lines",
            &lines,
        ],
    )
}

fn reconcile_income(db: &Path, invoice: &str, needle: &str) {
    let tx = tx_id(db, needle);
    unlocked(db)
        .args([
            "bank",
            "reconcile",
            "--transaction",
            &tx,
            "--invoice",
            invoice,
        ])
        .assert()
        .success();
}

fn seed(db: &Path) {
    provision_lumen(db);

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
            "--director-gross",
            "2800",
            "--director-charge-ratio",
            "45",
            "--iban",
            "FR7630006000011234567890189",
        ])
        .assert()
        .success();

    let opening = db.with_file_name("opening.txt");
    std::fs::write(&opening, OPENING).unwrap();
    unlocked(db)
        .args([
            "year",
            "opening",
            "set",
            "--opens-on",
            "2025-10-01",
            "--source",
            "bilan au 30/09/2025, cabinet Leroy",
            "--lines-file",
        ])
        .arg(&opening)
        .assert()
        .success();

    unlocked(db)
        .args([
            "follow-up",
            "from",
            "--email",
            "nora@lumen-conseil.example",
            "--name",
            "Nora Lumen",
        ])
        .assert()
        .success();

    let atlas = result_id(db, &["client", "create", "--name", "Atlas Digital"]);
    let hume = result_id(db, &["client", "create", "--name", "Studio Hume"]);

    let atlas_mission = result_id(
        db,
        &[
            "mission",
            "create",
            "--client",
            "Atlas Digital",
            "--name",
            "Régie produit",
            "--kind",
            "regie",
            "--daily-rate",
            "650",
            "--started-on",
            "2025-10-01",
        ],
    );
    let _hume_mission = result_id(
        db,
        &[
            "mission",
            "create",
            "--client",
            "Studio Hume",
            "--name",
            "Refonte maquettes",
            "--kind",
            "forfait",
            "--budget",
            "13250",
            "--started-on",
            "2026-07-01",
            "--milestone",
            "Maquettes:4000:2026-09-22",
        ],
    );

    unlocked(db)
        .args([
            "client",
            "contact",
            "add",
            "--client",
            "Atlas Digital",
            "--name",
            "Léa Marin",
            "--email",
            "lea@atlas-digital.example",
        ])
        .assert()
        .success();
    unlocked(db)
        .args([
            "client",
            "contact",
            "add",
            "--client",
            "Studio Hume",
            "--name",
            "Hugo Hume",
            "--email",
            "hugo@studio-hume.example",
        ])
        .assert()
        .success();

    let paid = [
        ("2025-10-31", "8", "oct 2025"),
        ("2025-11-28", "7", "nov 2025"),
        ("2025-12-31", "8", "dec 2025"),
        ("2026-01-30", "6", "jan 2026"),
        ("2026-02-27", "8", "feb 2026"),
        ("2026-03-31", "7", "mar 2026"),
        ("2026-04-30", "8", "apr 2026"),
        ("2026-05-29", "6", "may 2026"),
        ("2026-06-30", "8", "jun 2026"),
    ];
    let mut paid_ids = Vec::new();
    for (issued, days, tag) in paid {
        let id = emit_atlas(db, &atlas, &atlas_mission, issued, days);
        paid_ids.push((id, tag));
    }
    let overdue = emit_atlas(db, &atlas, &atlas_mission, "2026-07-30", "8");
    let _august = emit_atlas(db, &atlas, &atlas_mission, "2026-08-28", "6");

    let hume_inv = result_id(
        db,
        &[
            "invoice",
            "emit",
            "--client",
            &hume,
            "--mission",
            &_hume_mission,
            "--issued-on",
            "2026-08-18",
            "--payment-terms-days",
            "30",
            "--lines",
            r#"[{"description":"Jalon maquettes","quantity":1,"unit_price":530000,"vat_rate":"Standard"}]"#,
        ],
    );

    unlocked(db)
        .args([
            "prospect",
            "create",
            "--prospect",
            "Camille Rivière",
            "--email",
            "camille@atelier-nord.example",
            "--name",
            "Accompagnement identité",
            "--amount",
            "8400",
            "--probability",
            "60",
            "--next-action",
            TODAY,
            "--street",
            "12 quai Saint-Vincent",
            "--postal-code",
            "69001",
            "--city",
            "Lyon",
            "--country",
            "FR",
        ])
        .assert()
        .success();
    unlocked(db)
        .args([
            "prospect",
            "log-interaction",
            "Accompagnement identité",
            "--kind",
            "meeting",
            "--note",
            "Premier café, elle veut une identité pour Atelier Nord.",
            "--occurred-on",
            "2026-03-12",
        ])
        .assert()
        .success();
    unlocked(db)
        .args([
            "quote",
            "create",
            "--client",
            "Camille Rivière",
            "--opportunity",
            "Accompagnement identité",
            "--line",
            "Accompagnement identité:forfait:8400.00",
            "--valid-until",
            "2026-10-05",
        ])
        .assert()
        .success();
    unlocked(db)
        .args(["quote", "send", "Camille Rivière"])
        .assert()
        .success();

    let csv = db.with_file_name("qonto.csv");
    std::fs::write(&csv, QONTO).unwrap();
    unlocked(db)
        .args(["bank", "import", "--format", "csv"])
        .arg(&csv)
        .assert()
        .success();

    for (id, tag) in &paid_ids {
        reconcile_income(db, id, tag);
    }
    reconcile_income(db, &hume_inv, "Hume");

    for (needle, month) in [
        ("Qonto oct 2025", "octobre 2025"),
        ("Qonto nov 2025", "novembre 2025"),
        ("Qonto dec 2025", "décembre 2025"),
        ("Qonto jan 2026", "janvier 2026"),
        ("Qonto feb 2026", "février 2026"),
        ("Qonto mar 2026", "mars 2026"),
        ("Qonto apr 2026", "avril 2026"),
        ("Qonto may 2026", "mai 2026"),
        ("Qonto jun 2026", "juin 2026"),
        ("Qonto jul 2026", "juillet 2026"),
        ("Qonto aug 2026", "août 2026"),
    ] {
        expense_from_tx(
            db,
            &format!("Frais de tenue de compte {month}"),
            "bank_charges",
            None,
            "zero",
            "0",
            needle,
        );
    }
    for (needle, month) in [
        ("Atelier du Mail oct 2025", "octobre 2025"),
        ("Atelier du Mail nov 2025", "novembre 2025"),
        ("Atelier du Mail dec 2025", "décembre 2025"),
        ("Atelier du Mail jan 2026", "janvier 2026"),
        ("Atelier du Mail feb 2026", "février 2026"),
        ("Atelier du Mail mar 2026", "mars 2026"),
        ("Atelier du Mail apr 2026", "avril 2026"),
        ("Atelier du Mail may 2026", "mai 2026"),
        ("Atelier du Mail jun 2026", "juin 2026"),
        ("Atelier du Mail jul 2026", "juillet 2026"),
        ("Atelier du Mail aug 2026", "août 2026"),
    ] {
        expense_from_tx(
            db,
            &format!("Cowork {month}"),
            "office",
            Some("Atelier du Mail"),
            "standard",
            "70",
            needle,
        );
    }
    for (needle, month) in [
        ("Notion Labs oct 2025", "octobre 2025"),
        ("Notion Labs nov 2025", "novembre 2025"),
        ("Notion Labs dec 2025", "décembre 2025"),
        ("Notion Labs jan 2026", "janvier 2026"),
        ("Notion Labs feb 2026", "février 2026"),
        ("Notion Labs mar 2026", "mars 2026"),
        ("Notion Labs apr 2026", "avril 2026"),
        ("Notion Labs may 2026", "mai 2026"),
        ("Notion Labs jun 2026", "juin 2026"),
        ("Notion Labs jul 2026", "juillet 2026"),
        ("Notion Labs aug 2026", "août 2026"),
    ] {
        expense_from_tx(
            db,
            &format!("Notion {month}"),
            "software",
            None,
            "standard",
            "4.83",
            needle,
        );
    }
    for (needle, month) in [
        ("Nora Lumen oct 2025", "octobre 2025"),
        ("Nora Lumen nov 2025", "novembre 2025"),
        ("Nora Lumen dec 2025", "décembre 2025"),
        ("Nora Lumen jan 2026", "janvier 2026"),
        ("Nora Lumen feb 2026", "février 2026"),
        ("Nora Lumen mar 2026", "mars 2026"),
        ("Nora Lumen apr 2026", "avril 2026"),
        ("Nora Lumen may 2026", "mai 2026"),
        ("Nora Lumen jun 2026", "juin 2026"),
        ("Nora Lumen jul 2026", "juillet 2026"),
        ("Nora Lumen aug 2026", "août 2026"),
    ] {
        expense_from_tx(
            db,
            &format!("Rémunération présidente {month}"),
            "other",
            None,
            "zero",
            "0",
            needle,
        );
    }
    for (needle, month) in [
        ("URSSAF oct 2025", "octobre 2025"),
        ("URSSAF nov 2025", "novembre 2025"),
        ("URSSAF dec 2025", "décembre 2025"),
        ("URSSAF jan 2026", "janvier 2026"),
        ("URSSAF feb 2026", "février 2026"),
        ("URSSAF mar 2026", "mars 2026"),
        ("URSSAF apr 2026", "avril 2026"),
        ("URSSAF may 2026", "mai 2026"),
    ] {
        expense_from_tx(
            db,
            &format!("Cotisations URSSAF {month}"),
            "taxes",
            None,
            "zero",
            "0",
            needle,
        );
    }
    expense_from_tx(
        db,
        "Honoraires cabinet Leroy — bilan 2025",
        "fees",
        Some("Cabinet Leroy"),
        "standard",
        "240",
        "Cabinet Leroy 2025",
    );
    unlocked(db)
        .args([
            "expense",
            "record",
            "--label",
            "Honoraires cabinet Leroy — 2026",
            "--category",
            "fees",
            "--supplier",
            "Cabinet Leroy",
            "--amount",
            "1440",
            "--vat-rate",
            "standard",
            "--vat-deductible",
            "240",
            "--incurred-on",
            "2026-09-02",
        ])
        .assert()
        .success();

    unlocked(db)
        .args([
            "follow-up",
            "snooze",
            &overdue,
            "--until",
            "2026-09-20",
            "--today",
            TODAY,
        ])
        .assert()
        .success();

    for period in [
        "2025-10", "2025-11", "2025-12", "2026-01", "2026-02", "2026-03", "2026-04", "2026-05",
        "2026-06", "2026-07",
    ] {
        unlocked(db)
            .args([
                "society", "filed", "ca3", "--period", period, "--today", TODAY,
            ])
            .assert()
            .success();
    }
    for period in ["2025-12-15", "2026-03-15", "2026-06-15"] {
        unlocked(db)
            .args([
                "society",
                "filed",
                "is_acompte",
                "--period",
                period,
                "--today",
                TODAY,
            ])
            .assert()
            .success();
    }
}

#[test]
fn lumen_demo_vault_shows_a_living_sasu() {
    let db = vault_path();
    seed(&db);

    let mast = json(&db, &["day", "mast", "--today", TODAY]);
    assert_eq!(mast["open_conversations"], 1, "{mast}");
    let bank = mast["bank"].as_i64().expect("banque en centimes");
    assert!(
        (bank - 1_284_300).abs() < 50_000,
        "banque {bank} hors ~12 843 €"
    );
    let runway = mast["runway_months"].as_u64().expect("piste");
    assert!((3..=5).contains(&runway), "piste {runway} mois");
    assert_eq!(mast["receivables"][0]["party"], "Atlas Digital", "{mast}");
    let overdue = mast["receivables"][0]["days_overdue"].as_i64().unwrap();
    assert_eq!(overdue, 7, "retard Atlas {overdue}");

    let people = json(&db, &["people", "list", "--today", TODAY]);
    let conv = people["conversations"].as_array().unwrap();
    assert_eq!(conv.len(), 1, "{people}");
    assert_eq!(conv[0]["name"], "Camille Rivière", "{people}");
    let missions = people["missions"].as_array().unwrap();
    assert!(
        missions.iter().any(|m| m["name"] == "Atlas Digital"),
        "{people}"
    );
    assert!(
        missions.iter().any(|m| m["name"] == "Studio Hume"),
        "{people}"
    );
    let outgoing = people["outgoing"].as_array().unwrap();
    assert!(
        outgoing
            .iter()
            .any(|o| o["name"].as_str().unwrap().contains("Leroy")),
        "{people}"
    );

    let gestes = json(&db, &["day", "gestures", "--today", TODAY]);
    let verbs: Vec<&str> = gestes
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["verb"].as_str().unwrap())
        .collect();
    assert!(verbs.contains(&"write"), "écrire à Camille : {gestes}");
    assert!(
        verbs.contains(&"file_statement"),
        "ranger le relevé : {gestes}"
    );
    assert!(verbs.contains(&"know_vat"), "TVA : {gestes}");

    let unmatched = json(&db, &["bank", "list", "--unmatched"])
        .as_array()
        .unwrap()
        .len();
    assert_eq!(unmatched, 3, "trois mouvements non lus, pas {unmatched}");
}
