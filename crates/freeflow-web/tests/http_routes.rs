//! Tests HTTP directs sur le `Router`, via `tower::ServiceExt::oneshot` — rapides, sans
//! navigateur. Base SQLCipher réelle et temporaire à chaque test, pas de mock. Le trousseau OS
//! réel n'est jamais touché : tous les coffres sont ouverts avec `remember = false`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use freeflow_core::app::{Actor, ExecutionContext, Executor};
use freeflow_core::clients::CreateClient;
use freeflow_core::store::{Passphrase, Store};
use freeflow_web::AppState;
use http_body_util::BodyExt;
use tower::ServiceExt;

const PASSPHRASE: &str = "s3cret";

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

/// Crée un coffre neuf et renvoie un `AppState` déjà déverrouillé dessus (`remember = false` :
/// jamais de mise en cache dans le trousseau OS réel de la machine qui exécute les tests).
async fn unlocked_state(db_path: &Path) -> AppState {
    Store::create(db_path, &Passphrase::from(PASSPHRASE)).unwrap();
    let state = AppState::new(db_path.to_path_buf());
    state
        .unlock(&Passphrase::from(PASSPHRASE), false)
        .await
        .unwrap();
    state
}

async fn unlocked_state_with_client(db_path: &Path) -> AppState {
    let mut store = Store::create(db_path, &Passphrase::from(PASSPHRASE)).unwrap();
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
    drop(store);

    let state = AppState::new(db_path.to_path_buf());
    state
        .unlock(&Passphrase::from(PASSPHRASE), false)
        .await
        .unwrap();
    state
}

#[tokio::test]
async fn a_direct_navigation_returns_the_full_shell_page() {
    let state = unlocked_state(&test_db_path("full-page")).await;
    let router = freeflow_web::router(state);

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
    let state = unlocked_state(&test_db_path("fragment")).await;
    let router = freeflow_web::router(state);

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
    let state = unlocked_state_with_client(&test_db_path("all-screens")).await;
    let router = freeflow_web::router(state);

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
    let state = unlocked_state(&db_path).await;
    let router = freeflow_web::router(state);

    // `--db` doit correspondre au coffre déjà ouvert par la fenêtre : la console l'exécute
    // contre ce `Store` emprunté (`VaultAccess::Borrowed`), jamais une seconde connexion.
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
async fn the_console_refuses_a_db_flag_pointing_elsewhere() {
    let db_path = test_db_path("console-elsewhere");
    let state = unlocked_state(&db_path).await;
    let router = freeflow_web::router(state);

    let other = test_db_path("console-elsewhere-other");
    let other_arg = other.to_string_lossy();
    let line = format!("client+list+--db+{other_arg}+--json");
    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/console/run")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!("line={line}")))
                .unwrap(),
        )
        .await
        .unwrap();
    let body = body_text(response).await;
    assert!(
        body.contains("class=\"out err\""),
        "un --db différent du coffre ouvert par la fenêtre doit être refusé : {body}"
    );
}

#[tokio::test]
async fn the_console_refuses_unlock_lock_init_and_backup_restore() {
    let db_path = test_db_path("console-session-commands");
    let state = unlocked_state(&db_path).await;
    let router = freeflow_web::router(state);

    for line in ["lock", "unlock", "init"] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/console/run")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from(format!("line={line}")))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_text(response).await;
        assert!(
            body.contains("class=\"out err\""),
            "`{line}` doit être refusée depuis la console : {body}"
        );
    }
}

