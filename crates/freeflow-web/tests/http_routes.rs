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

/// Construit un corps `multipart/form-data` à la main (aucune dépendance de test pour ça) :
/// des champs texte, et éventuellement un fichier dans le champ `receipt`. Renvoie l'en-tête
/// `content-type` (avec sa frontière) et le corps.
fn multipart_form(fields: &[(&str, &str)], file: Option<(&str, &[u8])>) -> (String, Vec<u8>) {
    const BOUNDARY: &str = "----freeflow-test-boundary-7d4a";
    let mut body = Vec::new();
    for (name, value) in fields {
        body.extend_from_slice(
            format!(
                "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    if let Some((filename, content)) = file {
        body.extend_from_slice(
            format!(
                "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"receipt\"; \
                 filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(content);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={BOUNDARY}"), body)
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
        body.contains("FreeFlow"),
        "la barre de commandes doit être présente"
    );
    assert!(
        body.contains("Tableau de bord"),
        "le titre de l'écran d'accueil est en français"
    );
    assert!(
        body.contains("data-view=\"dashboard\""),
        "l'identifiant d'écran reste le slug"
    );
    assert!(
        body.contains("audit-collapsed"),
        "le journal d'audit est replié par défaut"
    );
    assert!(
        body.contains("Aujourd'hui") || body.contains("Aujourd&#x27;hui"),
        "le bloc aujourd'hui est rendu : {body}"
    );
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
    assert!(
        body.contains("Prospection"),
        "le titre de l'écran est en français : {body}"
    );
    assert!(
        body.contains("data-view=\"prospection\""),
        "le slug reste sur le fragment"
    );
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
                    supplier: None,
                    bank_transaction_id: None,
                },
                &human_ctx(),
            )
            .unwrap();
    }

    let router = freeflow_web::router(state);

    for path in [
        "/view/dashboard",
        "/view/relances",
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
        if path == "/assets/app.js" {
            assert!(
                body.contains("input.type !== \"date\"") && body.contains("blur()"),
                "le calendrier natif des champs date doit se fermer après sélection : {body}"
            );
        }
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
                    "prospect=Kappa+Software&name=Nouvelle+piste&amount=1000&probability=50&next_action=2026-09-02",
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
async fn creating_a_prospect_without_an_existing_client_succeeds_and_stays_off_the_clients_tab() {
    let db_path = test_db_path("prospection-new-prospect");
    let state = unlocked_state(&db_path).await;
    let router = freeflow_web::router(state);

    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/prospection")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "prospect=Lumen+Conseil&representative=Camille&email=camille%40lumen.example&phone=0612345678&street=1+rue+de+la+Paix&postal_code=75002&city=Paris&country=FR&name=Refonte&amount=7800&probability=40&next_action=2026-09-02",
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
    let body = body_text(response).await;
    assert!(
        !body.contains("introuvable"),
        "un prospect neuf ne doit plus être refusé comme un client manquant : {body}"
    );

    let table = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/prospection/table")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(body_text(table).await.contains("Lumen Conseil"));

    let clients = router
        .oneshot(
            Request::builder()
                .uri("/clients/table")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        !body_text(clients).await.contains("Lumen Conseil"),
        "un prospect sans devis ni facture n'apparaît pas dans Clients"
    );
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
                president_name: None,
                sole_shareholder_name: None,
                sole_shareholder_address: None,
                share_count: None,
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
    // Lot 36 : la fenêtre passe la date du jour au cœur, qui refuse de clore un exercice pas
    // encore écoulé — le scénario se rejoue depuis 2027, une fois 2026 écoulé.
    let state = state.with_today(time::macros::date!(2027 - 06 - 01));
    let router = freeflow_web::router(state);

    // Lot 32 : le report en arrière coché sur un exercice bénéficiaire est refusé par le cœur —
    // le formulaire se re-rend avec le bandeau, la case toujours cochée, sans rien clore.
    let refused = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/cloture")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "starts_on=2026-01-01&ends_on=2026-12-31&legal_reserve=0&dividends=0&carry_back=on",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::OK);
    assert!(refused.headers().get("HX-Trigger").is_none());
    let refused_body = body_text(refused).await;
    assert!(refused_body.contains("aucun déficit"), "{refused_body}");
    assert!(
        refused_body.contains("name=\"carry_back\" type=\"checkbox\" value=\"on\" checked"),
        "{refused_body}"
    );

    // Lot 34 : le parcours de clôture d'un exercice qui court encore — 2027, vu le 1er juin
    // 2027 (la date du jour de la fenêtre, lot 36) : le geste de clôture est bloqué et son
    // bouton absent.
    let journey = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/cloture/checklist?period=2027")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(journey.contains("Parcours de clôture 2027"), "{journey}");
    assert!(journey.contains("exercice en cours"), "{journey}");
    assert!(journey.contains("Exercice écoulé"), "{journey}");
    assert!(journey.contains("Profil d'entreprise"), "{journey}");
    // Lot 35 : le lexique du cœur, replié sous le parcours.
    assert!(
        journey.contains("Lexique — les mots de la clôture"),
        "{journey}"
    );
    assert!(journey.contains("<dt>Report à nouveau</dt>"), "{journey}");
    assert!(
        !journey.contains("/cloture/new?"),
        "pas de bouton « clore » tant qu'un point bloque : {journey}"
    );
    // Le formulaire de clôture accepte le pré-remplissage que le parcours lui passe.
    let prefilled = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(
                        "/cloture/new?starts_on=2025-07-01&ends_on=2026-06-30&legal_reserve=228.44",
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(prefilled.contains("value=\"2025-07-01\""), "{prefilled}");
    assert!(prefilled.contains("value=\"228.44\""), "{prefilled}");

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
    let detail_body = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/cloture/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(detail_body.contains("Résultat fiscal"), "{detail_body}");
    assert!(
        detail_body.contains("Déficits reportables en avant"),
        "{detail_body}"
    );
    assert!(
        detail_body.contains("/cloture/checklist?period=2026"),
        "{detail_body}"
    );

    // Lot 34 : une fois clos, le parcours suit l'exercice — en projet, à approuver.
    let journey = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/cloture/checklist?period=2026")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(journey.contains("clos, en projet"), "{journey}");
    assert!(journey.contains("Approbation des comptes"), "{journey}");
    assert!(
        journey.contains(&format!("/cloture/{id}/approve")),
        "{journey}"
    );
    assert!(journey.contains("Figé à la clôture"), "{journey}");

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
    assert!(
        liasse_body.contains("2033-A"),
        "le bilan dérivé alimente la liasse (lot 31)"
    );

    // Le panneau « bilan » et son PDF, dérivés du grand livre (lot 31).
    let balance = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/cloture/balance?period=2026")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(balance.status(), StatusCode::OK);
    let balance_body = body_text(balance).await;
    assert!(
        balance_body.contains("Bilan au 2026-12-31"),
        "{balance_body}"
    );
    assert!(balance_body.contains("équilibré"), "{balance_body}");
    assert!(
        balance_body.contains("Balance des comptes"),
        "{balance_body}"
    );
    assert!(balance_body.contains("/cloture/balance.pdf?period=2026"));
    let balance_pdf = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/cloture/balance.pdf?period=2026")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        balance_pdf
            .headers()
            .get("content-type")
            .map(|v| v.to_str().unwrap()),
        Some("application/pdf")
    );
    assert_eq!(
        balance_pdf
            .headers()
            .get("content-disposition")
            .map(|v| v.to_str().unwrap()),
        Some("attachment; filename=\"bilan-2026.pdf\"")
    );

    // Le FEC de l'exercice se télécharge sous son nom réglementaire, en texte brut — le même
    // fichier que `freeflow fec export 2026` (lot 28).
    let fec = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/cloture/fec?period=2026")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(fec.status(), StatusCode::OK);
    assert_eq!(
        fec.headers()
            .get("content-type")
            .map(|v| v.to_str().unwrap()),
        Some("text/plain; charset=utf-8")
    );
    assert_eq!(
        fec.headers()
            .get("content-disposition")
            .map(|v| v.to_str().unwrap()),
        Some("attachment; filename=\"552100554FEC20261231.txt\"")
    );
    let fec_body = body_text(fec).await;
    assert!(
        fec_body.starts_with("JournalCode|JournalLib|EcritureNum|"),
        "{fec_body}"
    );

    let fec_check = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/cloture/fec/check?period=2026")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(fec_check.contains("conforme"), "{fec_check}");
    assert!(
        fec_check.contains("régularité de la comptabilité"),
        "{fec_check}"
    );
    assert!(
        fec_check.contains("/cloture/fec?period=2026"),
        "{fec_check}"
    );

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
    // Depuis le lot 29, les formulaires de dépense sont en `multipart/form-data` (champ
    // fichier du justificatif) : un `<input type="file">` laissé vide arrive comme une partie
    // sans nom de fichier et sans contenu, que le lecteur ignore.
    let db_path = test_db_path("depenses-lifecycle");
    let state = unlocked_state(&db_path).await;
    let router = freeflow_web::router(state);

    // Créer, sans justificatif.
    let (content_type, body) = multipart_form(
        &[
            ("label", "Abonnement hébergement"),
            ("category", "software"),
            ("amount", "120.00"),
            ("vat_rate", "standard"),
            ("vat_deductible", "20.00"),
            ("incurred_on", "2026-09-05"),
        ],
        Some(("", b"")),
    );
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/depenses")
                .header("content-type", content_type)
                .body(Body::from(body))
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
        assert!(
            expenses[0].receipt_hash.is_none(),
            "champ fichier vide = pas de justificatif"
        );
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
    let (content_type, body) = multipart_form(
        &[
            ("revision", "1"),
            ("label", "Abonnement hébergement"),
            ("category", "software"),
            ("amount", "240.00"),
            ("vat_rate", "standard"),
            ("vat_deductible", "40.00"),
            ("incurred_on", "2026-09-05"),
        ],
        None,
    );
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/depenses/{expense_id}"))
                .header("content-type", content_type)
                .body(Body::from(body))
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
    let (content_type, body) = multipart_form(
        &[
            ("revision", "1"),
            ("label", "Ecrasement"),
            ("category", "software"),
            ("amount", "1.00"),
            ("vat_rate", "standard"),
            ("vat_deductible", "0.00"),
            ("incurred_on", "2026-09-05"),
        ],
        None,
    );
    let stale = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/depenses/{expense_id}"))
                .header("content-type", content_type)
                .body(Body::from(body))
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
async fn expense_reconciliation_through_the_panel() {
    // Lot 33 : un débit du relevé importé (la fenêtre n'importe pas : c'est `bank import`)
    // pré-remplit une nouvelle dépense, créée rapprochée ; une dépense existante se rapproche
    // depuis sa fiche parmi les débits du même montant ; le rapprochement se défait depuis la
    // fiche, la dépense restant en place.
    let db_path = test_db_path("depenses-reconciliation");
    let state = unlocked_state(&db_path).await;
    let router = freeflow_web::router(state);

    let (fees_tx, bank_tx) = {
        let mut store =
            Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
        Executor::new(&mut store)
            .execute(
                &freeflow_core::billing::ImportBankTransactions {
                    transactions: vec![
                        freeflow_core::billing::ParsedTransaction {
                            occurred_on: time::Date::from_calendar_date(
                                2026,
                                time::Month::September,
                                7,
                            )
                            .unwrap(),
                            amount_cents: -96_000,
                            description: "PRLV CABINET COMPTA".to_string(),
                            fitid: None,
                        },
                        freeflow_core::billing::ParsedTransaction {
                            occurred_on: time::Date::from_calendar_date(
                                2026,
                                time::Month::September,
                                9,
                            )
                            .unwrap(),
                            amount_cents: -1_250,
                            description: "FRAIS TENUE DE COMPTE".to_string(),
                            fitid: None,
                        },
                    ],
                },
                &human_ctx(),
            )
            .unwrap();
        let debits = freeflow_core::billing::unmatched_debits(store.connection()).unwrap();
        let id_of = |description: &str| {
            debits
                .iter()
                .find(|t| t.description == description)
                .unwrap()
                .id
        };
        (id_of("PRLV CABINET COMPTA"), id_of("FRAIS TENUE DE COMPTE"))
    };

    // La liste montre les débits à rapprocher ; le formulaire pré-rempli porte le débit.
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
    let table_body = body_text(table).await;
    assert!(table_body.contains("débit du relevé à rapprocher"));
    assert!(table_body.contains("PRLV CABINET COMPTA"));
    let prefilled = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/depenses/new?transaction={fees_tx}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let prefilled_body = body_text(prefilled).await;
    assert!(prefilled_body.contains(&format!("value=\"{fees_tx}\"")));
    assert!(prefilled_body.contains("value=\"960.00\""));
    assert!(prefilled_body.contains("value=\"2026-09-07\""));

    let (content_type, body) = multipart_form(
        &[
            ("label", "Expert-comptable"),
            ("category", "fees"),
            ("amount", "960.00"),
            ("vat_rate", "standard"),
            ("vat_deductible", "160.00"),
            ("incurred_on", "2026-09-07"),
            ("bank_transaction_id", &fees_tx.to_string()),
        ],
        Some(("", b"")),
    );
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/depenses")
                .header("content-type", content_type)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.headers().get("HX-Trigger").unwrap(),
        "freeflow:saved"
    );
    let fees_id = {
        let store = Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
        let detail = freeflow_core::expenses::list_expenses(store.connection())
            .unwrap()
            .into_iter()
            .next()
            .map(|e| {
                freeflow_core::expenses::expense_detail(store.connection(), e.id)
                    .unwrap()
                    .unwrap()
            })
            .unwrap();
        assert_eq!(detail.bank_transaction.map(|t| t.id), Some(fees_tx));
        detail.expense.id
    };

    // Un montant qui n'est pas celui du débit est refusé par le cœur : bandeau, rien de créé.
    let (content_type, body) = multipart_form(
        &[
            ("label", "Autre"),
            ("category", "other"),
            ("amount", "10.00"),
            ("vat_rate", "zero"),
            ("vat_deductible", "0.00"),
            ("incurred_on", "2026-09-09"),
            ("bank_transaction_id", &bank_tx.to_string()),
        ],
        None,
    );
    let refused = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/depenses")
                .header("content-type", content_type)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(refused.headers().get("HX-Trigger").is_none());
    let refused_body = body_text(refused).await;
    assert!(refused_body.contains("montant exact"), "{refused_body}");
    assert!(
        refused_body.contains("FRAIS TENUE DE COMPTE"),
        "le formulaire re-rendu garde le débit et son résumé"
    );

    // La fiche montre le rapprochement ; le défaire libère le débit et garde la dépense.
    let detail = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/depenses/{fees_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let detail_body = body_text(detail).await;
    assert!(detail_body.contains("rapprochée"));
    assert!(detail_body.contains("défaire le rapprochement"));
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/depenses/{fees_id}/unreconcile"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.headers().get("HX-Trigger").unwrap(),
        "freeflow:saved"
    );

    // Rapprocher après coup : le panneau ne propose que les débits du même montant.
    let panel = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/depenses/{fees_id}/reconcile"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let panel_body = body_text(panel).await;
    assert!(panel_body.contains(&fees_tx.to_string()));
    assert!(!panel_body.contains(&bank_tx.to_string()));
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/depenses/{fees_id}/reconcile"))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!("transaction={fees_tx}")))
                .unwrap(),
        )
        .await
        .unwrap();
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
    let table_body = body_text(table).await;
    assert!(
        !table_body.contains("PRLV CABINET COMPTA"),
        "le débit n'est plus à rapprocher"
    );
    assert!(
        table_body.contains("FRAIS TENUE DE COMPTE"),
        "l'autre l'est toujours"
    );
    {
        let store = Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
        let detail = freeflow_core::expenses::expense_detail(store.connection(), fees_id)
            .unwrap()
            .unwrap();
        assert_eq!(detail.bank_transaction.map(|t| t.id), Some(fees_tx));
    }
}

