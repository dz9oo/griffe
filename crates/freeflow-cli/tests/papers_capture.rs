//! Lot 58 : émettre, importer, clôturer, approuver fige les originaux. Un re-rendu ne les
//! remplace pas.

use std::path::Path;

mod common;
use common::{create_client, freeflow, json_result, provision, temp_db};

fn set_company_profile(db: &Path) {
    freeflow()
        .env("FREEFLOW_DB", db)
        .args([
            "company",
            "set-profile",
            "--name",
            "Argon Digital",
            "--legal-form",
            "SASU",
            "--siren",
            "552100554",
            "--street",
            "12 rue de la Paix",
            "--postal-code",
            "75002",
            "--city",
            "Paris",
            "--country",
            "FR",
            "--share-capital",
            "1000",
            "--fiscal-year-end",
            "31/12",
        ])
        .assert()
        .success();
}

fn papers_of_kind(db: &Path, kind: &str) -> serde_json::Value {
    json_result(
        &freeflow()
            .env("FREEFLOW_DB", db)
            .args(["--json", "papers", "list", "--kind", kind])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone(),
    )
}

fn receipts_dir(db: &Path) -> std::path::PathBuf {
    let mut name = db.as_os_str().to_owned();
    name.push(".receipts");
    std::path::PathBuf::from(name)
}

#[test]
fn emitting_an_invoice_archives_the_facturx_and_a_later_render_does_not_replace_it() {
    let db = temp_db("papers-emit");
    provision(&db);
    set_company_profile(&db);
    let client_id = create_client(&db, "Kappa Software");
    let lines =
        r#"[{"description":"Prestation","quantity":1,"unit_price":100000,"vat_rate":"Standard"}]"#;
    let emit_out = json_result(
        &freeflow()
            .env("FREEFLOW_DB", &db)
            .args([
                "--json",
                "invoice",
                "emit",
                "--client",
                &client_id,
                "--lines",
                lines,
                "--issued-on",
                "2026-03-10",
            ])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone(),
    );
    let invoice_id = emit_out["result"]["id"].as_str().unwrap().to_string();
    let listed = papers_of_kind(&db, "issued_invoice");
    assert_eq!(listed.as_array().unwrap().len(), 1, "{listed}");
    assert_eq!(listed[0]["origin"], "issued");
    assert_eq!(listed[0]["invoice_id"], invoice_id);
    let first_hash = listed[0]["content_hash"].as_str().unwrap().to_string();
    let first_id = listed[0]["id"].as_str().unwrap().to_string();
    let dir = receipts_dir(&db);
    assert!(dir.is_dir(), "{}", dir.display());
    let files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    assert_eq!(files.len(), 1, "un seul original chiffré");

    let copy = db.with_file_name("re-rendu.pdf");
    freeflow()
        .env("FREEFLOW_DB", &db)
        .args(["invoice", "render", "--id", &invoice_id, "--out"])
        .arg(&copy)
        .assert()
        .success();
    assert!(copy.exists());
    let listed = papers_of_kind(&db, "issued_invoice");
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["id"], first_id);
    assert_eq!(listed[0]["content_hash"], first_hash);
    let files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    assert_eq!(files.len(), 1, "le re-rendu n'écrit pas un second blob");
}

#[test]
fn bank_import_archives_the_source_file() {
    let db = temp_db("papers-bank");
    provision(&db);
    let statement = db.with_file_name("releve.csv");
    std::fs::write(
        &statement,
        "date;description;montant\n2026-03-15;Frais;-12.50\n",
    )
    .unwrap();
    freeflow()
        .env("FREEFLOW_DB", &db)
        .args(["bank", "import"])
        .arg(&statement)
        .assert()
        .success();
    let listed = papers_of_kind(&db, "bank_statement");
    assert_eq!(listed.as_array().unwrap().len(), 1, "{listed}");
    assert_eq!(listed[0]["origin"], "imported");
    assert_eq!(listed[0]["original_name"], "releve.csv");
    assert_eq!(listed[0]["kind"], "bank_statement");
}

#[test]
fn approving_a_year_archives_minutes_appropriation_fec_and_liasse() {
    let db = temp_db("papers-year");
    provision(&db);
    set_company_profile(&db);
    freeflow()
        .env("FREEFLOW_DB", &db)
        .args(["year", "close", "--period", "2026", "--today", "2027-01-05"])
        .assert()
        .success();
    freeflow()
        .env("FREEFLOW_DB", &db)
        .args([
            "year",
            "approve",
            "2026",
            "--approved-on",
            "2027-05-15",
            "--today",
            "2027-06-01",
        ])
        .assert()
        .success();
    for kind in [
        "minutes",
        "appropriation",
        "synthesis",
        "balance_sheet",
        "inventory",
        "efi_notice",
        "liasse",
        "fec",
    ] {
        let listed = papers_of_kind(&db, kind);
        assert_eq!(
            listed.as_array().unwrap().len(),
            1,
            "attendu un original {kind}, obtenu {listed}"
        );
        assert_eq!(listed[0]["origin"], "issued", "{kind}");
        assert_eq!(listed[0]["period"], 2026, "{kind}");
    }
}

#[test]
fn papers_checklist_json_shows_issued_invoice_todo_then_done() {
    let db = temp_db("papers-checklist");
    provision(&db);
    let client_id = create_client(&db, "Kappa Software");
    let lines =
        r#"[{"description":"Prestation","quantity":1,"unit_price":100000,"vat_rate":"Standard"}]"#;
    let emit_out = json_result(
        &freeflow()
            .env("FREEFLOW_DB", &db)
            .args([
                "--json",
                "invoice",
                "emit",
                "--client",
                &client_id,
                "--lines",
                lines,
                "--issued-on",
                "2026-03-10",
            ])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone(),
    );
    let invoice_id = emit_out["result"]["id"].as_str().unwrap().to_string();
    assert!(
        papers_of_kind(&db, "issued_invoice")
            .as_array()
            .unwrap()
            .is_empty(),
        "sans profil, l'émission n'a pas figé l'original"
    );
    set_company_profile(&db);
    freeflow()
        .env("FREEFLOW_DB", &db)
        .args(["year", "close", "--period", "2026", "--today", "2027-01-05"])
        .assert()
        .success();
    freeflow()
        .env("FREEFLOW_DB", &db)
        .args([
            "year",
            "approve",
            "2026",
            "--approved-on",
            "2027-05-15",
            "--today",
            "2027-06-01",
        ])
        .assert()
        .success();

    let before = json_result(
        &freeflow()
            .env("FREEFLOW_DB", &db)
            .args([
                "--json",
                "papers",
                "checklist",
                "2026",
                "--today",
                "2027-06-01",
            ])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone(),
    );
    let invoice_item = before["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["kind"] == "issued_invoice")
        .expect("une ligne issued_invoice");
    assert_eq!(invoice_item["status"], "todo", "{before}");
    assert_eq!(invoice_item["required"], true);

    let pdf = db.with_file_name("FA.pdf");
    freeflow()
        .env("FREEFLOW_DB", &db)
        .args(["invoice", "render", "--id", &invoice_id, "--out"])
        .arg(&pdf)
        .assert()
        .success();

    let after = json_result(
        &freeflow()
            .env("FREEFLOW_DB", &db)
            .args([
                "--json",
                "papers",
                "checklist",
                "2026",
                "--today",
                "2027-06-01",
            ])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone(),
    );
    let invoice_item = after["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["kind"] == "issued_invoice")
        .expect("une ligne issued_invoice");
    assert_eq!(invoice_item["status"], "done", "{after}");
}
