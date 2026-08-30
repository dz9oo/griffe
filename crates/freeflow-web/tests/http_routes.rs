//! Tests HTTP directs sur le `Router`, via `tower::ServiceExt::oneshot` — rapides, sans
//! navigateur, exactement comme le prévoit le plan pour ce lot. Base SQLCipher réelle et
//! temporaire à chaque test, pas de mock.

use std::path::PathBuf;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use freeflow_core::app::{Actor, ExecutionContext, Executor};
use freeflow_core::clients::CreateClient;
use freeflow_core::store::Store;
use freeflow_web::AppState;
use http_body_util::BodyExt;
use tower::ServiceExt;

fn test_db_path(label: &str) -> PathBuf {
    std::env::temp_dir()
        .join(format!(
            "freeflow-web-test-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ))
        .join("vault.db")
}

fn human_ctx() -> ExecutionContext {
    ExecutionContext::new(Actor::Human, false)
}

async fn body_text(response: axum::response::Response) -> String {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test]
async fn a_direct_navigation_returns_the_full_shell_page() {
    let store = Store::open_with_passphrase(&test_db_path("full-page"), "s3cret").unwrap();
    let router = freeflow_web::router(AppState::new(store));

    let response = router
        .oneshot(
            Request::builder()
                .uri("/view/dashboard")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = body_text(response).await;
    assert!(
        body.contains("<!DOCTYPE html>"),
        "chargement direct : page complète attendue"
    );
    assert!(
        body.contains("freeflow"),
        "la barre de commandes doit être présente"
    );
    assert!(body.contains("dashboard"), "l'onglet actif doit apparaître");
}

#[tokio::test]
async fn an_htmx_boosted_navigation_returns_only_the_view_fragment() {
    let store = Store::open_with_passphrase(&test_db_path("fragment"), "s3cret").unwrap();
    let router = freeflow_web::router(AppState::new(store));

    let response = router
        .oneshot(
            Request::builder()
                .uri("/view/prospection")
                .header("HX-Request", "true")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = body_text(response).await;
    assert!(
        !body.contains("<!DOCTYPE html>") && !body.contains("<body"),
        "une navigation boostée ne doit renvoyer que le contenu de #content, pas la coque"
    );
    assert!(body.contains("prospection"));
}

#[tokio::test]
async fn every_screen_renders_successfully_against_a_freshly_seeded_vault() {
    let mut store = Store::open_with_passphrase(&test_db_path("all-screens"), "s3cret").unwrap();

    // Un client réel, créé via la même couche applicative que la CLI et le serveur MCP —
    // aucune des cinq écrans ne devrait planter sur une base non vide.
    Executor::new(&mut store)
        .execute(
            &CreateClient {
                name: "Kappa Software".to_string(),
                siren: None,
                vat_number: None,
                address: None,
            },
            &human_ctx(),
        )
        .unwrap();

    let router = freeflow_web::router(AppState::new(store));
    for path in [
        "/view/dashboard",
        "/view/prospection",
        "/view/missions",
        "/view/facturation",
    ] {
        let response = router
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "échec sur {path}");
        let body = body_text(response).await;
        assert!(
            !body.contains("erreur de lecture"),
            "{path} a levé une erreur de rendu : {body}"
        );
    }
}

#[tokio::test]
async fn the_console_executes_real_cli_commands_against_the_same_vault() {
    let db_path = test_db_path("console");
    let store = Store::open_with_passphrase(&db_path, "s3cret").unwrap();
    let router = freeflow_web::router(AppState::new(store));

    // `--db` est un flag global clap (`global = true`) : le taper après la sous-commande
    // fonctionne comme avant elle. Ça évite toute mutation de `std::env` dans le test (interdite
    // par le lint `unsafe_code = "forbid"` du workspace depuis l'édition 2024) tout en exerçant
    // le même chemin non interactif — la clé mise en cache par `open_with_passphrase` ci-dessus
    // est ce que `run_capturing` retrouve via `Store::open_cached`.
    let db_arg = db_path.to_string_lossy();
    let create_line = format!("client+create+--db+{db_arg}+--name+%22Kappa+Software%22");
    let create = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/console/run")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!("line={create_line}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::OK);
    let create_body = body_text(create).await;
    assert!(
        create_body.contains("client create"),
        "la ligne tapée doit être échoée verbatim"
    );
    assert!(
        !create_body.contains("class=\"out err\""),
        "la commande doit réussir : {create_body}"
    );

    let list_line = format!("client+list+--db+{db_arg}+--json");
    let list = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/console/run")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!("line={list_line}")))
                .unwrap(),
        )
        .await
        .unwrap();
    let list_body = body_text(list).await;
    assert!(
        list_body.contains("Kappa Software"),
        "la console doit voir le client créé par la commande précédente, sur le même coffre : {list_body}"
    );
}

#[tokio::test]
async fn static_assets_are_served_with_the_right_content_type() {
    let store = Store::open_with_passphrase(&test_db_path("assets"), "s3cret").unwrap();
    let router = freeflow_web::router(AppState::new(store));

    for (path, content_type_prefix) in [
        ("/assets/app.css", "text/css"),
        ("/assets/app.js", "text/javascript"),
        ("/assets/htmx.min.js", "text/javascript"),
    ] {
        let response = router
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "échec sur {path}");
        let content_type = response
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(content_type.starts_with(content_type_prefix));
        let body = body_text(response).await;
        assert!(!body.is_empty(), "{path} ne doit pas être vide");
    }
}

#[tokio::test]
async fn audit_recent_reflects_a_mutation_made_through_the_same_router() {
    let store = Store::open_with_passphrase(&test_db_path("audit-recent"), "s3cret").unwrap();
    let state = AppState::new(store);

    {
        let mut store = state.store.lock().await;
        Executor::new(&mut store)
            .execute(
                &CreateClient {
                    name: "Kappa Software".to_string(),
                    siren: None,
                    vat_number: None,
                    address: None,
                },
                &human_ctx(),
            )
            .unwrap();
    }

    let router = freeflow_web::router(state);
    let response = router
        .oneshot(
            Request::builder()
                .uri("/audit/recent")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_text(response).await;
    assert!(body.contains("clients.create_client"));
    assert!(body.contains("appliqué"));
}
