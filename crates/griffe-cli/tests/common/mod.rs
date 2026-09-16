//! Aides partagées par les tests d'intégration de la CLI (`cli_integration.rs`,
//! `closing_scenario.rs`) : coffre temporaire propre à chaque test, invocation du binaire
//! compilé, lecture des sorties `--json`.
//!
//! Ces tests lancent un vrai sous-processus (`assert_cmd`), donc ne peuvent pas injecter de
//! trousseau en mémoire (réservé aux tests dans le même process, via
//! `griffe_core::store::testing`). [`provision`] crée le coffre avec `--passphrase-file` et
//! `--remember` : une seule écriture dans le trousseau OS réel par coffre de test (identifié par
//! un `vault_id` aléatoire propre au sidecar — aucune collision possible entre exécutions).

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use assert_cmd::Command;

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// Un répertoire propre à ce test, pas directement `<tmp>/vault.db` : entre autres,
/// `db_path.with_file_name("backups")` (utilisé aussi bien par la sauvegarde automatique que par
/// `passphrase change`) doit rester propre à un seul test, jamais un `<tmp>/backups` partagé par
/// tous les tests tournant en parallèle dans le même process.
pub fn temp_db(label: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let uniq = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir()
        .join(format!(
            "griffe-cli-test-{label}-{}-{n}-{uniq}",
            std::process::id()
        ))
        .join("vault.db")
}

pub fn freeflow() -> Command {
    Command::cargo_bin("griffe").expect("le binaire griffe doit être compilé pour les tests")
}

/// Écrit une passphrase dans un fichier temporaire en 0600 (Unix) et renvoie son chemin.
pub fn passphrase_file(db: &Path, passphrase: &str) -> PathBuf {
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
pub fn provision(db: &Path) {
    let pass_file = passphrase_file(db, "s3cret");
    freeflow()
        .env("FREEFLOW_DB", db)
        .args(["--passphrase-file"])
        .arg(&pass_file)
        .args(["init", "--remember"])
        .assert()
        .success();
}

pub fn json_result(output: &[u8]) -> serde_json::Value {
    serde_json::from_slice(output).expect("une sortie --json doit être du JSON valide")
}

pub fn create_client(db: &Path, name: &str) -> String {
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