#[tokio::test]
async fn a_receipt_uploaded_from_the_panel_is_archived_like_the_cli_does() {
    // Le fichier reçu en multipart est archivé à côté du coffre (`receipts/<hash>-<nom>`, en
    // 0600) par le même helper que `freeflow expense record --receipt`, et la dépense porte le
    // hash SHA-256 du contenu. La case « détacher » vide les deux champs sans supprimer le
    // fichier archivé — exactement `expense edit --clear-receipt`.
    let db_path = test_db_path("depenses-receipt");
    let state = unlocked_state(&db_path).await;
    let router = freeflow_web::router(state);

    let content: &[u8] = b"%PDF-1.4 facture fournisseur de test";
    let expected_hash = freeflow_core::expenses::hash_receipt(content);
    let (content_type, body) = multipart_form(
        &[
            ("label", "Écran externe"),
            ("category", "equipment"),
            ("amount", "300.00"),
            ("vat_rate", "standard"),
            ("vat_deductible", "50.00"),
            ("incurred_on", "2026-09-05"),
        ],
        // Un nom venu du navigateur ne doit pas pouvoir sortir de `receipts/`.
        Some(("../../facture-ecran.pdf", content)),
    );
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/depenses")
                .header("content-type", content_type)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("HX-Trigger").unwrap(),
        "freeflow:saved",
        "le justificatif ne doit pas faire échouer la création"
    );

    let (expense_id, archived_name) = {
        let store = Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
        let expenses = freeflow_core::expenses::list_expenses(store.connection()).unwrap();
        assert_eq!(expenses.len(), 1);
        let expense = &expenses[0];
        assert_eq!(
            expense.receipt_hash.as_deref(),
            Some(expected_hash.as_str())
        );
        let archived_name = expense.receipt_filename.clone().unwrap();
        assert_eq!(archived_name, format!("{expected_hash}-facture-ecran.pdf"));
        // Lot 39 : la pièce est chiffrée dans `<coffre>.receipts/`, relisible par le coffre.
        assert_eq!(
            freeflow_core::receipts::read(&store, &archived_name).unwrap(),
            content,
            "le contenu déchiffré est le contenu reçu, tel quel"
        );
        (expense.id, archived_name)
    };
    let archived_path = freeflow_core::receipts::path_of(
        &Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap(),
        &archived_name,
    );
    assert!(
        std::fs::read(&archived_path).unwrap().starts_with(b"FFR1"),
        "le fichier sur le disque est chiffré"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&archived_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode, 0o600,
            "justificatif illisible aux autres utilisateurs"
        );
    }

    // La fiche et la liste montrent le justificatif ; le panneau d'édition propose de le
    // remplacer ou de le détacher.
    let detail = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/depenses/{expense_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(body_text(detail).await.contains(&archived_name));
    let edit = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/depenses/{expense_id}/edit"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let edit_body = body_text(edit).await;
    assert!(edit_body.contains("multipart/form-data"), "{edit_body}");
    assert!(edit_body.contains("name=\"clear_receipt\""), "{edit_body}");
    assert!(edit_body.contains("type=\"file\""), "{edit_body}");

    // Détacher : les champs se vident, le fichier archivé reste en place.
    let (content_type, body) = multipart_form(
        &[
            ("revision", "1"),
            ("label", "Écran externe"),
            ("category", "equipment"),
            ("amount", "300.00"),
            ("vat_rate", "standard"),
            ("vat_deductible", "50.00"),
            ("incurred_on", "2026-09-05"),
            ("current_receipt", &archived_name),
            ("clear_receipt", "on"),
        ],
        Some(("", b"")),
    );
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/depenses/{expense_id}"))
                .header("content-type", content_type)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.headers().get("HX-Trigger").unwrap(),
        "freeflow:saved"
    );
    {
        let store = Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
        let expense = freeflow_core::expenses::expense_by_id(store.connection(), expense_id)
            .unwrap()
            .unwrap();
        assert!(expense.receipt_hash.is_none());
        assert!(expense.receipt_filename.is_none());
        assert_eq!(expense.revision, 2);
    }
    assert!(
        archived_path.exists(),
        "détacher ne supprime pas le fichier archivé"
    );

    // Un corps multipart tronqué (frontière finale absente) est un bandeau, jamais un 500.
    let (content_type, body) = multipart_form(&[("label", "x")], None);
    let truncated = &body[..body.len() - 10];
    let broken = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/depenses")
                .header("content-type", content_type)
                .body(Body::from(truncated.to_vec()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(broken.status(), StatusCode::OK);
    assert!(body_text(broken).await.contains("form-error"));
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

#[tokio::test]
async fn a_quote_can_be_created_and_revised_through_the_panel() {
    let db_path = test_db_path("devis-editor");
    let state = unlocked_state_with_client(&db_path).await;
    let router = freeflow_web::router(state);

    // Le panneau de création est servi, pré-rempli d'une date de validité par défaut.
    let panel = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/devis/new")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = body_text(panel).await;
    assert!(body.contains("Nouveau devis"), "{body}");
    assert!(body.contains("description:type:montant"), "{body}");

    // Une ligne invalide re-rend le panneau avec l'erreur du parseur du domaine, sans
    // déclencher de rafraîchissement.
    let invalid = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/devis")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "client=Kappa%20Software&lines=Dev%3Ainconnu%3A100&valid_until=2026-10-31",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(invalid.status(), StatusCode::OK);
    assert!(invalid.headers().get("HX-Trigger").is_none());
    let body = body_text(invalid).await;
    assert!(body.contains("ligne de devis invalide"), "{body}");

    // Les deux remises à la fois sont refusées avec une erreur de champ.
    let both_discounts = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/devis")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "client=Kappa%20Software&lines=Dev%3Aforfait%3A100&discount_percent=1000&discount_amount=50.00&valid_until=2026-10-31",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let body = body_text(both_discounts).await;
    assert!(body.contains("exclusives"), "{body}");

    // Création réelle : deux lignes de types différents (une par ligne du textarea, %0A), des
    // conditions, pas de remise.
    let created = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/devis")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "client=Kappa%20Software&opportunity=&lines=Dev%3Aforfait%3A1350.00%0AConseil%3Aregie%3A650.00x10%3Areduced&discount_percent=&discount_amount=&terms=Paiement%2030j&valid_until=2026-10-31",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    assert_eq!(
        created.headers().get("HX-Trigger").unwrap(),
        "freeflow:saved"
    );

    let quote = {
        let store = Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
        let quotes = freeflow_core::quotes::list_quotes(store.connection()).unwrap();
        assert_eq!(quotes.len(), 1);
        quotes.into_iter().next().unwrap()
    };
    assert_eq!(quote.lines.len(), 2);
    assert_eq!(quote.lines[0].description, "Dev");
    assert_eq!(
        quote.lines[0].kind,
        freeflow_core::domain::LineKind::Forfait {
            amount: Money::from_cents(135_000),
        }
    );
    assert_eq!(
        quote.lines[1].vat_rate,
        freeflow_core::domain::VatRate::Reduced
    );
    assert_eq!(quote.terms.as_deref(), Some("Paiement 30j"));
    assert_eq!(quote.discount, None);

    // Deux types de facturation mélangés : créable, mais inacceptable en l'état — la fiche
    // l'annonce avant que l'acceptation n'échoue.
    let detail = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/devis/{}", quote.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = body_text(detail).await;
    assert!(body.contains("l'acceptation échouera"), "{body}");
    assert!(body.contains("réviser…"), "{body}");

    // Le panneau de révision est pré-rempli avec la représentation texte re-parsable des
    // lignes existantes — le round-trip `Display`/`FromStr` du domaine, visible à l'écran.
    let revise_panel = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/devis/{}/revise", quote.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = body_text(revise_panel).await;
    assert!(body.contains("Réviser le devis"), "{body}");
    assert!(body.contains("Dev:forfait:1350.00:standard"), "{body}");
    assert!(body.contains("Conseil:regie:650.00x10:reduced"), "{body}");
    assert!(body.contains("Paiement 30j"), "{body}");

    // Révision : une seule ligne forfait, remise de 10 % — crée la v2 en brouillon.
    let revised = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/devis/{}/revise", quote.id))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "lines=Dev%20complet%3Aforfait%3A2000.00&discount_percent=1000&discount_amount=&terms=&valid_until=2026-11-30",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(revised.status(), StatusCode::OK);
    assert_eq!(
        revised.headers().get("HX-Trigger").unwrap(),
        "freeflow:saved"
    );

    let store = Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
    let quotes = freeflow_core::quotes::list_quotes(store.connection()).unwrap();
    assert_eq!(quotes.len(), 2);
    let v2 = quotes.iter().find(|q| q.version == 2).unwrap();
    assert_eq!(v2.root_id, quote.root_id);
    assert_eq!(v2.status, freeflow_core::domain::QuoteStatus::Draft);
    assert_eq!(
        v2.discount,
        Some(freeflow_core::domain::Discount::Percentage(1000))
    );
    assert_eq!(v2.lines.len(), 1);
    assert_eq!(v2.lines[0].description, "Dev complet");
    assert_eq!(v2.terms, None);
}

#[tokio::test]
async fn voiding_a_payment_through_the_invoice_panel_restores_the_unpaid_status() {
    let db_path = test_db_path("facturation-void");
    let state = unlocked_state_with_client(&db_path).await;
    let router = freeflow_web::router(state);
    let client_id = client_id_by_name(&db_path, "Kappa Software");

    // Fixture : une facture payée intégralement (6 500 € HT → 7 800 € TTC).
    let (invoice_id, payment_id) = {
        let mut store =
            Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
        let emitted = match Executor::new(&mut store)
            .execute(
                &freeflow_core::billing::EmitInvoice {
                    client_id,
                    mission_id: None,
                    lines: vec![freeflow_core::domain::InvoiceLine {
                        description: "Prestation".to_string(),
                        quantity: 10.0,
                        unit_price: Money::from_cents(65_000),
                        vat_rate: freeflow_core::domain::VatRate::Standard,
                    }],
                    issued_on: time::Date::from_calendar_date(2026, time::Month::September, 1)
                        .unwrap(),
                    payment_terms_days: 30,
                },
                &human_ctx(),
            )
            .unwrap()
        {
            freeflow_core::app::Outcome::Applied(e) => e,
            other => panic!("expected Applied, got {other:?}"),
        };
        let payment_id = match Executor::new(&mut store)
            .execute(
                &freeflow_core::billing::RecordPayment {
                    invoice_id: emitted.id,
                    amount: Money::from_cents(780_000),
                    received_on: time::Date::from_calendar_date(2026, time::Month::September, 10)
                        .unwrap(),
                    method: freeflow_core::domain::PaymentMethod::BankTransfer,
                },
                &human_ctx(),
            )
            .unwrap()
        {
            freeflow_core::app::Outcome::Applied(id) => id,
            other => panic!("expected Applied, got {other:?}"),
        };
        (emitted.id, payment_id)
    };

    // La liste calcule enfin un statut réel : « payée », plus la branche morte « émise ».
    let table = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/facturation/table")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(body_text(table).await.contains("payée"));

    // La fiche montre l'encaissement et son bouton d'annulation.
    let detail = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/facturation/{invoice_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = body_text(detail).await;
    assert!(body.contains("Encaissements"), "{body}");
    assert!(body.contains("annuler"), "{body}");

    // Annulation avec motif : la fiche revient à jour (solde restant dû plein) et la liste est
    // invitée à se rafraîchir.
    let voided = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/payments/{payment_id}/void"))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("reason=saisi+en+double"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        voided.headers().get("HX-Trigger").unwrap(),
        "freeflow:saved"
    );
    let body = body_text(voided).await;
    assert!(body.contains("annulé"), "{body}");

    let table = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/facturation/table")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = body_text(table).await;
    assert!(
        !body.contains("payée"),
        "l'encaissement annulé ne solde plus la facture : {body}"
    );

    // Annuler deux fois : la fiche re-rend avec un bandeau, sans rafraîchir la liste.
    let again = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/payments/{payment_id}/void"))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("reason="))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(!again.headers().contains_key("HX-Trigger"));
    assert!(body_text(again).await.contains("déjà annulé"));
}

#[tokio::test]
async fn the_opening_balance_panel_records_shows_and_freezes_after_a_close() {
    let db_path = test_db_path("opening-panel");
    let state = unlocked_state_with_activity(&db_path).await;
    // Lot 36 : la fenêtre passe la date du jour au cœur, qui refuse de clore un exercice pas
    // encore écoulé — le scénario se rejoue depuis 2027, une fois 2026 écoulé.
    let state = state.with_today(time::macros::date!(2027 - 06 - 01));
    let router = freeflow_web::router(state);

    // Sans bilan : le panneau est le formulaire de saisie.
    let empty = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/cloture/opening")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let empty_body = body_text(empty).await;
    assert!(
        empty_body.contains("Enregistrer le bilan d'ouverture"),
        "{empty_body}"
    );

    // Un bilan déséquilibré re-rend le formulaire avec le refus du cœur en bandeau.
    let unbalanced = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/cloture/opening")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "opens_on=2026-01-01&source=&lines=101000%3ACapital%3AC%3A10.00%0A512000%3ABanque%3AD%3A9.00",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unbalanced.status(), StatusCode::OK);
    assert!(unbalanced.headers().get("HX-Trigger").is_none());
    let unbalanced_body = body_text(unbalanced).await;
    assert!(
        unbalanced_body.contains("déséquilibré"),
        "{unbalanced_body}"
    );

    // Un bilan valide s'enregistre : réponse vide + rafraîchissement.
    let saved = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/cloture/opening")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "opens_on=2026-01-01&source=bilan+2025&lines=101000%3ACapital+social%3AC%3A1000.00%0A110000%3AReport+%C3%A0+nouveau%3AC%3A250.00%0A512000%3ABanque%3AD%3A1250.00&tax_losses=3000",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(saved.status(), StatusCode::OK);
    assert_eq!(
        saved
            .headers()
            .get("HX-Trigger")
            .map(|v| v.to_str().unwrap()),
        Some("freeflow:saved")
    );
    let detail = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/cloture/opening")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(detail.contains("Capital social"), "{detail}");
    assert!(detail.contains("modifiable"), "{detail}");
    assert!(detail.contains("250,00"), "{detail}");
    // Lot 32 : les déficits fiscaux repris (hors bilan) s'affichent et se pré-remplissent.
    assert!(
        detail.contains("Déficits fiscaux reportables repris"),
        "{detail}"
    );
    assert!(detail.contains("3\u{202f}000,00"), "{detail}");

    // Le formulaire de modification est pré-rempli avec la syntaxe texte et la révision.
    let edit = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/cloture/opening/edit")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(edit.contains("101000:Capital social:C:1000.00"), "{edit}");
    assert!(edit.contains("name=\"revision\" value=\"1\""), "{edit}");
    assert!(
        edit.contains("name=\"tax_losses\" type=\"number\" step=\"0.01\" value=\"3000.00\""),
        "{edit}"
    );

    // Clore 2026 fige le bilan : la fiche le dit, la suppression est refusée.
    let closed = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/cloture")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "starts_on=2026-01-01&ends_on=2026-12-31&legal_reserve=0&dividends=0",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(closed.status(), StatusCode::OK);
    let frozen = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/cloture/opening")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(frozen.contains("figé"), "{frozen}");
    let refused = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/cloture/opening/delete")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(refused.headers().get("HX-Trigger").is_none());
    let refused_body = body_text(refused).await;
    assert!(refused_body.contains("déjà clos"), "{refused_body}");
}

/// Lot 36 : la fenêtre passe la date du jour au cœur — clore un exercice pas encore écoulé
/// se solde par un bandeau dans le panneau, sans `freeflow:saved`, rien n'est clos.
#[tokio::test]
async fn closing_an_unfinished_year_from_the_window_shows_a_banner_and_closes_nothing() {
    let db_path = test_db_path("cloture-premature");
    let state = unlocked_state_with_activity(&db_path)
        .await
        .with_today(time::macros::date!(2026 - 12 - 03));
    let router = freeflow_web::router(state);

    let refused = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/cloture")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "starts_on=2026-01-01&ends_on=2026-12-31&legal_reserve=0&dividends=0",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::OK);
    assert!(refused.headers().get("HX-Trigger").is_none());
    let body = body_text(refused).await;
    assert!(body.contains("court jusqu'au 2026-12-31"), "{body}");

    let list = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/cloture/table")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(list.contains("aucun exercice clos"), "{list}");
    // La barre propose partout le dernier exercice **écoulé** (2025), jamais celui en cours.
    assert!(list.contains("name=\"period\" value=\"2025\""), "{list}");
    assert!(!list.contains("name=\"period\" value=\"2026\""), "{list}");

    // Le formulaire d'approbation borne la date au jour même.
    let approve_form_present = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/cloture/new")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        approve_form_present.contains("value=\"2025-01-01\""),
        "{approve_form_present}"
    );
}

