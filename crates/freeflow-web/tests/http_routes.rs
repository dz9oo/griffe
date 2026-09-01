//! Tests HTTP directs sur le `Router`, via `tower::ServiceExt::oneshot` — rapides, sans
//! navigateur. Base SQLCipher réelle et temporaire à chaque test, pas de mock. Le trousseau OS
//! réel n'est jamais touché : tous les coffres sont ouverts avec `remember = false`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use freeflow_core::app::{Actor, ExecutionContext, Executor};
use freeflow_core::clients::{CreateClient, list_clients};
use freeflow_core::domain::{ClientId, MissionId, Money, OpportunityId, Probability};
use freeflow_core::missions::CreateMission;
use freeflow_core::prospection::{CreateOpportunity, list_opportunities};
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

fn opportunity_id_by_name(db_path: &Path, name: &str) -> OpportunityId {
    let store = Store::open_with_passphrase(db_path, &Passphrase::from(PASSPHRASE)).unwrap();
    list_opportunities(store.connection())
        .unwrap()
        .into_iter()
        .find(|o| o.name == name)
        .unwrap_or_else(|| panic!("aucune opportunité nommée {name}"))
        .id
}

/// Coffre avec un client et une opportunité ouverte le référençant — pour les tests
/// prospection/missions qui ont besoin d'un point de départ réel.
async fn unlocked_state_with_opportunity(db_path: &Path) -> (AppState, ClientId) {
    let mut store = Store::create(db_path, &Passphrase::from(PASSPHRASE)).unwrap();
    let client_id = match Executor::new(&mut store)
        .execute(
            &CreateClient {
                name: "Kappa Software".to_string(),
                siren: None,
                vat_number: None,
                address: None,
            },
            &human_ctx(),
        )
        .unwrap()
    {
        freeflow_core::app::Outcome::Applied(id) => id,
        other => panic!("expected Applied, got {other:?}"),
    };
    Executor::new(&mut store)
        .execute(
            &CreateOpportunity {
                client_id,
                name: "Refonte plateforme".to_string(),
                amount: Money::from_cents(7_800_000),
                probability: Probability::new(40).unwrap(),
                next_action_at: time::Date::from_calendar_date(2026, time::Month::September, 2)
                    .unwrap(),
                source: None,
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
    (state, client_id)
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
    let db_path = test_db_path("all-screens");
    let (state, client_id) = unlocked_state_with_opportunity(&db_path).await;

    // Une opportunité archivée et une mission à la fois close et archivée : sans elles, les
    // branches de rendu correspondantes (lot 16) ne seraient jamais exercées par ce test.
    {
        let mut store =
            Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
        let opportunity_id = match Executor::new(&mut store)
            .execute(
                &CreateOpportunity {
                    client_id,
                    name: "Piste froide".to_string(),
                    amount: Money::from_cents(500_000),
                    probability: Probability::new(10).unwrap(),
                    next_action_at: time::Date::from_calendar_date(2026, time::Month::September, 2)
                        .unwrap(),
                    source: None,
                },
                &human_ctx(),
            )
            .unwrap()
        {
            freeflow_core::app::Outcome::Applied(id) => id,
            other => panic!("expected Applied, got {other:?}"),
        };
        Executor::new(&mut store)
            .execute(
                &freeflow_core::prospection::ArchiveOpportunity {
                    id: opportunity_id,
                    revision: 1,
                },
                &human_ctx(),
            )
            .unwrap();

        let mission_id = match Executor::new(&mut store)
            .execute(
                &CreateMission {
                    client_id,
                    quote_id: None,
                    name: "Maintenance close et archivée".to_string(),
                    kind: freeflow_core::domain::MissionKind::Recurrent {
                        monthly_amount: Money::from_cents(200_000),
                    },
                    milestones: Vec::new(),
                    started_on: time::Date::from_calendar_date(2026, time::Month::January, 1)
                        .unwrap(),
                },
                &human_ctx(),
            )
            .unwrap()
        {
            freeflow_core::app::Outcome::Applied(id) => id,
            other => panic!("expected Applied, got {other:?}"),
        };
        Executor::new(&mut store)
            .execute(
                &freeflow_core::missions::CloseMission {
                    id: mission_id,
                    revision: 1,
                    ended_on: time::Date::from_calendar_date(2026, time::Month::June, 30).unwrap(),
                },
                &human_ctx(),
            )
            .unwrap();
        Executor::new(&mut store)
            .execute(
                &freeflow_core::missions::ArchiveMission {
                    id: mission_id,
                    revision: 2,
                },
                &human_ctx(),
            )
            .unwrap();

        // Un devis (avec remise) et une dépense, pour que les écrans du lot 21 aient du contenu
        // à rendre — lignes, totaux nets, badges de statut.
        Executor::new(&mut store)
            .execute(
                &freeflow_core::quotes::CreateQuote {
                    client_id,
                    opportunity_id: None,
                    lines: vec![freeflow_core::domain::QuoteLine {
                        description: "Refonte plateforme".to_string(),
                        kind: freeflow_core::domain::LineKind::Forfait {
                            amount: Money::from_cents(4_500_000),
                        },
                        vat_rate: freeflow_core::domain::VatRate::Standard,
                    }],
                    discount: Some(freeflow_core::domain::Discount::Percentage(1_000)),
                    terms: None,
                    valid_until: time::Date::from_calendar_date(2026, time::Month::October, 31)
                        .unwrap(),
                },
                &human_ctx(),
            )
            .unwrap();
        Executor::new(&mut store)
            .execute(
                &freeflow_core::expenses::RecordExpense {
                    label: "Abonnement hébergement".to_string(),
                    category: freeflow_core::domain::ExpenseCategory::Software,
                    amount: Money::from_cents(12_000),
                    vat_rate: freeflow_core::domain::VatRate::Standard,
                    vat_deductible: Money::from_cents(2_000),
                    incurred_on: time::Date::from_calendar_date(2026, time::Month::September, 5)
                        .unwrap(),
                    receipt_hash: None,
                    receipt_filename: None,
                },
                &human_ctx(),
            )
            .unwrap();
    }

    let router = freeflow_web::router(state);

    for path in [
        "/view/dashboard",
        "/view/prospection",
        "/view/devis",
        "/view/missions",
        "/view/facturation",
        "/view/depenses",
        "/view/clients",
        "/view/cloture",
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

    // Les tables filtrées (archivées/closes comprises) doivent aussi rendre sans erreur — c'est
    // là que vivent les branches ajoutées au lot 16 (badges « archivée »/« clôturée »).
    for path in [
        "/prospection/table?closed=true&archived=true",
        "/missions/table?ended=true&archived=true",
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

// -- Prospection (lot 16) --------------------------------------------------------------------

#[tokio::test]
async fn creating_an_opportunity_through_the_panel_appears_in_the_table_and_triggers_a_refresh() {
    let db_path = test_db_path("prospection-create");
    let state = unlocked_state_with_client(&db_path).await;
    let router = freeflow_web::router(state);

    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/prospection")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "client=Kappa+Software&name=Nouvelle+piste&amount=1000&probability=50&next_action=2026-09-02",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("HX-Trigger").unwrap(),
        "freeflow:saved"
    );

    let table = router
        .oneshot(
            Request::builder()
                .uri("/prospection/table")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(body_text(table).await.contains("Nouvelle piste"));
}

#[tokio::test]
async fn editing_an_opportunity_with_a_stale_revision_shows_a_conflict_banner_and_writes_nothing() {
    let db_path = test_db_path("prospection-edit-conflict");
    let (state, _client_id) = unlocked_state_with_opportunity(&db_path).await;
    let router = freeflow_web::router(state);
    let id = opportunity_id_by_name(&db_path, "Refonte plateforme");

    router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/prospection/{id}"))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "revision=1&name=Refonte+plateforme+v2&amount=7800&probability=40",
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/prospection/{id}"))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "revision=1&name=Ecrasement+tente&amount=1&probability=1",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response.headers().contains_key("HX-Trigger"));
    let body = body_text(response).await;
    assert!(body.contains("form-conflict"));
    assert!(body.contains("changé depuis sa lecture"));

    let detail = router
        .oneshot(
            Request::builder()
                .uri(format!("/prospection/{id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(body_text(detail).await.contains("Refonte plateforme v2"));
}

#[tokio::test]
async fn winning_an_opportunity_through_the_panel_creates_a_mission_visible_on_the_missions_screen()
{
    let db_path = test_db_path("prospection-win");
    let (state, _client_id) = unlocked_state_with_opportunity(&db_path).await;
    let router = freeflow_web::router(state);
    let id = opportunity_id_by_name(&db_path, "Refonte plateforme");

    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/prospection/{id}/win"))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("started_on=2026-09-03"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("HX-Trigger").unwrap(),
        "freeflow:saved"
    );

    let missions_table = router
        .oneshot(
            Request::builder()
                .uri("/missions/table")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        body_text(missions_table)
            .await
            .contains("Refonte plateforme"),
        "la mission créée par le gain doit apparaître sur l'écran missions"
    );
}

#[tokio::test]
async fn archiving_an_opportunity_removes_it_from_the_default_table_but_not_from_the_all_table() {
    let db_path = test_db_path("prospection-archive");
    let (state, _client_id) = unlocked_state_with_opportunity(&db_path).await;
    let router = freeflow_web::router(state);
    let id = opportunity_id_by_name(&db_path, "Refonte plateforme");

    router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/prospection/{id}/archive"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let active = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/prospection/table")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(!body_text(active).await.contains("Refonte plateforme"));

    let all = router
        .oneshot(
            Request::builder()
                .uri("/prospection/table?closed=true&archived=true")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(body_text(all).await.contains("Refonte plateforme"));
}

#[tokio::test]
async fn interaction_lifecycle_through_the_panel() {
    let db_path = test_db_path("prospection-interactions");
    let (state, _client_id) = unlocked_state_with_opportunity(&db_path).await;
    let router = freeflow_web::router(state);
    let id = opportunity_id_by_name(&db_path, "Refonte plateforme");

    let create = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/prospection/{id}/interactions"))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("kind=call&note=Premier+contact"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::OK);
    let detail_body = body_text(create).await;
    assert!(detail_body.contains("Premier contact"));

    let interaction_id = {
        let store = Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
        freeflow_core::prospection::list_interactions(store.connection(), id)
            .unwrap()
            .first()
            .unwrap()
            .id
    };

    let update = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/interactions/{interaction_id}"))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("revision=1&kind=meeting&note=Rendez-vous"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(body_text(update).await.contains("Rendez-vous"));

    let deleted = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/interactions/{interaction_id}/delete"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(!body_text(deleted).await.contains("Rendez-vous"));
}

// -- Missions (lot 16) ------------------------------------------------------------------------

async fn unlocked_state_with_mission(db_path: &Path) -> (AppState, ClientId, MissionId) {
    let mut store = Store::create(db_path, &Passphrase::from(PASSPHRASE)).unwrap();
    let client_id = match Executor::new(&mut store)
        .execute(
            &CreateClient {
                name: "Kappa Software".to_string(),
                siren: None,
                vat_number: None,
                address: None,
            },
            &human_ctx(),
        )
        .unwrap()
    {
        freeflow_core::app::Outcome::Applied(id) => id,
        other => panic!("expected Applied, got {other:?}"),
    };
    let mission_id = match Executor::new(&mut store)
        .execute(
            &CreateMission {
                client_id,
                quote_id: None,
                name: "Refonte dashboard".to_string(),
                kind: freeflow_core::domain::MissionKind::Forfait {
                    budget: Money::from_cents(4_500_000),
                },
                milestones: Vec::new(),
                started_on: time::Date::from_calendar_date(2026, time::Month::September, 1)
                    .unwrap(),
            },
            &human_ctx(),
        )
        .unwrap()
    {
        freeflow_core::app::Outcome::Applied(id) => id,
        other => panic!("expected Applied, got {other:?}"),
    };
    drop(store);

    let state = AppState::new(db_path.to_path_buf());
    state
        .unlock(&Passphrase::from(PASSPHRASE), false)
        .await
        .unwrap();
    (state, client_id, mission_id)
}

#[tokio::test]
async fn closing_a_mission_removes_it_from_the_active_table_but_not_from_the_all_table() {
    let db_path = test_db_path("missions-close");
    let (state, _client_id, id) = unlocked_state_with_mission(&db_path).await;
    let router = freeflow_web::router(state);

    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/missions/{id}/close"))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("ended_on=2026-12-31"))
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
                .uri("/missions/table")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(!body_text(active).await.contains("Refonte dashboard"));

    let all = router
        .oneshot(
            Request::builder()
                .uri("/missions/table?ended=true&archived=true")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(body_text(all).await.contains("Refonte dashboard"));
}

#[tokio::test]
async fn deleting_a_mission_with_time_entries_shows_what_blocks_it() {
    let db_path = test_db_path("missions-delete-blocked");
    let (state, _client_id, id) = unlocked_state_with_mission(&db_path).await;
    let router = freeflow_web::router(state);

    router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/missions/{id}/time"))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("worked_on=2026-09-10&days=2&category=billable"))
                .unwrap(),
        )
        .await
        .unwrap();

    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/missions/{id}/delete"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response.headers().contains_key("HX-Trigger"));
    let body = body_text(response).await;
    assert!(body.contains("saisie"));
}

#[tokio::test]
async fn time_entry_lifecycle_through_the_panel() {
    let db_path = test_db_path("missions-time-lifecycle");
    let (state, _client_id, id) = unlocked_state_with_mission(&db_path).await;
    let router = freeflow_web::router(state);

    let create = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/missions/{id}/time"))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "worked_on=2026-09-10&days=2&category=billable&note=Premiere+saisie",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(body_text(create).await.contains("Premiere saisie"));

    let entry_id = {
        let store = Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
        freeflow_core::missions::list_time_entries(store.connection(), id)
            .unwrap()
            .first()
            .unwrap()
            .id
    };

    let update = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/time-entries/{entry_id}"))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "revision=1&worked_on=2026-09-11&days=3.5&category=admin&note=Corrigee",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(body_text(update).await.contains("Corrigee"));

    let deleted = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/time-entries/{entry_id}/delete"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(!body_text(deleted).await.contains("Corrigee"));
}

// -------------------------------------------------------------------------------------------
// Clôture d'exercice (lot 20)
// -------------------------------------------------------------------------------------------

/// Coffre avec profil (exercice civil, capital 1 000 €) et une facture émise en 2026 — le
/// minimum pour qu'une clôture ait un résultat non nul à figer.
async fn unlocked_state_with_activity(db_path: &Path) -> AppState {
    let mut store = Store::create(db_path, &Passphrase::from(PASSPHRASE)).unwrap();
    Executor::new(&mut store)
        .execute(
            &freeflow_core::company::SetCompanyProfile {
                name: "Argon Digital".to_string(),
                legal_form: "SASU".to_string(),
                siren: freeflow_core::domain::Siren::parse("552100554").unwrap(),
                vat_number: None,
                address: freeflow_core::domain::Address {
                    street: "12 rue de la Paix".to_string(),
                    postal_code: "75002".to_string(),
                    city: "Paris".to_string(),
                    country: "FR".to_string(),
                },
                share_capital: Some(Money::from_cents(100_000)),
                rcs_city: Some("Paris".to_string()),
                iban: None,
                fiscal_year_end: Some(freeflow_core::domain::FiscalYearEnd::CALENDAR),
                vat_regime: None,
                director_monthly_gross: None,
                director_charge_ratio_bps: None,
            },
            &human_ctx(),
        )
        .unwrap();
    let client_id = match Executor::new(&mut store)
        .execute(
            &CreateClient {
                name: "Kappa Software".to_string(),
                siren: None,
                vat_number: None,
                address: None,
            },
            &human_ctx(),
        )
        .unwrap()
    {
        freeflow_core::app::Outcome::Applied(id) => id,
        other => panic!("expected Applied, got {other:?}"),
    };
    Executor::new(&mut store)
        .execute(
            &freeflow_core::billing::EmitInvoice {
                client_id,
                mission_id: None,
                lines: vec![freeflow_core::domain::InvoiceLine {
                    description: "Prestation".to_string(),
                    quantity: 9.5,
                    unit_price: Money::from_cents(65_000),
                    vat_rate: freeflow_core::domain::VatRate::Standard,
                }],
                issued_on: time::Date::from_calendar_date(2026, time::Month::September, 30)
                    .unwrap(),
                payment_terms_days: 30,
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

fn fiscal_year_id(db_path: &Path) -> freeflow_core::domain::FiscalYearId {
    let store = Store::open_with_passphrase(db_path, &Passphrase::from(PASSPHRASE)).unwrap();
    freeflow_core::fiscal_year::list_fiscal_years(store.connection())
        .unwrap()
        .first()
        .expect("un exercice clos attendu")
        .id
}

#[tokio::test]
async fn closing_a_year_from_the_window_then_downloading_its_documents() {
    let db_path = test_db_path("cloture-e2e");
    let state = unlocked_state_with_activity(&db_path).await;
    let router = freeflow_web::router(state);

    // Clore 2026 depuis le formulaire du panneau.
    let closed = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/cloture")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "starts_on=2026-01-01&ends_on=2026-12-31&legal_reserve=50&dividends=1000",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(closed.status(), StatusCode::OK);
    assert_eq!(
        closed
            .headers()
            .get("HX-Trigger")
            .map(|v| v.to_str().unwrap()),
        Some("freeflow:saved"),
        "une clôture réussie doit fermer le panneau et rafraîchir la liste"
    );
    let id = fiscal_year_id(&db_path);

    // La liste et le panneau de détail rendent l'exercice.
    let table = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/cloture/table")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let table_body = body_text(table).await;
    assert!(table_body.contains("2026-12-31"));
    assert!(table_body.contains("projet"));

    // Les documents se téléchargent avec le bon type de contenu — liasse JSON et PV PDF (rendu
    // par le vrai binaire typst, comme les tests de freeflow-docs).
    let liasse = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/cloture/{id}/doc/liasse"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        liasse
            .headers()
            .get("content-type")
            .map(|v| v.to_str().unwrap()),
        Some("application/json")
    );
    let liasse_body = body_text(liasse).await;
    assert!(liasse_body.contains("2065"));

    let minutes = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/cloture/{id}/doc/minutes"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        minutes
            .headers()
            .get("content-type")
            .map(|v| v.to_str().unwrap()),
        Some("application/pdf")
    );
    let pdf_bytes = minutes.into_body().collect().await.unwrap().to_bytes();
    assert!(pdf_bytes.starts_with(b"%PDF-"));

    // Approuver, puis vérifier que la révision d'affectation est refusée avec un bandeau.
    let approved = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/cloture/{id}/approve"))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("approved_on=2027-05-15"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        approved
            .headers()
            .get("HX-Trigger")
            .map(|v| v.to_str().unwrap()),
        Some("freeflow:saved")
    );

    let amended = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/cloture/{id}"))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("revision=2&legal_reserve=0&dividends=0"))
                .unwrap(),
        )
        .await
        .unwrap();
    let amended_body = body_text(amended).await;
    assert!(
        amended_body.contains("approuvé"),
        "réviser un exercice approuvé doit re-rendre le panneau avec l'erreur : {amended_body}"
    );
}

#[tokio::test]
async fn expense_lifecycle_through_the_panel() {
    let db_path = test_db_path("depenses-lifecycle");
    let state = unlocked_state(&db_path).await;
    let router = freeflow_web::router(state);

    // Créer.
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/depenses")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "label=Abonnement+h%C3%A9bergement&category=software&amount=120.00\
                     &vat_rate=standard&vat_deductible=20.00&incurred_on=2026-09-05",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("HX-Trigger").unwrap(),
        "freeflow:saved"
    );

    let expense_id = {
        let store = Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
        let expenses = freeflow_core::expenses::list_expenses(store.connection()).unwrap();
        assert_eq!(expenses.len(), 1);
        assert_eq!(expenses[0].amount, Money::from_cents(12_000));
        expenses[0].id
    };

    let table = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/depenses/table")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(body_text(table).await.contains("Abonnement hébergement"));

    // Modifier (révision 1 → 2) : seul le montant change, le reste est resoumis tel quel.
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/depenses/{expense_id}"))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "revision=1&label=Abonnement+h%C3%A9bergement&category=software\
                     &amount=240.00&vat_rate=standard&vat_deductible=40.00&incurred_on=2026-09-05",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("HX-Trigger").unwrap(),
        "freeflow:saved"
    );

    // Une resoumission avec la révision périmée est un conflit, pas un écrasement silencieux.
    let stale = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/depenses/{expense_id}"))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "revision=1&label=Ecrasement&category=software\
                     &amount=1.00&vat_rate=standard&vat_deductible=0.00&incurred_on=2026-09-05",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let stale_body = body_text(stale).await;
    assert!(
        stale_body.contains("form-conflict"),
        "un conflit de révision doit proposer un rechargement : {stale_body}"
    );

    // Supprimer (la révision est relue côté serveur au moment du clic).
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/depenses/{expense_id}/delete"))
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

    let table = router
        .oneshot(
            Request::builder()
                .uri("/depenses/table")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(body_text(table).await.contains("aucune dépense"));
}

#[tokio::test]
async fn a_quote_can_be_sent_and_accepted_through_the_panel() {
    let db_path = test_db_path("devis-lifecycle");
    let state = unlocked_state_with_client(&db_path).await;
    let router = freeflow_web::router(state);
    let client_id = client_id_by_name(&db_path, "Kappa Software");

    let quote_id = {
        let mut store =
            Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
        match Executor::new(&mut store)
            .execute(
                &freeflow_core::quotes::CreateQuote {
                    client_id,
                    opportunity_id: None,
                    lines: vec![freeflow_core::domain::QuoteLine {
                        description: "Refonte plateforme".to_string(),
                        kind: freeflow_core::domain::LineKind::Forfait {
                            amount: Money::from_cents(4_500_000),
                        },
                        vat_rate: freeflow_core::domain::VatRate::Standard,
                    }],
                    discount: None,
                    terms: None,
                    valid_until: time::Date::from_calendar_date(2026, time::Month::October, 31)
                        .unwrap(),
                },
                &human_ctx(),
            )
            .unwrap()
        {
            freeflow_core::app::Outcome::Applied(id) => id,
            other => panic!("expected Applied, got {other:?}"),
        }
    };

    // La fiche du brouillon propose « marquer envoyé », pas encore « accepter ».
    let detail = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/devis/{quote_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = body_text(detail).await;
    assert!(body.contains("marquer envoyé"), "{body}");
    assert!(!body.contains("accepter…"));

    let sent = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/devis/{quote_id}/send"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(sent.status(), StatusCode::OK);
    assert_eq!(sent.headers().get("HX-Trigger").unwrap(), "freeflow:saved");

    let accepted = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/devis/{quote_id}/accept"))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("started_on=2026-11-01"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(accepted.status(), StatusCode::OK);
    assert_eq!(
        accepted.headers().get("HX-Trigger").unwrap(),
        "freeflow:saved"
    );

    // L'acceptation a créé la mission, visible sur l'écran missions, et la fiche du devis
    // documente cette lignée.
    let missions_table = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/missions/table")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        body_text(missions_table)
            .await
            .contains("Refonte plateforme")
    );

    let detail = router
        .oneshot(
            Request::builder()
                .uri(format!("/devis/{quote_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = body_text(detail).await;
    assert!(body.contains("accepté"), "{body}");
    assert!(body.contains("1 mission(s)"), "{body}");
}
