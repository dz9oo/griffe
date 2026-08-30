//! Binaire de développement : sert le même `freeflow_web::router` que la coque Tauri, mais sur
//! un vrai port TCP local — pour l'itération dans un navigateur ordinaire, ou pour un test e2e
//! qui a besoin d'une URL. Ce n'est pas le point d'entrée de production : celui-ci reste
//! `freeflow-desktop` (protocole URI custom, aucun port). Comme la coque desktop, ce binaire
//! démarre toujours — verrouillé ou non — et sert `/unlock`/`/setup` comme n'importe quel écran.

use std::path::PathBuf;

use freeflow_core::store::Store;
use freeflow_web::AppState;

fn resolve_db_path() -> PathBuf {
    if let Ok(from_env) = std::env::var("FREEFLOW_DB") {
        return PathBuf::from(from_env);
    }
    Store::default_vault_path().unwrap_or_else(|e| {
        eprintln!("✗ {e} — utilisez FREEFLOW_DB");
        std::process::exit(1);
    })
}

#[tokio::main]
async fn main() {
    let db_path = resolve_db_path();
    let state = AppState::new(db_path);
    state.try_open_cached().await;
    let router = freeflow_web::router(state);

    let addr = std::env::var("FREEFLOW_WEB_ADDR").unwrap_or_else(|_| "127.0.0.1:3417".to_string());
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|e| panic!("impossible d'écouter sur {addr} : {e}"));
    println!("freeflow-web-dev : http://{addr}");
    axum::serve(listener, router)
        .await
        .expect("le serveur de dev s'est arrêté de façon inattendue");
}