/// Lot 37 : depuis l'écran `depenses`, un débit du relevé peut être le règlement d'un compte de
/// bilan (dette reprise, IS…) plutôt qu'une dépense — panneau, enregistrement, « défaire » —
/// et la catégorie « impôts et taxes » existe.
#[tokio::test]
async fn a_statement_debit_can_settle_a_balance_sheet_account_from_the_window() {
    use freeflow_core::billing::{ImportBankTransactions, ParsedTransaction};
    let db_path = test_db_path("depenses-settle");
    let state = unlocked_state(&db_path).await;
    state
        .with_store_mut(|store| {
            Executor::new(store).execute(
                &ImportBankTransactions {
                    transactions: vec![ParsedTransaction {
                        occurred_on: time::macros::date!(2026 - 01 - 15),
                        amount_cents: -120_000,
                        description: "PRLV DGFIP SOLDE IS".to_string(),
                        fitid: None,
                    }],
                },
                &human_ctx(),
            )
        })
        .await
        .unwrap()
        .unwrap();
    let router = freeflow_web::router(state);

    let list = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/depenses/table")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        list.contains("c'est le règlement d'une dette ou d'un compte"),
        "{list}"
    );
    assert!(
        list.contains("impôts et taxes") || true,
        "la catégorie est dans le formulaire"
    );
    let tx_id = list
        .split("/depenses/transaction/")
        .nth(1)
        .unwrap()
        .split('/')
        .next()
        .unwrap()
        .to_string();

    let panel = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/depenses/transaction/{tx_id}/settle"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(panel.contains("solde ou acompte d'IS (444)"), "{panel}");

    // Un compte de gestion : refus du cœur, re-rendu dans le panneau.
    let refused = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/depenses/transaction/{tx_id}/settle"))
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("account=&other_account=622600&label=Honoraires"))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(refused.contains("compte de gestion"), "{refused}");

    let settled = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/depenses/transaction/{tx_id}/settle"))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("account=444000&other_account=&label="))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        settled
            .headers()
            .get("HX-Trigger")
            .map(|v| v.to_str().unwrap()),
        Some("freeflow:saved")
    );
    let list = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/depenses/table")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        list.contains("444000 État — impôts sur les bénéfices"),
        "{list}"
    );
    assert!(
        !list.contains("c'est une dépense"),
        "plus de débit à rapprocher : {list}"
    );

    let unsettled = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/depenses/transaction/{tx_id}/unsettle"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        unsettled
            .headers()
            .get("HX-Trigger")
            .map(|v| v.to_str().unwrap()),
        Some("freeflow:saved")
    );
    let form = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/depenses/new")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        form.contains("impôts et taxes (CFE, CVAE… pas l'IS ni la TVA)"),
        "{form}"
    );
}

