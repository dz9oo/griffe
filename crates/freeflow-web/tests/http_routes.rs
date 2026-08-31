//! Tests HTTP directs sur le `Router`, via `tower::ServiceExt::oneshot` — rapides, sans
//! navigateur. Base SQLCipher réelle et temporaire à chaque test, pas de mock. Le trousseau OS
//! réel n'est jamais touché : tous les coffres sont ouverts avec `remember = false`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use freeflow_core::app::{Actor, ExecutionContext, Executor};
use freeflow_core::clients::{CreateClient, list_clients};
use freeflow_core::domain::ClientId;
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

/// Retrouve un client par son nom via une connexion directe au coffre — la réponse d'une
/// mutation réussie côté GUI est volontairement vide (voir `crate::clients` dans le crate
/// `freeflow-web` : elle ne porte que l'en-tête `HX-Trigger`), donc les tests qui ont besoin de
/// l'identifiant créé le retrouvent ainsi plutôt que de le parser depuis une réponse HTML.
fn client_id_by_name(db_path: &Path, name: &str) -> ClientId {
    let store = Store::open_with_passphrase(db_path, &Passphrase::from(PASSPHRASE)).unwrap();
    list_clients(store.connection())
        .unwrap()
        .into_iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("aucun client nommé {name}"))
        .id
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
        "/view/clients",
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
async fn the_console_refuses_unlock_lock_init_passphrase_change_and_backup_restore() {
    let db_path = test_db_path("console-session-commands");
    let state = unlocked_state(&db_path).await;
    let router = freeflow_web::router(state);

    // `--db` doit correspondre au coffre déjà ouvert par la fenêtre — sans lui, ces commandes
    // seraient refusées pour la mauvaise raison (le contrôle `--db`, qui s'exécute avant celui
    // qui nous intéresse ici) et le test ne prouverait rien sur le refus qu'il prétend vérifier.
    let db_arg = db_path.to_string_lossy();
    for line in [
        format!("lock --db {db_arg}"),
        format!("unlock --db {db_arg}"),
        format!("init --db {db_arg}"),
        format!("passphrase change --db {db_arg}"),
        format!("backup restore --db {db_arg} --from /nonexistent-a.db --to /nonexistent-b.db"),
    ] {
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
        assert!(
            !body.contains("ne peut pas pointer ailleurs"),
            "`{line}` doit être refusée pour elle-même, pas pour un `--db` mal formé : {body}"
        );
    }
}

#[tokio::test]
async fn the_console_allows_vault_status_a_read_only_command() {
    // `vault status` est en lecture seule et sans effet sur la session de la fenêtre,
    // contrairement à `init`/`unlock`/`lock`/`passphrase change` : elle doit donc réussir
    // plutôt que de heurter le `unreachable!` qui protège les autres commandes de session
    // (régression : ce cas était auparavant absent de la liste de refus tout en n'étant pas
    // non plus traité, ce qui faisait paniquer la console).
    let db_path = test_db_path("console-vault-status");
    let state = unlocked_state(&db_path).await;
    let router = freeflow_web::router(state);

    // `--db` doit correspondre au coffre déjà ouvert par la fenêtre, comme pour toute autre
    // commande passée par la console (voir `the_console_executes_real_cli_commands_against_the_same_vault`).
    let db_arg = db_path.to_string_lossy();
    let line = format!("vault+status+--db+{db_arg}");
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
        !body.contains("class=\"out err\""),
        "`vault status` est une lecture seule, elle ne doit pas être refusée : {body}"
    );
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

fn sidecar_path_for(db_path: &Path) -> PathBuf {
    let mut os_string = db_path.as_os_str().to_owned();
    os_string.push(".kdf");
    PathBuf::from(os_string)
}

fn staged_sidecar_path_for(db_path: &Path) -> PathBuf {
    let mut os_string = db_path.as_os_str().to_owned();
    os_string.push(".kdf.new");
    PathBuf::from(os_string)
}

