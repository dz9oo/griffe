//! Parcours réels via `assert_cmd` (le binaire compilé, pas la bibliothèque), snapshots
//! `insta` du contrat `--help` et des sorties `--json`, et vérification qu'un code de sortie
//! distinct existe par famille d'erreur.
//!
//! Les aides (coffre temporaire, `provision`, lecture JSON) vivent dans `common/mod.rs`,
//! partagées avec le scénario de preuve de bout en bout (`closing_scenario.rs`, lot 35).

use std::path::{Path, PathBuf};

use predicates::prelude::*;

mod common;
use common::{create_client, freeflow, json_result, passphrase_file, provision, temp_db, unlocked};

fn papers_of_kind(db: &Path, kind: &str) -> serde_json::Value {
    json_result(
        &unlocked(db)
            .args(["--json", "papers", "list", "--kind", kind])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone(),
    )
}

#[test]
fn golden_path_from_prospection_to_paid_invoice() {
    let db = temp_db("golden-path");
    provision(&db);

    let client_out = unlocked(&db)
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

    let opp_out = unlocked(&db)
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

    let win_out = unlocked(&db)
        .args([
            "--json",
            "prospect",
            "win",
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

    unlocked(&db)
        .args([
            "mission",
            "log-time",
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
    let pending_out = unlocked(&db)
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
    let confirm_out = unlocked(&db)
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

    unlocked(&db)
        .args(["invoice", "verify-chain"])
        .assert()
        .success();
    unlocked(&db)
        .args(["audit", "verify-chain"])
        .assert()
        .success();

    unlocked(&db)
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

    let aged_out = unlocked(&db)
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
    unlocked(&db)
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
    unlocked(&db)
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
    unlocked(&db).arg("lock").assert().success();
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
    unlocked(&db).arg("lock").assert().success();
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
    unlocked(&db)
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
    unlocked(&db).arg("lock").assert().success();
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
    let present = unlocked(&db)
        .args(["vault", "status", "--json"])
        .output()
        .unwrap();
    let present_value: serde_json::Value = serde_json::from_slice(&present.stdout).unwrap();
    assert_eq!(present_value["exists"], true);
    // Un coffre neuf est v3 depuis le lot 24 : clé maître enveloppée, changement de passphrase
    // sans re-chiffrement.
    assert_eq!(present_value["sidecar_version"], 3);
    // Session OS : présente avec un trousseau, `null` sur un runner CI (pas de Secret Service).
    assert!(
        present_value["session_expires_at"].is_string()
            || present_value["session_expires_at"].is_null()
    );
}

/// Écrit une NOUVELLE passphrase dans un fichier temporaire distinct de celui de `provision()`,
/// en 0600 (Unix) — même idiome que [`passphrase_file`].
fn new_passphrase_file(db: &Path, passphrase: &str) -> PathBuf {
    let path = db.with_extension("new-passphrase");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, passphrase).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    path
}

#[test]
fn passphrase_change_rotates_the_passphrase_end_to_end() {
    let db = temp_db("passphrase-change-rotates");
    provision(&db);
    let old_file = passphrase_file(&db, "s3cret");
    let new_file = new_passphrase_file(&db, "new-s3cret");

    freeflow()
        .env("FREEFLOW_DB", &db)
        .args(["--passphrase-file"])
        .arg(&old_file)
        .arg("passphrase")
        .arg("change")
        .arg("--new-passphrase-file")
        .arg(&new_file)
        .assert()
        .success();

    // L'ancienne passphrase n'ouvre plus le coffre.
    freeflow()
        .env("FREEFLOW_DB", &db)
        .args(["--passphrase-file"])
        .arg(&old_file)
        .args(["--non-interactive", "client", "list", "--json"])
        .assert()
        .failure()
        .code(3); // StoreError::WrongPassphrase, via CliError::Store

    // La nouvelle passphrase, si.
    freeflow()
        .env("FREEFLOW_DB", &db)
        .args(["--passphrase-file"])
        .arg(&new_file)
        .args(["client", "list", "--json"])
        .assert()
        .success();
}

#[test]
fn passphrase_change_refuses_a_cached_session_as_proof_of_the_old_passphrase() {
    let db = temp_db("passphrase-change-needs-old");
    provision(&db); // laisse une session active dans le trousseau OS
    let new_file = new_passphrase_file(&db, "new-s3cret");

    // Aucune source pour l'ANCIENNE passphrase, et non-interactif : la session en cache ne
    // peut pas en tenir lieu, même si elle prouve la possession de la clé.
    unlocked(&db)
        .args([
            "--non-interactive",
            "passphrase",
            "change",
            "--new-passphrase-file",
        ])
        .arg(&new_file)
        .assert()
        .failure()
        .code(2); // CliError::Locked

    // Rien n'a changé : la passphrase d'origine ouvre toujours le coffre.
    let old_file = passphrase_file(&db, "s3cret");
    freeflow()
        .env("FREEFLOW_DB", &db)
        .args(["--passphrase-file"])
        .arg(&old_file)
        .args(["client", "list", "--json"])
        .assert()
        .success();
}

#[test]
fn passphrase_change_dry_run_writes_nothing() {
    let db = temp_db("passphrase-change-dry-run");
    provision(&db);
    let old_file = passphrase_file(&db, "s3cret");
    let new_file = new_passphrase_file(&db, "new-s3cret");

    let output = freeflow()
        .env("FREEFLOW_DB", &db)
        .args(["--json", "--passphrase-file"])
        .arg(&old_file)
        .args(["--dry-run", "passphrase", "change", "--new-passphrase-file"])
        .arg(&new_file)
        .output()
        .unwrap();
    assert!(output.status.success());
    let value = json_result(&output.stdout);
    assert_eq!(value["changed"], false);
    // Coffre v3 : le dry-run annonce un simple ré-enveloppement, zéro octet à ré-chiffrer.
    assert_eq!(value["reencrypts_database"], false);
    assert_eq!(value["bytes_to_reencrypt"], 0);

    // L'ancienne passphrase ouvre toujours le coffre.
    freeflow()
        .env("FREEFLOW_DB", &db)
        .args(["--passphrase-file"])
        .arg(&old_file)
        .args(["client", "list", "--json"])
        .assert()
        .success();

    let staged = db.with_file_name(format!(
        "{}.kdf.new",
        db.file_name().unwrap().to_string_lossy()
    ));
    assert!(
        !staged.exists(),
        "--dry-run ne doit jamais écrire de sidecar en attente"
    );
    let backups_dir = db.with_file_name("backups");
    let has_prechange_backup = std::fs::read_dir(&backups_dir)
        .map(|entries| {
            entries.filter_map(Result::ok).any(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("pre-passphrase-change-")
            })
        })
        .unwrap_or(false);
    assert!(
        !has_prechange_backup,
        "--dry-run ne doit écrire aucune sauvegarde préalable"
    );
}

#[test]
fn passphrase_change_writes_a_backup_and_names_it_in_its_json_output() {
    let db = temp_db("passphrase-change-json");
    provision(&db);
    let old_file = passphrase_file(&db, "s3cret");
    let new_file = new_passphrase_file(&db, "new-s3cret");

    let output = freeflow()
        .env("FREEFLOW_DB", &db)
        .args(["--json", "--passphrase-file"])
        .arg(&old_file)
        .arg("passphrase")
        .arg("change")
        .arg("--new-passphrase-file")
        .arg(&new_file)
        .output()
        .unwrap();
    let value = json_result(&output.stdout);
    assert_eq!(value["changed"], true);
    assert_eq!(value["dry_run"], false);
    assert_eq!(
        value["reencrypted"], false,
        "un coffre v3 change de passphrase sans ré-chiffrer la base"
    );
    assert_eq!(value["argon2"]["m_cost"], 65536);
    let backup_path = value["backup"].as_str().unwrap();
    assert!(
        Path::new(backup_path).exists(),
        "le chemin de sauvegarde renvoyé doit exister : {backup_path}"
    );
    assert!(backup_path.contains("pre-passphrase-change-"));
}

#[test]
fn the_audit_chain_stays_intact_across_a_passphrase_change() {
    let db = temp_db("passphrase-change-audit");
    provision(&db);
    let old_file = passphrase_file(&db, "s3cret");
    let new_file = new_passphrase_file(&db, "new-s3cret");

    freeflow()
        .env("FREEFLOW_DB", &db)
        .args(["--passphrase-file"])
        .arg(&old_file)
        .arg("passphrase")
        .arg("change")
        .arg("--new-passphrase-file")
        .arg(&new_file)
        .assert()
        .success();

    freeflow()
        .env("FREEFLOW_DB", &db)
        .args(["--passphrase-file"])
        .arg(&new_file)
        .args(["audit", "verify-chain"])
        .assert()
        .success();
}

#[test]
fn a_new_passphrase_file_readable_by_others_is_refused() {
    let db = temp_db("passphrase-change-insecure-new-file");
    provision(&db);
    let old_file = passphrase_file(&db, "s3cret");
    let new_file = new_passphrase_file(&db, "new-s3cret");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&new_file, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    let assertion = freeflow()
        .env("FREEFLOW_DB", &db)
        .args(["--passphrase-file"])
        .arg(&old_file)
        .arg("passphrase")
        .arg("change")
        .arg("--new-passphrase-file")
        .arg(&new_file)
        .assert();
    #[cfg(unix)]
    assertion
        .failure()
        .stderr(predicate::str::contains("chmod 600"));
    #[cfg(not(unix))]
    let _ = assertion;
}

#[test]
fn follow_up_queue_lists_a_due_opportunity_and_drafts_without_sending() {
    let db = temp_db("follow-up-queue");
    provision(&db);
    unlocked(&db)
        .args(["client", "create", "--name", "Acme"])
        .assert()
        .success();
    unlocked(&db)
        .args([
            "client",
            "contact",
            "add",
            "--client",
            "Acme",
            "--name",
            "Marie",
            "--email",
            "marie@acme.test",
        ])
        .assert()
        .success();
    unlocked(&db)
        .args([
            "prospect",
            "create",
            "--client",
            "Acme",
            "--name",
            "Refonte",
            "--amount",
            "4500",
            "--probability",
            "40",
            "--next-action",
            "2026-09-02",
        ])
        .assert()
        .success();
    unlocked(&db)
        .args([
            "follow-up",
            "from",
            "--email",
            "nicolas@lumen.test",
            "--name",
            "Nicolas",
        ])
        .assert()
        .success();
    let queue = unlocked(&db)
        .args(["--json", "follow-up", "queue", "--today", "2026-09-05"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let cards = json_result(&queue);
    assert_eq!(cards.as_array().unwrap().len(), 1);
    assert_eq!(cards[0]["title"], "Refonte");
    unlocked(&db)
        .args([
            "follow-up",
            "draft",
            "Refonte",
            "--today",
            "2026-09-05",
            "--no-open",
        ])
        .assert()
        .success();
    unlocked(&db)
        .args(["follow-up", "sent", "Refonte", "--today", "2026-09-05"])
        .assert()
        .success();
    let show = unlocked(&db)
        .args([
            "--json",
            "follow-up",
            "show",
            "Refonte",
            "--today",
            "2026-09-05",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let card = json_result(&show);
    assert_eq!(card["step_key"], "bump");
}

#[test]
fn day_mast_json_on_an_empty_vault_has_zero_bank_and_typed_signals() {
    let db = temp_db("day-mast");
    provision(&db);
    let output = unlocked(&db)
        .args(["--json", "day", "mast", "--today", "2026-09-05"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(value["bank"], 0);
    assert!(value["runway_months"].is_null(), "{value}");
    assert_eq!(value["open_conversations"], 0);
    let signals = value["signals"].as_array().unwrap();
    assert!(
        signals.iter().any(|s| s["kind"] == "pipeline_empty"),
        "{value}"
    );
}

#[test]
fn day_help_is_a_stable_interface_contract() {
    let output = freeflow().args(["day", "--help"]).output().unwrap();
    insta::assert_snapshot!(String::from_utf8(output.stdout).unwrap());
}

#[test]
fn people_help_is_a_stable_interface_contract() {
    let output = freeflow().args(["people", "--help"]).output().unwrap();
    insta::assert_snapshot!(String::from_utf8(output.stdout).unwrap());
}

#[test]
fn society_help_is_a_stable_interface_contract() {
    let output = freeflow().args(["society", "--help"]).output().unwrap();
    insta::assert_snapshot!(String::from_utf8(output.stdout).unwrap());
}

#[test]
fn society_duty_is_acompte_json_on_an_empty_vault_has_the_path() {
    let db = temp_db("society-duty");
    provision(&db);
    let output = unlocked(&db)
        .args([
            "--json",
            "society",
            "duty",
            "is_acompte",
            "--today",
            "2026-09-05",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(value["kind"], "IsAcompte");
    assert_eq!(value["form"], "2571");
    assert_eq!(value["amount"]["kind"], "unknown");
    assert_eq!(value["amount"]["reason"], "no_profile");
    let path = value["path"].as_array().expect("path");
    assert_eq!(path[0], "Déclarer");
    assert_eq!(path[1], "Impôt sur les sociétés");
    let boxes = value["boxes"].as_array().expect("boxes");
    assert_eq!(boxes[0]["case"], "03");
    assert_eq!(boxes[1]["case"], "10");
}

#[test]
fn society_vat_credit_seeds_the_next_ca3() {
    let db = temp_db("society-vat-credit");
    provision(&db);
    unlocked(&db)
        .args([
            "company",
            "set-profile",
            "--name",
            "Lumen Conseil",
            "--legal-form",
            "SASU",
            "--siren",
            "552100554",
            "--street",
            "18 rue des Ateliers",
            "--postal-code",
            "69003",
            "--city",
            "Lyon",
            "--country",
            "FR",
            "--fiscal-year-end",
            "30/09",
            "--vat-regime",
            "real_normal_monthly",
        ])
        .assert()
        .success();
    unlocked(&db)
        .args([
            "society",
            "vat-credit",
            "set",
            "--after",
            "2026-08",
            "--amount",
            "324.00",
        ])
        .assert()
        .success();
    let show = unlocked(&db)
        .args(["--json", "society", "vat-credit", "show"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: serde_json::Value = serde_json::from_slice(&show).unwrap();
    assert_eq!(value["after_period"], "2026-08");
    assert_eq!(value["credit"], 32400);
    let duty = unlocked(&db)
        .args([
            "--json",
            "society",
            "duty",
            "ca3",
            "--period",
            "2026-09",
            "--today",
            "2026-09-08",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let duty: serde_json::Value = serde_json::from_slice(&duty).unwrap();
    let boxes = duty["boxes"].as_array().expect("boxes");
    let case22 = boxes.iter().find(|b| b["case"] == "22").expect("case 22");
    assert_eq!(case22["amount"], 32400);
    let case25 = boxes.iter().find(|b| b["case"] == "25").expect("case 25");
    assert_eq!(case25["amount"], 32400);
    let case27 = boxes.iter().find(|b| b["case"] == "27").expect("case 27");
    assert_eq!(case27["amount"], 32400);
    let again = unlocked(&db)
        .args([
            "society",
            "vat-credit",
            "set",
            "--after",
            "2026-08",
            "--amount",
            "100.00",
        ])
        .output()
        .unwrap();
    assert!(!again.status.success(), "le crédit repris est figé");
    let stderr = String::from_utf8_lossy(&again.stderr);
    assert!(
        stderr.contains("figé") || stderr.contains("existe déjà"),
        "{stderr}"
    );
}

#[test]
fn society_vat_refund_on_december_clears_january() {
    let db = temp_db("society-vat-refund");
    provision(&db);
    unlocked(&db)
        .args([
            "company",
            "set-profile",
            "--name",
            "Lumen Conseil",
            "--legal-form",
            "SASU",
            "--siren",
            "552100554",
            "--street",
            "18 rue des Ateliers",
            "--postal-code",
            "69003",
            "--city",
            "Lyon",
            "--country",
            "FR",
            "--fiscal-year-end",
            "30/09",
            "--vat-regime",
            "real_normal_monthly",
        ])
        .assert()
        .success();
    unlocked(&db)
        .args([
            "society",
            "vat-credit",
            "set",
            "--after",
            "2026-08",
            "--amount",
            "324.00",
        ])
        .assert()
        .success();

    let too_soon = unlocked(&db)
        .args([
            "society",
            "vat-refund",
            "request",
            "2026-09",
            "--amount",
            "324.00",
            "--today",
            "2026-09-08",
        ])
        .output()
        .unwrap();
    assert!(!too_soon.status.success(), "324 € < 760 € en septembre");
    let stderr = String::from_utf8_lossy(&too_soon.stderr);
    assert!(stderr.contains("760"), "{stderr}");

    unlocked(&db)
        .args([
            "society",
            "vat-refund",
            "request",
            "2026-12",
            "--today",
            "2026-12-08",
        ])
        .assert()
        .success();

    let december = unlocked(&db)
        .args([
            "--json",
            "society",
            "duty",
            "ca3",
            "--period",
            "2026-12",
            "--today",
            "2026-12-08",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let december: serde_json::Value = serde_json::from_slice(&december).unwrap();
    let boxes = december["boxes"].as_array().expect("boxes");
    let case26 = boxes.iter().find(|b| b["case"] == "26").expect("case 26");
    assert_eq!(case26["amount"], 32400);
    assert!(
        boxes.iter().all(|b| b["case"] != "27"),
        "pas de case 27 : {boxes:?}"
    );

    let january = unlocked(&db)
        .args([
            "--json",
            "society",
            "duty",
            "ca3",
            "--period",
            "2027-01",
            "--today",
            "2027-01-08",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let january: serde_json::Value = serde_json::from_slice(&january).unwrap();
    let jan_boxes = january["boxes"].as_array().expect("boxes");
    assert!(
        jan_boxes.iter().all(|b| b["case"] != "22"),
        "le 26 de décembre a coupé la chaîne : {jan_boxes:?}"
    );
}

#[test]
fn society_vat_reversal_drops_september_credit_to_319() {
    let db = temp_db("society-vat-reversal");
    provision(&db);
    unlocked(&db)
        .args([
            "company",
            "set-profile",
            "--name",
            "Lumen Conseil",
            "--legal-form",
            "SASU",
            "--siren",
            "552100554",
            "--street",
            "18 rue des Ateliers",
            "--postal-code",
            "69003",
            "--city",
            "Lyon",
            "--country",
            "FR",
            "--fiscal-year-end",
            "30/09",
            "--vat-regime",
            "real_normal_monthly",
        ])
        .assert()
        .success();
    unlocked(&db)
        .args([
            "society",
            "vat-credit",
            "set",
            "--after",
            "2026-08",
            "--amount",
            "324.00",
        ])
        .assert()
        .success();
    unlocked(&db)
        .args([
            "society",
            "vat-reversal",
            "record",
            "2026-09",
            "--amount",
            "5.00",
            "--today",
            "2026-09-15",
        ])
        .assert()
        .success();

    let duty = unlocked(&db)
        .args([
            "--json",
            "society",
            "duty",
            "ca3",
            "--period",
            "2026-09",
            "--today",
            "2026-09-15",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let duty: serde_json::Value = serde_json::from_slice(&duty).unwrap();
    let boxes = duty["boxes"].as_array().expect("boxes");
    let case15 = boxes.iter().find(|b| b["case"] == "15").expect("case 15");
    assert_eq!(case15["amount"], 500);
    let case22 = boxes.iter().find(|b| b["case"] == "22").expect("case 22");
    assert_eq!(case22["amount"], 32400);
    let case25 = boxes.iter().find(|b| b["case"] == "25").expect("case 25");
    assert_eq!(case25["amount"], 31900);
    let case27 = boxes.iter().find(|b| b["case"] == "27").expect("case 27");
    assert_eq!(case27["amount"], 31900);

    let shown = unlocked(&db)
        .args(["--json", "society", "show", "--today", "2026-10-08"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let shown: serde_json::Value = serde_json::from_slice(&shown).unwrap();
    assert_eq!(shown["vat_position"]["kind"], "credit");
    assert_eq!(shown["vat_position"]["amount"], 31900);

    let october = unlocked(&db)
        .args([
            "--json",
            "society",
            "duty",
            "ca3",
            "--period",
            "2026-10",
            "--today",
            "2026-10-08",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let october: serde_json::Value = serde_json::from_slice(&october).unwrap();
    let oct_boxes = october["boxes"].as_array().expect("boxes");
    assert!(
        oct_boxes.iter().all(|b| b["case"] != "15"),
        "pas de case 15 en octobre : {oct_boxes:?}"
    );
    let oct22 = oct_boxes
        .iter()
        .find(|b| b["case"] == "22")
        .expect("case 22");
    assert_eq!(oct22["amount"], 31900);
}

#[test]
fn society_filed_marks_the_current_is_acompte() {
    let db = temp_db("society-filed");
    provision(&db);
    unlocked(&db)
        .args(["society", "filed", "is_acompte", "--today", "2026-09-05"])
        .assert()
        .success();
    let output = unlocked(&db)
        .args([
            "--json",
            "society",
            "duty",
            "is_acompte",
            "--period",
            "2026-09-15",
            "--today",
            "2026-09-05",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(value["filed_on"], "2026-09-05");
    assert_eq!(value["due_on"], "2026-09-15");
}

#[test]
fn society_pay_on_an_empty_vault_closes_the_dividend() {
    let db = temp_db("society-pay");
    provision(&db);
    let output = unlocked(&db)
        .args(["--json", "society", "pay", "--today", "2026-09-05"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(value["possible"], 0);
    assert_eq!(value["dividend"]["kind"], "closed");
    assert_eq!(value["dividend"]["reason"], "year_not_closed");
}

#[test]
fn people_list_json_on_an_empty_vault_has_three_empty_chapters() {
    let db = temp_db("people-list");
    provision(&db);
    let output = unlocked(&db)
        .args(["--json", "people", "list", "--today", "2026-09-05"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let value: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert!(
        value["conversations"].as_array().unwrap().is_empty(),
        "{value}"
    );
    assert!(value["missions"].as_array().unwrap().is_empty(), "{value}");
    assert!(value["outgoing"].as_array().unwrap().is_empty(), "{value}");
}

#[test]
fn top_level_help_is_a_stable_interface_contract() {
    let output = freeflow().arg("--help").output().unwrap();
    insta::assert_snapshot!(String::from_utf8(output.stdout).unwrap());
}

#[test]
fn invoice_import_keeps_the_tiime_number_and_archives_the_file() {
    let db = temp_db("invoice-import");
    provision(&db);
    let client_id = create_client(&db, "Camille");
    let pdf = db.parent().unwrap().join("FAC-2026-0042.pdf");
    std::fs::write(&pdf, b"%PDF-1.7 camille").unwrap();
    let lines = r#"[{"description":"Mission Camille","quantity":1,"unit_price":500000,"vat_rate":"Standard"}]"#;
    let out = json_result(
        &unlocked(&db)
            .args([
                "--json",
                "invoice",
                "import",
                "--file",
                pdf.to_str().unwrap(),
                "--client",
                &client_id,
                "--number",
                "FAC-2026-0042",
                "--lines",
                lines,
                "--issued-on",
                "2026-09-16",
            ])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone(),
    );
    assert_eq!(out["result"]["number"], "FAC-2026-0042");
    let papers = papers_of_kind(&db, "issued_invoice");
    assert_eq!(papers[0]["origin"], "imported");
}

#[test]
fn invoice_help_is_a_stable_interface_contract() {
    let output = freeflow().args(["invoice", "--help"]).output().unwrap();
    insta::assert_snapshot!(String::from_utf8(output.stdout).unwrap());
}

#[test]
fn passphrase_change_help_is_a_stable_interface_contract() {
    let output = freeflow()
        .args(["passphrase", "change", "--help"])
        .output()
        .unwrap();
    insta::assert_snapshot!(String::from_utf8(output.stdout).unwrap());
}

#[test]
fn client_create_json_output_matches_the_documented_shape() {
    let db = temp_db("json-shape");
    provision(&db);
    let output = unlocked(&db)
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
    let output = unlocked(&db)
        .args(["--json", "prospect", "pipeline"])
        .output()
        .unwrap();
    insta::assert_json_snapshot!(json_result(&output.stdout));
}

#[test]
fn client_help_is_a_stable_interface_contract() {
    let output = freeflow().args(["client", "--help"]).output().unwrap();
    insta::assert_snapshot!(String::from_utf8(output.stdout).unwrap());
}

#[test]
fn prospect_help_is_a_stable_interface_contract() {
    let output = freeflow().args(["prospect", "--help"]).output().unwrap();
    insta::assert_snapshot!(String::from_utf8(output.stdout).unwrap());
}

#[test]
fn mission_help_is_a_stable_interface_contract() {
    let output = freeflow().args(["mission", "--help"]).output().unwrap();
    insta::assert_snapshot!(String::from_utf8(output.stdout).unwrap());
}

#[test]
fn quote_help_is_a_stable_interface_contract() {
    let output = freeflow().args(["quote", "--help"]).output().unwrap();
    insta::assert_snapshot!(String::from_utf8(output.stdout).unwrap());
}

#[test]
fn expense_help_is_a_stable_interface_contract() {
    let output = freeflow().args(["expense", "--help"]).output().unwrap();
    insta::assert_snapshot!(String::from_utf8(output.stdout).unwrap());
}

#[test]
fn client_edit_changes_only_the_fields_provided_and_bumps_the_revision() {
    let db = temp_db("client-edit");
    provision(&db);
    let id = create_client(&db, "Kappa Software");

    unlocked(&db)
        .args(["client", "edit", &id, "--siren", "552100554"])
        .assert()
        .success();

    let show_out = unlocked(&db)
        .args(["--json", "client", "show", &id])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let client = json_result(&show_out);
    assert_eq!(
        client["name"], "Kappa Software",
        "nom conservé, non fourni à `edit`"
    );
    assert_eq!(client["siren"], "552100554");
    assert_eq!(client["revision"], 2);
}

#[test]
fn client_edit_clear_flags_erase_optional_fields_that_or_current_could_never_erase() {
    // Le patron `champ.or(current.champ)` de `edit` conserve toujours un optionnel existant :
    // sans `--clear-*`, un SIREN saisi par erreur serait indélébile depuis la CLI (trou noté au
    // lot 16 et comblé ici). `--siren` et `--clear-siren` sont exclusifs, refusés par clap
    // avant tout accès au coffre.
    let db = temp_db("client-edit-clear");
    provision(&db);
    let id = create_client(&db, "Kappa Software");

    unlocked(&db)
        .args([
            "client",
            "edit",
            &id,
            "--siren",
            "552100554",
            "--vat-number",
            "FR96552100554",
            "--street",
            "1 rue Haute",
            "--postal-code",
            "69001",
            "--city",
            "Lyon",
            "--country",
            "FR",
        ])
        .assert()
        .success();

    unlocked(&db)
        .args([
            "client",
            "edit",
            &id,
            "--siren",
            "552100554",
            "--clear-siren",
        ])
        .assert()
        .failure()
        .code(2);

    unlocked(&db)
        .args([
            "client",
            "edit",
            &id,
            "--clear-siren",
            "--clear-vat-number",
            "--clear-address",
        ])
        .assert()
        .success();

    let show_out = unlocked(&db)
        .args(["--json", "client", "show", &id])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let client = json_result(&show_out);
    assert_eq!(client["name"], "Kappa Software", "nom conservé");
    assert!(client["siren"].is_null(), "SIREN effacé : {client}");
    assert!(client["vat_number"].is_null(), "TVA effacée : {client}");
    assert!(client["address"].is_null(), "adresse effacée : {client}");
    assert_eq!(client["revision"], 3);
}

#[test]
fn editing_twice_in_a_row_reads_the_fresh_revision_each_time() {
    // `client edit` fait un lire-modifier-écrire dans la même invocation (jamais de révision
    // exposée comme argument nu — voir `CLAUDE.md`) : deux éditions successives voient chacune
    // la révision la plus fraîche et réussissent toutes les deux. Le chemin de conflit lui-même
    // (deux écritures concurrentes construites sur la même révision) est couvert au niveau du
    // cœur, avec un vrai accès concurrent — voir
    // `griffe-core/src/clients.rs::tests::updating_with_a_stale_revision_is_a_conflict…` — et
    // la traduction en code de sortie 9 est couverte par
    // `error::tests::a_conflict_maps_to_its_own_exit_code`.
    let db = temp_db("client-edit-twice");
    provision(&db);
    let id = create_client(&db, "Kappa Software");

    unlocked(&db)
        .args(["client", "edit", &id, "--name", "Kappa Software SASU"])
        .assert()
        .success();
    unlocked(&db)
        .args(["client", "edit", &id, "--name", "Kappa Software (bis)"])
        .assert()
        .success();

    let show_out = unlocked(&db)
        .args(["--json", "client", "show", &id])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let client = json_result(&show_out);
    assert_eq!(client["name"], "Kappa Software (bis)");
    assert_eq!(client["revision"], 3);
}

#[test]
fn prospect_create_without_an_existing_client_stays_off_the_client_list() {
    let db = temp_db("prospect-without-client");
    provision(&db);

    unlocked(&db)
        .args([
            "prospect",
            "create",
            "--prospect",
            "Lumen Conseil",
            "--representative",
            "Camille Martin",
            "--email",
            "camille@lumen.example",
            "--name",
            "Refonte",
            "--amount",
            "78000",
            "--probability",
            "40",
            "--next-action",
            "2026-09-02",
        ])
        .assert()
        .success();

    let listed = unlocked(&db)
        .args(["--json", "client", "list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let clients = json_result(&listed);
    assert!(
        clients.as_array().is_some_and(|arr| arr.is_empty()),
        "un prospect sans devis ni facture n'apparaît pas dans client list : {clients}"
    );

    let prospects = unlocked(&db)
        .args(["--json", "prospect", "list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let listed_prospects = json_result(&prospects);
    let names: Vec<&str> = listed_prospects
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["Refonte"]);
}

#[test]
fn rm_refuses_a_client_still_referenced_by_an_open_opportunity() {
    let db = temp_db("client-rm-referenced");
    provision(&db);
    let id = create_client(&db, "Kappa Software");

    unlocked(&db)
        .args([
            "prospect",
            "create",
            "--client",
            &id,
            "--name",
            "Mission régie",
            "--amount",
            "10000",
            "--probability",
            "50",
            "--next-action",
            "2026-09-02",
        ])
        .assert()
        .success();

    unlocked(&db)
        .args(["client", "rm", &id])
        .assert()
        .failure()
        .code(4)
        .stderr(predicate::str::contains("1 opportunité"));

    unlocked(&db)
        .args(["client", "show", &id])
        .assert()
        .success();
}

#[test]
fn archiving_then_unarchiving_a_client_round_trips_through_the_active_list() {
    let db = temp_db("client-archive-roundtrip");
    provision(&db);
    let id = create_client(&db, "Kappa Software");

    unlocked(&db)
        .args(["client", "archive", &id])
        .assert()
        .success();

    let active_out = unlocked(&db)
        .args(["--json", "client", "list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(json_result(&active_out).as_array().unwrap().len(), 0);

    let all_out = unlocked(&db)
        .args(["--json", "client", "list", "--archived"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(json_result(&all_out).as_array().unwrap().len(), 1);

    unlocked(&db)
        .args(["client", "unarchive", &id])
        .assert()
        .success();

    let active_again = unlocked(&db)
        .args(["--json", "client", "list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(json_result(&active_again).as_array().unwrap().len(), 1);
}

#[test]
fn showing_an_ambiguous_name_prefix_lists_every_candidate() {
    let db = temp_db("client-ambiguous");
    provision(&db);
    create_client(&db, "Argon Digital");
    create_client(&db, "Argon Studio");

    unlocked(&db)
        .args(["client", "show", "argon"])
        .assert()
        .failure()
        .code(4)
        .stderr(predicate::str::contains("Argon Digital"))
        .stderr(predicate::str::contains("Argon Studio"));
}

#[test]
fn resolving_a_client_by_accented_name_ignores_case_and_diacritics() {
    let db = temp_db("client-accents");
    provision(&db);
    let id = create_client(&db, "Société Générale");

    let show_out = unlocked(&db)
        .args(["--json", "client", "show", "societe generale"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(json_result(&show_out)["id"], id);
}

#[test]
fn contact_lifecycle_add_list_edit_rm() {
    let db = temp_db("contact-lifecycle");
    provision(&db);
    let client_id = create_client(&db, "Kappa Software");

    let add_out = unlocked(&db)
        .args([
            "--json",
            "client",
            "contact",
            "add",
            "--client",
            &client_id,
            "--name",
            "Alex Martin",
            "--email",
            "alex@kappa.example",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let contact_id = json_result(&add_out)["result"]
        .as_str()
        .unwrap()
        .to_string();

    let list_out = unlocked(&db)
        .args([
            "--json", "client", "contact", "list", "--client", &client_id,
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(json_result(&list_out).as_array().unwrap().len(), 1);

    unlocked(&db)
        .args(["client", "contact", "edit", &contact_id, "--role", "DAF"])
        .assert()
        .success();

    // `--clear-email` efface un optionnel que `--email` seul ne pourrait jamais vider ; le rôle
    // posé juste avant et le nom, non mentionnés, survivent.
    unlocked(&db)
        .args(["client", "contact", "edit", &contact_id, "--clear-email"])
        .assert()
        .success();
    let cleared_out = unlocked(&db)
        .args([
            "--json", "client", "contact", "list", "--client", &client_id,
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let contact = &json_result(&cleared_out)[0];
    assert!(contact["email"].is_null(), "e-mail effacé : {contact}");
    assert_eq!(contact["role"], "DAF");
    assert_eq!(contact["name"], "Alex Martin");

    unlocked(&db)
        .args(["client", "contact", "rm", &contact_id])
        .assert()
        .success();

    let empty_out = unlocked(&db)
        .args([
            "--json", "client", "contact", "list", "--client", &client_id,
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(json_result(&empty_out).as_array().unwrap().len(), 0);
}

// -------------------------------------------------------------------------------------------
// Clôture d'exercice (lot 20)
// -------------------------------------------------------------------------------------------

#[test]
fn year_help_is_a_stable_interface_contract() {
    let output = freeflow().args(["year", "--help"]).output().unwrap();
    insta::assert_snapshot!(String::from_utf8(output.stdout).unwrap());
}

/// Profil minimal avec exercice civil et 1 000 € de capital (plafond de réserve légale 100 €).
fn set_company_profile(db: &Path) {
    unlocked(db)
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

#[test]
fn company_show_reports_the_derived_vat_filing_rule() {
    let db = temp_db("company-show");
    provision(&db);
    set_company_profile(&db);
    let out = unlocked(&db)
        .args(["--json", "company", "show"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let shown = json_result(&out);
    // Les champs du profil restent au premier niveau : le contrat n'est qu'enrichi.
    assert_eq!(shown["name"], "Argon Digital");
    assert_eq!(shown["siren"], "552100554");
    // SASU parisienne au SIREN 55… : le 23 du mois ; régime non renseigné → mensuel supposé.
    assert_eq!(shown["vat_filing"]["rule"]["day"], 23);
    assert_eq!(shown["vat_filing"]["scheme"], "ca3_monthly");
    let note = shown["vat_filing"]["note"].as_str().unwrap();
    assert!(note.contains("le 23 du mois"), "{note}");
    assert!(note.contains("mensuel supposé"), "{note}");
}

#[test]
fn fec_export_writes_the_regulatory_file_for_the_exercise() {
    let db = temp_db("fec-export");
    provision(&db);
    set_company_profile(&db);
    let client_out = unlocked(&db)
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
    let lines =
        r#"[{"description":"Prestation","quantity":2,"unit_price":100000,"vat_rate":"Standard"}]"#;
    unlocked(&db)
        .args([
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
        .success();
    unlocked(&db)
        .args([
            "expense",
            "record",
            "--label",
            "Licence IDE",
            "--category",
            "software",
            "--amount",
            "120",
            "--vat-rate",
            "standard",
            "--vat-deductible",
            "20",
            "--incurred-on",
            "2026-03-12",
        ])
        .assert()
        .success();

    // `--out` sur un répertoire : le fichier prend son nom réglementaire.
    let dir = db.parent().unwrap().to_path_buf();
    unlocked(&db)
        .args(["fec", "export", "2026", "--out"])
        .arg(&dir)
        .assert()
        .success()
        .stdout(predicate::str::contains("552100554FEC20261231.txt"));
    let content = std::fs::read_to_string(dir.join("552100554FEC20261231.txt")).unwrap();
    let mut records = content.lines();
    assert_eq!(
        records.next().unwrap(),
        "JournalCode|JournalLib|EcritureNum|EcritureDate|CompteNum|CompteLib|CompAuxNum|CompAuxLib|PieceRef|PieceDate|EcritureLib|Debit|Credit|EcritureLet|DateLet|ValidDate|Montantdevise|Idevise"
    );
    let body: Vec<&str> = records.collect();
    // Facture : 411 / 706 / 445710 ; dépense : 651 / 445660 / 512 ; puis l'IS de clôture en OD
    // (lot 31) : 15 % du bénéfice de 1 900 € = 285 €, 695 / 444.
    assert_eq!(body.len(), 8, "{content}");
    assert!(
        body[6].starts_with("OD|Opérations diverses|1|20261231|695000|"),
        "{content}"
    );
    assert!(body[6].ends_with("|285,00|0,00|||20261231||"), "{content}");
    assert!(
        body[0].starts_with("VE|Ventes|1|20260310|411000|Clients|"),
        "{content}"
    );
    assert!(
        body[0].contains("|Kappa Software|FA-2026-0001|20260310|"),
        "{content}"
    );
    assert!(body[0].ends_with("|2400,00|0,00|||20260310||"), "{content}");
    assert!(
        body[3].starts_with("AC|Achats|1|20260312|651000|"),
        "{content}"
    );
    assert!(body.iter().all(|l| l.split('|').count() == 18), "{content}");

    // `--out` sur un fichier, en JSON : le résumé porte le nom réglementaire et l'équilibre.
    let file = dir.join("export.txt");
    let out = unlocked(&db)
        .args(["--json", "fec", "export", "2026", "--out"])
        .arg(&file)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let shown = json_result(&out);
    assert_eq!(shown["path"], file.to_str().unwrap());
    assert_eq!(shown["summary"]["file_name"], "552100554FEC20261231.txt");
    assert_eq!(shown["summary"]["entries"], 3);
    assert_eq!(shown["summary"]["lines"], 8);
    assert_eq!(shown["summary"]["total_debit_cents"], 280_500);
    assert_eq!(shown["summary"]["total_credit_cents"], 280_500);
    assert!(file.exists());

    // Le FEC qu'on vient d'écrire passe le contrôle de structure, coffre ouvert (--period)
    // comme fichier sur disque (sans coffre).
    unlocked(&db)
        .args(["fec", "check", "--period", "2026"])
        .assert()
        .success()
        .stdout(predicate::str::contains("structure conforme"))
        .stdout(predicate::str::contains("régularité"));
    let regulatory = dir.join("552100554FEC20261231.txt");
    let checked = freeflow()
        .args(["--json", "fec", "check"])
        .arg(&regulatory)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report = serde_json::from_slice::<serde_json::Value>(&checked).unwrap();
    assert_eq!(report["conformant"], true, "{report}");
    assert_eq!(report["error_count"], 0, "{report}");
}

#[test]
fn fec_check_reads_a_file_without_a_vault() {
    let db = temp_db("fec-check-novault");
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();
    let header = "JournalCode|JournalLib|EcritureNum|EcritureDate|CompteNum|CompteLib|CompAuxNum|CompAuxLib|PieceRef|PieceDate|EcritureLib|Debit|Credit|EcritureLet|DateLet|ValidDate|Montantdevise|Idevise";
    let hostile = db.with_file_name("export.txt");
    std::fs::write(
        &hostile,
        format!("{header}\nVE|Ventes|1|20260310|411000|Clients|||FA-1|20260310|Facture|100.00|0,00|||20260310||\n"),
    )
    .unwrap();
    freeflow()
        .args(["fec", "check"])
        .arg(&hostile)
        .assert()
        .success()
        .stdout(predicate::str::contains("non conforme"))
        .stdout(predicate::str::contains("point"));
}

#[test]
fn fec_help_is_a_stable_interface_contract() {
    let output = freeflow().args(["fec", "--help"]).output().unwrap();
    insta::assert_snapshot!(String::from_utf8(output.stdout).unwrap());
}

#[test]
fn year_lifecycle_close_amend_approve_then_immutable() {
    let db = temp_db("year-lifecycle");
    provision(&db);
    set_company_profile(&db);
    let client_id = create_client(&db, "Kappa Software");
    let lines =
        r#"[{"description":"Prestation","quantity":9.5,"unit_price":65000,"vat_rate":"Standard"}]"#;
    unlocked(&db)
        .args([
            "invoice",
            "emit",
            "--client",
            &client_id,
            "--lines",
            lines,
            "--issued-on",
            "2026-09-30",
        ])
        .assert()
        .success();

    // Clôture par --period : la période dérive de la clôture 31/12 du profil.
    let close_out = unlocked(&db)
        .args([
            "--json",
            "year",
            "close",
            "--period",
            "2026",
            "--dividends",
            "1000",
            "--today",
            "2027-01-05",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(json_result(&close_out)["status"], "applied");

    let list_out = unlocked(&db)
        .args(["--json", "year", "list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let years = json_result(&list_out);
    let year = &years.as_array().unwrap()[0];
    assert_eq!(year["starts_on"], "2026-01-01");
    assert_eq!(year["ends_on"], "2026-12-31");
    // CA 6 175 € HT, aucune charge : IS 15 % (926,25 € → 926 €) → net 5 249 € ; dividendes
    // 1 000 € → report 4 249 €.
    assert_eq!(year["net_result_cents"], 524_900);
    assert_eq!(year["retained_earnings_cents"], 424_900);
    assert_eq!(year["approved_on"], serde_json::Value::Null);

    // Amender le projet : seule la valeur fournie change (patch CLI, état complet au cœur).
    unlocked(&db)
        .args(["year", "amend", "2026", "--legal-reserve", "50"])
        .assert()
        .success();
    let shown = json_result(
        &unlocked(&db)
            .args(["--json", "year", "show", "2026"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone(),
    );
    assert_eq!(shown["legal_reserve_cents"], 5_000);
    assert_eq!(shown["dividends_cents"], 100_000);
    assert_eq!(shown["revision"], 2);

    // L'export de liasse s'écrit et contient les cases attendues.
    let liasse_path = db.with_file_name("liasse.json");
    unlocked(&db)
        .args(["year", "render", "2026", "liasse", "--out"])
        .arg(&liasse_path)
        .assert()
        .success();
    let liasse: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&liasse_path).unwrap()).unwrap();
    assert_eq!(liasse["siren"], "552100554");
    assert!(
        liasse["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["form"] == "2065")
    );

    // Approbation, puis l'exercice est immuable : amender ou supprimer échoue en erreur métier.
    unlocked(&db)
        .args([
            "year",
            "approve",
            "2026",
            "--approved-on",
            "2027-05-15",
            "--today",
            "2027-05-15",
        ])
        .assert()
        .success();
    unlocked(&db)
        .args(["year", "amend", "2026", "--dividends", "0"])
        .assert()
        .failure()
        .code(4)
        .stderr(predicate::str::contains("déjà approuvé"));
    unlocked(&db)
        .args(["year", "rm", "2026"])
        .assert()
        .failure()
        .code(4);
}

#[test]
fn year_close_by_an_agent_stays_pending_until_a_human_confirms() {
    let db = temp_db("year-agent");
    provision(&db);
    set_company_profile(&db);

    let pending_out = unlocked(&db)
        .args([
            "--json",
            "--actor",
            "agent:test-session",
            "year",
            "close",
            "--period",
            "2026",
            "--today",
            "2027-01-05",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let pending = json_result(&pending_out);
    assert_eq!(pending["status"], "pending_confirmation");
    let pending_id = pending["pending_action_id"].as_str().unwrap().to_string();

    // Rien n'est clos tant qu'un humain n'a pas confirmé...
    let list_out = unlocked(&db)
        .args(["--json", "year", "list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(json_result(&list_out).as_array().unwrap().len(), 0);

    // ... et `freeflow confirm` sait rejouer cette commande-là.
    unlocked(&db)
        .args(["confirm", "--id", &pending_id])
        .assert()
        .success();
    let list_out = unlocked(&db)
        .args(["--json", "year", "list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(json_result(&list_out).as_array().unwrap().len(), 1);
}

#[test]
fn expense_lifecycle_record_edit_rm_by_reference() {
    let db = temp_db("expense-lifecycle");
    provision(&db);

    unlocked(&db)
        .args([
            "expense",
            "record",
            "--label",
            "Abonnement hébergement",
            "--category",
            "software",
            "--amount",
            "120.00",
            "--vat-rate",
            "standard",
            "--vat-deductible",
            "20.00",
            "--incurred-on",
            "2026-09-05",
        ])
        .assert()
        .success();

    // Édition par référence (préfixe de libellé, accents ignorés) : seul `--amount` change.
    unlocked(&db)
        .args([
            "expense",
            "edit",
            "abonnement",
            "--amount",
            "240.00",
            "--vat-deductible",
            "40.00",
        ])
        .assert()
        .success();

    let show_out = unlocked(&db)
        .args(["--json", "expense", "show", "abonnement"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let expense = json_result(&show_out);
    assert_eq!(expense["amount"], 24_000);
    assert_eq!(
        expense["label"], "Abonnement hébergement",
        "les champs non fournis à edit sont conservés"
    );
    assert_eq!(expense["revision"], 2, "l'édition bumpe la révision");

    unlocked(&db)
        .args(["expense", "rm", "abonnement"])
        .assert()
        .success();
    let list_out = unlocked(&db)
        .args(["--json", "expense", "list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(json_result(&list_out).as_array().unwrap().len(), 0);
}

#[test]
fn expense_reconciliation_with_a_statement_debit_by_cli() {
    // Lot 33 : un débit du relevé importé paie une dépense — créée depuis le débit (montant et
    // date repris), ou rapprochée après coup au montant exact ; `bank list --unmatched` ne le
    // montre plus, `bank unreconcile` le libère sans toucher à la dépense, et `expense rm`
    // libère le sien.
    let db = temp_db("expense-reconciliation");
    provision(&db);

    let statement = db.with_file_name("releve-debits.csv");
    std::fs::write(
        &statement,
        "date;description;montant\n2026-09-07;PRLV CABINET COMPTA;-960.00\n\
         2026-09-09;FRAIS TENUE DE COMPTE;-12.50\n2026-09-10;VIR CLIENT;500.00\n",
    )
    .unwrap();
    unlocked(&db)
        .args(["bank", "import", "--format", "csv"])
        .arg(&statement)
        .assert()
        .success();
    let list_out = unlocked(&db)
        .args(["--json", "bank", "list", "--unmatched"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let transactions = json_result(&list_out);
    assert_eq!(transactions.as_array().unwrap().len(), 3);
    let tx_of = |description: &str| {
        transactions
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["description"] == description)
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let fees_tx = tx_of("PRLV CABINET COMPTA");
    let bank_tx = tx_of("FRAIS TENUE DE COMPTE");
    let credit_tx = tx_of("VIR CLIENT");

    // Créée depuis le débit : `--amount`/`--incurred-on` omis, repris du relevé ; catégorie
    // « honoraires » (nouvelle au lot 33).
    let record_out = unlocked(&db)
        .args([
            "--json",
            "expense",
            "record",
            "--label",
            "Expert-comptable",
            "--category",
            "fees",
            "--vat-rate",
            "standard",
            "--vat-deductible",
            "160.00",
            "--transaction",
            &fees_tx,
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(json_result(&record_out)["status"], "applied");
    let show_out = unlocked(&db)
        .args(["--json", "expense", "show", "expert"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let shown = json_result(&show_out);
    assert_eq!(shown["amount"], 96_000, "montant repris du débit");
    // Lot 36 : les dates sortent en ISO 8601, plus jamais en `[année, jour ordinal]`.
    assert_eq!(
        shown["incurred_on"],
        serde_json::json!("2026-09-07"),
        "date reprise du débit"
    );
    assert_eq!(shown["category"], "Fees");
    assert_eq!(shown["bank_transaction"]["id"], fees_tx);

    // Sans `--transaction`, montant et date restent obligatoires — refusé par clap.
    unlocked(&db)
        .args([
            "expense",
            "record",
            "--label",
            "x",
            "--category",
            "other",
            "--vat-rate",
            "zero",
            "--vat-deductible",
            "0",
        ])
        .assert()
        .failure()
        .code(2);

    // Une dépense existante se rapproche après coup, au montant exact ; un crédit est refusé.
    unlocked(&db)
        .args([
            "expense",
            "record",
            "--label",
            "Frais bancaires",
            "--category",
            "bank_charges",
            "--amount",
            "12.50",
            "--vat-rate",
            "zero",
            "--vat-deductible",
            "0",
            "--incurred-on",
            "2026-09-09",
        ])
        .assert()
        .success();
    unlocked(&db)
        .args(["expense", "reconcile", "frais", "--transaction", &credit_tx])
        .assert()
        .failure()
        .code(4)
        .stderr(predicates::str::contains("crédit"));
    unlocked(&db)
        .args(["expense", "reconcile", "frais", "--transaction", &bank_tx])
        .assert()
        .success();

    let list_out = unlocked(&db)
        .args(["--json", "bank", "list", "--unmatched"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let remaining = json_result(&list_out);
    assert_eq!(remaining.as_array().unwrap().len(), 1);
    assert_eq!(remaining[0]["id"], credit_tx);
    let table = unlocked(&db)
        .args(["expense", "list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert!(String::from_utf8(table).unwrap().contains("2026-09-09"));

    // Le montant d'une dépense rapprochée est celui du relevé.
    unlocked(&db)
        .args(["expense", "edit", "frais", "--amount", "13.00"])
        .assert()
        .failure()
        .code(4)
        .stderr(predicates::str::contains("relevé"));

    // Défaire libère le débit, la dépense reste ; supprimer l'autre libère son débit.
    unlocked(&db)
        .args(["bank", "unreconcile", "--transaction", &bank_tx])
        .assert()
        .success();
    unlocked(&db)
        .args(["expense", "rm", "expert"])
        .assert()
        .success();
    let list_out = unlocked(&db)
        .args(["--json", "bank", "list", "--unmatched"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(json_result(&list_out).as_array().unwrap().len(), 3);
    let list_out = unlocked(&db)
        .args(["--json", "expense", "list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(json_result(&list_out).as_array().unwrap().len(), 1);
}

#[test]
fn quote_lifecycle_by_reference_from_creation_to_acceptance() {
    let db = temp_db("quote-lifecycle");
    provision(&db);
    create_client(&db, "Kappa Software");

    let lines = r#"[{"description":"Refonte plateforme","kind":{"Forfait":{"amount":4500000}},"vat_rate":"Standard"}]"#;
    unlocked(&db)
        .args([
            "quote",
            "create",
            "--client",
            "kappa",
            "--lines",
            lines,
            "--valid-until",
            "2026-10-31",
        ])
        .assert()
        .success();

    // Un devis créé est enfin lisible (lot 21) — et adressable par le nom de son client.
    let list_out = unlocked(&db)
        .args(["--json", "quote", "list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let quotes = json_result(&list_out);
    assert_eq!(quotes.as_array().unwrap().len(), 1);
    assert_eq!(quotes[0]["status"], "Draft");

    unlocked(&db)
        .args(["quote", "send", "kappa"])
        .assert()
        .success();
    unlocked(&db)
        .args(["quote", "accept", "kappa", "--started-on", "2026-11-01"])
        .assert()
        .success();

    let show_out = unlocked(&db)
        .args(["--json", "quote", "show", "kappa"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let quote = json_result(&show_out);
    assert_eq!(quote["status"], "Accepted");
    assert_eq!(quote["total_net_ht"], 4_500_000);
    assert_eq!(
        quote["references"]["missions"], 1,
        "l'acceptation a produit une mission, visible dans la lignée du devis"
    );
}

#[test]
fn quote_lines_are_expressible_in_the_shared_text_syntax() {
    let db = temp_db("quote-line-syntax");
    provision(&db);
    create_client(&db, "Kappa Software");

    // `--line`, répétable (lot 23) : la même syntaxe `description:type:montant[:taux]` que le
    // textarea de la fenêtre — clap conserve chaque occurrence, dans l'ordre.
    unlocked(&db)
        .args([
            "quote",
            "create",
            "--client",
            "kappa",
            "--line",
            "Cadrage:forfait:1350.00",
            "--line",
            "Conseil:regie:650.00x10:reduced",
            "--valid-until",
            "2026-10-31",
        ])
        .assert()
        .success();

    let show_out = unlocked(&db)
        .args(["--json", "quote", "show", "kappa"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let quote = json_result(&show_out);
    let lines = quote["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["description"], "Cadrage");
    assert_eq!(lines[0]["kind"]["Forfait"]["amount"], 135_000);
    assert_eq!(lines[1]["kind"]["Regie"]["daily_rate"], 65_000);
    assert_eq!(lines[1]["vat_rate"], "Reduced");
    assert_eq!(quote["total_net_ht"], 135_000 + 650_000);

    // Une spec invalide échoue avec le rappel de syntaxe du parseur du domaine.
    unlocked(&db)
        .args([
            "quote",
            "revise",
            "kappa",
            "--line",
            "Cadrage:inconnu:100",
            "--valid-until",
            "2026-11-30",
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains("ligne de devis invalide"));

    // `--lines` (JSON) et `--line` (texte) sont exclusifs — refusé par clap avant tout accès
    // au coffre.
    freeflow()
        .args([
            "quote",
            "create",
            "--client",
            "kappa",
            "--lines",
            "[]",
            "--line",
            "Cadrage:forfait:100",
            "--valid-until",
            "2026-10-31",
        ])
        .assert()
        .failure();
}

#[test]
fn payment_help_is_a_stable_interface_contract() {
    let output = freeflow().args(["payment", "--help"]).output().unwrap();
    insta::assert_snapshot!(String::from_utf8(output.stdout).unwrap());
}

#[test]
fn bank_help_is_a_stable_interface_contract() {
    let output = freeflow().args(["bank", "--help"]).output().unwrap();
    insta::assert_snapshot!(String::from_utf8(output.stdout).unwrap());
}

#[test]
fn payment_corrections_from_reconciliation_to_unreconcile_and_void() {
    let db = temp_db("payment-corrections");
    provision(&db);
    let client_id = create_client(&db, "Kappa Software");

    let lines = r#"[{"description":"Prestation","quantity":10.0,"unit_price":65000,"vat_rate":"Standard"}]"#;
    let emit_out = unlocked(&db)
        .args([
            "--json",
            "invoice",
            "emit",
            "--client",
            &client_id,
            "--lines",
            lines,
            "--issued-on",
            "2026-09-01",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let invoice_id = json_result(&emit_out)["result"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Import d'un relevé : 7 800,00 € = TTC de la facture (6 500 € HT + TVA 20 %).
    let statement = db.with_file_name("releve.csv");
    std::fs::write(
        &statement,
        "date;description;montant\n2026-09-05;Virement Kappa;7800.00\n",
    )
    .unwrap();
    unlocked(&db)
        .args(["bank", "import", "--format", "csv"])
        .arg(&statement)
        .assert()
        .success();

    // `bank list` comble le trou du lot 5 : l'id d'une transaction est enfin accessible.
    let list_out = unlocked(&db)
        .args(["--json", "bank", "list", "--unmatched"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let transactions = json_result(&list_out);
    assert_eq!(transactions.as_array().unwrap().len(), 1);
    let tx_id = transactions[0]["id"].as_str().unwrap().to_string();

    unlocked(&db)
        .args([
            "bank",
            "reconcile",
            "--transaction",
            &tx_id,
            "--invoice",
            &invoice_id,
        ])
        .assert()
        .success();
    let aged_out = unlocked(&db)
        .args(["--json", "invoice", "aged-balance", "--today", "2026-10-15"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert!(json_result(&aged_out).as_array().unwrap().is_empty());

    // Rapprocher deux fois la même transaction est refusé (bug latent corrigé au lot 22).
    unlocked(&db)
        .args([
            "bank",
            "reconcile",
            "--transaction",
            &tx_id,
            "--invoice",
            &invoice_id,
        ])
        .assert()
        .failure()
        .code(4);

    // Défaire : la transaction est libérée, l'encaissement issu du rapprochement est annulé.
    unlocked(&db)
        .args(["bank", "unreconcile", "--transaction", &tx_id])
        .assert()
        .success();
    let unmatched_again = unlocked(&db)
        .args(["--json", "bank", "list", "--unmatched"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(json_result(&unmatched_again).as_array().unwrap().len(), 1);
    let aged_out = unlocked(&db)
        .args(["--json", "invoice", "aged-balance", "--today", "2026-10-15"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(
        json_result(&aged_out).as_array().unwrap().len(),
        1,
        "la facture redevient impayée"
    );

    // Encaissement manuel, puis annulation avec motif : la contre-écriture reste listée.
    unlocked(&db)
        .args([
            "payment",
            "record",
            "--invoice",
            &invoice_id,
            "--amount",
            "7800.00",
            "--received-on",
            "2026-09-20",
            "--method",
            "check",
        ])
        .assert()
        .success();
    let payments_out = unlocked(&db)
        .args(["--json", "payment", "list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let payments = json_result(&payments_out);
    let manual = payments
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["voided_at"].is_null() && p["bank_transaction_id"].is_null())
        .expect("l'encaissement manuel doit être listé, non annulé");
    let payment_id = manual["id"].as_str().unwrap().to_string();

    unlocked(&db)
        .args([
            "payment",
            "void",
            "--id",
            &payment_id,
            "--reason",
            "saisi en double",
        ])
        .assert()
        .success();
    let payments_out = unlocked(&db)
        .args(["--json", "payment", "list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let payments = json_result(&payments_out);
    assert_eq!(
        payments.as_array().unwrap().len(),
        2,
        "les encaissements annulés restent dans l'historique"
    );
    assert!(
        payments
            .as_array()
            .unwrap()
            .iter()
            .all(|p| !p["voided_at"].is_null()),
        "les deux encaissements sont annulés"
    );
}

#[test]
fn year_deficits_are_carried_forward_then_back_from_the_cli() {
    let db = temp_db("year-deficits");
    provision(&db);
    set_company_profile(&db);

    // Bilan d'ouverture avec 30 € de déficits antérieurs reportables (hors bilan).
    unlocked(&db)
        .args([
            "year",
            "opening",
            "set",
            "--opens-on",
            "2026-01-01",
            "--line",
            "101000:Capital social:C:1000.00",
            "--line",
            "512000:Banque:D:1000.00",
            "--tax-losses",
            "30",
        ])
        .assert()
        .success();
    let opening = json_result(
        &unlocked(&db)
            .args(["--json", "year", "opening", "show"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone(),
    );
    assert_eq!(opening["tax_losses_cents"], 3_000);
    unlocked(&db)
        .args(["year", "opening", "show"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Déficits fiscaux reportables repris : 30,00\u{a0}€",
        ));

    // 2026 : CA 6 175 € HT → 30 € imputés, IS 15 % sur 6 145 € = 921,75 €.
    let client_id = create_client(&db, "Kappa Software");
    let lines =
        r#"[{"description":"Prestation","quantity":9.5,"unit_price":65000,"vat_rate":"Standard"}]"#;
    unlocked(&db)
        .args([
            "invoice",
            "emit",
            "--client",
            &client_id,
            "--lines",
            lines,
            "--issued-on",
            "2026-09-30",
        ])
        .assert()
        .success();
    // Le report en arrière est refusé sur un bénéfice : rien n'est clos.
    unlocked(&db)
        .args([
            "year",
            "close",
            "--period",
            "2026",
            "--carry-back",
            "--today",
            "2027-01-05",
        ])
        .assert()
        .failure()
        .code(4)
        .stderr(predicate::str::contains("aucun déficit"));
    unlocked(&db)
        .args(["year", "close", "--period", "2026", "--today", "2027-01-05"])
        .assert()
        .success();
    let shown = json_result(
        &unlocked(&db)
            .args(["--json", "year", "show", "2026"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone(),
    );
    assert_eq!(shown["result_before_tax_cents"], 617_500);
    assert_eq!(shown["losses_imputed_cents"], 3_000);
    assert_eq!(shown["taxable_result_cents"], 614_500);
    assert_eq!(shown["corporate_tax_cents"], 92_200);
    assert_eq!(shown["net_result_cents"], 525_300);
    assert_eq!(shown["carried_back_cents"], 0);
    assert_eq!(shown["losses_carried_forward_cents"], 0);

    // 2027 : une dépense de 80 € nets, aucune facture → déficit de 80 €, reporté en arrière
    // sur le bénéfice 2026 : créance de 15 % × 80 = 12 €, net −68 €.
    unlocked(&db)
        .args([
            "expense",
            "record",
            "--label",
            "Honoraires",
            "--category",
            "professional",
            "--amount",
            "96",
            "--vat-rate",
            "standard",
            "--vat-deductible",
            "16",
            "--incurred-on",
            "2027-03-05",
        ])
        .assert()
        .success();
    unlocked(&db)
        .args([
            "year",
            "close",
            "--period",
            "2027",
            "--carry-back",
            "--today",
            "2028-01-05",
        ])
        .assert()
        .success();
    let shown = json_result(
        &unlocked(&db)
            .args(["--json", "year", "show", "2027"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone(),
    );
    assert_eq!(shown["taxable_result_cents"], -8_000);
    assert_eq!(shown["corporate_tax_cents"], 0);
    assert_eq!(shown["carried_back_cents"], 8_000);
    assert_eq!(shown["carry_back_credit_cents"], 1_200);
    assert_eq!(shown["net_result_cents"], -6_800);
    assert_eq!(shown["losses_carried_forward_cents"], 0);
    unlocked(&db)
        .args(["year", "show", "2027"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Créance de report en arrière : 12,00\u{a0}€",
        ));

    // La liasse porte les cases de suivi des déficits, et le FEC la créance 444/699.
    let liasse_path = db.with_file_name("liasse-2027.json");
    unlocked(&db)
        .args(["year", "render", "2027", "liasse", "--out"])
        .arg(&liasse_path)
        .assert()
        .success();
    let liasse: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&liasse_path).unwrap()).unwrap();
    let entries = liasse["entries"].as_array().unwrap();
    assert!(
        entries
            .iter()
            .any(|e| e["form"] == "2033-B" && e["case"] == "356" && e["amount_cents"] == 8_000),
        "{entries:#?}"
    );
    let fec_path = db.with_file_name("fec-2027.txt");
    unlocked(&db)
        .args(["fec", "export", "2027", "--out"])
        .arg(&fec_path)
        .assert()
        .success();
    let fec = std::fs::read_to_string(&fec_path).unwrap();
    assert!(fec.contains("OD-RAD"), "{fec}");
    assert!(fec.contains("699000"), "{fec}");
}

#[test]
fn year_opening_balance_set_show_then_chains_into_the_first_close() {
    let db = temp_db("year-opening");
    provision(&db);
    set_company_profile(&db);

    // Un agent ne pose pas un bilan d'ouverture seul : action en attente, puis confirmation.
    let lines_file = db.with_extension("balance.txt");
    std::fs::write(
        &lines_file,
        "# bilan repris\n101000:Capital social:C:1000.00\n106100:Réserve légale:C:60.00\n\n",
    )
    .unwrap();
    let pending_out = unlocked(&db)
        .args([
            "--json",
            "--actor",
            "agent:test-session",
            "year",
            "opening",
            "set",
            "--opens-on",
            "2026-01-01",
            "--source",
            "bilan au 31/12/2025, cabinet X",
            "--line",
            "110000:Report à nouveau:C:250.00",
            "--line",
            "512000:Banque:D:1310.00",
            "--lines-file",
        ])
        .arg(&lines_file)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let pending = json_result(&pending_out);
    assert_eq!(pending["status"], "pending_confirmation");
    let pending_id = pending["pending_action_id"].as_str().unwrap().to_string();
    let none_out = unlocked(&db)
        .args(["--json", "year", "opening", "show"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert!(json_result(&none_out).is_null());
    unlocked(&db)
        .args(["confirm", "--id", &pending_id])
        .assert()
        .success();

    let show_out = unlocked(&db)
        .args(["--json", "year", "opening", "show"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let opening = json_result(&show_out);
    assert_eq!(opening["opens_on"], "2026-01-01");
    assert_eq!(opening["lines"].as_array().unwrap().len(), 4);
    assert_eq!(opening["total_debit_cents"], 131_000);
    assert_eq!(opening["total_credit_cents"], 131_000);
    assert_eq!(opening["equity"]["legal_reserve_cents"], 6_000);
    assert_eq!(opening["equity"]["retained_earnings_cents"], 25_000);
    assert_eq!(opening["revision"], 1);
    unlocked(&db)
        .args(["year", "opening", "show"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Capital social"))
        .stdout(predicate::str::contains("report à nouveau 250,00\u{a0}€"));

    // Un bilan déséquilibré est refusé par le cœur, avant toute écriture.
    unlocked(&db)
        .args([
            "year",
            "opening",
            "set",
            "--opens-on",
            "2026-01-01",
            "--line",
            "101000:Capital:C:10.00",
            "--line",
            "512000:Banque:D:9.00",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("déséquilibré"));

    // La clôture du premier exercice hérite du report repris : CA 6 175 € HT, IS 926 €,
    // net 5 249 € ; report = 250 + 5 249 = 5 499 €.
    let client_id = create_client(&db, "Kappa Software");
    let lines =
        r#"[{"description":"Prestation","quantity":9.5,"unit_price":65000,"vat_rate":"Standard"}]"#;
    unlocked(&db)
        .args([
            "invoice",
            "emit",
            "--client",
            &client_id,
            "--lines",
            lines,
            "--issued-on",
            "2026-09-30",
        ])
        .assert()
        .success();
    // Lot 34 : le parcours de clôture, avant de clore. Vu de juin 2026, l'exercice court
    // encore ; vu de janvier 2027, il est prêt — bilan d'ouverture repris, dotation minimale à
    // la réserve légale de 40 € (5 % de 5 248,75 € = 262,44 €, plafonnés par les 40 € qu'il
    // reste avant 10 % du capital de 1 000 €, 60 € étant déjà repris).
    let running = unlocked(&db)
        .args([
            "--json",
            "year",
            "checklist",
            "2026",
            "--today",
            "2026-06-01",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(json_result(&running)["stage"], "not_ended");
    let ready = unlocked(&db)
        .args([
            "--json",
            "year",
            "checklist",
            "2026",
            "--today",
            "2027-01-15",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let ready = json_result(&ready);
    assert_eq!(ready["stage"], "ready");
    assert_eq!(ready["blocked"], 0);
    assert_eq!(ready["minimum_legal_reserve_cents"], 4_000);
    assert_eq!(ready["result"]["net_result_cents"], 524_900);
    let steps = ready["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 18);
    let step = |key: &str| steps.iter().find(|s| s["key"] == key).unwrap().clone();
    assert_eq!(step("opening_balance")["status"], "done");
    assert_eq!(step("previous_year")["status"], "info");
    assert_eq!(step("close")["status"], "todo");
    assert_eq!(step("close")["amount_cents"], 4_000);
    assert_eq!(step("approve")["status"], "later");
    assert_eq!(step("approve")["due_on"], "2027-06-30");
    unlocked(&db)
        .args(["year", "checklist", "2026", "--today", "2027-01-15"])
        .assert()
        .success()
        .stdout(predicate::str::contains("PRÊT À CLORE"))
        .stdout(predicate::str::contains(
            "freeflow year close --period 2026 --legal-reserve 40.00",
        ));

    unlocked(&db)
        .args(["year", "close", "--period", "2026", "--today", "2027-01-05"])
        .assert()
        .success();
    // Clos sans dotation : le parcours signale la dotation insuffisante et le prochain geste.
    unlocked(&db)
        .args(["year", "checklist", "2026", "--today", "2027-03-01"])
        .assert()
        .success()
        .stdout(predicate::str::contains("CLOS, EN PROJET"))
        .stdout(predicate::str::contains(
            "Dotation à la réserve légale insuffisante",
        ))
        .stdout(predicate::str::contains(
            "freeflow year amend 2026 --legal-reserve 40.00",
        ))
        .stdout(predicate::str::contains(
            "freeflow year approve 2026 --approved-on AAAA-MM-JJ",
        ));
    let year_out = unlocked(&db)
        .args(["--json", "year", "show", "2026"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(json_result(&year_out)["retained_earnings_cents"], 549_900);

    // Le bilan est figé par ce snapshot ; le FEC de l'exercice porte ses à-nouveaux.
    unlocked(&db)
        .args(["year", "opening", "rm"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("déjà clos"));
    let out_dir = db.parent().unwrap().join("fec");
    std::fs::create_dir_all(&out_dir).unwrap();
    unlocked(&db)
        .args(["fec", "export", "2026", "--out"])
        .arg(&out_dir)
        .assert()
        .success();
    let fec = std::fs::read_to_string(out_dir.join("552100554FEC20261231.txt")).unwrap();
    assert!(
        fec.contains("AN|À-nouveaux|1|20260101|101000|Capital social|"),
        "{fec}"
    );
    // Lot 31 : l'IS de clôture est une écriture OD du FEC, et le bilan dérivé est équilibré —
    // actif = clients 7 410 + banque 1 310 = 8 720 € ; passif = capitaux propres repris + résultat
    // net 5 249 + TVA collectée 1 235 + IS dû 926.
    assert!(
        fec.contains("OD|Opérations diverses|1|20261231|695000|Impôts sur les bénéfices|||OD-IS|20261231|Impôt sur les sociétés de l'exercice|926,00|0,00|"),
        "{fec}"
    );
    let balance_out = unlocked(&db)
        .args(["--json", "year", "balance", "2026"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let balance = json_result(&balance_out);
    assert_eq!(balance["balance_sheet"]["balanced"], true);
    assert_eq!(balance["balance_sheet"]["total_assets_net_cents"], 872_000);
    assert_eq!(balance["balance_sheet"]["result_cents"], 524_900);
    let rows = balance["trial_balance"]["rows"].as_array().unwrap();
    assert!(
        rows.iter()
            .any(|r| r["account"] == "444000" && r["balance_cents"] == -92_600)
    );
    unlocked(&db)
        .args(["year", "balance", "2026"])
        .assert()
        .success()
        .stdout(predicate::str::contains("présentation 2033-A"))
        .stdout(predicate::str::contains("Clients et comptes rattachés"));
    let bilan = db.parent().unwrap().join("bilan-2026.pdf");
    unlocked(&db)
        .args(["year", "render", "2026", "balance-sheet", "--out"])
        .arg(&bilan)
        .assert()
        .success();
    assert!(std::fs::read(&bilan).unwrap().starts_with(b"%PDF-"));
    let liasse = db.parent().unwrap().join("liasse-2026.json");
    unlocked(&db)
        .args(["year", "render", "2026", "liasse", "--out"])
        .arg(&liasse)
        .assert()
        .success();
    let liasse: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&liasse).unwrap()).unwrap();
    let case_180 = liasse["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["form"] == "2033-A" && e["case"] == "180")
        .expect("total général du passif");
    assert_eq!(case_180["amount_cents"], 872_000);
}

// ---------------------------------------------------------------------------------------------
// Lot 36 : garde-fous de clôture, `init` sur un nom nu, sorties lisibles, dates ISO.
// ---------------------------------------------------------------------------------------------

/// `freeflow init --db nom.db` depuis le répertoire courant laissait un sidecar orphelin
/// (`sync_dir("")` échouait après l'écriture du `.kdf`) : le coffre doit se créer, et se rouvrir.
#[test]
fn init_with_a_bare_file_name_creates_a_usable_vault_in_the_current_directory() {
    let db = temp_db("init-bare-name");
    let dir = db.parent().unwrap();
    std::fs::create_dir_all(dir).unwrap();
    let pass_file = passphrase_file(&db, "s3cret");
    freeflow()
        .current_dir(dir)
        .args(["--passphrase-file"])
        .arg(&pass_file)
        .args(["init", "--db", "nom.db"])
        .assert()
        .success();
    assert!(dir.join("nom.db").exists(), "la base doit exister");
    assert!(dir.join("nom.db.kdf").exists(), "le sidecar doit exister");
    freeflow()
        .current_dir(dir)
        .args(["--passphrase-file"])
        .arg(&pass_file)
        .args(["--db", "nom.db", "vault", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("nom.db"));
}

/// La clôture d'un exercice pas encore écoulé est refusée (le seul geste irréversible relevé
/// par l'audit), une approbation datée dans le futur aussi ; une approbation tardive passe,
/// avec son retard affiché et la sauvegarde préalable nommée.
#[test]
fn closing_early_and_approving_in_the_future_are_refused_and_approval_backs_up_first() {
    let db = temp_db("close-guards");
    provision(&db);
    set_company_profile(&db);

    unlocked(&db)
        .args(["year", "close", "--period", "2026", "--today", "2026-12-03"])
        .assert()
        .failure()
        .code(4)
        .stderr(predicate::str::contains("court jusqu'au 2026-12-31"));
    unlocked(&db)
        .args(["year", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("aucun").or(predicate::str::contains("Clôture")));

    unlocked(&db)
        .args(["year", "close", "--period", "2026", "--today", "2027-01-05"])
        .assert()
        .success()
        .stdout(predicate::str::contains("✓ exercice clos en projet"));

    unlocked(&db)
        .args([
            "year",
            "approve",
            "2026",
            "--approved-on",
            "2027-07-20",
            "--today",
            "2027-03-01",
        ])
        .assert()
        .failure()
        .code(4)
        .stderr(predicate::str::contains("dans le futur"));

    let approved = unlocked(&db)
        .args([
            "year",
            "approve",
            "2026",
            "--approved-on",
            "2027-07-20",
            "--today",
            "2027-07-20",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "✓ approuvé (révision 2) — ⚠ 20 jour(s)",
        ))
        .stdout(predicate::str::contains("Sauvegarde préalable : "))
        .get_output()
        .stdout
        .clone();
    let backups = db.with_file_name("backups");
    let pre_approve: Vec<_> = std::fs::read_dir(&backups)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.starts_with("pre-approve-2026-") && name.ends_with(".db")
        })
        .collect();
    // Deux sauvegardes (chacune avec son sidecar) : une avant l'approbation refusée — la
    // sauvegarde précède toujours la commande —, une avant l'approbation réussie.
    assert_eq!(
        pre_approve.len(),
        2,
        "{}",
        String::from_utf8_lossy(&approved)
    );

    // La liasse est un JSON : une extension `.pdf` est refusée avant d'écrire quoi que ce soit.
    let out = db.with_file_name("liasse.pdf");
    unlocked(&db)
        .args(["year", "render", "2026", "liasse", "--out"])
        .arg(&out)
        .assert()
        .failure()
        .code(4)
        .stderr(predicate::str::contains("fichier JSON"));
    assert!(!out.exists());

    // Les dates d'un exercice sortent en ISO 8601.
    let shown = unlocked(&db)
        .args(["--json", "year", "show", "2026"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(json_result(&shown)["approved_on"], "2027-07-20");
}

/// `freeflow confirm <ID>` positionnel (lot 36) ; l'ancien `--id` reste accepté un lot.
#[test]
fn confirm_takes_the_pending_action_id_as_a_positional_argument() {
    let db = temp_db("confirm-positional");
    provision(&db);
    set_company_profile(&db);
    let pending_out = unlocked(&db)
        .args([
            "--json",
            "--actor",
            "agent:test-session",
            "year",
            "close",
            "--period",
            "2026",
            "--today",
            "2027-01-05",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let pending_id = json_result(&pending_out)["pending_action_id"]
        .as_str()
        .unwrap()
        .to_string();
    unlocked(&db)
        .args(["pending", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("fiscal.close_year"))
        .stdout(predicate::str::contains(
            "Confirmer : freeflow confirm <id>",
        ));
    unlocked(&db)
        .args(["confirm", &pending_id])
        .assert()
        .success()
        .stdout(predicate::str::starts_with("✓ "));
    unlocked(&db)
        .args(["confirm", &pending_id, "--id", &pending_id])
        .assert()
        .failure();
}

/// Plus aucune sortie texte n'est la structure Rust brute : `company show`, `fiscal calendar`,
/// `audit verify-chain` et `expense show` parlent français.
#[test]
fn human_outputs_are_sentences_and_tables_not_debug_dumps() {
    let db = temp_db("human-outputs");
    provision(&db);
    set_company_profile(&db);
    unlocked(&db)
        .args(["company", "show"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Dénomination        : Argon Digital (SASU)",
        ))
        .stdout(predicate::str::contains("Télédéclaration TVA"))
        .stdout(predicate::str::contains("CompanyProfile").not());
    let calendar = unlocked(&db)
        .args(["fiscal", "calendar", "--today", "2026-10-01"])
        .assert()
        .success()
        .stdout(predicate::str::contains("FiscalDeadline").not())
        .get_output()
        .stdout
        .clone();
    insta::assert_snapshot!(String::from_utf8(calendar).unwrap());
    unlocked(&db)
        .args(["audit", "verify-chain"])
        .assert()
        .success()
        .stdout(predicate::str::contains("✓ journal d'audit intact"));
    unlocked(&db)
        .args([
            "expense",
            "record",
            "--label",
            "Hébergement",
            "--category",
            "software",
            "--amount",
            "120",
            "--vat-rate",
            "standard",
            "--vat-deductible",
            "20",
            "--incurred-on",
            "2026-09-05",
        ])
        .assert()
        .success();
    unlocked(&db)
        .args(["expense", "show", "Hébergement"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Dépense        : Hébergement"))
        .stdout(predicate::str::contains("montant TTC    : 120,00"))
        .stdout(predicate::str::contains("relevé         : non rapprochée"));
}

/// Une dépense datée avant le bilan d'ouverture est refusée à la saisie (lot 36).
#[test]
fn an_expense_before_the_opening_balance_is_refused_by_the_cli() {
    let db = temp_db("expense-before-opening");
    provision(&db);
    set_company_profile(&db);
    unlocked(&db)
        .args([
            "year",
            "opening",
            "set",
            "--opens-on",
            "2026-01-01",
            "--line",
            "101000:Capital social:C:1000.00",
            "--line",
            "512000:Banque:D:1000.00",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "✓ bilan d'ouverture enregistré (révision 1)",
        ));
    unlocked(&db)
        .args([
            "expense",
            "record",
            "--label",
            "Vieille facture",
            "--category",
            "software",
            "--amount",
            "120",
            "--vat-rate",
            "standard",
            "--vat-deductible",
            "20",
            "--incurred-on",
            "2025-12-15",
        ])
        .assert()
        .failure()
        .code(4)
        .stderr(predicate::str::contains(
            "antérieure au bilan d'ouverture (2026-01-01)",
        ));
}

// ---------------------------------------------------------------------------------------------
// Lot 38 : import bancaire réel.
// ---------------------------------------------------------------------------------------------

/// Les exports de banques réelles s'importent sans option ; `--dry-run` annonce ce qui a été
/// compris ; un fichier illisible dit le format attendu ; `bank rm` supprime une ligne importée
/// par erreur.
#[test]
fn real_bank_exports_import_without_options_and_the_dry_run_explains_itself() {
    // Un coffre par export : deux banques décrivent le même mouvement avec le même libellé
    // (« FRAIS TENUE DE COMPTE » chez Crédit Agricole et en OFX), et le triplet
    // date/montant/libellé dédoublonnerait légitimement le second.
    let fixtures: [(&str, &[u8]); 10] = [
        (
            "qonto.csv",
            include_bytes!("../../griffe-core/src/billing/fixtures/qonto.csv"),
        ),
        (
            "qonto-fr.csv",
            include_bytes!("../../griffe-core/src/billing/fixtures/qonto-fr.csv"),
        ),
        (
            "shine.csv",
            include_bytes!("../../griffe-core/src/billing/fixtures/shine.csv"),
        ),
        (
            "boursorama.csv",
            include_bytes!("../../griffe-core/src/billing/fixtures/boursorama.csv"),
        ),
        (
            "credit-agricole.csv",
            include_bytes!("../../griffe-core/src/billing/fixtures/credit-agricole.csv"),
        ),
        (
            "bnp.csv",
            include_bytes!("../../griffe-core/src/billing/fixtures/bnp.csv"),
        ),
        (
            "lcl.csv",
            include_bytes!("../../griffe-core/src/billing/fixtures/lcl.csv"),
        ),
        (
            "banque-postale.csv",
            include_bytes!("../../griffe-core/src/billing/fixtures/banque-postale.csv"),
        ),
        (
            "ofx1.ofx",
            include_bytes!("../../griffe-core/src/billing/fixtures/ofx1.ofx"),
        ),
        (
            "ofx2.ofx",
            include_bytes!("../../griffe-core/src/billing/fixtures/ofx2.ofx"),
        ),
    ];
    let mut vaults = std::collections::HashMap::new();
    for (name, bytes) in fixtures {
        let db = temp_db(&format!("bank-import-{name}"));
        provision(&db);
        let path = db.with_file_name(name);
        std::fs::write(&path, bytes).unwrap();
        vaults.insert(name, db.clone());
        unlocked(&db)
            .args(["bank", "import"])
            .arg(&path)
            .assert()
            .success()
            .stdout(predicate::str::contains("✓ 3 nouvelle(s) transaction(s)"))
            .stdout(predicate::str::contains("pied de page").or(predicate::str::is_empty().not()));
        // Réimporter la même chose n'ajoute rien.
        unlocked(&db)
            .args(["bank", "import"])
            .arg(&path)
            .assert()
            .success()
            .stdout(predicate::str::contains("✓ 0 nouvelle(s)"));
    }
    let db = vaults["credit-agricole.csv"].clone();
    let dry = unlocked(&db)
        .args(["--dry-run", "bank", "import"])
        .arg(db.with_file_name("credit-agricole.csv"))
        .assert()
        .success()
        .stdout(predicate::str::contains("(dry-run) relevé lu comme CSV en windows-1252, séparateur « ; », décimale « , », dates JJ/MM/AAAA, en-tête ligne 4"))
        .stdout(predicate::str::contains("montant ← Crédit euros / Débit euros"))
        .stdout(predicate::str::contains("3 transaction(s), 0 nouvelle(s), 3 doublon(s) déjà en base"))
        .stdout(predicate::str::contains("déjà importée"))
        .stdout(predicate::str::contains("1 ligne(s) sautée(s) : ligne 8"))
        .get_output()
        .stdout
        .clone();
    assert!(
        !String::from_utf8_lossy(&dry).contains("Solde au"),
        "le pied de page n'est pas un mouvement"
    );
    let qonto = vaults["qonto.csv"].clone();
    let dry_json = json_result(
        &unlocked(&qonto)
            .args(["--json", "--dry-run", "bank", "import"])
            .arg(qonto.with_file_name("qonto.csv"))
            .assert()
            .success()
            .get_output()
            .stdout
            .clone(),
    );
    assert_eq!(dry_json["dialect"]["separator"], ",");
    assert_eq!(dry_json["dialect"]["encoding"], "utf-8");
    assert_eq!(dry_json["duplicates"], 3);
    assert_eq!(dry_json["preview"][0]["fitid"], "qonto-tx-0001");

    // Un fichier binaire : un message qui dit le format attendu, jamais une panique.
    let garbage = db.with_file_name("garbage.bin");
    std::fs::write(&garbage, [0u8, 1, 2, 255, 254, 100, 200]).unwrap();
    unlocked(&db)
        .args(["bank", "import"])
        .arg(&garbage)
        .assert()
        .failure()
        .code(4)
        .stderr(predicate::str::contains(
            "Attendu : un export CSV de votre banque",
        ));
    // `--format` force la détection.
    let lcl = vaults["lcl.csv"].clone();
    unlocked(&lcl)
        .args(["bank", "import", "--format", "ofx"])
        .arg(lcl.with_file_name("lcl.csv"))
        .assert()
        .failure()
        .code(4)
        .stderr(predicate::str::contains("aucun bloc <STMTTRN>"));

    // `bank rm` : un agent propose, un humain confirme ; une ligne rapprochée est refusée.
    let listed = json_result(
        &unlocked(&db)
            .args(["--json", "bank", "list"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone(),
    );
    let before = listed.as_array().unwrap().len();
    let victim = listed[0]["id"].as_str().unwrap().to_string();
    let pending = json_result(
        &unlocked(&db)
            .args(["--json", "--actor", "agent:audit", "bank", "rm", &victim])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone(),
    );
    assert_eq!(pending["status"], "pending_confirmation");
    unlocked(&db)
        .args(["confirm", pending["pending_action_id"].as_str().unwrap()])
        .assert()
        .success();
    let after = json_result(
        &unlocked(&db)
            .args(["--json", "bank", "list"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone(),
    );
    assert_eq!(after.as_array().unwrap().len(), before - 1);
    unlocked(&db)
        .args(["bank", "rm", &victim])
        .assert()
        .failure()
        .stderr(predicate::str::contains("introuvable"));
}

// ---------------------------------------------------------------------------------------------
// Lot 39 : premier lancement, justificatifs chiffrés, pièce après clôture.
// ---------------------------------------------------------------------------------------------

/// `setup status` dit ce qui manque et le prochain geste ; « société nouvelle » règle l'origine ;
/// le profil complété (associé unique, président) fait passer la liste au vert.
#[test]
fn setup_status_lists_what_is_missing_then_goes_green() {
    let db = temp_db("setup-status");
    provision(&db);
    unlocked(&db)
        .args(["setup", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "✗ profil de la société — manquant",
        ))
        .stdout(predicate::str::contains(
            "Prochain geste : Dites qui vous êtes",
        ));
    set_company_profile(&db);
    let status = json_result(
        &unlocked(&db)
            .args(["--json", "setup", "status"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone(),
    );
    assert_eq!(status["profile"]["state"], "incomplete");
    assert_eq!(status["next_step"], "origin");
    unlocked(&db)
        .args(["setup", "new-company"])
        .assert()
        .success()
        .stdout(predicate::str::contains("société déclarée nouvelle"));
    unlocked(&db)
        .args(["setup", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "✓ point de départ (société nouvelle",
        ))
        .stdout(predicate::str::contains(
            "! profil de la société — à compléter : régime de TVA, associé unique, président",
        ))
        .stdout(predicate::str::contains("freeflow bank import"));
    unlocked(&db)
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
            "--share-count",
            "100",
            "--fiscal-year-end",
            "31/12",
            "--vat-regime",
            "real_normal_monthly",
            "--president",
            "Léa Martin",
            "--sole-shareholder",
            "Léa Martin",
            "--sole-shareholder-address",
            "12 rue de la Paix, 75002 Paris",
        ])
        .assert()
        .success();
    unlocked(&db)
        .args(["company", "show"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Président           : Léa Martin"))
        .stdout(predicate::str::contains(
            "Associé unique      : Léa Martin, 12 rue de la Paix, 75002 Paris",
        ))
        .stdout(predicate::str::contains("Actions             : 100"));
    let path = db.with_file_name("releve.csv");
    std::fs::write(&path, "date;description;montant\n2026-01-15;FRAIS;-12.50\n").unwrap();
    unlocked(&db)
        .args(["bank", "import"])
        .arg(&path)
        .assert()
        .success();
    let status = json_result(
        &unlocked(&db)
            .args(["--json", "setup", "status"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone(),
    );
    assert_eq!(status["done"], true, "{status}");
}

/// Les justificatifs sont chiffrés dans `<coffre>.receipts/` (plus de `receipts/` en clair, et
/// les anciennes pièces y sont migrées au premier passage) ; `expense attach` accepte une pièce
/// après la clôture ; `expense receipt --out` la déchiffre.
#[test]
fn receipts_are_encrypted_migrated_and_attachable_after_the_close() {
    let db = temp_db("receipts-cli");
    provision(&db);
    set_company_profile(&db);
    // Une pièce en clair de l'ancien format, à migrer.
    let legacy_dir = db.with_file_name("receipts");
    std::fs::create_dir_all(&legacy_dir).unwrap();
    let legacy_hash = griffe_core::expenses::hash_receipt(b"ancienne piece");
    std::fs::write(
        legacy_dir.join(format!("{legacy_hash}-ancienne.pdf")),
        b"ancienne piece",
    )
    .unwrap();

    let receipt = db.with_file_name("facture-cabinet.pdf");
    std::fs::write(&receipt, b"%PDF-1.4 facture du cabinet").unwrap();
    unlocked(&db)
        .args([
            "expense",
            "record",
            "--label",
            "Honoraires cabinet",
            "--category",
            "fees",
            "--amount",
            "600",
            "--vat-rate",
            "standard",
            "--vat-deductible",
            "100",
            "--incurred-on",
            "2026-03-05",
            "--receipt",
        ])
        .arg(&receipt)
        .assert()
        .success();
    let receipts_dir = {
        let mut name = db.as_os_str().to_owned();
        name.push(".receipts");
        std::path::PathBuf::from(name)
    };
    assert!(
        !legacy_dir.exists(),
        "l'ancien dossier est migré puis retiré"
    );
    let stored: Vec<_> = std::fs::read_dir(&receipts_dir)
        .unwrap()
        .flatten()
        .collect();
    assert_eq!(stored.len(), 2, "la pièce migrée et la nouvelle");
    for entry in &stored {
        let raw = std::fs::read(entry.path()).unwrap();
        assert!(raw.starts_with(b"FFR1"));
        assert!(!raw.windows(4).any(|w| w == b"%PDF" || w == b"anci"));
    }

    // Clore l'exercice, puis joindre une nouvelle pièce : accepté malgré la clôture.
    unlocked(&db)
        .args(["year", "close", "--period", "2026", "--today", "2027-01-05"])
        .assert()
        .success();
    let later = db.with_file_name("facture-definitive.pdf");
    std::fs::write(&later, b"%PDF-1.4 facture definitive").unwrap();
    unlocked(&db)
        .args(["expense", "attach", "Honoraires"])
        .arg(&later)
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "✓ justificatif joint (révision 2)",
        ));
    unlocked(&db)
        .args([
            "expense",
            "edit",
            "Honoraires",
            "--label",
            "Honoraires 2026",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("exercice déjà clôturé"));

    let out = db.with_file_name("dechiffree.pdf");
    unlocked(&db)
        .args(["expense", "receipt", "Honoraires", "--out"])
        .arg(&out)
        .assert()
        .success()
        .stdout(predicate::str::contains("justificatif déchiffré dans"))
        .stdout(predicate::str::contains("(ouvert)").not());
    assert_eq!(std::fs::read(&out).unwrap(), b"%PDF-1.4 facture definitive");
}

// ---------------------------------------------------------------------------------------------
// Lot 40 : reprise depuis une balance de cabinet ou un FEC.
// ---------------------------------------------------------------------------------------------

/// La balance de clôture du cabinet s'importe sans saisir une ligne : aperçu en `--dry-run`,
/// puis bilan d'ouverture enregistré (résultat dérivé en 120, capitaux propres reconstitués,
/// avertissement sur l'amortissement) ; le calendrier dit « IS de référence inconnu » tant que
/// `--prior-is` n'est pas donné, puis chiffre l'acompte.
#[test]
fn a_cabinet_balance_is_imported_as_the_opening_balance_without_typing_a_line() {
    let db = temp_db("opening-import");
    provision(&db);
    unlocked(&db)
        .args([
            "company",
            "set-profile",
            "--name",
            "Nova Dev",
            "--legal-form",
            "SASU",
            "--siren",
            "889112348",
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
            "--fiscal-year-end",
            "30/09",
            "--vat-regime",
            "real_normal_monthly",
        ])
        .assert()
        .success();
    let balance = db.with_file_name("balance-cabinet.csv");
    std::fs::write(
        &balance,
        include_bytes!("../../griffe-core/src/opening_balance/fixtures/balance-cabinet.csv"),
    )
    .unwrap();
    unlocked(&db)
        .args(["--dry-run", "year", "opening", "import"])
        .arg(&balance)
        .args(["--opens-on", "2025-10-01"])
        .assert()
        .success()
        .stdout(predicate::str::contains("(dry-run) bilan d'ouverture au 2025-10-01 lu depuis une balance générale — 11 compte(s)"))
        .stdout(predicate::str::contains("Résultat dérivé des comptes 6/7 : 1\u{202f}200,00\u{a0}€ (posé en 120)"))
        .stdout(predicate::str::contains("⚠ Immobilisation(s) reprise(s)"));
    let preview = json_result(
        &unlocked(&db)
            .args(["--json", "--dry-run", "year", "opening", "import"])
            .arg(&balance)
            .args(["--opens-on", "2025-10-01"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone(),
    );
    assert_eq!(preview["balanced"], true);
    assert_eq!(preview["derived_result_cents"], 120_000);
    unlocked(&db)
        .args(["year", "opening", "show"])
        .assert()
        .success()
        .stdout(predicate::str::contains("aucun bilan d'ouverture"));

    unlocked(&db)
        .args(["year", "opening", "import"])
        .arg(&balance)
        .args(["--opens-on", "2025-10-01"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "✓ bilan d'ouverture repris depuis",
        ))
        .stdout(predicate::str::contains(
            "11 compte(s), résultat dérivé 1\u{202f}200,00\u{a0}€",
        ));
    let shown = json_result(
        &unlocked(&db)
            .args(["--json", "year", "opening", "show"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone(),
    );
    assert_eq!(shown["equity"]["share_capital_cents"], 100_000);
    assert_eq!(shown["equity"]["legal_reserve_cents"], 10_000);
    assert_eq!(shown["equity"]["retained_earnings_cents"], 635_000);
    assert_eq!(shown["total_debit_cents"], shown["total_credit_cents"]);
    assert_eq!(shown["prior_corporate_tax_cents"], serde_json::Value::Null);

    // Le calendrier ne présume plus la dispense d'acompte : base inconnue, dit tel quel.
    unlocked(&db)
        .args(["fiscal", "calendar", "--today", "2025-10-02"])
        .assert()
        .success()
        .stdout(predicate::str::contains("IS de référence inconnu"));
    // Le FEC du même exercice donne le même bilan (remplacement, avec les références).
    let fec = db.with_file_name("fec-tiime.txt");
    std::fs::write(
        &fec,
        include_bytes!("../../griffe-core/src/opening_balance/fixtures/fec-tiime.txt"),
    )
    .unwrap();
    unlocked(&db)
        .args(["year", "opening", "import"])
        .arg(&fec)
        .args([
            "--opens-on",
            "2025-10-01",
            "--prior-is",
            "1200",
            "--prior-vat",
            "0",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("(révision 2)"));
    let again = json_result(
        &unlocked(&db)
            .args(["--json", "year", "opening", "show"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone(),
    );
    // Mêmes comptes et montants (les libellés, eux, viennent de chaque fichier).
    let strip = |v: &serde_json::Value| -> Vec<(String, String, i64)> {
        v["lines"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| {
                (
                    l["account"].as_str().unwrap().to_string(),
                    l["side"].as_str().unwrap().to_string(),
                    l["amount_cents"].as_i64().unwrap(),
                )
            })
            .collect()
    };
    assert_eq!(strip(&again), strip(&shown));
    assert_eq!(again["prior_corporate_tax_cents"], 120_000);
    unlocked(&db)
        .args(["fiscal", "calendar", "--today", "2025-10-02"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "dispense d'acompte (IS de référence < 3 000 €)",
        ));
    // Un fichier illisible dit le format attendu.
    let junk = db.with_file_name("junk.csv");
    std::fs::write(&junk, "rien;du;tout\n1;2;3\n").unwrap();
    unlocked(&db)
        .args(["year", "opening", "import"])
        .arg(&junk)
        .args(["--opens-on", "2025-10-01"])
        .assert()
        .failure()
        .code(4)
        .stderr(predicate::str::contains("Attendu : une balance générale"));
}

#[test]
fn papers_help_is_a_stable_interface_contract() {
    let output = freeflow().args(["papers", "--help"]).output().unwrap();
    insta::assert_snapshot!(String::from_utf8(output.stdout).unwrap());
}

/// Lot 57 : un humain dépose les statuts, les retrouve, un agent ne peut pas les purger tout
/// seul, un fichier illisible sort en code 4.
#[test]
fn papers_add_lists_and_shows_statutes_and_an_agent_cannot_purge_them() {
    let db = temp_db("papers-add");
    provision(&db);
    let pdf = db.with_file_name("statuts.pdf");
    std::fs::write(&pdf, b"%PDF-1.4 statuts SASU").unwrap();
    let added = json_result(
        &unlocked(&db)
            .args(["--json", "papers", "add"])
            .arg(&pdf)
            .args(["--kind", "statutes"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone(),
    );
    assert_eq!(added["status"], "applied");
    assert_eq!(added["result"]["kind"], "statutes");
    assert_eq!(added["result"]["origin"], "uploaded");
    assert_eq!(added["result"]["original_name"], "statuts.pdf");

    let listed = json_result(
        &unlocked(&db)
            .args(["--json", "papers", "list"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone(),
    );
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["kind"], "statutes");

    let shown = json_result(
        &unlocked(&db)
            .args(["--json", "papers", "show", "statuts.pdf"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone(),
    );
    assert_eq!(shown["original_name"], "statuts.pdf");
    assert_eq!(shown["mime"], "application/pdf");

    let pending = json_result(
        &unlocked(&db)
            .args([
                "--json",
                "--actor",
                "agent:audit",
                "papers",
                "rm",
                "statuts.pdf",
            ])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone(),
    );
    assert_eq!(pending["status"], "pending_confirmation");
    // L'humain qui confirmerait se heurte encore au délai (statuts : jusqu'à radiation).
    unlocked(&db)
        .args(["confirm", pending["pending_action_id"].as_str().unwrap()])
        .assert()
        .failure()
        .code(4)
        .stderr(predicate::str::contains("délai de conservation"));

    unlocked(&db)
        .args([
            "papers",
            "add",
            "/no/such/statuts.pdf",
            "--kind",
            "statutes",
        ])
        .assert()
        .failure()
        .code(4)
        .stderr(predicate::str::contains("lecture"));
}