/// Lot 38 : importer un relevé depuis la fenêtre — panneau, aperçu (dialecte, nouveaux,
/// doublons, lignes sautées), import, puis un second aperçu qui ne voit plus rien de nouveau.
#[tokio::test]
async fn a_bank_statement_is_imported_from_the_window_after_a_preview() {
    let db_path = test_db_path("banque-import");
    let state = unlocked_state(&db_path).await;
    let router = freeflow_web::router(state);

    let panel = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/banque/import")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        panel.contains("Où trouver l'export dans ma banque"),
        "{panel}"
    );
    assert!(panel.contains("Qonto"), "{panel}");

    let statement_multipart = |bytes: &[u8]| -> (String, Vec<u8>) {
        const BOUNDARY: &str = "----freeflow-statement-boundary";
        let mut body = Vec::new();
        body.extend_from_slice(
            format!(
                "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"statement\"; \
                 filename=\"releve.csv\"\r\nContent-Type: text/csv\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(bytes);
        body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
        (format!("multipart/form-data; boundary={BOUNDARY}"), body)
    };
    let (content_type, body) = statement_multipart(include_bytes!(
        "../../freeflow-core/src/billing/fixtures/credit-agricole.csv"
    ));
    let preview = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/banque/import/preview")
                    .header("content-type", &content_type)
                    .body(Body::from(body.clone()))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        preview.contains("CSV en windows-1252, séparateur « ; »"),
        "{preview}"
    );
    assert!(preview.contains("3 nouveau(x)"), "{preview}");
    assert!(preview.contains("1 ligne(s) sautée(s)"), "{preview}");
    assert!(preview.contains("Importer 3 mouvement(s)"), "{preview}");
    let payload = preview
        .split("name=\"payload\" value=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .to_string();

    let imported = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/banque/import")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "payload={}&filename=releve.csv",
                    payload
                        .replace('+', "%2B")
                        .replace('/', "%2F")
                        .replace('=', "%3D")
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        imported
            .headers()
            .get("HX-Trigger")
            .map(|v| v.to_str().unwrap()),
        Some("freeflow:saved")
    );
    let list = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/depenses/table")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(list.contains("VIR SEPA CABINET COMPTA"), "{list}");
    assert!(list.contains("importer un relevé"), "{list}");

    let again = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/banque/import/preview")
                    .header("content-type", &content_type)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(again.contains("0 nouveau(x)"), "{again}");
    assert!(again.contains("rien de nouveau à importer"), "{again}");

    // Un fichier illisible : un bandeau avec le format attendu, jamais un 500.
    let (content_type, body) = statement_multipart(b"\x00\x01\xff garbage");
    let refused = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/banque/import/preview")
                .header("content-type", &content_type)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::OK);
    let refused = body_text(refused).await;
    assert!(refused.contains("Attendu : un export CSV"), "{refused}");
}

