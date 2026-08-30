//! Numérotation de facture sous concurrence réelle : threads *et* processus séparés,
//! exactement ce que le plan exige pour ce lot — la garantie « aucun trou, aucun doublon » ne
//! vaut que si elle tient à travers de vraies frontières de processus (CLI, GUI, MCP tournent
//! chacun dans le leur), pas seulement entre threads d'un même process partageant un cache.

use std::collections::HashSet;
use std::env;
use std::path::PathBuf;
use std::process::Command;
use std::thread;

use freeflow_core::app::{Actor, ExecutionContext, Executor};
use freeflow_core::billing::EmitInvoice;
use freeflow_core::domain::{ClientId, InvoiceLine, Money, VatRate};
use freeflow_core::store::{Passphrase, Store};
use time::{Date, Month};

const ENV_DB_PATH: &str = "FREEFLOW_TEST_DB_PATH";
const ENV_CLIENT_ID: &str = "FREEFLOW_TEST_CLIENT_ID";
const THREAD_COUNT: usize = 4;
const PROCESS_COUNT: usize = 4;

fn sample_line() -> InvoiceLine {
    InvoiceLine {
        description: "Prestation".to_string(),
        quantity: 1.0,
        unit_price: Money::from_cents(10_000),
        vat_rate: VatRate::Standard,
    }
}

fn emit_one(db_path: &std::path::Path, client_id: ClientId) {
    let mut store = Store::open_with_passphrase(db_path, &Passphrase::from("s3cret")).unwrap();
    let cmd = EmitInvoice {
        client_id,
        mission_id: None,
        lines: vec![sample_line()],
        issued_on: Date::from_calendar_date(2026, Month::September, 1).unwrap(),
        payment_terms_days: 30,
    };
    Executor::new(&mut store)
        .execute(&cmd, &ExecutionContext::new(Actor::Human, false))
        .unwrap();
}

/// N'est pas un test au sens habituel : c'est le "worker" ré-invoqué en sous-processus par
/// `numbering_has_no_gap_or_duplicate_across_threads_and_processes`. En exécution normale
/// (sous `cargo test`/`cargo nextest`), la variable d'environnement est absente et cette
/// fonction ne fait rien — c'est le pattern standard pour tester du vrai multi-process en Rust
/// sans dépendance supplémentaire.
#[test]
fn invoice_worker() {
    let Ok(db_path) = env::var(ENV_DB_PATH) else {
        return;
    };
    let client_id: ClientId = env::var(ENV_CLIENT_ID).unwrap().parse().unwrap();
    emit_one(&PathBuf::from(db_path), client_id);
}

#[test]
fn numbering_has_no_gap_or_duplicate_across_threads_and_processes() {
    if env::var(ENV_DB_PATH).is_ok() {
        return; // ce process est un worker relancé ci-dessous, rien à orchestrer ici.
    }

    let dir = std::env::temp_dir().join(format!(
        "freeflow-invoice-numbering-mp-{}-{}",
        std::process::id(),
        uuid::Uuid::now_v7()
    ));
    let db_path = dir.join("vault.db");

    let client_id = {
        let store = Store::create(&db_path, &Passphrase::from("s3cret")).unwrap();
        let id = ClientId::new();
        store.connection().execute("INSERT INTO clients (id, name, created_at) VALUES (?1, 'Argon Digital', '2026-01-01T00:00:00Z')", [id.to_string()]).unwrap();
        id
    }; // le Store se ferme ici : chaque worker (thread ou process) rouvre sa propre connexion.

    let thread_handles: Vec<_> = (0..THREAD_COUNT)
        .map(|_| {
            let db_path = db_path.clone();
            thread::spawn(move || emit_one(&db_path, client_id))
        })
        .collect();

    let exe = env::current_exe().expect("le binaire de test courant doit être re-invocable");
    let mut children = Vec::new();
    for _ in 0..PROCESS_COUNT {
        let child = Command::new(&exe)
            .arg("invoice_worker")
            .arg("--exact")
            .arg("--test-threads=1")
            .env(ENV_DB_PATH, &db_path)
            .env(ENV_CLIENT_ID, client_id.to_string())
            .spawn()
            .expect("le worker doit pouvoir démarrer");
        children.push(child);
    }

    for handle in thread_handles {
        handle
            .join()
            .expect("un thread émetteur de facture ne doit jamais paniquer");
    }
    for mut child in children {
        let status = child.wait().expect("le worker doit pouvoir se terminer");
        assert!(
            status.success(),
            "un worker en sous-processus a échoué : {status:?}"
        );
    }

    let store = Store::open_with_passphrase(&db_path, &Passphrase::from("s3cret")).unwrap();
    let mut stmt = store
        .connection()
        .prepare("SELECT number FROM invoices ORDER BY sequence ASC")
        .unwrap();
    let numbers: Vec<String> = stmt
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();

    assert_eq!(
        numbers.len(),
        THREAD_COUNT + PROCESS_COUNT,
        "chaque appel doit avoir produit exactement une facture"
    );

    let unique: HashSet<&String> = numbers.iter().collect();
    assert_eq!(
        unique.len(),
        numbers.len(),
        "aucun numéro ne doit être dupliqué"
    );

    let mut suffixes: Vec<u32> = numbers
        .iter()
        .map(|n| {
            let suffix = n
                .rsplit('-')
                .next()
                .expect("le format FA-AAAA-NNNN comporte toujours un tiret");
            suffix.parse().expect("le suffixe est toujours numérique")
        })
        .collect();
    suffixes.sort_unstable();
    let expected: Vec<u32> = (1..=u32::try_from(THREAD_COUNT + PROCESS_COUNT).unwrap()).collect();
    assert_eq!(suffixes, expected, "aucun trou dans la séquence");
}
