//! Le gabarit SQLCipher `/tmp/griffe-empty-vault-v*` est partagé entre process
//! nextest. Un `rename` du *fichier* `.db` par-dessus un gabarit déjà publié
//! remplace la clé maître (HMAC page 1 → `WrongPassphrase` dans `provision()`).

use std::env;
use std::path::PathBuf;
use std::process::Command;

use griffe_core::store::{Passphrase, Store};

const ENV_DB: &str = "FREEFLOW_TEST_TEMPLATE_WORKER_DB";
const PROCESS_COUNT: usize = 8;

#[test]
fn empty_template_worker() {
    let Ok(db) = env::var(ENV_DB) else {
        return;
    };
    Store::create(&PathBuf::from(db), &Passphrase::from("s3cret")).unwrap();
}

#[test]
fn concurrent_processes_can_create_from_the_shared_empty_template() {
    if env::var(ENV_DB).is_ok() {
        return;
    }

    let island = std::env::temp_dir().join(format!(
        "griffe-empty-template-mp-{}-{}",
        std::process::id(),
        uuid::Uuid::now_v7()
    ));
    std::fs::create_dir_all(&island).unwrap();

    let exe = env::current_exe().expect("le binaire de test courant doit être re-invocable");
    let mut children = Vec::new();
    for i in 0..PROCESS_COUNT {
        let db = island.join(format!("child-{i}")).join("vault.db");
        let child = Command::new(&exe)
            .arg("empty_template_worker")
            .arg("--exact")
            .arg("--test-threads=1")
            .env(ENV_DB, &db)
            .env("TMPDIR", &island)
            .env("GRIFFE_TEST_KDF", "1")
            .spawn()
            .expect("le worker doit pouvoir démarrer");
        children.push(child);
    }
    for mut child in children {
        let status = child.wait().expect("le worker doit pouvoir se terminer");
        assert!(
            status.success(),
            "Store::create a échoué dans un process concurrent (gabarit écrasé ?) : {status:?}"
        );
    }
}