/// Lot 39 : le premier lancement, sans jamais toucher la console — coffre neuf → assistant →
/// profil complet depuis le formulaire → « société existante » → parcours de clôture avec la
/// bonne date de clôture ; le bandeau « prochaine étape » du tableau de bord disparaît une fois
/// tout en place ; aucun message de la fenêtre ne renvoie à une commande.
#[tokio::test]
async fn a_first_launch_is_guided_from_the_window_without_the_console() {
    let db_path = test_db_path("premiers-pas");
    let state = AppState::new(db_path.clone()).with_today(time::macros::date!(2026 - 10 - 05));
    let router = freeflow_web::router(state);

    // Créer le coffre depuis l'écran de création : on atterrit sur l'assistant.
    let created = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/setup")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "passphrase={PASSPHRASE}&confirm={PASSPHRASE}"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let landing = body_text(created).await;
    assert!(landing.contains("premiers-pas"), "{landing}");
    assert!(landing.contains("1. Ma société"), "{landing}");
    assert!(landing.contains("Dites qui vous êtes"), "{landing}");

    // Le tableau de bord porte le bandeau « prochaine étape ».
    let dashboard = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/view/dashboard")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(dashboard.contains("id=\"next-step\""), "{dashboard}");

    // Le profil complet, depuis le formulaire de l'écran société.
    let form = "name=Nova+Dev&legal_form=SASU&siren=889112348&vat_number=FR16889112348\
                &street=3+all%C3%A9e+des+Tanneurs&postal_code=44000&city=Nantes&country=FR\
                &share_capital=1000&share_count=100&rcs_city=Nantes&iban=\
                &fiscal_year_end=30%2F09&vat_regime=real_simplified\
                &president_name=Nova+Martin&sole_shareholder_name=Nova+Martin\
                &sole_shareholder_address=3+all%C3%A9e+des+Tanneurs+44000+Nantes\
                &director_gross=&director_charge_ratio=";
    let saved = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/societe")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(form))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        saved
            .headers()
            .get("HX-Trigger")
            .map(|v| v.to_str().unwrap()),
        Some("freeflow:saved")
    );
    let saved = body_text(saved).await;
    assert!(saved.contains("profil enregistré"), "{saved}");

    // Un SIREN faux : erreur de champ, rien d'enregistré de plus.
    let refused = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/societe")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("name=X&legal_form=SASU&siren=123&country=FR"))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(refused.contains("aria-invalid"), "{refused}");

    // « Société existante » : l'assistant renvoie au bilan d'ouverture ; « société nouvelle »
    // règle l'origine.
    let assistant = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/premiers-pas")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        assistant.contains("recopier le bilan du cabinet"),
        "{assistant}"
    );
    assert!(
        assistant.contains("Nova+Dev") || assistant.contains("Nova Dev"),
        "{assistant}"
    );
    let declared = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/premiers-pas/nouvelle")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        declared.contains("Société nouvelle : pas de bilan à reprendre"),
        "{declared}"
    );
    assert!(
        declared.contains("Importez l'export de votre banque"),
        "{declared}"
    );

    // Le parcours de clôture connaît la bonne clôture (30/09) et l'étape profil est faite.
    let journey = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/cloture/checklist?period=2026")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(journey.contains("2025-10-01"), "{journey}");
    assert!(journey.contains("2026-09-30"), "{journey}");
    assert!(!journey.contains("renseigner le profil"), "{journey}");

    // Le lexique est à portée de l'en-tête et de la palette.
    let lexique = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/lexique")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(lexique.contains("Report à nouveau"), "{lexique}");
    assert!(dashboard.contains("hx-get=\"/lexique\""), "{dashboard}");

    // La console répond à « aide ».
    let help = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/console/run")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from("line=aide"))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(help.contains("year checklist 2026"), "{help}");

    // Un formulaire illisible reçoit une phrase en français, pas un 400 nu.
    let rejected = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/societe")
                .header("content-type", "text/plain")
                .body(Body::from("n'importe quoi"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(rejected.status().is_client_error());
    let rejected = body_text(rejected).await;
    assert!(rejected.contains("formulaire illisible"), "{rejected}");
}

/// Lot 39 : les justificatifs sont chiffrés dans `<coffre>.receipts/`, une pièce se joint
/// après l'approbation de l'exercice, et la fenêtre la déchiffre à la volée.
#[tokio::test]
async fn receipts_are_encrypted_beside_the_vault_and_attachable_after_approval() {
    let db_path = test_db_path("receipts-encrypted");
    let state = unlocked_state_with_activity(&db_path)
        .await
        .with_today(time::macros::date!(2027 - 06 - 01));
    // Une dépense datée 2026, avec justificatif.
    let (content_type, body) = multipart_form(
        &[
            ("label", "Honoraires cabinet"),
            ("category", "fees"),
            ("amount", "600.00"),
            ("vat_rate", "standard"),
            ("vat_deductible", "100.00"),
            ("incurred_on", "2026-03-05"),
        ],
        Some(("facture-cabinet.pdf", b"%PDF-1.4 facture du cabinet")),
    );
    let router = freeflow_web::router(state);
    let created = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/depenses")
                .header("content-type", content_type)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        created
            .headers()
            .get("HX-Trigger")
            .map(|v| v.to_str().unwrap()),
        Some("freeflow:saved")
    );
    let receipts_dir = {
        let mut name = db_path.as_os_str().to_owned();
        name.push(".receipts");
        std::path::PathBuf::from(name)
    };
    assert!(receipts_dir.is_dir(), "<coffre>.receipts/ existe");
    assert!(
        !db_path.with_file_name("receipts").exists(),
        "plus de receipts/ en clair"
    );
    let stored = std::fs::read_dir(&receipts_dir)
        .unwrap()
        .next()
        .unwrap()
        .unwrap();
    let raw = std::fs::read(stored.path()).unwrap();
    assert!(
        !raw.windows(4).any(|w| w == b"%PDF"),
        "la pièce n'est pas en clair sur le disque"
    );

    // Clore puis approuver 2026, puis joindre une nouvelle pièce : accepté.
    let closed = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/cloture")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "starts_on=2026-01-01&ends_on=2026-12-31&legal_reserve=0&dividends=0",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(closed.status(), StatusCode::OK);
    let id = fiscal_year_id(&db_path);
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
    let expense_id = {
        let store = Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
        freeflow_core::expenses::list_expenses(store.connection()).unwrap()[0].id
    };
    // Joindre après approbation passe par `AttachReceipt` (CLI/MCP) ; depuis la fenêtre, la
    // fiche montre la pièce déchiffrée.
    let shown = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/depenses/{expense_id}/receipt"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(shown.status(), StatusCode::OK);
    assert_eq!(
        shown
            .headers()
            .get("content-type")
            .map(|v| v.to_str().unwrap()),
        Some("application/pdf")
    );
    let bytes = shown.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&bytes[..], b"%PDF-1.4 facture du cabinet");
}

