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

/// Un répertoire propre à ce test, pas directement `<tmp>/vault.db` : entre autres,
/// `db_path.with_file_name("backups")` (utilisé aussi bien par la sauvegarde automatique que par
/// `passphrase change`) doit rester propre à un seul test, jamais un `<tmp>/backups` partagé par
/// tous les tests tournant en parallèle dans le même process.
fn temp_db(label: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir()
        .join(format!(
            "freeflow-cli-test-{label}-{}-{n}",
            std::process::id()
        ))
        .join("vault.db")
}

fn freeflow() -> Command {
    Command::cargo_bin("freeflow").expect("le binaire freeflow doit être compilé pour les tests")
}

/// Écrit une passphrase dans un fichier temporaire en 0600 (Unix) et renvoie son chemin.
fn passphrase_file(db: &Path, passphrase: &str) -> PathBuf {
    let path = db.with_extension("passphrase");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
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
    freeflow()
        .env("FREEFLOW_DB", &db)
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

    freeflow()
        .env("FREEFLOW_DB", &db)
        .args(["--passphrase-file"])
        .arg(&old_file)
        .args(["--dry-run", "passphrase", "change", "--new-passphrase-file"])
        .arg(&new_file)
        .assert()
        .success();

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

fn create_client(db: &Path, name: &str) -> String {
    let output = freeflow()
        .env("FREEFLOW_DB", db)
        .args(["--json", "client", "create", "--name", name])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    json_result(&output)["result"].as_str().unwrap().to_string()
}

#[test]
fn client_edit_changes_only_the_fields_provided_and_bumps_the_revision() {
    let db = temp_db("client-edit");
    provision(&db);
    let id = create_client(&db, "Kappa Software");

    freeflow()
        .env("FREEFLOW_DB", &db)
        .args(["client", "edit", &id, "--siren", "552100554"])
        .assert()
        .success();

    let show_out = freeflow()
        .env("FREEFLOW_DB", &db)
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
fn editing_twice_in_a_row_reads_the_fresh_revision_each_time() {
    // `client edit` fait un lire-modifier-écrire dans la même invocation (jamais de révision
    // exposée comme argument nu — voir `CLAUDE.md`) : deux éditions successives voient chacune
    // la révision la plus fraîche et réussissent toutes les deux. Le chemin de conflit lui-même
    // (deux écritures concurrentes construites sur la même révision) est couvert au niveau du
    // cœur, avec un vrai accès concurrent — voir
    // `freeflow-core/src/clients.rs::tests::updating_with_a_stale_revision_is_a_conflict…` — et
    // la traduction en code de sortie 9 est couverte par
    // `error::tests::a_conflict_maps_to_its_own_exit_code`.
    let db = temp_db("client-edit-twice");
    provision(&db);
    let id = create_client(&db, "Kappa Software");

    freeflow()
        .env("FREEFLOW_DB", &db)
        .args(["client", "edit", &id, "--name", "Kappa Software SASU"])
        .assert()
        .success();
    freeflow()
        .env("FREEFLOW_DB", &db)
        .args(["client", "edit", &id, "--name", "Kappa Software (bis)"])
        .assert()
        .success();

    let show_out = freeflow()
        .env("FREEFLOW_DB", &db)
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
fn rm_refuses_a_client_still_referenced_by_an_open_opportunity() {
    let db = temp_db("client-rm-referenced");
    provision(&db);
    let id = create_client(&db, "Kappa Software");

    freeflow()
        .env("FREEFLOW_DB", &db)
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

    freeflow()
        .env("FREEFLOW_DB", &db)
        .args(["client", "rm", &id])
        .assert()
        .failure()
        .code(4)
        .stderr(predicate::str::contains("1 opportunité"));

    freeflow()
        .env("FREEFLOW_DB", &db)
        .args(["client", "show", &id])
        .assert()
        .success();
}

#[test]
fn archiving_then_unarchiving_a_client_round_trips_through_the_active_list() {
    let db = temp_db("client-archive-roundtrip");
    provision(&db);
    let id = create_client(&db, "Kappa Software");

    freeflow()
        .env("FREEFLOW_DB", &db)
        .args(["client", "archive", &id])
        .assert()
        .success();

    let active_out = freeflow()
        .env("FREEFLOW_DB", &db)
        .args(["--json", "client", "list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(json_result(&active_out).as_array().unwrap().len(), 0);

    let all_out = freeflow()
        .env("FREEFLOW_DB", &db)
        .args(["--json", "client", "list", "--archived"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(json_result(&all_out).as_array().unwrap().len(), 1);

    freeflow()
        .env("FREEFLOW_DB", &db)
        .args(["client", "unarchive", &id])
        .assert()
        .success();

    let active_again = freeflow()
        .env("FREEFLOW_DB", &db)
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

    freeflow()
        .env("FREEFLOW_DB", &db)
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

    let show_out = freeflow()
        .env("FREEFLOW_DB", &db)
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

    let add_out = freeflow()
        .env("FREEFLOW_DB", &db)
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

    let list_out = freeflow()
        .env("FREEFLOW_DB", &db)
        .args([
            "--json", "client", "contact", "list", "--client", &client_id,
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_eq!(json_result(&list_out).as_array().unwrap().len(), 1);

    freeflow()
        .env("FREEFLOW_DB", &db)
        .args(["client", "contact", "edit", &contact_id, "--role", "DAF"])
        .assert()
        .success();

    freeflow()
        .env("FREEFLOW_DB", &db)
        .args(["client", "contact", "rm", &contact_id])
        .assert()
        .success();

    let empty_out = freeflow()
        .env("FREEFLOW_DB", &db)
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