#[tokio::test]
async fn static_assets_are_served_with_the_right_content_type() {
    let state = unlocked_state(&test_db_path("assets")).await;
    let router = freeflow_web::router(state);

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
async fn assets_remain_reachable_while_locked() {
    // /assets/* est explicitement exempté du middleware d'authentification : sans ça, l'écran
    // de déverrouillage lui-même ne pourrait pas charger sa feuille de style.
    let state = AppState::new(test_db_path("assets-locked"));
    let router = freeflow_web::router(state);

    let response = router
        .oneshot(
            Request::builder()
                .uri("/assets/app.css")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn audit_recent_reflects_a_mutation_made_through_the_same_router() {
    let state = unlocked_state_with_client(&test_db_path("audit-recent")).await;
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

#[tokio::test]
async fn a_view_request_while_locked_renders_the_unlock_screen_directly() {
    // Pas de `303 Location` pour une navigation de premier niveau : le protocole URI custom de
    // la coque desktop (WebKitGTK) ne le suit pas de façon fiable (page blanche silencieuse,
    // observé en pratique). Le middleware rend l'écran cible directement, en 200.
    let db_path = test_db_path("locked-redirect");
    Store::create(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
    let state = AppState::new(db_path); // jamais déverrouillé

    let router = freeflow_web::router(state);
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
    assert!(!response.headers().contains_key("location"));
    let body = body_text(response).await;
    assert!(body.contains("Coffre verrouillé"));
}

#[tokio::test]
async fn an_htmx_request_while_locked_answers_204_with_hx_redirect() {
    let db_path = test_db_path("locked-htmx-redirect");
    Store::create(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
    let state = AppState::new(db_path);

    let router = freeflow_web::router(state);
    let response = router
        .oneshot(
            Request::builder()
                .uri("/audit/recent")
                .header("HX-Request", "true")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(response.headers().get("HX-Redirect").unwrap(), "/unlock");
}

#[tokio::test]
async fn no_vault_renders_the_setup_screen_directly_instead_of_unlock() {
    let db_path = test_db_path("absent-vault");
    let state = AppState::new(db_path); // ni .db ni .kdf n'existent

    let router = freeflow_web::router(state);
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
    assert!(body.contains("Créer le coffre"));
}

#[tokio::test]
async fn posting_the_right_passphrase_unlocks_and_leaks_nothing_in_the_response() {
    let db_path = test_db_path("unlock-success");
    Store::create(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
    let state = AppState::new(db_path);
    let router = freeflow_web::router(state);

    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/unlock")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!("passphrase={PASSPHRASE}")))
                .unwrap(),
        )
        .await
        .unwrap();
    // Le formulaire /unlock n'est pas boosté par htmx (bare_page ne charge même pas htmx.min.js) :
    // sa soumission est une navigation de premier niveau ordinaire, donc son succès rend le
    // tableau de bord directement plutôt que de rediriger vers `/`.
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response.headers().contains_key("location"));
    let body = body_text(response).await;
    assert!(body.contains("dashboard"));
    assert!(!body.contains(PASSPHRASE));
}

#[tokio::test]
async fn posting_a_wrong_passphrase_re_renders_the_form_and_stays_locked() {
    let db_path = test_db_path("unlock-wrong");
    Store::create(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
    let state = AppState::new(db_path);
    let router = freeflow_web::router(state.clone());

    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/unlock")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("passphrase=not-the-passphrase"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_text(response).await;
    assert!(
        !body.contains("not-the-passphrase"),
        "la passphrase incorrecte ne doit jamais réapparaître dans la réponse"
    );
    assert!(
        body.contains("passphrase incorrecte")
            || body.contains("WrongPassphrase")
            || body.contains("corrompu")
    );
}

#[tokio::test]
async fn the_creation_screen_appears_when_no_vault_exists() {
    let db_path = test_db_path("setup-screen");
    let state = AppState::new(db_path);
    let router = freeflow_web::router(state);

    let response = router
        .oneshot(
            Request::builder()
                .uri("/setup")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_text(response).await;
    assert!(body.contains("Créer le coffre"));
    assert!(
        body.contains("aucune"),
        "l'avertissement d'irrécupérabilité doit être visible"
    );
}

#[tokio::test]
async fn setup_refuses_a_mismatched_confirmation() {
    let db_path = test_db_path("setup-mismatch");
    let state = AppState::new(db_path.clone());
    let router = freeflow_web::router(state);

    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/setup")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("passphrase=abcdefgh&confirm=different"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_text(response).await;
    assert!(body.contains("ne correspondent pas"));
    assert!(!db_path.exists(), "aucun coffre ne doit avoir été créé");
}

#[tokio::test]
async fn locking_redirects_the_next_request_to_unlock() {
    let db_path = test_db_path("lock-button");
    let state = unlocked_state(&db_path).await;
    let router = freeflow_web::router(state);

    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/lock")
                .header("HX-Request", "true")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(response.headers().get("HX-Redirect").unwrap(), "/unlock");

    let after = router
        .oneshot(
            Request::builder()
                .uri("/view/dashboard")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(after.status(), StatusCode::OK);
    let body = body_text(after).await;
    assert!(body.contains("Coffre verrouillé"));
}

#[tokio::test]
async fn an_idle_session_locks_itself_and_polling_audit_recent_does_not_keep_it_alive() {
    let db_path = test_db_path("idle-timeout");
    Store::create(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
    let state = AppState::with_idle_timeout(db_path, Duration::from_millis(150));
    state
        .unlock(&Passphrase::from(PASSPHRASE), false)
        .await
        .unwrap();
    let router = freeflow_web::router(state);

    // Le polling du rail d'audit, plusieurs fois dans la fenêtre d'inactivité, ne doit jamais
    // compter comme activité.
    for _ in 0..3 {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/audit/recent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "encore dans la fenêtre d'inactivité"
        );
        tokio::time::sleep(Duration::from_millis(30)).await;
    }

    tokio::time::sleep(Duration::from_millis(150)).await;
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
        body.contains("Coffre verrouillé"),
        "au-delà du délai d'inactivité, malgré le polling continu de /audit/recent, la session \
         doit avoir expiré : {body}"
    );
}

#[tokio::test]
async fn a_real_navigation_keeps_an_idle_session_alive() {
    let db_path = test_db_path("idle-touch");
    Store::create(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
    let state = AppState::with_idle_timeout(db_path, Duration::from_millis(400));
    state
        .unlock(&Passphrase::from(PASSPHRASE), false)
        .await
        .unwrap();
    let router = freeflow_web::router(state);

    tokio::time::sleep(Duration::from_millis(200)).await;
    let touched = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/view/dashboard")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        touched.status(),
        StatusCode::OK,
        "une vraie navigation doit prolonger la session"
    );

    tokio::time::sleep(Duration::from_millis(200)).await;
    let still_alive = router
        .oneshot(
            Request::builder()
                .uri("/view/dashboard")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        still_alive.status(),
        StatusCode::OK,
        "200ms + 200ms < 400ms depuis la dernière vraie navigation : la session doit être encore active"
    );
}
