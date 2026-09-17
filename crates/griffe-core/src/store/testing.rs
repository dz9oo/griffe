//! Coffre `SQLCipher` temporaire pour les tests. Toujours [`Store::create`], jamais un mock
//! du SGBD : chaque appel isole un répertoire (pid + UUID), y compris pour un même `label`.

use std::path::PathBuf;

use super::{Passphrase, Store};

/// Passphrase des coffres de test (convention du dépôt, gabarit inclus).
pub const PASSPHRASE: &str = "s3cret";

/// Crée un coffre `SQLCipher` neuf, isolé, déjà migré.
///
/// # Panics
///
/// Si `Store::create` échoue (disque plein, permissions).
#[must_use]
pub fn test_store(label: &str) -> Store {
    Store::create(&temp_db_path(label), &Passphrase::from(PASSPHRASE))
        .unwrap_or_else(|e| panic!("impossible de créer le coffre de test `{label}`: {e}"))
}

fn temp_db_path(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "griffe-test-{label}-{}-{}",
        std::process::id(),
        uuid::Uuid::now_v7()
    ));
    dir.join("vault.db")
}

#[cfg(test)]
mod tests {
    use super::{PASSPHRASE, test_store};
    use crate::store::{Passphrase, Store};

    #[test]
    fn two_test_stores_with_the_same_label_do_not_share_data() {
        // Ce qui ferait échouer : un helper qui réutilise le chemin pour un `label`
        // donné, ou un `fs::copy` du `.db` SQLCipher (données et clé HKDF partagées).
        let a = test_store("shared-label");
        a.connection()
            .execute(
                "INSERT INTO clients (id, name, created_at) VALUES ('aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa', 'Seul', '2026-01-01T00:00:00Z')",
                [],
            )
            .unwrap();
        let b = test_store("shared-label");
        let count: i64 = b
            .connection()
            .query_row("SELECT count(*) FROM clients", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0, "chaque appel doit produire un coffre isolé");
        assert_ne!(a.db_path(), b.db_path());
    }

    #[test]
    fn a_test_store_reopens_with_the_shared_test_passphrase() {
        let store = test_store("reopen");
        let path = store.db_path().to_path_buf();
        drop(store);
        Store::open_with_passphrase(&path, &Passphrase::from(PASSPHRASE)).unwrap();
    }
}
