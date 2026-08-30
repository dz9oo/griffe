//! Parcours réels via `assert_cmd` (le binaire compilé, pas la bibliothèque), snapshots
//! `insta` du contrat `--help` et des sorties `--json`, et vérification qu'un code de sortie
//! distinct existe par famille d'erreur.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use assert_cmd::Command;
use predicates::prelude::*;

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn temp_db(label: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir().join(format!(
        "freeflow-cli-test-{label}-{}-{n}",
        std::process::id()
    ))
}

fn freeflow() -> Command {
    Command::cargo_bin("freeflow").expect("le binaire freeflow doit être compilé pour les tests")
}

fn unlock(db: &Path) {
    freeflow()
        .env("FREEFLOW_DB", db)
        .env("FREEFLOW_PASSPHRASE", "s3cret")
        .arg("unlock")
        .assert()
        .success();
}

fn json_result(output: &[u8]) -> serde_json::Value {
    serde_json::from_slice(output).expect("une sortie --json doit être du JSON valide")
}

#[test]
fn golden_path_from_prospection_to_paid_invoice() {
    let db = temp_db("golden-path");
    unlock(&db);

    let client_out = freeflow()
        .env("FREEFLOW_DB", &db)
        .args(["--json", "client", "create", "--name", "Kappa Software"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let client_id = json_result(&client_out)["result"]
        .as_str()
        .unwrap()
        .to_string();

    let opp_out = freeflow()
        .env("FREEFLOW_DB", &db)
        .args([
            "--json",
            "prospect",
            "create",
            "--client",
            &client_id,
            "--name",
            "Mission régie 6 mois",
            "--amount",
            "78000",
            "--probability",
            "40",
            "--next-action",
            "2026-09-02",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let opportunity_id = json_result(&opp_out)["result"]
        .as_str()
        .unwrap()
        .to_string();

    let win_out = freeflow()
        .env("FREEFLOW_DB", &db)
        .args([
            "--json",
            "prospect",
            "win",
            "--id",
            &opportunity_id,
            "--started-on",
            "2026-09-03",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let mission_id = json_result(&win_out)["result"]
        .as_str()
        .unwrap()
        .to_string();

    freeflow()
        .env("FREEFLOW_DB", &db)
        .args([
            "mission",
            "log-time",
            "--mission",
            &mission_id,
            "--worked-on",
            "2026-09-10",
            "--days",
            "9.5",
            "--category",
            "billable",
        ])
        .assert()
        .success();

    let lines =
        r#"[{"description":"Prestation","quantity":9.5,"unit_price":65000,"vat_rate":"Standard"}]"#;

    // Un agent ne peut pas émettre directement : l'action reste en attente.
    let pending_out = freeflow()
        .env("FREEFLOW_DB", &db)
        .args([
            "--json",
            "--actor",
            "agent:test-session",
            "invoice",
            "emit",
            "--client",
            &client_id,
            "--mission",
            &mission_id,
            "--lines",
            lines,
            "--issued-on",
            "2026-09-30",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let pending = json_result(&pending_out);
    assert_eq!(pending["status"], "pending_confirmation");
    let pending_id = pending["pending_action_id"].as_str().unwrap().to_string();

    // Confirmation humaine : l'action en attente s'applique désormais.
    let confirm_out = freeflow()
        .env("FREEFLOW_DB", &db)
        .args(["--json", "confirm", "--id", &pending_id])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let confirmed = json_result(&confirm_out);
    assert_eq!(confirmed["status"], "applied");
    let invoice_id = confirmed["result"]["id"].as_str().unwrap().to_string();
    assert_eq!(confirmed["result"]["number"], "FA-2026-0001");

    freeflow()
        .env("FREEFLOW_DB", &db)
        .args(["invoice", "verify-chain"])
        .assert()
        .success();
    freeflow()
        .env("FREEFLOW_DB", &db)
        .args(["audit", "verify-chain"])
        .assert()
        .success();

    freeflow()
        .env("FREEFLOW_DB", &db)
        .args([
            "payment",
            "record",
            "--invoice",
            &invoice_id,
            "--amount",
            "7410.00",
            "--received-on",
            "2026-10-05",
            "--method",
            "bank_transfer",
        ])
        .assert()
        .success();

    let aged_out = freeflow()
        .env("FREEFLOW_DB", &db)
        .args(["--json", "invoice", "aged-balance", "--today", "2026-10-06"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let aged = json_result(&aged_out);
    assert!(
        aged.as_array().unwrap().is_empty(),
        "la facture entièrement payée ne doit plus apparaître dans la balance âgée"
    );
}

#[test]
fn emitting_an_invoice_with_no_lines_fails_with_the_domain_exit_code() {
    let db = temp_db("empty-invoice");
    unlock(&db);
    freeflow()
        .env("FREEFLOW_DB", &db)
        .args([
            "invoice",
            "emit",
            "--client",
            "00000000-0000-7000-8000-000000000000",
            "--lines",
            "[]",
            "--issued-on",
            "2026-09-30",
        ])
        .assert()
        .failure()
        .code(4)
        .stderr(predicate::str::contains("au moins une ligne"));
}

#[test]
fn a_missing_invoice_fails_with_the_domain_exit_code() {
    let db = temp_db("missing-invoice");
    unlock(&db);
    freeflow()
        .env("FREEFLOW_DB", &db)
        .args([
            "invoice",
            "credit-note",
            "--id",
            "00000000-0000-7000-8000-000000000000",
            "--issued-on",
            "2026-09-30",
        ])
        .assert()
        .failure()
        .code(4)
        .stderr(predicate::str::contains("introuvable"));
}

#[test]
fn a_locked_vault_fails_with_its_own_exit_code() {
    let db = temp_db("locked-vault");
    unlock(&db);
    freeflow()
        .env("FREEFLOW_DB", &db)
        .arg("lock")
        .assert()
        .success();
    freeflow()
        .env("FREEFLOW_DB", &db)
        .args(["prospect", "pipeline"])
        .assert()
        .failure()
        .code(2);
}

#[test]
fn invalid_lines_json_fails_with_its_own_exit_code() {
    let db = temp_db("invalid-lines-json");
    unlock(&db);
    freeflow()
        .env("FREEFLOW_DB", &db)
        .args([
            "invoice",
            "emit",
            "--client",
            "00000000-0000-7000-8000-000000000000",
            "--lines",
            "pas du json",
            "--issued-on",
            "2026-09-30",
        ])
        .assert()
        .failure()
        .code(6);
}

#[test]
fn a_bad_argument_fails_with_clap_s_own_usage_exit_code() {
    freeflow()
        .args(["prospect", "create", "--unknown-flag"])
        .assert()
        .failure()
        .code(2);
}

#[test]
fn top_level_help_is_a_stable_interface_contract() {
    let output = freeflow().arg("--help").output().unwrap();
    insta::assert_snapshot!(String::from_utf8(output.stdout).unwrap());
}

#[test]
fn invoice_help_is_a_stable_interface_contract() {
    let output = freeflow().args(["invoice", "--help"]).output().unwrap();
    insta::assert_snapshot!(String::from_utf8(output.stdout).unwrap());
}

#[test]
fn client_create_json_output_matches_the_documented_shape() {
    let db = temp_db("json-shape");
    unlock(&db);
    let output = freeflow()
        .env("FREEFLOW_DB", &db)
        .args(["--json", "client", "create", "--name", "Kappa Software"])
        .output()
        .unwrap();
    let value = json_result(&output.stdout);
    insta::assert_json_snapshot!(value, { ".result" => "[client_id]" });
}

#[test]
fn empty_pipeline_json_output_matches_the_documented_shape() {
    let db = temp_db("pipeline-shape");
    unlock(&db);
    let output = freeflow()
        .env("FREEFLOW_DB", &db)
        .args(["--json", "prospect", "pipeline"])
        .output()
        .unwrap();
    insta::assert_json_snapshot!(json_result(&output.stdout));
}
