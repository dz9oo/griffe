//! Binaire de développement : sert le même `freeflow_web::router` que la coque Tauri (lot 9),
//! mais sur un vrai port TCP local — pour l'itération dans un navigateur ordinaire, ou pour un
//! test e2e qui a besoin d'une URL. Ce n'est pas le point d'entrée de production : celui-ci
//! reste `freeflow-desktop` (protocole URI custom, aucun port).

use std::path::PathBuf;

use freeflow_core::store::{Store, StoreError};
use freeflow_web::AppState;

fn resolve_db_path() -> PathBuf {
    std::env::var("FREEFLOW_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            eprintln!("✗ aucun coffre indiqué : définissez FREEFLOW_DB");
            std::process::exit(1);
        })
}

fn open_store(db_path: &std::path::Path) -> Store {
    match Store::open_cached(db_path) {
        Ok(store) => return store,
        Err(StoreError::Locked) => {}
        Err(e) => {
            eprintln!("✗ {e}");
            std::process::exit(1);
        }
    }
    let Ok(passphrase) = std::env::var("FREEFLOW_PASSPHRASE") else {
        eprintln!("✗ coffre verrouillé : définissez FREEFLOW_PASSPHRASE");
        std::process::exit(1);
    };
    Store::open_with_passphrase(db_path, &passphrase).unwrap_or_else(|e| {
        eprintln!("✗ {e}");
        std::process::exit(1);
    })
}

#[tokio::main]
async fn main() {
    let db_path = resolve_db_path();
    let store = open_store(&db_path);
    let router = freeflow_web::router(AppState::new(store));

    let addr = std::env::var("FREEFLOW_WEB_ADDR").unwrap_or_else(|_| "127.0.0.1:3417".to_string());
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|e| panic!("impossible d'écouter sur {addr} : {e}"));
    println!("freeflow-web-dev : http://{addr}");
    axum::serve(listener, router)
        .await
        .expect("le serveur de dev s'est arrêté de façon inattendue");
}