#[tokio::test]
async fn the_unlock_screen_warns_about_an_interrupted_passphrase_change() {
    // Fabrique la fenêtre F4 (voir `freeflow_core::store` : base déjà basculée sur la nouvelle
    // clé, sidecar committé encore l'ancien) en repartant d'un changement réellement mené à son
    // terme — même construction que les tests de reprise du cœur.
    let db_path = test_db_path("unlock-passphrase-change-interrupted");
    let mut store = Store::create(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
    let backups_dir = db_path.with_file_name("backups");
    let report = store
        .change_passphrase(
            &Passphrase::from(PASSPHRASE),
            &Passphrase::from("new-s3cret"),
            &backups_dir,
        )
        .unwrap();
    drop(store);

    std::fs::copy(
        sidecar_path_for(&db_path),
        staged_sidecar_path_for(&db_path),
    )
    .unwrap();
    std::fs::copy(
        sidecar_path_for(&report.backup_path),
        sidecar_path_for(&db_path),
    )
    .unwrap();

    // `AppState::new` sans `try_open_cached` : la session démarre verrouillée, comme au premier
    // lancement de la fenêtre sur un coffre existant.
    let state = AppState::new(db_path);
    let router = freeflow_web::router(state);

    let response = router
        .oneshot(
            Request::builder()
                .uri("/unlock")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_text(response).await;
    assert!(
        body.contains("changement de passphrase a été interrompu"),
        "l'écran de déverrouillage doit signaler le changement interrompu avant toute tentative \
         de saisie : {body}"
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

#[tokio::test]
async fn creating_a_client_through_the_panel_appears_in_the_table_and_triggers_a_refresh() {
    let db_path = test_db_path("clients-create");
    let state = unlocked_state(&db_path).await;
    let router = freeflow_web::router(state);

    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/clients")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("name=Kappa+Software"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("HX-Trigger").unwrap(),
        "freeflow:saved",
        "le conteneur de liste écoute cet événement pour se rafraîchir tout seul"
    );
    assert!(
        body_text(response).await.is_empty(),
        "un corps vide vide le panneau par le swap lui-même (htmx ne swap jamais un 204)"
    );

    let table = router
        .oneshot(
            Request::builder()
                .uri("/clients/table")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = body_text(table).await;
    assert!(body.contains("Kappa Software"));
}

#[tokio::test]
async fn creating_a_client_with_an_empty_name_re_renders_the_form_with_a_field_error() {
    let db_path = test_db_path("clients-create-invalid");
    let state = unlocked_state(&db_path).await;
    let router = freeflow_web::router(state);

    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/clients")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("name="))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        !response.headers().contains_key("HX-Trigger"),
        "une erreur de validation ne doit pas déclencher de rafraîchissement de la liste"
    );
    let body = body_text(response).await;
    assert!(body.contains("field-error"));
    assert!(body.contains("obligatoire"));
}

#[tokio::test]
async fn editing_a_client_changes_only_the_provided_fields_and_bumps_the_revision() {
    let db_path = test_db_path("clients-edit");
    let state = unlocked_state_with_client(&db_path).await;
    let router = freeflow_web::router(state);
    let id = client_id_by_name(&db_path, "Kappa Software");

    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/clients/{id}"))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("revision=1&name=Kappa+Software&siren=552100554"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("HX-Trigger").unwrap(),
        "freeflow:saved"
    );

    let detail = router
        .oneshot(
            Request::builder()
                .uri(format!("/clients/{id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = body_text(detail).await;
    assert!(body.contains("Kappa Software"), "nom conservé : {body}");
    assert!(body.contains("552100554"), "siren appliqué : {body}");
}

#[tokio::test]
async fn editing_with_a_stale_revision_shows_a_conflict_banner_and_writes_nothing() {
    let db_path = test_db_path("clients-edit-conflict");
    let state = unlocked_state_with_client(&db_path).await;
    let router = freeflow_web::router(state);
    let id = client_id_by_name(&db_path, "Kappa Software");

    // Une première édition fait passer la révision en base à 2.
    router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/clients/{id}"))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("revision=1&name=Kappa+Software+SASU"))
                .unwrap(),
        )
        .await
        .unwrap();

    // Rejouer une édition construite sur la révision 1 (déjà périmée) doit être un conflit.
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/clients/{id}"))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("revision=1&name=Ecrasement+tente"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        !response.headers().contains_key("HX-Trigger"),
        "un conflit n'est pas un succès"
    );
    let body = body_text(response).await;
    assert!(body.contains("form-conflict"));
    assert!(body.contains("changé depuis sa lecture"));

    let detail = router
        .oneshot(
            Request::builder()
                .uri(format!("/clients/{id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = body_text(detail).await;
    assert!(
        body.contains("Kappa Software SASU"),
        "la première édition doit rester en place, la seconde n'a rien écrasé : {body}"
    );
}

#[tokio::test]
async fn archiving_removes_a_client_from_the_default_table_but_not_from_the_all_table() {
    let db_path = test_db_path("clients-archive");
    let state = unlocked_state_with_client(&db_path).await;
    let router = freeflow_web::router(state);
    let id = client_id_by_name(&db_path, "Kappa Software");

    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/clients/{id}/archive"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("HX-Trigger").unwrap(),
        "freeflow:saved"
    );

    let active = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/clients/table")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(!body_text(active).await.contains("Kappa Software"));

    let all = router
        .oneshot(
            Request::builder()
                .uri("/clients/table?archived=true")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(body_text(all).await.contains("Kappa Software"));
}

#[tokio::test]
async fn deleting_a_client_still_referenced_by_an_opportunity_shows_what_blocks_it() {
    let db_path = test_db_path("clients-delete-referenced");
    let mut store = Store::create(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
    let ctx = human_ctx();
    let client_id = {
        let freeflow_core::app::Outcome::Applied(id) = Executor::new(&mut store)
            .execute(
                &CreateClient {
                    name: "Kappa Software".to_string(),
                    siren: None,
                    vat_number: None,
                    address: None,
                },
                &ctx,
            )
            .unwrap()
        else {
            panic!("expected Applied")
        };
        id
    };
    Executor::new(&mut store)
        .execute(
            &freeflow_core::prospection::CreateOpportunity {
                client_id,
                name: "Refonte".to_string(),
                amount: freeflow_core::domain::Money::from_cents(10_000),
                probability: freeflow_core::domain::Probability::new(50).unwrap(),
                next_action_at: time::Date::from_calendar_date(2026, time::Month::September, 2)
                    .unwrap(),
                source: None,
            },
            &ctx,
        )
        .unwrap();
    drop(store);

    let state = AppState::new(db_path);
    state
        .unlock(&Passphrase::from(PASSPHRASE), false)
        .await
        .unwrap();
    let router = freeflow_web::router(state);

    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/clients/{client_id}/delete"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response.headers().contains_key("HX-Trigger"));
    let body = body_text(response).await;
    assert!(body.contains("1 opportunité"), "{body}");

    let still_there = router
        .oneshot(
            Request::builder()
                .uri(format!("/clients/{client_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(still_there.status(), StatusCode::OK);
    assert!(body_text(still_there).await.contains("Kappa Software"));
}

#[tokio::test]
async fn contact_lifecycle_through_the_panel() {
    let db_path = test_db_path("clients-contacts");
    let state = unlocked_state_with_client(&db_path).await;
    let router = freeflow_web::router(state);
    let client_id = client_id_by_name(&db_path, "Kappa Software");

    router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/clients/{client_id}/contacts"))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("name=Alex+Martin&email=alex%40kappa.example"))
                .unwrap(),
        )
        .await
        .unwrap();

    let detail = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/clients/{client_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = body_text(detail).await;
    assert!(body.contains("Alex Martin"), "{body}");

    let store = Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
    let contact_id = freeflow_core::clients::list_contacts(store.connection(), client_id)
        .unwrap()
        .into_iter()
        .find(|c| c.name == "Alex Martin")
        .unwrap()
        .id;
    drop(store);

    router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/contacts/{contact_id}"))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("revision=1&name=Alex+Martin&role=DAF"))
                .unwrap(),
        )
        .await
        .unwrap();

    router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/contacts/{contact_id}/delete"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let after = router
        .oneshot(
            Request::builder()
                .uri(format!("/clients/{client_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(!body_text(after).await.contains("Alex Martin"));
}

#[tokio::test]
async fn an_unknown_client_id_renders_a_friendly_message_not_a_crash() {
    let db_path = test_db_path("clients-unknown");
    let state = unlocked_state(&db_path).await;
    let router = freeflow_web::router(state);
    let unknown = ClientId::new();

    let response = router
        .oneshot(
            Request::builder()
                .uri(format!("/clients/{unknown}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(body_text(response).await.contains("introuvable"));
}
