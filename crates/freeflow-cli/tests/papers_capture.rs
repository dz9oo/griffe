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

/// Lot 60 : un dossier neuf, un inventaire, un PDF relisible, pas d'écrasement.
#[test]
fn papers_export_writes_a_cleartext_pack_and_refuses_to_overwrite() {
    let db = temp_db("papers-export");
    provision(&db);
    let pdf = db.with_file_name("statuts.pdf");
    std::fs::write(&pdf, b"%PDF-1.4 statuts SASU").unwrap();
    freeflow()
        .env("FREEFLOW_DB", &db)
        .args(["papers", "add"])
        .arg(&pdf)
        .args(["--kind", "statutes"])
        .assert()
        .success();

    let dest = db.with_file_name("pack");
    let text = String::from_utf8(
        freeflow()
            .env("FREEFLOW_DB", &db)
            .args(["papers", "export", "2026", "--out"])
            .arg(&dest)
            .args(["--today", "2026-09-09"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone(),
    )
    .unwrap();
    assert!(text.contains("Dossier d'un contrôle"), "{text}");
    assert!(
        text.contains("Ces fichiers ne sont plus chiffrés"),
        "{text}"
    );

    let inventory = dest.join("inventaire.txt");
    let inventory_text = std::fs::read_to_string(&inventory).unwrap();
    assert!(
        inventory_text.contains("Dossier d'un contrôle — exercice 2026"),
        "{inventory_text}"
    );
    assert!(inventory_text.contains("statuts"), "{inventory_text}");

    let mut found_pdf = false;
    for entry in walkdir_files(&dest) {
        let bytes = std::fs::read(&entry).unwrap();
        if bytes.starts_with(b"%PDF") {
            found_pdf = true;
            break;
        }
    }
    assert!(
        found_pdf,
        "au moins un PDF relisible dans {}",
        dest.display()
    );

    let json = json_result(
        &freeflow()
            .env("FREEFLOW_DB", &db)
            .args(["--json", "papers", "export", "2026", "--out"])
            .arg(db.with_file_name("pack-json"))
            .assert()
            .success()
            .get_output()
            .stdout
            .clone(),
    );
    assert_eq!(json["period"], 2026);
    assert!(json["files"].as_u64().unwrap() >= 2, "{json}");
    assert!(
        json["warning"]
            .as_str()
            .unwrap()
            .contains("ne sont plus chiffrés"),
        "{json}"
    );

    let stderr = String::from_utf8_lossy(
        &freeflow()
            .env("FREEFLOW_DB", &db)
            .args(["papers", "export", "2026", "--out"])
            .arg(&dest)
            .assert()
            .failure()
            .code(4)
            .get_output()
            .stderr,
    )
    .into_owned();
    assert!(stderr.contains("existe déjà"), "{stderr}");
}

fn walkdir_files(root: &Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    fn walk(dir: &Path, files: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, files);
            } else {
                files.push(path);
            }
        }
    }
    walk(root, &mut files);
    files
}