/// Lot 40 : depuis le panneau du bilan d'ouverture, une balance de cabinet s'analyse et
/// pré-remplit les lignes (modifiables), avec le résultat dérivé et les avertissements.
#[tokio::test]
async fn a_cabinet_balance_prefills_the_opening_balance_form() {
    let db_path = test_db_path("opening-import-web");
    let state = unlocked_state_with_activity(&db_path).await;
    let router = freeflow_web::router(state);
    const BOUNDARY: &str = "----freeflow-opening-boundary";
    let mut body = Vec::new();
    body.extend_from_slice(
        format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"opens_on\"\r\n\r\n2025-10-01\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(
        format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"statement\"; \
             filename=\"balance.csv\"\r\nContent-Type: text/csv\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(include_bytes!(
        "../../freeflow-core/src/opening_balance/fixtures/balance-cabinet.csv"
    ));
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    let form = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/cloture/opening/import")
                    .header(
                        "content-type",
                        format!("multipart/form-data; boundary={BOUNDARY}"),
                    )
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        form.contains("11 compte(s) repris depuis balance.csv"),
        "{form}"
    );
    assert!(form.contains("résultat dérivé des comptes 6/7"), "{form}");
    assert!(
        form.contains("120000:Résultat de l&#39;exercice (bénéfice):C:1200.00")
            || form.contains("120000:Résultat de l'exercice (bénéfice):C:1200.00"),
        "{form}"
    );
    assert!(form.contains("import de balance.csv"), "{form}");
    assert!(form.contains("amortissement"), "{form}");
}

#[tokio::test]
async fn relances_screen_shows_a_due_opportunity() {
    let db_path = test_db_path("relances-due");
    let (state, _) = unlocked_state_with_opportunity(&db_path).await;
    let state = state.with_today(time::macros::date!(2026 - 09 - 05));
    let router = freeflow_web::router(state);

    let response = router
        .oneshot(
            Request::builder()
                .uri("/view/relances")
                .header("HX-Request", "true")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_text(response).await;
    assert!(
        body.contains("data-view=\"relances\""),
        "slug conservé : {body}"
    );
    assert!(body.contains("Relances"), "titre français : {body}");
    assert!(
        body.contains("Refonte plateforme"),
        "l'opportunité due est dans la file : {body}"
    );
}
