//! Parcours réels via `assert_cmd` (le binaire compilé, pas la bibliothèque), snapshots
//! `insta` du contrat `--help` et des sorties `--json`, et vérification qu'un code de sortie
//! distinct existe par famille d'erreur.
//!
//! Ces tests lancent un vrai sous-processus (`assert_cmd`), donc ne peuvent pas injecter de
//! trousseau en mémoire (réservé aux tests dans le même process, via
//! `freeflow_core::store::testing`). `provision()` crée le coffre avec `--passphrase-file` et
//! `--remember` : une seule écriture dans le trousseau OS réel par coffre de test (identifié par
//! un `vault_id` aléatoire propre au sidecar — aucune collision possible entre exécutions), très
//! en-deçà du volume d'écritures d'avant ce lot (une par commande, implicitement).

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

/// Écrit une passphrase dans un fichier temporaire en 0600 (Unix) et renvoie son chemin.
fn passphrase_file(db: &Path, passphrase: &str) -> PathBuf {
    let path = db.with_extension("passphrase");
    std::fs::write(&path, passphrase).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    path
}

/// Crée le coffre et met la clé en cache dans le trousseau OS (`--remember`) : les commandes
/// suivantes de ce test, chacune un nouveau sous-processus, la retrouvent via
/// `Store::open_cached` sans avoir à repasser de passphrase.
fn provision(db: &Path) {
    let pass_file = passphrase_file(db, "s3cret");
    freeflow()
        .env("FREEFLOW_DB", db)
        .args(["--passphrase-file"])
        .arg(&pass_file)
        .args(["init", "--remember"])
        .assert()
        .success();
}

fn json_result(output: &[u8]) -> serde_json::Value {
    serde_json::from_slice(output).expect("une sortie --json doit être du JSON valide")
}

#[test]
fn golden_path_from_prospection_to_paid_invoice() {
    let db = temp_db("golden-path");
    provision(&db);

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
    provision(&db);
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
    provision(&db);
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
    provision(&db);
    freeflow()
        .env("FREEFLOW_DB", &db)
        .arg("lock")
        .assert()
        .success();
    freeflow()
        .env("FREEFLOW_DB", &db)
        // Positionnée sur le sous-processus, pas sur ce process de test : prouve que la
        // variable n'est plus lue nulle part, pas seulement qu'elle est absente de l'environnement.
        .env("FREEFLOW_PASSPHRASE", "s3cret")
        .args(["prospect", "pipeline"])
        .assert()
        .failure()
        .code(2);
}

#[test]
fn freeflow_passphrase_env_var_is_never_read_even_when_set() {
    let db = temp_db("passphrase-env-ignored");
    provision(&db);
    freeflow()
        .env("FREEFLOW_DB", &db)
        .arg("lock")
        .assert()
        .success();
    // Coffre verrouillé, FREEFLOW_PASSPHRASE positionnée mais non-interactif : doit échouer
    // avec le code « verrouillé », jamais réussir en lisant l'ancienne variable d'environnement.
    freeflow()
        .env("FREEFLOW_DB", &db)
        .env("FREEFLOW_PASSPHRASE", "s3cret")
        .args(["--non-interactive", "client", "list", "--json"])
        .assert()
        .failure()
        .code(2);
}

#[test]
fn invalid_lines_json_fails_with_its_own_exit_code() {
    let db = temp_db("invalid-lines-json");
    provision(&db);
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
fn init_refuses_an_existing_vault() {
    let db = temp_db("init-twice");
    provision(&db);
    let pass_file = passphrase_file(&db, "another");
    freeflow()
        .env("FREEFLOW_DB", &db)
        .args(["--passphrase-file"])
        .arg(&pass_file)
        .arg("init")
        .assert()
        .failure()
        .code(3); // StoreError::VaultAlreadyExists, via CliError::Store
}

#[test]
fn unlock_never_creates_a_vault() {
    let db = temp_db("unlock-no-create");
    let pass_file = passphrase_file(&db, "s3cret");
    freeflow()
        .env("FREEFLOW_DB", &db)
        .args(["--passphrase-file"])
        .arg(&pass_file)
        .arg("unlock")
        .assert()
        .failure()
        .code(7); // StoreError::VaultNotFound, via CliError::NoVault
    assert!(!db.exists(), "unlock ne doit jamais créer le coffre");
}

#[test]
fn a_command_with_no_passphrase_source_and_non_interactive_exits_2() {
    let db = temp_db("no-source-non-interactive");
    provision(&db);
    freeflow()
        .env("FREEFLOW_DB", &db)
        .arg("lock")
        .assert()
        .success();
    freeflow()
        .env("FREEFLOW_DB", &db)
        .args(["--non-interactive", "client", "list", "--json"])
        .assert()
        .failure()
        .code(2);
}

#[test]
fn a_passphrase_file_readable_by_others_is_refused() {
    let db = temp_db("insecure-passphrase-file");
    let pass_file = passphrase_file(&db, "s3cret");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&pass_file, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    let assertion = freeflow()
        .env("FREEFLOW_DB", &db)
        .args(["--passphrase-file"])
        .arg(&pass_file)
        .arg("init")
        .assert();
    #[cfg(unix)]
    assertion
        .failure()
        .stderr(predicate::str::contains("chmod 600"));
    #[cfg(not(unix))]
    let _ = assertion; // le contrôle de permissions est spécifique à Unix.
}

#[test]
fn vault_status_reports_absence_then_presence() {
    let db = temp_db("vault-status");
    let absent = freeflow()
        .env("FREEFLOW_DB", &db)
        .args(["vault", "status", "--json"])
        .output()
        .unwrap();
    let absent_value: serde_json::Value = serde_json::from_slice(&absent.stdout).unwrap();
    assert_eq!(absent_value["exists"], false);

    provision(&db);
    let present = freeflow()
        .env("FREEFLOW_DB", &db)
        .args(["vault", "status", "--json"])
        .output()
        .unwrap();
    let present_value: serde_json::Value = serde_json::from_slice(&present.stdout).unwrap();
    assert_eq!(present_value["exists"], true);
    assert_eq!(present_value["sidecar_version"], 2);
    assert!(present_value["session_expires_at"].is_string());
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
    provision(&db);
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
    provision(&db);
    let output = freeflow()
        .env("FREEFLOW_DB", &db)
        .args(["--json", "prospect", "pipeline"])
        .output()
        .unwrap();
    insta::assert_json_snapshot!(json_result(&output.stdout));
}
