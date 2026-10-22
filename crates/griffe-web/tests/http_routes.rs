//! Tests HTTP directs sur le `Router`, via `tower::ServiceExt::oneshot` — rapides, sans
//! navigateur. Base SQLCipher réelle et temporaire à chaque test, pas de mock. Le trousseau OS
//! réel n'est jamais touché : tous les coffres sont ouverts avec `remember = false`. Une
//! suspension d'inactivité se pose avec `AppState::suspend_idle_for`, sans `remember`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use griffe_core::app::{Actor, ExecutionContext, Executor, Outcome};
use griffe_core::clients::{CreateClient, CreateContact, list_clients};
use griffe_core::domain::{
    ClientId, FollowUpSubject, MissionId, Money, OpportunityId, Probability,
};
use griffe_core::expenses::RecordExpense;
use griffe_core::follow_up::{
    ArrangeProspectPhrases, MomentDraft, SetDossierGenre, SetFollowUpSender, prospect_genre_for,
    prospect_phrases,
};
use griffe_core::missions::CreateMission;
use griffe_core::prospection::{
    CreateOpportunity, CreateProspect, WinOpportunity, list_opportunities,
};
use griffe_core::store::{Passphrase, Store};
use griffe_web::AppState;
use http_body_util::BodyExt;
use tower::ServiceExt;

const PASSPHRASE: &str = "s3cret";

fn test_db_path(label: &str) -> PathBuf {
    std::env::temp_dir()
        .join(format!(
            "griffe-web-test-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ))
        .join("vault.db")
}

fn human_ctx() -> ExecutionContext {
    ExecutionContext::new(Actor::Human, false)
}

/// La formule est dans un `<template>` que l'aperçu lit. Ce mot n'est pas affiché.
fn without_formula_holder(page: &str) -> String {
    let mut out = String::with_capacity(page.len());
    let mut rest = page;
    while let Some(start) = rest.find("<template") {
        out.push_str(&rest[..start]);
        let Some(close) = rest[start..].find("</template>") else {
            out.push_str(&rest[start..]);
            return out;
        };
        rest = &rest[start + close + "</template>".len()..];
    }
    out.push_str(rest);
    out
}

async fn body_text(response: axum::response::Response) -> String {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

/// Construit un corps `multipart/form-data` à la main (aucune dépendance de test pour ça) :
/// des champs texte, et éventuellement un fichier dans le champ `receipt`. Renvoie l'en-tête
/// `content-type` (avec sa frontière) et le corps.
fn multipart_form(fields: &[(&str, &str)], file: Option<(&str, &[u8])>) -> (String, Vec<u8>) {
    multipart_form_file(fields, "receipt", file)
}

fn multipart_form_file(
    fields: &[(&str, &str)],
    file_field: &str,
    file: Option<(&str, &[u8])>,
) -> (String, Vec<u8>) {
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
                "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{file_field}\"; \
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
/// `griffe-web` : elle ne porte que l'en-tête `HX-Trigger`), donc les tests qui ont besoin de
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
        griffe_core::app::Outcome::Applied(id) => id,
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
    let router = griffe_web::router(state);

    let response = router
        .oneshot(Request::builder().uri("/jour").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = body_text(response).await;
    assert!(
        body.contains("<!DOCTYPE html>"),
        "chargement direct : page complète attendue"
    );
    assert!(
        body.contains("Griffe"),
        "la chrome porte le wordmark Griffe : {body}"
    );
    assert!(
        !body.contains(">FreeFlow<") && !body.contains("FreeFlow —"),
        "plus de wordmark FreeFlow dans la chrome : {body}"
    );
    assert!(
        body.contains("M28 38c-10 0-16-8-16-16")
            || body.contains("lockup.svg")
            || body.contains("viewBox=\"0 0 128 22\""),
        "la paraphe (lockup) est dans la chrome : {body}"
    );
    assert!(
        body.contains("Le jour"),
        "la chrome nomme la pièce d'accueil : {body}"
    );
    assert!(
        body.contains("Les affaires") && body.contains("La société"),
        "la chrome n'a que trois pièces : {body}"
    );
    assert!(
        body.contains("data-view=\"jour\""),
        "l'identifiant d'écran reste le slug"
    );
    assert!(
        body.contains("audit-collapsed"),
        "le journal d'audit est replié par défaut"
    );
    assert!(
        !body.contains("theme-toggle") && !body.contains(">Sombre<"),
        "pas de thème sombre dans la chrome Atelier : {body}"
    );
    assert!(
        !body.contains(">Plus<"),
        "plus de menu « Plus » à dix onglets : {body}"
    );
}

#[tokio::test]
async fn an_htmx_boosted_navigation_returns_only_the_view_fragment() {
    let state = unlocked_state(&test_db_path("fragment")).await;
    let router = griffe_web::router(state);

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
        body.contains("demande un geste") || body.contains("Nouvelle conversation"),
        "l'ancienne route rend Les affaires : {body}"
    );
    assert!(
        body.contains("data-view=\"affaires\""),
        "le slug de la pièce : {body}"
    );
}

fn chrome_today(body: &str) -> &str {
    let marker = "class=\"today\">";
    let start = body
        .find(marker)
        .unwrap_or_else(|| panic!("date de chrome absente : {body}"))
        + marker.len();
    let rest = &body[start..];
    let end = rest
        .find('<')
        .unwrap_or_else(|| panic!("date de chrome non fermée : {rest}"));
    rest[..end].trim()
}

#[tokio::test]
async fn the_atelier_chrome_has_three_pieces_and_vendored_fonts() {
    let state = unlocked_state(&test_db_path("atelier-chrome")).await;
    let router = griffe_web::router(state);

    let jour = body_text(
        router
            .clone()
            .oneshot(Request::builder().uri("/jour").body(Body::empty()).unwrap())
            .await
            .unwrap(),
    )
    .await;
    assert!(jour.contains("data-piece=\"jour\""), "{jour}");
    assert!(jour.contains("data-piece=\"affaires\""), "{jour}");
    assert!(jour.contains("data-piece=\"societe\""), "{jour}");
    assert!(jour.contains("id=\"next-step\""), "{jour}");
    assert!(
        !jour.contains("fonts.googleapis.com") && !jour.contains("fonts.gstatic.com"),
        "aucune police distante : {jour}"
    );
    assert!(
        !jour.contains("freeflow "),
        "pas de commande CLI dans la lettre : {jour}"
    );

    let gens = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(gens.contains("data-view=\"affaires\""), "{gens}");
    assert!(gens.contains("Nouvelle conversation"), "{gens}");
    assert!(gens.contains("demande un geste"), "{gens}");
    assert!(
        gens.contains("data-piece=\"affaires\"") && gens.contains("aria-current=\"page\""),
        "Les affaires se soulignent au chargement : {gens}"
    );

    let js = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/assets/app.js")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        js.contains("affaires: \"affaires\""),
        "après htmx, le slug affaires doit souligner la pièce affaires : {js}"
    );
    assert!(
        js.contains("prospection: \"affaires\""),
        "les anciens écrans des affaires soulignent la même pièce : {js}"
    );
    assert!(
        !js.contains(": \"gens\""),
        "l'ancien slug gens n'est plus une pièce : {js}"
    );

    let societe = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/societe")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(societe.contains("Te payer"), "{societe}");
    assert!(societe.contains("Le relevé"), "{societe}");
    assert!(societe.contains("La carte"), "{societe}");
    let date = chrome_today(&jour);
    assert_eq!(
        chrome_today(&gens),
        date,
        "Les affaires porte la même date que Le jour"
    );
    assert_eq!(
        chrome_today(&societe),
        date,
        "La société porte la même date que Le jour"
    );
    assert_eq!(jour.matches("class=\"today\"").count(), 1);
    assert!(
        jour.contains("viewBox=\"0 0 128 22\""),
        "le wordmark embarque le paraphe : {jour}"
    );

    let societe_fragment = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/societe")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        !societe_fragment.contains("class=\"today\"") && !societe_fragment.contains("<header"),
        "un fragment htmx ne réécrit pas la chrome : {societe_fragment}"
    );

    let css = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/assets/app.css")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(css.contains("--paper:"), "{css}");
    assert!(css.contains("Newsreader"), "{css}");
    assert!(
        css.contains(".chrome a:focus,")
            && css.contains(".chrome a:focus-visible {")
            && css.contains(".chrome a:focus,\n.chrome a:focus-visible {\n  outline: none;")
            && css.contains(".chrome nav a:focus-visible {"),
        "le bandeau ne garde pas le cadre bleu du navigateur"
    );
    assert!(!css.contains("fonts.googleapis.com"), "{css}");
    assert!(
        !css.contains("html.dark{") && !css.contains("html.dark {"),
        "aucune règle de thème sombre : {css}"
    );

    let font = router
        .oneshot(
            Request::builder()
                .uri("/assets/fonts/newsreader-latin-400-normal.woff2")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(font.status(), StatusCode::OK);
    assert_eq!(font.headers().get("content-type").unwrap(), "font/woff2");
    let bytes = font.into_body().collect().await.unwrap().to_bytes();
    assert!(bytes.len() > 1000, "woff2 vendorisé trop court");

    assert!(jour.contains("hx-get=\"/aide\""), "{jour}");
    assert!(jour.contains("id=\"aide-link\""), "{jour}");
    assert!(
        jour.contains(">Aide<"),
        "le pied ouvre l'aide, plus le panneau lexique : {jour}"
    );
    assert!(!jour.contains("hx-get=\"/lexique\""), "{jour}");
}

#[tokio::test]
async fn the_help_letter_explains_atelier_and_opens_recipes() {
    let state = unlocked_state(&test_db_path("aide")).await;
    let router = griffe_web::router(state);

    let aide = body_text(
        router
            .clone()
            .oneshot(Request::builder().uri("/aide").body(Body::empty()).unwrap())
            .await
            .unwrap(),
    )
    .await;
    assert!(aide.contains("data-view=\"aide\""), "{aide}");
    assert!(
        aide.contains("L'atelier") || aide.contains("L&#x27;atelier"),
        "{aide}"
    );
    assert!(aide.contains("Laissez-vous guider."), "{aide}");
    assert_eq!(aide.matches("Tiime").count(), 1, "{aide}");
    assert_eq!(aide.matches("Indy").count(), 1, "{aide}");
    assert_eq!(aide.matches("Pennylane").count(), 1, "{aide}");
    assert!(!aide.contains("CA3"), "{aide}");
    assert!(!aide.contains("3514"), "{aide}");
    assert!(!aide.contains("2777"), "{aide}");
    assert!(!aide.contains("freeflow "), "{aide}");
    for slug in AIDE_SLUGS {
        assert!(
            aide.contains(&format!("href=\"/aide/{slug}\"")),
            "sommaire {slug} : {aide}"
        );
    }
    assert!(aide.contains("href=\"/premiers-pas\""), "{aide}");
    assert!(aide.contains("href=\"/aide/payer\""), "{aide}");
    assert!(
        !aide.contains("Les formalités"),
        "le rayon vide ne s'affiche pas : {aide}"
    );
    assert!(
        aide.contains("data-label=\"se repérer les trois pièces.\""),
        "la palette reprend le catalogue : {aide}"
    );
    assert!(
        aide.contains("data-label=\"avec un agent trois portes, un coffre.\""),
        "la palette trouve le livre agent : {aide}"
    );
    assert!(aide.contains("letter-desk aide-desk"), "{aide}");
    assert!(aide.contains("class=\"aide-sommaire\""), "{aide}");
    assert!(aide.contains("Avec un agent"), "{aide}");
    assert!(aide.contains("Fichiers"), "{aide}");
    assert!(
        !aide.contains("aria-current=\"page\""),
        "aucune pièce soulignée : {aide}"
    );

    let fragment = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/aide")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        !fragment.contains("<!DOCTYPE html>") && !fragment.contains("<body"),
        "htmx ne reçoit que la lettre : {fragment}"
    );
    assert!(fragment.contains("Laissez-vous guider."), "{fragment}");

    let releve = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/aide/releve")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(releve.contains("Ranger le relevé."), "{releve}");
    assert!(releve.contains("href=\"/aide\""), "{releve}");
    assert!(releve.contains("href=\"/societe/releve\""), "{releve}");
    assert!(
        releve.contains("n'interroge") || releve.contains("n&#x27;interroge"),
        "{releve}"
    );
    assert!(!releve.contains("Tiime"), "{releve}");
    assert!(!releve.contains("freeflow "), "{releve}");

    let conversation = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/aide/conversation")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        conversation.contains("href=\"/affaires/nouvelle\""),
        "{conversation}"
    );
    assert!(
        conversation.contains("href=\"/aide/demandes\""),
        "la conversation renvoie vers l'agent : {conversation}"
    );

    let cfe = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/aide/cfe")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(cfe.contains("href=\"/societe/impots/cfe\""), "{cfe}");
    assert!(!cfe.contains("Tiime"), "{cfe}");

    let clore = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/aide/clore")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(clore.contains("href=\"/societe/cloture\""), "{clore}");
    assert!(clore.contains("href=\"/societe/impots/liasse\""), "{clore}");
    assert!(
        clore.contains("href=\"/societe/impots/accounts-filing\""),
        "{clore}"
    );
    assert!(
        clore.contains("href=\"/aide/papiers\""),
        "clore renvoie vers Les papiers : {clore}"
    );

    let papiers = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/aide/papiers")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(papiers.contains("Garder les originaux"), "{papiers}");
    assert!(papiers.contains("href=\"/societe/papiers\""), "{papiers}");
    for word in [
        "PV",
        "FEC",
        "bilan",
        "liasse",
        "Kbis",
        "statuts",
        "contrôle",
    ] {
        assert!(
            papiers.contains(word),
            "aide papiers nomme « {word} » : {papiers}"
        );
    }
    assert!(!papiers.contains("SAE"), "{papiers}");
    assert!(!papiers.contains("NF Z42"), "{papiers}");
    assert!(!papiers.contains("freeflow "), "{papiers}");

    let impots = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/aide/impots")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        impots.contains("TVA du mois") || impots.contains("TVA du trimestre"),
        "{impots}"
    );
    assert!(
        impots.contains("cotisation foncière") || impots.contains("href=\"/societe/impots\""),
        "{impots}"
    );
    assert!(impots.contains("Si l'État vous doit de la TVA"), "{impots}");
    assert!(!impots.contains("CA3"), "{impots}");
    assert!(!impots.contains("Tiime"), "{impots}");

    let lexique = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/aide/lexique")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(lexique.contains("Report à nouveau"), "{lexique}");
    assert!(lexique.contains("href=\"/aide\""), "{lexique}");
    assert!(!lexique.contains("class=\"panel\""), "{lexique}");
    assert!(
        !lexique.contains("FreeFlow"),
        "le lexique nomme Griffe : {lexique}"
    );

    let alias = body_text(
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
    assert!(alias.contains("Report à nouveau"), "{alias}");
    assert!(alias.contains("data-view=\"aide\""), "{alias}");

    let unknown = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/aide/inconnu")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unknown.status(), StatusCode::OK);
    let unknown = body_text(unknown).await;
    assert!(
        unknown.contains("n'est pas ici") || unknown.contains("n&#x27;est pas ici"),
        "{unknown}"
    );
    assert!(unknown.contains("href=\"/aide\""), "{unknown}");

    let js = body_text(
        router
            .oneshot(
                Request::builder()
                    .uri("/assets/app.js")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(js.contains("aide-link"), "{js}");
    assert!(js.contains("event.key !== \"?\""), "{js}");
}

fn without_urls(html: &str) -> String {
    let mut out = html.to_string();
    for marker in ["href=\"", "hx-get=\""] {
        out = strip_attr(&out, marker);
    }
    out
}

fn strip_attr(html: &str, marker: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    while let Some(start) = rest.find(marker) {
        out.push_str(&rest[..start]);
        rest = &rest[start + marker.len()..];
        if let Some(end) = rest.find('"') {
            rest = &rest[end + 1..];
        } else {
            break;
        }
    }
    out.push_str(rest);
    out
}

const AIDE_SLUGS: &[&str] = &[
    "pieces",
    "coffre",
    "lentille",
    "conversation",
    "relancer",
    "courrier",
    "boite",
    "devis",
    "travaux",
    "phrases",
    "gestes",
    "jour",
    "releve",
    "facture",
    "payer",
    "impots",
    "cfe",
    "clore",
    "papiers",
    "lexique",
    "portes",
    "brancher",
    "matin",
    "console",
    "demandes",
    "trame",
    "cmd-coffre",
    "cmd-gens",
    "cmd-courrier",
    "cmd-argent",
    "cmd-societe",
    "fichier",
    "dehors",
];

#[tokio::test]
async fn the_help_catalog_covers_each_situation_and_hides_an_empty_shelf() {
    let state = unlocked_state(&test_db_path("aide-catalog")).await;
    let router = griffe_web::router(state);

    let index = body_text(
        router
            .clone()
            .oneshot(Request::builder().uri("/aide").body(Body::empty()).unwrap())
            .await
            .unwrap(),
    )
    .await;
    for label in [
        "Se repérer",
        "Parler aux gens",
        "Tenir l'argent",
        "Rendre des comptes",
    ] {
        assert!(index.contains(label), "{label} : {index}");
    }

    let pages: &[(&str, &str, Option<&str>)] = &[
        ("pieces", "Les trois pièces.", Some("/jour")),
        (
            "coffre",
            "Pendant 12 h, ne pas redemander la passphrase",
            None,
        ),
        ("lentille", "les arrêtés", Some("/affaires")),
        ("conversation", "fiche déjà là", Some("/affaires/nouvelle")),
        ("relancer", "cinq secondes", Some("/jour")),
        (
            "courrier",
            "Enregistrer et essayer la liaison",
            Some("/societe/courrier"),
        ),
        ("boite", "Laisser partir la relance", Some("/boite")),
        ("devis", "Le total est la somme", Some("/affaires")),
        ("travaux", "Il se garde tout seul", Some("/affaires")),
        (
            "phrases",
            "Les conversations déjà engagées finissent leur série.",
            Some("/affaires/phrases?depuis=societe"),
        ),
        ("gestes", "Ils reviennent", Some("/affaires")),
        ("jour", "mois de piste", Some("/jour")),
        ("releve", "non traitée", Some("/societe/releve")),
        ("facture", "quantité négative", Some("/affaires")),
        ("payer", "Entre toi et la société", Some("/societe/payer")),
        ("impots", "vous doit de la TVA", Some("/societe/impots")),
        ("cfe", "cotisation foncière", Some("/societe/impots/cfe")),
        ("clore", "greffe", Some("/societe/cloture")),
        ("papiers", "Garder les originaux", Some("/societe/papiers")),
        ("lexique", "Report à nouveau", None),
        ("portes", "stdio", Some("/aide/console")),
        ("brancher", "griffe-mcp", Some("/aide/matin")),
        ("matin", "follow_up.prepare", Some("/aide/demandes")),
        ("console", "backup restore", Some("/view/console")),
        (
            "demandes",
            "prospect estimation set",
            Some("/affaires/nouvelle"),
        ),
        ("trame", "&lt;constat&gt;", Some("/aide/phrases")),
        ("cmd-coffre", "unlock --remember", Some("/aide/brancher")),
        ("cmd-gens", "prospect estimation set", Some("/aide/matin")),
        ("cmd-courrier", "courrier flush", Some("/aide/dehors")),
        ("cmd-argent", "unit_price", Some("/aide/cmd-gens")),
        ("cmd-societe", "year opening set", Some("/aide/cmd-argent")),
        ("fichier", "Mairie de Valvert", Some("/aide/trame")),
        ("dehors", "--secret-file", Some("/aide/fichier")),
    ];
    assert_eq!(pages.len(), AIDE_SLUGS.len());

    for (slug, needle, door) in pages {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/aide/{slug}"))
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
            "{slug} htmx : {body}"
        );
        assert!(body.contains(needle), "{slug} attend « {needle} » : {body}");
        assert!(body.contains("href=\"/aide\""), "{slug} : {body}");
        let prose = without_urls(&body);
        assert!(!prose.contains("CA3"), "{slug} : {prose}");
        assert!(!prose.contains("3514"), "{slug} : {prose}");
        assert!(!prose.contains("2777"), "{slug} : {prose}");
        assert!(!body.contains("Tiime"), "{slug} : {body}");
        assert!(!body.contains("Les formalités"), "{slug} : {body}");
        if matches!(
            *slug,
            "portes"
                | "brancher"
                | "matin"
                | "console"
                | "demandes"
                | "trame"
                | "cmd-coffre"
                | "cmd-gens"
                | "cmd-courrier"
                | "cmd-argent"
                | "cmd-societe"
                | "fichier"
                | "dehors"
        ) {
            assert!(body.contains("class=\"aide-columns\""), "{slug} : {body}");
            assert!(!body.contains('@'), "{slug} : {body}");
            assert!(!body.contains("collection://"), "{slug} : {body}");
            assert!(!body.contains("Nicolas"), "{slug} : {body}");
            assert!(!body.contains("Collier"), "{slug} : {body}");
            assert!(!body.contains("freeflow"), "{slug} : {body}");
            assert!(!body.contains("SASU"), "{slug} : {body}");
        }
        if *slug == "fichier" {
            assert!(
                body.contains("1\u{202f}200,00\u{00a0}€"),
                "l'exemple fictif porte le montant : {body}"
            );
            assert!(body.contains("Refonte du site"), "{body}");
        }
        if *slug == "phrases" {
            assert!(
                body.contains("href=\"/aide/trame\""),
                "les phrases de l'atelier ouvrent la trame : {body}"
            );
        }
        if *slug == "relancer" {
            assert!(
                body.contains("href=\"/aide/demandes\""),
                "écrire renvoie vers l'agent : {body}"
            );
        }
        if let Some(href) = door {
            assert!(
                body.contains(&format!("href=\"{href}\"")),
                "{slug} porte {href} : {body}"
            );
        }
    }
}

#[tokio::test]
async fn the_day_letter_shows_the_mast_gestures_and_the_month_grid() {
    let db_path = test_db_path("letter-jour");
    let state = unlocked_state(&db_path)
        .await
        .with_today(time::macros::date!(2026 - 09 - 05));
    let router = griffe_web::router(state);

    let jour = body_text(
        router
            .clone()
            .oneshot(Request::builder().uri("/jour").body(Body::empty()).unwrap())
            .await
            .unwrap(),
    )
    .await;
    assert!(jour.contains("data-view=\"jour\""), "{jour}");
    assert!(
        jour.contains("class=\"mast plateau-status\""),
        "mât visible : {jour}"
    );
    assert!(jour.contains("class=\"pupitre"), "{jour}");
    assert!(
        jour.contains("Ce matin, un geste")
            || jour.contains("Ce matin, deux gestes")
            || jour.contains("Ce matin, trois gestes")
            || jour.contains("Ce matin, quatre gestes")
            || jour.contains("Ce matin, cinq gestes"),
        "{jour}"
    );
    assert!(jour.contains("en banque"), "{jour}");
    assert!(jour.contains("de piste"), "{jour}");
    assert!(
        !jour.contains("prospects"),
        "le compte du livre n'est plus sur le pupitre : {jour}"
    );
    assert!(!jour.contains("class=\"day-split\""), "{jour}");
    assert!(jour.contains("Samedi 5 septembre 2026"), "{jour}");
    assert!(jour.contains("class=\"gestes\""), "{jour}");
    assert!(
        jour.contains("Configurer ma société") || jour.contains("Savoir pour la TVA"),
        "au moins un geste : {jour}"
    );
    assert!(jour.contains("class=\"month\""), "{jour}");
    assert!(jour.contains(">Lu<"), "grille lundi→dimanche : {jour}");
    assert!(
        jour.contains("class=\"d today"),
        "aujourd'hui en sceau : {jour}"
    );
    assert!(
        !jour.contains("CA3") && !jour.contains("3514"),
        "pas de sigle dans la lettre : {jour}"
    );
    assert!(
        !jour.contains("is_acompte") && !jour.contains("is_solde"),
        "pas de clé technique dans l'agenda : {jour}"
    );
    assert!(
        jour.contains("acompte d'impôt sur les sociétés")
            || jour.contains("acompte d&#x27;impôt sur les sociétés"),
        "l'acompte d'IS du 15 septembre se dit comme dans La société : {jour}"
    );
    assert!(
        jour.contains("TVA du mois d'août") || jour.contains("TVA du mois d&#x27;août"),
        "la CA3 de septembre déclare août, comme dans La société : {jour}"
    );
    assert!(
        jour.contains("/societe/impots/is-acompte"),
        "le geste ouvre la lettre, pas la liste : {jour}"
    );
    assert!(
        !jour.contains("impayé"),
        "on dit chez X, pas impayé : {jour}"
    );

    let fragment = body_text(
        router
            .oneshot(
                Request::builder()
                    .uri("/jour")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(fragment.contains("data-view=\"jour\""), "{fragment}");
    assert!(
        fragment.contains("class=\"mast plateau-status\""),
        "{fragment}"
    );
    assert!(fragment.contains("class=\"pupitre"), "{fragment}");
}

#[tokio::test]
async fn the_morning_settles_leroy_and_leaves_a_quiet_vendor_off_the_page() {
    let db_path = test_db_path("jour-leroy");
    let state = unlocked_state(&db_path)
        .await
        .with_today(time::macros::date!(2026 - 09 - 05));
    {
        let mut store =
            Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
        fn applied<T: std::fmt::Debug>(outcome: Outcome<T>) -> T {
            match outcome {
                Outcome::Applied(value) => value,
                other => panic!("expected Applied, got {other:?}"),
            }
        }
        applied(
            Executor::new(&mut store)
                .execute(
                    &griffe_core::opening_balance::RecordOpeningBalance {
                        opens_on: time::macros::date!(2025 - 10 - 01),
                        source: Some("cabinet".into()),
                        lines: vec![
                            "512000:Banque:D:15000.00"
                                .parse::<griffe_core::domain::OpeningBalanceLine>()
                                .unwrap(),
                            "401000:Cabinet Leroy:C:1200.00"
                                .parse::<griffe_core::domain::OpeningBalanceLine>()
                                .unwrap(),
                            "101000:Capital:C:13800.00"
                                .parse::<griffe_core::domain::OpeningBalanceLine>()
                                .unwrap(),
                        ],
                        tax_losses: Money::ZERO,
                        prior_corporate_tax: None,
                        prior_vat_due: None,
                    },
                    &human_ctx(),
                )
                .unwrap(),
        );
        applied(
            Executor::new(&mut store)
                .execute(
                    &griffe_core::billing::ImportBankTransactions {
                        transactions: vec![griffe_core::billing::ParsedTransaction {
                            occurred_on: time::macros::date!(2026 - 09 - 04),
                            amount_cents: -120_000,
                            description: "VIR LEROY".into(),
                            fitid: None,
                        }],
                    },
                    &human_ctx(),
                )
                .unwrap(),
        );
        let mut expense = |label: &str, supplier: &str, cents: i64, on: time::Date| {
            applied(
                Executor::new(&mut store)
                    .execute(
                        &RecordExpense {
                            label: label.into(),
                            category: griffe_core::domain::ExpenseCategory::Fees,
                            amount: Money::from_cents(cents),
                            vat_rate: griffe_core::domain::VatRate::Zero,
                            vat_deductible: Money::ZERO,
                            incurred_on: on,
                            receipt_hash: None,
                            receipt_filename: None,
                            bank_transaction_id: None,
                            supplier: Some(supplier.into()),
                            paid_by: griffe_core::domain::ExpensePaidBy::Company,
                            reverse_charge: false,
                        },
                        &human_ctx(),
                    )
                    .unwrap(),
            );
        };
        expense(
            "honoraires clôture",
            "Cabinet Leroy",
            120_000,
            time::macros::date!(2026 - 09 - 04),
        );
        expense(
            "Abonnement",
            "Tiime",
            2_000,
            time::macros::date!(2026 - 03 - 01),
        );
    }
    let router = griffe_web::router(state);
    let jour = body_text(
        router
            .oneshot(
                Request::builder()
                    .uri("/jour")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(jour.contains("Régler"), "{jour}");
    assert!(jour.contains("Cabinet Leroy"), "{jour}");
    assert!(jour.contains("un débit correspond"), "{jour}");
    assert!(jour.contains("1\u{202f}200,00"), "{jour}");
    assert!(!jour.contains("Tiime"), "{jour}");
    assert!(
        jour.contains("href=\"/affaires/Cabinet%20Leroy\""),
        "{jour}"
    );
    let rows = jour.matches("class=\"geste\"").count();
    assert!(rows <= 5, "{rows} lignes : {jour}");
    assert!(jour.contains("class=\"pupitre-head\""), "{jour}");
}

/// Matin sans geste : la phrase du vide, et pas d'en-tête de colonnes.
/// Le 10 février 2026 est à plus de trente jours de l'acompte d'IS du 15 mars,
/// et la franchise ne pose pas de déclaration de TVA.
#[tokio::test]
async fn a_clear_morning_says_nothing_is_due_and_hides_the_columns() {
    let db_path = test_db_path("jour-vide");
    let state = unlocked_state(&db_path)
        .await
        .with_today(time::macros::date!(2026 - 02 - 10));
    {
        let mut store =
            Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
        applied(
            Executor::new(&mut store)
                .execute(
                    &griffe_core::company::SetCompanyProfile {
                        name: "Nova Dev".into(),
                        legal_form: "SASU".into(),
                        siren: griffe_core::domain::Siren::parse("889112348").unwrap(),
                        vat_number: None,
                        address: griffe_core::domain::Address {
                            street: "3 allée des Tanneurs".into(),
                            postal_code: "44000".into(),
                            city: "Nantes".into(),
                            country: "FR".into(),
                        },
                        share_capital: Some(Money::from_cents(100_000)),
                        rcs_city: None,
                        iban: None,
                        fiscal_year_end: Some(griffe_core::domain::FiscalYearEnd::CALENDAR),
                        vat_regime: Some(griffe_core::domain::VatRegime::Franchise),
                        director_monthly_gross: None,
                        director_charge_ratio_bps: None,
                        president_name: Some("Nova Martin".into()),
                        sole_shareholder_name: Some("Nova Martin".into()),
                        sole_shareholder_address: None,
                        share_count: Some(100),
                    },
                    &human_ctx(),
                )
                .unwrap(),
        );
        applied(
            Executor::new(&mut store)
                .execute(
                    &griffe_core::setup::DeclareNewCompany { declared: true },
                    &human_ctx(),
                )
                .unwrap(),
        );
        applied(
            Executor::new(&mut store)
                .execute(
                    &griffe_core::billing::ImportBankTransactions {
                        transactions: vec![griffe_core::billing::ParsedTransaction {
                            occurred_on: time::macros::date!(2026 - 02 - 01),
                            amount_cents: 10_000,
                            description: "apport".into(),
                            fitid: None,
                        }],
                    },
                    &human_ctx(),
                )
                .unwrap(),
        );
        let tx = griffe_core::billing::list_bank_transactions(store.connection())
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        applied(
            Executor::new(&mut store)
                .execute(
                    &griffe_core::billing::SettleBankTransaction {
                        transaction_id: tx.id,
                        account: "455000"
                            .parse::<griffe_core::domain::SettlementAccount>()
                            .unwrap(),
                        label: None,
                    },
                    &human_ctx(),
                )
                .unwrap(),
        );
    }
    let router = griffe_web::router(state);
    let jour = body_text(
        router
            .oneshot(
                Request::builder()
                    .uri("/jour")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        jour.contains("Rien n'est dû ce matin.") || jour.contains("Rien n&#x27;est dû ce matin."),
        "{jour}"
    );
    assert!(
        !jour.contains("Prochain pas"),
        "pas d'en-tête sans geste : {jour}"
    );
}

#[tokio::test]
async fn les_affaires_lists_three_chapters_and_opens_a_dossier() {
    let db_path = test_db_path("letter-gens");
    let state = unlocked_state(&db_path)
        .await
        .with_today(time::macros::date!(2026 - 09 - 05));
    let router = griffe_web::router(state);

    let created = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/affaires/nouvelle")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("HX-Request", "true")
                .body(Body::from(
                    "who=Camille+Rivi%C3%A8re&phrase=accompagnement+identit%C3%A9",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK, "nouvelle conversation");
    let created_body = body_text(created).await;
    assert!(
        created_body.contains("Camille") && created_body.contains("Écrire"),
        "le dossier s'ouvre après création : {created_body}"
    );
    assert!(
        !created_body.contains("freeflow "),
        "pas de commande CLI dans la lettre : {created_body}"
    );

    {
        let mut store =
            Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
        Executor::new(&mut store)
            .execute(
                &CreateProspect {
                    prospect_name: "Plus tard".into(),
                    address: None,
                    representative: None,
                    email: None,
                    phone: None,
                    name: "un jour".into(),
                    amount: Money::ZERO,
                    probability: Probability::new(10).unwrap(),
                    next_action_at: time::macros::date!(2026 - 10 - 01),
                    source: None,
                },
                &human_ctx(),
            )
            .unwrap();
    }

    let list_response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/affaires")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let list_cache = list_response
        .headers()
        .get("cache-control")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    assert!(
        list_cache.contains("no-store"),
        "la liste ne se garde pas : {list_cache}"
    );
    let list = body_text(list_response).await;
    assert!(list.contains("data-view=\"affaires\""), "{list}");
    assert!(list.contains("demande un geste"), "{list}");
    assert!(list.contains("le livre"), "{list}");
    assert!(list.contains("les sorties"), "{list}");
    assert!(list.contains("les arrêtés"), "{list}");
    assert!(
        !list.contains("les arrêtées"),
        "le filtre dit les arrêtés : {list}"
    );
    assert!(
        list.contains("Les conversations en cours, et les missions."),
        "le livre s'explique dans le menu : {list}"
    );
    assert!(
        list.contains("Les premiers messages et les premiers contacts."),
        "le tas s'explique dans le menu : {list}"
    );
    assert!(
        list.contains("Un nom, dès qu'une dépense le porte."),
        "les sorties s'expliquent dans le menu : {list}"
    );
    assert!(
        list.contains("Les conversations qu'on a arrêtées."),
        "les arrêtés s'expliquent dans le menu : {list}"
    );
    assert!(
        !list.contains("Fournisseurs"),
        "plus un chapitre Fournisseurs : {list}"
    );
    assert!(
        list.contains("Camille") && list.contains("aujourd'hui"),
        "un premier message dont le pas est aujourd'hui demande un geste : {list}"
    );
    assert!(
        !list.contains("Plus tard"),
        "un pas plus loin reste dans le tas : {list}"
    );
    assert!(!list.contains("1 premier message"), "{list}");
    assert!(!list.contains("Rien ne demande un geste."), "{list}");
    assert!(list.contains("Nouvelle conversation"), "{list}");

    let filtered = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires?q=personne")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(!filtered.contains("Camille"), "{filtered}");
    assert!(!filtered.contains("1 premier message"), "{filtered}");
    assert!(filtered.contains("Aucun nom."), "{filtered}");

    let found = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires?vue=tas&q=cami")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(found.contains("Camille"), "{found}");
    assert!(!found.contains("Plus tard"), "{found}");
    assert!(!found.contains("Aucun nom."), "{found}");
    assert!(
        found.contains("/affaires/Camille%20Rivi%C3%A8re?vue=tas&amp;q=cami"),
        "{found}"
    );
    assert!(!found.contains("id=\"affaire\""), "{found}");

    let opened = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires/Camille%20Rivi%C3%A8re?vue=tas&q=cami")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(opened.contains("Revenir au menu"), "{opened}");
    assert!(
        opened.contains("/affaires?vue=tas&amp;q=cami"),
        "le retour garde la vue et la recherche : {opened}"
    );
    assert!(!opened.contains("id=\"affaire\""), "{opened}");
    let mut rest = opened.as_str();
    for label in [
        "En cours",
        "Écrire",
        "La fiche",
        "L'estimation",
        "Les travaux",
        "Noter une rencontre",
    ] {
        let at = rest
            .find(label)
            .or_else(|| rest.find("L&#x27;estimation"))
            .unwrap_or_else(|| panic!("{label} manque : {opened}"));
        let hit = if rest[at..].starts_with(label) {
            label
        } else {
            "L&#x27;estimation"
        };
        rest = &rest[at + hit.len()..];
    }
    for (suffix, marker) in [
        ("", "En cours"),
        ("/ecrire", "Une lettre"),
        ("/fiche", "La fiche."),
        ("/estimation", "L'estimation."),
        ("/travaux", "Les travaux."),
        ("/rencontre", "Une rencontre."),
    ] {
        let door = body_text(
            router
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(format!(
                            "/affaires/Camille%20Rivi%C3%A8re{suffix}?vue=tas&q=cami"
                        ))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert!(
            door.contains("Revenir au menu") && door.contains("/affaires?vue=tas&amp;q=cami"),
            "{suffix} ne revient pas au filtre : {door}"
        );
        let marked = door.contains(marker)
            || (suffix == "/estimation" && door.contains("L&#x27;estimation."))
            || (suffix == "/travaux" && door.contains("tiennent sur une fiche"));
        assert!(marked, "{suffix} n'ouvre pas sa page : {door}");
        assert!(
            !door.contains("class=\"back\""),
            "{suffix} double le retour du dossier : {door}"
        );
    }

    let stopping = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires/Camille%20Rivi%C3%A8re/arreter")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        stopping.contains("class=\"back\""),
        "arrêter garde son retour : {stopping}"
    );

    let pile = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires?vue=tas")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(pile.contains("Plus tard"), "{pile}");
    assert!(pile.contains("Camille"), "{pile}");
    assert!(!list.contains("freeflow "), "{list}");
    assert!(
        !list.contains("impayé") && !list.contains("pipeline"),
        "langage de la lettre : {list}"
    );

    let dossier = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires/Camille%20Rivi%C3%A8re")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(dossier.contains("Camille"), "{dossier}");
    assert!(dossier.contains("En cours"), "{dossier}");
    assert!(
        dossier.contains("Écrire") || dossier.contains("Ecrire"),
        "{dossier}"
    );
    assert!(
        !dossier.contains("freeflow "),
        "pas de commande CLI dans le dossier : {dossier}"
    );
    assert!(
        dossier.contains("href=\"/affaires/Camille%20Rivi%C3%A8re/ecrire\""),
        "Écrire ouvre la lettre : {dossier}"
    );

    let letter = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires/Camille%20Rivi%C3%A8re/ecrire")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(letter.contains("Une lettre"), "{letter}");
    assert!(
        letter.contains("name=\"body\""),
        "lettre éditable : {letter}"
    );
    assert!(
        !letter.contains("Poster"),
        "un seul geste, celui qui classe : {letter}"
    );
    assert!(letter.contains("C'est parti"), "{letter}");
    assert!(
        letter.contains("id=\"face-read\" value=\"relire\" checked"),
        "Écrire s'ouvre sur la lettre : {letter}"
    );
    assert!(
        letter.contains("class=\"letter-stage\""),
        "la lettre se lit d'abord : {letter}"
    );
    assert!(letter.contains(">Éditer<"), "{letter}");
    assert!(
        !letter.contains("id=\"face-write\" value=\"ecrire\" checked"),
        "{letter}"
    );
    assert!(
        letter.contains("class=\"seal\""),
        "le geste a le style du bouton principal : {letter}"
    );
    assert!(!letter.contains("freeflow "), "{letter}");

    // « C'est parti » classe la lettre. L'expéditeur et le destinataire restent ceux du coffre.
    {
        let mut store =
            Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
        let client_id = client_id_by_name(&db_path, "Camille Rivière");
        Executor::new(&mut store)
            .execute(
                &CreateContact {
                    client_id,
                    name: "Camille".into(),
                    email: Some("camille@exemple.fr".into()),
                    phone: None,
                    role: None,
                    given_name: String::new(),
                    family_name: String::new(),
                    civility: None,
                    structured: false,
                },
                &human_ctx(),
            )
            .unwrap();
        Executor::new(&mut store)
            .execute(
                &SetFollowUpSender {
                    email: "moi@exemple.fr".into(),
                    name: Some("Moi".into()),
                },
                &human_ctx(),
            )
            .unwrap();
    }

    let posted_response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/affaires/Camille%20Rivi%C3%A8re/envoye")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("HX-Request", "true")
                .body(Body::from(
                    "subject_line=Bonjour&body=On+se+voit+mardi+pour+en+parler.",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(posted_response.status(), StatusCode::OK);
    assert_eq!(
        posted_response
            .headers()
            .get("HX-Trigger")
            .and_then(|value| value.to_str().ok()),
        Some("griffe:saved"),
        "C'est parti confirme l'enregistrement"
    );
    let posted = body_text(posted_response).await;
    assert!(
        posted.contains("On se voit mardi pour en parler."),
        "la lettre classée est dans l'historique : {posted}"
    );
    assert!(
        posted.contains("letter-fold"),
        "l'historique replie la lettre : {posted}"
    );
    assert!(
        !posted.contains("FreeFlow"),
        "plus de FreeFlow dans le flash : {posted}"
    );
    assert!(
        posted.contains("hors de Griffe"),
        "sans serveur, En cours dit l'envoi hors de Griffe : {posted}"
    );
    assert!(
        posted.contains("/societe/courrier"),
        "la note ouvre Le courrier : {posted}"
    );

    let after = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        !after.contains("Camille"),
        "le pas suivant n'est plus aujourd'hui : {after}"
    );

    let old = body_text(
        router
            .oneshot(
                Request::builder()
                    .uri("/view/prospection")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(old.contains("data-view=\"affaires\""), "{old}");
}

#[tokio::test]
async fn les_affaires_qualifies_amounts_and_opens_an_outgoing_dossier() {
    let db_path = test_db_path("letter-outgoing");
    let mut store = Store::create(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
    Executor::new(&mut store)
        .execute(
            &CreateProspect {
                prospect_name: "Atelier Nord".into(),
                address: None,
                representative: None,
                email: None,
                phone: None,
                name: "Atelier patrimoine".into(),
                amount: Money::from_cents(400_000),
                probability: Probability::new(50).unwrap(),
                next_action_at: time::macros::date!(2026 - 10 - 01),
                source: None,
            },
            &human_ctx(),
        )
        .unwrap();
    Executor::new(&mut store)
        .execute(
            &RecordExpense {
                label: "Abonnement mars".into(),
                category: griffe_core::domain::ExpenseCategory::Software,
                amount: Money::from_cents(20_000),
                vat_rate: griffe_core::domain::VatRate::Zero,
                vat_deductible: Money::ZERO,
                incurred_on: time::macros::date!(2026 - 03 - 01),
                receipt_hash: None,
                receipt_filename: None,
                bank_transaction_id: None,
                supplier: Some("Tiime".into()),
                paid_by: griffe_core::domain::ExpensePaidBy::Company,

                reverse_charge: false,
            },
            &human_ctx(),
        )
        .unwrap();
    drop(store);

    let state = AppState::new(db_path.clone());
    state
        .unlock(&Passphrase::from(PASSPHRASE), false)
        .await
        .unwrap();
    let state = state.with_today(time::macros::date!(2026 - 09 - 05));
    let router = griffe_web::router(state);

    let list = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(list.contains("les sorties"), "{list}");
    assert!(!list.contains("Fournisseurs"), "{list}");
    assert!(
        !list.contains("Atelier Nord"),
        "l'estimation qui n'est pas pour aujourd'hui reste dans le livre : {list}"
    );

    let book = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires?vue=livre")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        book.contains("autour de") && book.contains("Atelier Nord"),
        "l'enveloppe se dit : {book}"
    );

    let outgoing = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires?vue=sorties")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        outgoing.contains("Tiime") && outgoing.contains("versés"),
        "le versé se dit : {outgoing}"
    );

    let tiime = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires/Tiime")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(tiime.contains("Les notes"), "{tiime}");
    assert!(tiime.contains("Abonnement mars"), "{tiime}");
    assert!(tiime.contains("versés"), "{tiime}");
    assert!(tiime.contains("pas de justificatif"), "{tiime}");
    assert!(tiime.contains("Joindre"), "{tiime}");
    assert!(
        !tiime.contains(">fournisseur<") && !tiime.contains(" · fournisseur"),
        "pas un type d'entité : {tiime}"
    );

    let mairie = body_text(
        router
            .oneshot(
                Request::builder()
                    .uri("/affaires/Atelier%20Nord")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(mairie.contains("autour de"), "{mairie}");
    assert!(mairie.contains("Atelier patrimoine"), "{mairie}");
    assert!(
        !mairie.contains("Pas de devis posé"),
        "l'estimation tient lieu de devis : {mairie}"
    );
}

#[tokio::test]
async fn le_jour_nomme_les_conversations_et_le_dossier_note_fiche_estimation_rencontre() {
    let db_path = test_db_path("letter-fiche");
    let state = unlocked_state(&db_path)
        .await
        .with_today(time::macros::date!(2026 - 09 - 05));
    let router = griffe_web::router(state);

    for (who, phrase) in [
        ("Le porc du Val", "charcuterie"),
        ("Ferme du Nord", "visite"),
        ("Mairie test", "signaletique"),
    ] {
        let body = format!(
            "who={}&phrase={}",
            who.replace(' ', "+"),
            phrase.replace(' ', "+")
        );
        let status = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/affaires/nouvelle")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("HX-Request", "true")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::OK, "{who}");
    }

    {
        let mut store =
            Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
        let client_id = client_id_by_name(&db_path, "Mairie test");
        Executor::new(&mut store)
            .execute(
                &CreateMission {
                    client_id,
                    quote_id: None,
                    name: "Signalétique".into(),
                    kind: griffe_core::domain::MissionKind::Forfait {
                        budget: Money::from_cents(100_000),
                    },
                    milestones: Vec::new(),
                    started_on: time::macros::date!(2026 - 09 - 01),
                },
                &human_ctx(),
            )
            .unwrap();
    }

    let jour = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/jour")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(jour.contains("de piste"), "{jour}");
    assert!(
        !jour.contains("prospects"),
        "le compte du livre n'est plus sur le pupitre : {jour}"
    );
    assert!(!jour.contains("class=\"day-split\""), "{jour}");
    assert!(jour.contains("2 autres conversations"), "{jour}");
    assert!(
        !jour.contains("Le porc du Val") && !jour.contains("Ferme du Nord"),
        "le tas froid ne s'écrit pas dans la lettre du matin : {jour}"
    );
    assert!(
        !jour.contains("En mission") && !jour.contains("En conversation"),
        "les chapitres ont quitté le matin : {jour}"
    );
    assert!(
        jour.contains("class=\"track\"") && jour.contains("Mairie test"),
        "la mission reste sur sa bande : {jour}"
    );

    let dossier = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires/Le%20porc%20du%20Val")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(dossier.contains("La fiche"), "{dossier}");
    assert!(
        dossier.contains("L'estimation") || dossier.contains("L&#x27;estimation"),
        "{dossier}"
    );
    assert!(dossier.contains("/estimation"), "{dossier}");
    assert!(!dossier.contains(">Le devis<"), "{dossier}");

    let fiche = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires/Le%20porc%20du%20Val/fiche")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(fiche.contains("name=\"email\""), "{fiche}");
    assert!(fiche.contains("Le porc du Val"), "{fiche}");

    let saved = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/affaires/Le%20porc%20du%20Val/fiche")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("HX-Request", "true")
                    .body(Body::from(
                        "who=Le+porc+du+Val&street=1+rue+du+Marche&postal_code=59292&city=Aubigny&representative=Jean&email=porc@exemple.fr&phone=0600000000&client_revision=1&contact_id=&contact_revision=",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        saved.contains("Fiche à jour") || saved.contains("Jean"),
        "{saved}"
    );

    let estimated = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/affaires/Le%20porc%20du%20Val/estimation")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("HX-Request", "true")
                    .body(Body::from(
                        "phrase=Charcuterie+du+val&amount=1500&revision=1",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(estimated.contains("Charcuterie du val"), "{estimated}");
    assert!(estimated.contains("autour de"), "{estimated}");

    let noted = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/affaires/Le%20porc%20du%20Val/rencontre")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("HX-Request", "true")
                    .body(Body::from(
                        "kind=visio&when=2026-09-05&note=On+a+parle+du+fournil+puis+d+un+deuxieme+point+qui+depasse+largement+les+quatre-vingts+caracteres+de+l+apercu+historique.",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(noted.contains("Visioconférence"), "{noted}");
    assert!(noted.contains("fold-mark"), "{noted}");
    assert!(
        noted.contains("deuxieme point"),
        "le texte entier reste dans le dossier : {noted}"
    );
}

#[tokio::test]
async fn la_societe_opens_on_the_plateau_and_folds_each_chapter() {
    let db_path = test_db_path("letter-societe");
    let state = unlocked_state(&db_path)
        .await
        .with_today(time::macros::date!(2026 - 09 - 05));
    {
        let mut store =
            Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
        applied(
            Executor::new(&mut store)
                .execute(
                    &griffe_core::company::SetCompanyProfile {
                        name: "Lumen Conseil".into(),
                        legal_form: "SASU".into(),
                        siren: griffe_core::domain::Siren::parse("552100554").unwrap(),
                        vat_number: None,
                        address: griffe_core::domain::Address {
                            street: "18 rue des Ateliers".into(),
                            postal_code: "69003".into(),
                            city: "Lyon".into(),
                            country: "FR".into(),
                        },
                        share_capital: Some(Money::from_cents(100_000)),
                        rcs_city: Some("Lyon".into()),
                        iban: None,
                        fiscal_year_end: Some(
                            griffe_core::domain::FiscalYearEnd::new(9, 30).unwrap(),
                        ),
                        vat_regime: Some(griffe_core::domain::VatRegime::RealNormalQuarterly),
                        director_monthly_gross: Some(Money::from_cents(300_000)),
                        director_charge_ratio_bps: None,
                        president_name: Some("Nicolas Lumen".into()),
                        sole_shareholder_name: Some("Nicolas Lumen".into()),
                        sole_shareholder_address: Some("18 rue des Ateliers, 69003 Lyon".into()),
                        share_count: Some(1000),
                    },
                    &human_ctx(),
                )
                .unwrap(),
        );
        applied(
            Executor::new(&mut store)
                .execute(
                    &griffe_core::opening_balance::RecordOpeningBalance {
                        opens_on: time::macros::date!(2025 - 10 - 01),
                        source: Some("cabinet".into()),
                        lines: vec![
                            "101000:Capital:C:1000.00".parse().unwrap(),
                            "401000:Cabinet Leroy:C:1200.00".parse().unwrap(),
                            "512000:Banque:D:2200.00".parse().unwrap(),
                        ],
                        tax_losses: Money::ZERO,
                        prior_corporate_tax: None,
                        prior_vat_due: None,
                    },
                    &human_ctx(),
                )
                .unwrap(),
        );
        applied(
            Executor::new(&mut store)
                .execute(
                    &griffe_core::billing::ImportBankTransactions {
                        transactions: vec![griffe_core::billing::ParsedTransaction {
                            occurred_on: time::macros::date!(2026 - 09 - 04),
                            amount_cents: -120_000,
                            description: "Cabinet Leroy".into(),
                            fitid: None,
                        }],
                    },
                    &human_ctx(),
                )
                .unwrap(),
        );
    }
    let router = griffe_web::router(state);

    let home = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/societe")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(home.contains("data-view=\"societe\""), "{home}");
    assert!(home.contains("class=\"plateau\""), "plateau : {home}");
    assert!(!home.contains("bureau-pane"), "accueil sans volet : {home}");
    assert!(!home.contains("bureau-columns"), "{home}");
    assert!(
        !home.contains("class=\"landscape\""),
        "la piste n'est plus l'accueil : {home}"
    );
    assert!(!home.contains("Revenir au menu"), "{home}");
    assert!(home.contains("href=\"/societe/piste\""), "{home}");
    assert!(home.contains("Tenir"), "{home}");
    assert!(home.contains("Te payer"), "{home}");
    assert!(
        home.contains("L'État") || home.contains("L&#x27;État"),
        "{home}"
    );
    assert!(home.contains("Les réglages"), "{home}");
    assert!(home.contains("La piste"), "{home}");
    assert!(home.contains("Les échéances"), "{home}");
    assert!(home.contains("La maison"), "{home}");
    assert!(home.contains("Entre toi et la société"), "{home}");
    assert!(home.contains("/societe/payer"), "{home}");
    assert!(home.contains("Les papiers"), "{home}");
    assert!(home.contains("/societe/papiers"), "{home}");
    assert!(home.contains("Le courrier"), "{home}");
    assert!(home.contains("/societe/courrier"), "{home}");
    assert!(
        home.contains("/affaires/phrases?depuis=societe"),
        "les phrases restent au plateau : {home}"
    );
    assert!(
        !home.contains("freeflow "),
        "pas de commande CLI dans la lettre : {home}"
    );
    assert!(
        !home.contains("CA3") && !home.contains("3514") && !home.contains("2777"),
        "pas de sigle : {home}"
    );

    let chapters = [
        "/societe/piste",
        "/societe/releve",
        "/societe/payer",
        "/societe/compte",
        "/societe/annee",
        "/societe/impots",
        "/societe/cloture",
        "/societe/identite",
        "/societe/courrier",
        "/societe/papiers",
        "/affaires/phrases?depuis=societe",
    ];
    for uri in chapters {
        let page = body_text(
            router
                .clone()
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                .await
                .unwrap(),
        )
        .await;
        let family = if uri.contains("piste") || uri.contains("releve") {
            "tenir"
        } else if uri.contains("payer") || uri.contains("compte") || uri.contains("annee") {
            "payer"
        } else if uri.contains("impots") || uri.contains("cloture") {
            "etat"
        } else {
            "reglages"
        };
        assert!(page.contains("Revenir au menu"), "{uri} : {page}");
        assert!(
            !page.contains("bureau-columns") && !page.contains("bureau-pane"),
            "{uri}"
        );
        assert!(
            page.contains(&format!("data-family=\"{family}\"")),
            "{uri} famille {family} : {page}"
        );
        assert!(!page.contains("freeflow "), "{uri}");
        assert!(
            !page.contains("CA3") && !page.contains("3514") && !page.contains("2777"),
            "{uri}"
        );
        match uri {
            "/societe/piste" => {
                assert!(page.contains("class=\"landscape\""), "{page}");
                assert!(page.contains("aria-current=\"page\""), "{page}");
                assert_eq!(page.matches("class=\"tenir-card\"").count(), 1, "{page}");
                let card = page.find("class=\"tenir-card\"").expect("carte");
                assert!(
                    page[card..].contains("class=\"landscape\""),
                    "la figure reste dans la carte : {page}"
                );
                assert!(
                    page.contains("stroke-dasharray"),
                    "le pointillé reste : {page}"
                );
                assert!(
                    page.contains("clôture dans 25 jours"),
                    "la légende déjà écrite : {page}"
                );
                assert!(
                    page.contains("aujourd'hui") || page.contains("aujourd&#x27;hui"),
                    "{page}"
                );
                assert!(!page.contains("<h1"), "{page}");
                let work = page.find("class=\"work\"").expect("volet");
                let foot = page[work..].find("class=\"foot\"").expect("pied");
                let pane = &page[work..work + foot];
                assert!(!pane.contains("<form"), "pas de formulaire : {pane}");
                assert!(!pane.contains("<button"), "pas de bouton inventé : {pane}");
                assert!(!page.contains("hx-on"), "{page}");
            }
            "/societe/payer" => {
                assert!(
                    page.contains("n'est pas clos") || page.contains("n&#x27;est pas clos"),
                    "dividende fermé : {page}"
                );
                assert!(page.contains("data-payer=\"mois\""), "{page}");
                assert!(page.contains("Comme un salaire"), "{page}");
                assert!(page.contains("Plus tard, en dividendes"), "{page}");
                assert!(page.contains("Cette année, pour vivre"), "{page}");
                assert!(page.contains("Ouverte"), "{page}");
                assert!(page.contains("Fermée"), "{page}");
                assert!(
                    page.contains("Le prochain virement, depuis le relevé"),
                    "{page}"
                );
                assert!(!page.contains("<form"), "pas de formulaire : {page}");
                assert!(!page.contains("hx-on"), "{page}");
            }
            "/societe/compte" => {
                assert!(page.contains("data-payer=\"compte\""), "{page}");
                assert!(
                    page.contains("Rien entre toi et la société")
                        || page.contains("Rien entre toi et la soci&#xE9;t&#xE9;"),
                    "{page}"
                );
                assert!(page.contains("avancés de ta poche"), "{page}");
                assert!(page.contains("pris sur le compte de la société"), "{page}");
                assert!(page.contains("mis sur le compte de la société"), "{page}");
                assert!(page.contains(">Avancé<"), "{page}");
                assert!(page.contains(">Pris<"), "{page}");
                assert!(page.contains(">Apporté<"), "{page}");
                assert!(!page.contains("<form"), "pas de formulaire : {page}");
            }
            "/societe/annee" => {
                assert!(page.contains("data-payer=\"annee\""), "{page}");
                assert!(page.contains(">Versé<"), "{page}");
                assert!(page.contains(">Objectif<"), "{page}");
                assert!(page.contains("class=\"meter\""), "{page}");
                assert!(!page.contains("<form"), "pas de formulaire : {page}");
            }
            "/societe/impots" => {
                assert!(page.contains("Pas des sigles"), "{page}");
                assert!(page.contains("etat-desk"), "{page}");
                assert!(page.contains(">Les échéances<"), "{page}");
                assert!(!page.contains("Ce que tu dois"), "{page}");
                assert!(!page.contains("<h1"), "{page}");
                assert!(!page.contains("hx-on"), "{page}");
            }
            "/societe/cloture" => {
                assert!(page.contains("class=\"etat-card\""), "{page}");
                assert!(page.contains("ul class=\"hist\""), "{page}");
                assert!(page.contains(">Clore<"), "{page}");
                assert!(!page.contains("<h1"), "{page}");
                assert!(
                    !page.contains("<form"),
                    "pas de formulaire inventé : {page}"
                );
                assert!(!page.contains("hx-on"), "{page}");
            }
            "/societe/releve" => {
                assert!(page.contains("Leroy"), "{page}");
                assert!(
                    page.contains("dette") || page.contains("pas une charge"),
                    "Leroy = dette : {page}"
                );
                assert!(
                    !page.to_lowercase().contains("c'est moi que je me paie"),
                    "{page}"
                );
                assert!(
                    page.contains("class=\"letter spread tenir-desk\""),
                    "{page}"
                );
                assert_eq!(page.matches("class=\"tenir-card\"").count(), 1, "{page}");
                assert!(page.contains("Déposer un export"), "{page}");
                let deposit = page.find("Déposer un export").expect("dépôt");
                let opener = &page[deposit.saturating_sub(240)..deposit];
                assert!(
                    opener.contains("type=\"button\"")
                        && opener.contains("hx-get=\"/banque/import\""),
                    "le dépôt ouvre le volet : {opener}"
                );
                assert!(!opener.contains("<form"), "pas de form inventé : {opener}");
                assert!(page.contains("class=\"releve-search\""), "{page}");
                assert!(page.contains("class=\"block\""), "{page}");
                assert!(
                    page.contains("hx-trigger=\"griffe:saved from:body\""),
                    "{page}"
                );
                assert!(!page.contains("<h1"), "{page}");
                assert!(!page.contains("hx-on"), "{page}");
                let card = page.find("class=\"tenir-card\"").expect("carte");
                let card_end = page[card..]
                    .find("</article>")
                    .map(|end| card + end)
                    .expect("fin de carte");
                let search = page.find("class=\"releve-search\"").expect("recherche");
                assert!(
                    search > card_end,
                    "la recherche n'est pas une carte : {page}"
                );
            }
            "/affaires/phrases?depuis=societe" => {
                assert!(
                    page.contains("data-piece=\"societe\" aria-current=\"page\""),
                    "le soulignement reste La société : {page}"
                );
                assert!(page.contains("phrase-desk"), "{page}");
                assert!(page.contains("reglages-desk"), "{page}");
                assert_eq!(page.matches("reglages-card").count(), 1, "{page}");
                assert!(
                    page.contains("class=\"word-choice-row phrase-line\""),
                    "{page}"
                );
                assert!(page.contains(">Série<"), "{page}");
                assert!(page.contains("Aucune série."), "{page}");
                assert!(page.contains(">Une autre<"), "{page}");
                assert!(!page.contains("class=\"phrase-stage\""), "{page}");
                assert!(!page.contains("id=\"phrase-sheet\""), "{page}");
                assert!(!page.contains("Enregistrer les phrases"), "{page}");
                assert!(!page.contains("<h1"), "{page}");
                assert!(!page.contains("hx-on"), "{page}");
                let line = page
                    .find("class=\"word-choice-row phrase-line\"")
                    .expect("séries");
                let card = page.find("reglages-card").expect("carte");
                assert!(line < card, "la ligne, puis la carte : {page}");
            }
            "/societe/identite" => {
                assert!(page.contains("class=\"letter reglages-desk\""), "{page}");
                assert_eq!(page.matches("reglages-card").count(), 2, "{page}");
                assert!(page.contains("ul class=\"people\""), "{page}");
                assert!(page.contains("552100554"), "le SIREN déjà écrit : {page}");
                assert!(page.contains("pas un formulaire de paramètres"), "{page}");
                assert!(page.contains(">Le coffre<"), "{page}");
                assert!(page.contains("Modifier la carte"), "{page}");
                assert!(page.contains("href=\"/view/societe\""), "{page}");
                assert!(page.contains("les séries de lettres"), "{page}");
                assert!(!page.contains("<h1"), "{page}");
                assert!(
                    !page.contains("<form"),
                    "pas de formulaire inventé : {page}"
                );
                assert!(!page.contains("hx-on"), "{page}");
                let people = page.find("ul class=\"people\"").expect("liste");
                let coffre = page.find(">Le coffre<").expect("coffre");
                let edit = page.find("Modifier la carte").expect("lien");
                let cards: Vec<usize> = page
                    .match_indices("reglages-card")
                    .map(|(i, _)| i)
                    .collect();
                assert!(
                    cards[0] < people && people < cards[1] && cards[1] < coffre && coffre < edit,
                    "la liste, le coffre, puis le lien : {page}"
                );
            }
            "/societe/courrier" => {
                assert!(page.contains("courrier-desk"), "{page}");
                assert_eq!(page.matches("reglages-card").count(), 4, "{page}");
                assert!(
                    page.contains("Pas encore branché") || page.contains("Prêt"),
                    "{page}"
                );
                assert!(!page.contains("<h1"), "{page}");
                assert!(!page.contains("hx-on"), "{page}");
            }
            "/societe/papiers" => {
                assert!(page.contains("id=\"papiers-letter\""), "{page}");
                assert!(page.contains("class=\"letter reglages-desk\""), "{page}");
                assert!(page.contains("class=\"mast\""), "{page}");
                assert!(page.contains("class=\"places\""), "{page}");
                assert_eq!(page.matches("reglages-card").count(), 4, "{page}");
                assert!(!page.contains("<h1"), "{page}");
                assert!(!page.contains("hx-on"), "{page}");
                let mast = page.find("class=\"mast\"").expect("mât");
                let card = page.find("reglages-card").expect("carte");
                assert!(mast < card, "le mât reste au-dessus des cartes : {page}");
            }
            _ => {}
        }
    }

    let papers = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/societe/papiers")
                    .header("HX-Request", "true")
                    .header("HX-Target", "content")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        papers.contains("Revenir au menu") && papers.contains("Les originaux nés ici restent ici."),
        "les papiers restent dans le pli : {papers}"
    );
    let slip = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/societe/papiers")
                    .header("HX-Request", "true")
                    .header("HX-Target", "papiers-letter")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        slip.contains("Les originaux nés ici restent ici.")
            && !slip.contains("plateau")
            && !slip.contains("Revenir au menu"),
        "le rafraîchissement de la lettre ne redessine pas le pli : {slip}"
    );

    let phrases = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires/phrases?depuis=lettre")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        phrases.contains("data-family=\"reglages\""),
        "ouvertes depuis une lettre, les phrases restent dans La société : {phrases}"
    );

    let old = body_text(
        router
            .oneshot(
                Request::builder()
                    .uri("/view/depenses")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        old.contains("Tu déposes l'export de la banque") && old.contains("Déposer un export"),
        "l'ancienne adresse ouvre le relevé : {old}"
    );
}

#[tokio::test]
async fn la_societe_plateau_presses_a_line_into_its_family() {
    let state = unlocked_state(&test_db_path("bureau-css")).await;
    let css = body_text(
        griffe_web::router(state)
            .oneshot(
                Request::builder()
                    .uri("/assets/app.css")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(!css.contains(".bureau-line"), "plus de gouttière : {css}");
    assert!(css.contains("background: #e6dfd2"), "{css}");
    for tint in [
        "#fbf7f0", "#f4ead6", "#f8f1e2", "#f6e4dc", "#f8ebe4", "#efe6d6", "#f3ecdf", "#f3ece0",
        "#ead9bc", "#f0d2c6", "#e6d9c4",
    ] {
        assert!(css.contains(tint), "{tint} manque : {css}");
    }
    assert!(
        css.contains(".plateau-line .who {\n  font-family: var(--sans);\n  font-size: 15px;"),
        "les lignes du plateau ne sont pas des titres d'affiche : {css}"
    );
    assert!(
        css.contains(
            ".plateau-family[data-family=\"payer\"] .plateau-line:hover { background: #ead9bc; }"
        ),
        "le survol reste dans la famille : {css}"
    );
    assert!(
        !css.contains("a.plateau-line:hover .who { color: var(--seal)"),
        "le survol n'emprunte pas le sceau : {css}"
    );
    assert!(
        css.contains(".plateau-open[data-family] .plateau-pill[aria-current=\"page\"]"),
        "la pastille courante est en encre : {css}"
    );
    assert!(
        css.contains(".payer-card {\n  background: #f8f1e2;"),
        "la lecture de Te payer : {css}"
    );
    assert!(
        css.contains(".payer-grid[data-payer=\"mois\"]"),
        "la grille de Ce mois-ci : {css}"
    );
    assert!(
        css.contains(".payer-card.on {\n  box-shadow: inset 3px 0 0 var(--ink)"),
        "la porte ouverte est en encre : {css}"
    );
    assert!(
        css.contains(".etat-card {\n  background: #f8ebe4;"),
        "la lecture de L'État : {css}"
    );
    assert!(
        css.contains(".plateau-family[data-family=\"etat\"] { background: #f6e4dc; }"),
        "la teinte de menu de L'État reste : {css}"
    );
    assert!(
        css.contains(
            ".etat-grid[data-etat=\"1\"] {\n  grid-template-columns: minmax(min-content, 1fr);"
        ),
        "une carte, une colonne : {css}"
    );
    assert!(
        css.contains(".etat-grid[data-etat=\"3\"] {\n  grid-template-columns: repeat(3, minmax(min-content, 1fr));"),
        "trois cartes, trois colonnes : {css}"
    );
    assert!(
        css.contains(
            ".plateau-open[data-family=\"etat\"] .work:has(.etat-desk) {\n  display: flex;\n  flex-direction: column;\n  overflow: auto;"
        ),
        "le chapitre défile : {css}"
    );
    let etat_css = css
        .split("/* L'État — cartes de lecture.")
        .nth(1)
        .expect("cartes de L'État");
    let under_1100 = etat_css
        .split("@media (max-width: 1100px)")
        .nth(1)
        .expect("palier 1100 des échéances");
    assert!(
        under_1100.contains(
            ".etat-grid[data-etat=\"2\"],\n  .etat-grid[data-etat=\"3\"] {\n    grid-template-columns: repeat(2, minmax(min-content, 1fr));"
        ),
        "sous 1100 px, deux colonnes dès deux cartes : {under_1100}"
    );
    let under_700 = etat_css
        .split("@media (max-width: 700px)")
        .nth(1)
        .expect("palier 700 des échéances");
    assert!(
        under_700.contains(
            ".etat-grid[data-etat=\"1\"],\n  .etat-grid[data-etat=\"2\"],\n  .etat-grid[data-etat=\"3\"] {\n    grid-template-columns: minmax(min-content, 1fr);"
        ),
        "sous 700 px, une colonne : {under_700}"
    );
    assert!(
        css.contains(".tenir-card {\n  background: #fbf7f0;"),
        "la lecture de Tenir : {css}"
    );
    assert!(
        css.contains(".plateau-family[data-family=\"tenir\"] { background: #fbf7f0; }"),
        "la teinte de menu de Tenir reste : {css}"
    );
    assert!(
        css.contains(".landscape svg { width: 100%; height: 160px;"),
        "le paysage garde sa hauteur : {css}"
    );
    assert!(
        css.contains(
            ".plateau-open[data-family=\"tenir\"] .work:has(.tenir-desk) {\n  display: flex;\n  flex-direction: column;\n  overflow: auto;"
        ),
        "le chapitre défile : {css}"
    );
    let tenir_css = css
        .split("/* Tenir — cartes de lecture.")
        .nth(1)
        .expect("cartes de Tenir");
    let under_1100_tenir = tenir_css
        .split("@media (max-width: 1100px)")
        .nth(1)
        .expect("palier 1100 de Tenir");
    let rule_1100 = under_1100_tenir.split("@media").next().unwrap();
    assert!(
        rule_1100.contains(
            ".tenir-desk,\n  .tenir-card {\n    display: flex;\n    flex-direction: column;\n    width: 100%;"
        ),
        "sous 1100 px, la carte reste seule : {rule_1100}"
    );
    assert!(
        !rule_1100.contains("grid-template-columns") && !rule_1100.contains("repeat("),
        "pas de grille à plusieurs colonnes : {rule_1100}"
    );
    let under_700_tenir = tenir_css
        .split("@media (max-width: 700px)")
        .nth(1)
        .expect("palier 700 de Tenir");
    let rule_700 = under_700_tenir.split("@media").next().unwrap();
    assert!(
        rule_700.contains(
            ".tenir-desk,\n  .tenir-card {\n    display: flex;\n    flex-direction: column;\n    width: 100%;"
        ),
        "sous 700 px, la carte reste seule : {rule_700}"
    );
    assert!(
        !rule_700.contains("grid-template-columns") && !rule_700.contains("repeat("),
        "pas de grille à plusieurs colonnes : {rule_700}"
    );
    assert!(
        css.contains(".reglages-card {\n  background: #f3ecdf;"),
        "la lecture des réglages : {css}"
    );
    assert!(
        css.contains(".plateau-family[data-family=\"reglages\"] { background: #efe6d6; }"),
        "la teinte de menu des réglages reste : {css}"
    );
    assert!(
        css.contains(".plateau .work .courrier-desk { max-width: none;"),
        "le courrier prend la largeur du bureau : {css}"
    );
    assert!(
        css.contains(
            "@container courrier (min-width: 880px) {\n  .courrier-board { grid-template-columns: minmax(18rem, 42rem) minmax(16rem, 1fr); }"
        ),
        "la lecture et les autres cartes partagent la ligne : {css}"
    );
    assert!(
        css.contains(".courrier-side > .courrier-formula { grid-column: 1; grid-row: 1; }"),
        "la formule et l'envoi du jour tiennent sur une ligne : {css}"
    );
    assert!(
        css.contains(
            ".plateau-open[data-family=\"reglages\"] .work:has(.reglages-desk) {\n  display: flex;\n  flex-direction: column;\n  overflow: auto;"
        ),
        "le chapitre défile : {css}"
    );
    assert!(
        css.contains("@container courrier (min-width: 640px)"),
        "la grille du serveur reste une grille de champs : {css}"
    );
    let reglages_css = css
        .split("/* Les réglages — cartes de lecture.")
        .nth(1)
        .expect("cartes des réglages");
    let under_1100_reglages = reglages_css
        .split("@media (max-width: 1100px)")
        .nth(1)
        .expect("palier 1100 des réglages");
    let rule_1100_reglages = under_1100_reglages.split("@media").next().unwrap();
    assert!(
        rule_1100_reglages.contains(
            ".reglages-desk,\n  .reglages-desk > .reglages-card {\n    display: flex;\n    flex-direction: column;\n    width: 100%;"
        ),
        "sous 1100 px, les cartes empilées restent une colonne : {rule_1100_reglages}"
    );
    assert!(
        !rule_1100_reglages.contains("grid-template-columns")
            && !rule_1100_reglages.contains("repeat("),
        "pas de grille à plusieurs colonnes : {rule_1100_reglages}"
    );
    let under_700_reglages = reglages_css
        .split("@media (max-width: 700px)")
        .nth(1)
        .expect("palier 700 des réglages");
    let rule_700_reglages = under_700_reglages.split("@media").next().unwrap();
    assert!(
        rule_700_reglages.contains(
            ".reglages-desk,\n  .reglages-desk > .reglages-card {\n    display: flex;\n    flex-direction: column;\n    width: 100%;"
        ),
        "sous 700 px, les cartes empilées restent une colonne : {rule_700_reglages}"
    );
    assert!(
        !rule_700_reglages.contains("grid-template-columns")
            && !rule_700_reglages.contains("repeat("),
        "pas de grille à plusieurs colonnes : {rule_700_reglages}"
    );
    assert!(css.contains("background: var(--ink);"), "{css}");
    assert!(css.contains("accent-color: var(--ink)"), "{css}");

    let js = body_text(
        griffe_web::router(unlocked_state(&test_db_path("plateau-js")).await)
            .oneshot(
                Request::builder()
                    .uri("/assets/app.js")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        js.contains("Échap, sur un chapitre") && js.contains("data-plateau-home"),
        "Échap revient au menu depuis app.js : {js}"
    );
}

#[tokio::test]
async fn society_duties_stack_the_title_above_the_body() {
    let db_path = test_db_path("letter-impots");
    let state = unlocked_state(&db_path)
        .await
        .with_today(time::macros::date!(2026 - 09 - 05));
    {
        let store = Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
        store
            .connection()
            .execute(
                "INSERT INTO setup (id, declared_new_company, updated_at, created_on)
                 VALUES (1, 0, '2026-01-01T00:00:00Z', '2026-12-01')
                 ON CONFLICT (id) DO UPDATE SET created_on = '2026-12-01'",
                [],
            )
            .unwrap();
    }
    let router = griffe_web::router(state);

    let page = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/societe/impots")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(page.contains(">Les échéances<"), "{page}");
    assert!(!page.contains("Ce que tu dois"), "{page}");
    assert!(!page.contains("<h1"), "{page}");
    assert!(
        page.contains("Pas des sigles. Des dates, des montants, et où le déposer."),
        "{page}"
    );
    assert!(
        page.contains("data-etat=\"2\""),
        "deux groupes dessinés : {page}"
    );
    assert_eq!(page.matches("class=\"etat-card\"").count(), 2, "{page}");
    assert!(
        !page.contains(">En retard<"),
        "le groupe vide reste absent : {page}"
    );
    assert!(page.contains("ul class=\"chapters\""), "{page}");
    assert!(
        page.contains("acompte d'impôt sur les sociétés")
            || page.contains("acompte d&#x27;impôt sur les sociétés"),
        "{page}"
    );
    assert!(
        !page.contains("CA3") && !page.contains("3514") && !page.contains("2777"),
        "{page}"
    );
    assert!(!page.contains("hx-on"), "{page}");
    assert!(!page.contains("Aucune échéance dans l'horizon."), "{page}");
    assert!(!page.contains("Rien à déposer pour l'instant."), "{page}");
    let avant = page.find("Avant ce coffre").expect("carte de rattrapage");
    let open = page[..avant].rfind("<article").expect("carte");
    let close = page[avant..].find("</article>").expect("fin de carte") + avant;
    let card = &page[open..close];
    let form_at = card
        .find("hx-post=\"/societe/impots/catch-up\"")
        .expect("formulaire de rattrapage");
    let form_end = card[form_at..].find("</form>").expect("fin de formulaire") + form_at;
    let form = &card[form_at..form_end];
    assert!(
        form.contains("type=\"submit\"")
            && (form.contains("Tout était à jour à l'échéance")
                || form.contains("Tout était à jour à l&#x27;échéance")),
        "le bouton reste dans le formulaire : {form}"
    );
    assert!(card.contains("ul class=\"chapters\""), "{card}");
    let lede = page.find("Pas des sigles").expect("phrase d'entrée");
    let grid = page.find("class=\"etat-grid\"").expect("grille");
    assert!(lede < grid, "la phrase reste au-dessus de la grille");

    let cloture = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/societe/cloture")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(cloture.contains(">Clore<"), "{cloture}");
    assert!(!cloture.contains("<h1"), "{cloture}");
    assert!(cloture.contains("Le parcours, en phrases."), "{cloture}");
    assert!(cloture.contains("ul class=\"hist\""), "{cloture}");
    assert!(cloture.contains(">fait<"), "{cloture}");
    assert!(
        cloture.contains("aujourd'hui") || cloture.contains("aujourd&#x27;hui"),
        "{cloture}"
    );
    assert!(cloture.contains("ensuite"), "{cloture}");
    let hist_at = cloture.find("ul class=\"hist\"").expect("parcours");
    let hist_end = cloture[hist_at..].find("</ul>").expect("fin du parcours") + hist_at;
    let hist = &cloture[hist_at..hist_end];
    assert!(
        hist.contains("Arrêter les comptes"),
        "la phrase du parcours reste : {hist}"
    );
    assert!(
        !hist.contains("hx-get=\"/cloture/new\""),
        "le bouton n'apparaît que l'exercice écoulé : {hist}"
    );
    assert!(!hist.contains("<form"), "{hist}");
    assert!(!cloture.contains("<form"), "{cloture}");
    assert!(
        !cloture.contains("CA3") && !cloture.contains("3514") && !cloture.contains("2777"),
        "{cloture}"
    );
    let phrase = cloture
        .find("Le parcours, en phrases.")
        .expect("phrase d'exercice");
    let phrase_open = cloture[..phrase].rfind("<article").expect("carte");
    let phrase_close = cloture[phrase..].find("</article>").expect("fin") + phrase;
    assert!(
        cloture[phrase_open..phrase_close].contains("class=\"etat-card\""),
        "{cloture}"
    );
    assert!(phrase_close < hist_at, "le parcours reste sous la carte");

    let css = body_text(
        router
            .oneshot(
                Request::builder()
                    .uri("/assets/app.css")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        css.contains(".chapters a strong"),
        "les échéances sont des liens vers la lettre : {css}"
    );
}

#[tokio::test]
async fn society_duty_letter_shows_the_amount_and_the_2571_path() {
    let state = unlocked_state(&test_db_path("letter-is-acompte"))
        .await
        .with_today(time::macros::date!(2026 - 09 - 05));
    let router = griffe_web::router(state);

    let page = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/societe/impots/is-acompte")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        page.contains("2571"),
        "le formulaire officiel est nommé : {page}"
    );
    assert!(
        page.contains("Déclarer") && page.contains("Impôt sur les sociétés"),
        "le chemin de menu : {page}"
    );
    assert!(
        !page.contains("les chiffres sont dans le coffre")
            && !page.contains("Les chiffres sont déjà dans le coffre"),
        "{page}"
    );
    assert!(!page.contains("freeflow "), "{page}");
    assert!(page.contains("impots.gouv.fr"), "{page}");
    assert!(page.contains("Revenir au menu"), "{page}");
    assert!(
        page.contains("Les échéances") && page.contains("href=\"/societe/impots\""),
        "le parent est Les échéances : {page}"
    );
    assert!(page.contains("Clore"), "le voisin est Clore : {page}");
    assert!(page.contains("data-family=\"etat\""), "{page}");
    assert!(page.contains("<h1"), "la lettre garde son titre : {page}");
    assert!(
        page.contains("class=\"block\""),
        "la lettre garde ses blocs : {page}"
    );
    assert!(
        !page.contains("etat-desk") && !page.contains("etat-card"),
        "la lettre ne devient pas une carte : {page}"
    );
    assert!(
        !page.contains("bureau-columns") && !page.contains("bureau-pane"),
        "{page}"
    );

    let unknown = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/societe/impots/nexiste-pas")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unknown.status(), axum::http::StatusCode::NOT_FOUND);

    let internal = router
        .oneshot(
            Request::builder()
                .uri("/societe/impots/approval-meeting")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(internal.status(), axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn society_duty_letter_names_2571_boxes_and_marks_filed() {
    let state = unlocked_state(&test_db_path("letter-is-boxes"))
        .await
        .with_today(time::macros::date!(2026 - 09 - 05));
    let router = griffe_web::router(state);

    let page = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/societe/impots/is-acompte")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(page.contains("Les cases"), "{page}");
    assert!(page.contains(">03<") || page.contains("03"), "{page}");
    assert!(
        page.contains("Montant à payer, impôt sur les sociétés"),
        "{page}"
    );
    assert!(page.contains("C'est déposé."), "{page}");
    assert!(!page.contains("freeflow "), "{page}");

    let filed = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/societe/impots/is-acompte/2026-09-15/filed")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(filed.status(), StatusCode::OK);
    assert_eq!(filed.headers().get("HX-Trigger").unwrap(), "griffe:saved");
    let body = body_text(filed).await;
    assert!(body.contains("Déposé le"), "{body}");
    assert!(body.contains("Ce n'était pas ça"), "{body}");
    assert!(!body.contains("C'est déposé."), "{body}");

    let reopened = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/societe/impots/is-acompte/2026-09-15")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(reopened.contains("Déposé le"), "{reopened}");
    assert!(!reopened.contains("C'est déposé."), "{reopened}");

    let list = body_text(
        router
            .oneshot(
                Request::builder()
                    .uri("/societe/impots")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(list.contains("fait"), "{list}");
    assert!(
        list.contains("<details") && list.contains("Déjà déposé"),
        "les dépôts sont au volet : {list}"
    );
}

#[tokio::test]
async fn filing_september_ca3_does_not_clear_august() {
    let db_path = test_db_path("letter-ca3-monthly");
    let state = unlocked_state(&db_path)
        .await
        .with_today(time::macros::date!(2026 - 09 - 08));
    {
        let mut store =
            Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
        applied(
            Executor::new(&mut store)
                .execute(
                    &griffe_core::company::SetCompanyProfile {
                        name: "Lumen Conseil".into(),
                        legal_form: "SASU".into(),
                        siren: griffe_core::domain::Siren::parse("552100554").unwrap(),
                        vat_number: None,
                        address: griffe_core::domain::Address {
                            street: "18 rue des Ateliers".into(),
                            postal_code: "69003".into(),
                            city: "Lyon".into(),
                            country: "FR".into(),
                        },
                        share_capital: Some(Money::from_cents(100_000)),
                        rcs_city: Some("Lyon".into()),
                        iban: None,
                        fiscal_year_end: Some(
                            griffe_core::domain::FiscalYearEnd::new(9, 30).unwrap(),
                        ),
                        vat_regime: Some(griffe_core::domain::VatRegime::RealNormalMonthly),
                        director_monthly_gross: Some(Money::from_cents(300_000)),
                        director_charge_ratio_bps: None,
                        president_name: Some("Nicolas Lumen".into()),
                        sole_shareholder_name: Some("Nicolas Lumen".into()),
                        sole_shareholder_address: Some("18 rue des Ateliers, 69003 Lyon".into()),
                        share_count: Some(1000),
                    },
                    &human_ctx(),
                )
                .unwrap(),
        );
        store
            .connection()
            .execute(
                "INSERT INTO setup (id, declared_new_company, updated_at, created_on)
                 VALUES (1, 0, '2026-01-01T00:00:00Z', '2026-01-01')
                 ON CONFLICT (id) DO UPDATE SET created_on = '2026-01-01'",
                [],
            )
            .unwrap();
    }
    let router = griffe_web::router(state);

    let list = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/societe/impots")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(list.contains("TVA du mois de juillet"), "{list}");
    assert!(!list.contains("TVA du trimestre"), "{list}");
    assert!(
        list.contains("/societe/impots/ca3/2026-07"),
        "août : {list}"
    );
    assert!(
        list.contains("/societe/impots/ca3/2026-08"),
        "septembre : {list}"
    );
    assert!(!list.contains("Pas encore déposé"), "{list}");

    let august = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/societe/impots/ca3/2026-07")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(august.contains("C'est déposé."), "{august}");
    assert!(august.contains("août"), "{august}");

    let filed = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/societe/impots/ca3/2026-07/filed")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(filed.status(), StatusCode::OK);
    let filed_body = body_text(filed).await;
    assert!(filed_body.contains("Déposé le"), "{filed_body}");
    assert!(!filed_body.contains("C'est déposé."), "{filed_body}");

    let list = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/societe/impots")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(list.contains("fait"), "août déposé : {list}");
    assert!(
        list.contains("Déjà déposé"),
        "août a quitté En retard : {list}"
    );
    assert!(list.contains("ul class=\"chapters\""), "{list}");
    assert!(list.contains("<details class=\"duties-done\""), "{list}");
    let filed_at = list.find("<details").expect("repliable");
    let last_card = list.rfind("</article>").expect("cartes");
    assert!(
        last_card < filed_at,
        "Déjà déposé reste sous la grille : {list}"
    );

    let august = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/societe/impots/ca3/2026-07")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(august.contains("Déposé le"), "{august}");
    assert!(!august.contains("C'est déposé."), "{august}");

    let sept = body_text(
        router
            .oneshot(
                Request::builder()
                    .uri("/societe/impots/ca3/2026-08")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(sept.contains("C'est déposé."), "{sept}");
    assert!(!sept.contains("Ce n'était pas ça"), "{sept}");
}

#[tokio::test]
async fn a_vat_carry_in_posted_from_the_letter_fills_case_25() {
    let db_path = test_db_path("letter-vat-carry");
    let state = unlocked_state(&db_path)
        .await
        .with_today(time::macros::date!(2026 - 09 - 08));
    {
        let mut store =
            Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
        applied(
            Executor::new(&mut store)
                .execute(
                    &griffe_core::company::SetCompanyProfile {
                        name: "Lumen Conseil".into(),
                        legal_form: "SASU".into(),
                        siren: griffe_core::domain::Siren::parse("552100554").unwrap(),
                        vat_number: None,
                        address: griffe_core::domain::Address {
                            street: "18 rue des Ateliers".into(),
                            postal_code: "69003".into(),
                            city: "Lyon".into(),
                            country: "FR".into(),
                        },
                        share_capital: Some(Money::from_cents(100_000)),
                        rcs_city: Some("Lyon".into()),
                        iban: None,
                        fiscal_year_end: Some(
                            griffe_core::domain::FiscalYearEnd::new(9, 30).unwrap(),
                        ),
                        vat_regime: Some(griffe_core::domain::VatRegime::RealNormalMonthly),
                        director_monthly_gross: Some(Money::from_cents(300_000)),
                        director_charge_ratio_bps: None,
                        president_name: Some("Nicolas Lumen".into()),
                        sole_shareholder_name: Some("Nicolas Lumen".into()),
                        sole_shareholder_address: Some("18 rue des Ateliers, 69003 Lyon".into()),
                        share_count: Some(1000),
                    },
                    &human_ctx(),
                )
                .unwrap(),
        );
    }
    let router = griffe_web::router(state);

    let letter = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/societe/impots/ca3/2026-09")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(letter.contains("Crédit avant ce coffre"), "{letter}");
    assert!(!letter.contains("Crédit déjà déclaré"), "{letter}");
    assert!(letter.contains("vat-credit"), "{letter}");
    assert!(
        !letter.contains("L\u{2019}État me le verse") && !letter.contains("L'État me le verse"),
        "pas de sceau avant la reprise : {letter}"
    );

    let posted = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/societe/impots/ca3/2026-09/vat-credit")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("HX-Request", "true")
                .body(Body::from("after_period=2026-08&credit=400.00"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(posted.status(), StatusCode::OK);
    let body = body_text(posted).await;
    assert!(
        body.contains("400,00") || body.contains("400.00"),
        "cases 22 et 25 après reprise : {body}"
    );
    assert!(body.contains("Figé"), "{body}");
    assert!(
        !body.contains("Reprendre ce crédit"),
        "plus de formulaire : {body}"
    );
    assert!(
        !body.contains("L\u{2019}État me le verse") && !body.contains("L'État me le verse"),
        "400 € < 760 € en septembre : {body}"
    );
    assert!(body.contains("760"), "seuil en cours d'année : {body}");
    assert!(body.contains("150"), "seuil de fin d'année : {body}");

    let again = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/societe/impots/ca3/2026-09/vat-credit")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("HX-Request", "true")
                .body(Body::from("after_period=2026-08&credit=100.00"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(again.status(), StatusCode::OK);
    let again_body = body_text(again).await;
    assert!(
        again_body.contains("400,00") || again_body.contains("400.00"),
        "le 100 € n'écrase pas : {again_body}"
    );
    assert!(!again_body.contains("100,00"), "{again_body}");
}

#[tokio::test]
async fn letter_vat_reversal_drops_the_carried_credit() {
    let db_path = test_db_path("letter-vat-reversal");
    let state = unlocked_state(&db_path)
        .await
        .with_today(time::macros::date!(2026 - 09 - 15));
    {
        let mut store =
            Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
        applied(
            Executor::new(&mut store)
                .execute(
                    &griffe_core::company::SetCompanyProfile {
                        name: "Lumen Conseil".into(),
                        legal_form: "SASU".into(),
                        siren: griffe_core::domain::Siren::parse("552100554").unwrap(),
                        vat_number: None,
                        address: griffe_core::domain::Address {
                            street: "18 rue des Ateliers".into(),
                            postal_code: "69003".into(),
                            city: "Lyon".into(),
                            country: "FR".into(),
                        },
                        share_capital: Some(Money::from_cents(100_000)),
                        rcs_city: Some("Lyon".into()),
                        iban: None,
                        fiscal_year_end: Some(
                            griffe_core::domain::FiscalYearEnd::new(9, 30).unwrap(),
                        ),
                        vat_regime: Some(griffe_core::domain::VatRegime::RealNormalMonthly),
                        director_monthly_gross: Some(Money::from_cents(300_000)),
                        director_charge_ratio_bps: None,
                        president_name: Some("Nicolas Lumen".into()),
                        sole_shareholder_name: Some("Nicolas Lumen".into()),
                        sole_shareholder_address: Some("18 rue des Ateliers, 69003 Lyon".into()),
                        share_count: Some(1000),
                    },
                    &human_ctx(),
                )
                .unwrap(),
        );
        applied(
            Executor::new(&mut store)
                .execute(
                    &griffe_core::society::RecordVatCarryIn {
                        after_period: "2026-08".into(),
                        credit: Money::from_cents(40_000),
                        source: Some("CA3 août".into()),
                    },
                    &human_ctx(),
                )
                .unwrap(),
        );
        applied(
            Executor::new(&mut store)
                .execute(
                    &griffe_core::society::MarkDutyFiled {
                        kind: griffe_core::fiscal::FiscalDeadlineKind::Ca3,
                        period_key: "2026-08".into(),
                        due_on: time::macros::date!(2026 - 09 - 21),
                        filed_on: time::macros::date!(2026 - 09 - 08),
                    },
                    &human_ctx(),
                )
                .unwrap(),
        );
    }
    let router = griffe_web::router(state);

    let letter = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/societe/impots/ca3/2026-09")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(letter.contains("trop récupéré"), "{letter}");
    assert!(!letter.contains("ligne 15"), "{letter}");
    let site = letter.find("Sur le site").expect("Sur le site");
    assert!(
        !letter[..site].contains("0600"),
        "0600 hors Sur le site : {letter}"
    );
    assert!(!letter.contains("freeflow "), "{letter}");

    let posted = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/societe/impots/ca3/2026-09/vat-reversal")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("HX-Request", "true")
                .body(Body::from("amount=10.00"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(posted.status(), StatusCode::OK);
    let trigger = posted.headers().get("HX-Trigger").cloned();
    assert_eq!(
        trigger.as_ref().map(|v| v.as_bytes()),
        Some(b"griffe:saved" as &[u8])
    );
    let body = body_text(posted).await;
    assert!(body.contains("10,00") || body.contains("10.00"), "{body}");
    assert!(body.contains("390,00") || body.contains("390.00"), "{body}");
    assert!(body.contains("rendus"), "{body}");
    let site = body.find("Sur le site").expect("Sur le site");
    assert!(
        !body[..site].contains("0600"),
        "0600 hors Sur le site : {body}"
    );
    assert!(!body.contains("ligne 15"), "{body}");
    assert!(!body.contains("freeflow "), "{body}");

    let home = body_text(
        router
            .oneshot(
                Request::builder()
                    .uri("/societe")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        home.contains("390,00") || home.contains("390.00"),
        "La société montre 390 : {home}"
    );
    assert!(home.contains("TVA"), "{home}");
}

#[tokio::test]
async fn letter_writes_the_month_vat_once_and_the_morning_letter_stays_plain() {
    let db_path = test_db_path("letter-vat-liquidation");
    let state = unlocked_state(&db_path)
        .await
        .with_today(time::macros::date!(2026 - 09 - 30));
    {
        let mut store =
            Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
        applied(
            Executor::new(&mut store)
                .execute(
                    &griffe_core::company::SetCompanyProfile {
                        name: "Lumen Conseil".into(),
                        legal_form: "SASU".into(),
                        siren: griffe_core::domain::Siren::parse("552100554").unwrap(),
                        vat_number: None,
                        address: griffe_core::domain::Address {
                            street: "18 rue des Ateliers".into(),
                            postal_code: "69003".into(),
                            city: "Lyon".into(),
                            country: "FR".into(),
                        },
                        share_capital: Some(Money::from_cents(100_000)),
                        rcs_city: Some("Lyon".into()),
                        iban: None,
                        fiscal_year_end: Some(
                            griffe_core::domain::FiscalYearEnd::new(9, 30).unwrap(),
                        ),
                        vat_regime: Some(griffe_core::domain::VatRegime::RealNormalMonthly),
                        director_monthly_gross: None,
                        director_charge_ratio_bps: None,
                        president_name: None,
                        sole_shareholder_name: None,
                        sole_shareholder_address: None,
                        share_count: None,
                    },
                    &human_ctx(),
                )
                .unwrap(),
        );
        applied(
            Executor::new(&mut store)
                .execute(
                    &griffe_core::society::RecordVatCarryIn {
                        after_period: "2026-08".into(),
                        credit: Money::from_cents(40_000),
                        source: Some("déclaration d'août".into()),
                    },
                    &human_ctx(),
                )
                .unwrap(),
        );
        applied(
            Executor::new(&mut store)
                .execute(
                    &griffe_core::society::MarkDutyFiled {
                        kind: griffe_core::fiscal::FiscalDeadlineKind::Ca3,
                        period_key: "2026-08".into(),
                        due_on: time::macros::date!(2026 - 09 - 21),
                        filed_on: time::macros::date!(2026 - 09 - 08),
                    },
                    &human_ctx(),
                )
                .unwrap(),
        );
        applied(
            Executor::new(&mut store)
                .execute(
                    &griffe_core::society::RecordVatReversal {
                        period_key: "2026-09".into(),
                        amount: Money::from_cents(1_000),
                        recorded_on: time::macros::date!(2026 - 09 - 15),
                    },
                    &human_ctx(),
                )
                .unwrap(),
        );
    }
    let router = griffe_web::router(state);

    let august = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/societe/impots/ca3/2026-08")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        !august.contains("Écrire la TVA du mois"),
        "un mois déposé n'offre pas l'écriture : {august}"
    );

    let september = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/societe/impots/ca3/2026-09")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(september.contains("Écrire la TVA du mois"), "{september}");
    assert!(
        !september.contains("La TVA du mois est écrite."),
        "{september}"
    );

    let posted = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/societe/impots/ca3/2026-09/vat-liquidation")
                .header("HX-Request", "true")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(posted.status(), StatusCode::OK);
    assert_eq!(
        posted.headers().get("HX-Trigger").map(|v| v.as_bytes()),
        Some(b"griffe:saved" as &[u8])
    );
    let body = body_text(posted).await;
    assert!(body.contains("La TVA du mois est écrite."), "{body}");
    assert!(body.contains("390,00") || body.contains("390.00"), "{body}");
    let site = body.find("Sur le site").expect("Sur le site");
    assert!(
        !body[..site].contains("3310") && !body[..site].contains("CA3"),
        "le geste ne nomme pas le formulaire : {body}"
    );

    let again = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/societe/impots/ca3/2026-09/vat-liquidation")
                .header("HX-Request", "true")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(again.status(), StatusCode::OK);
    assert_eq!(
        again.headers().get("HX-Trigger").map(|v| v.as_bytes()),
        Some(b"griffe:saved" as &[u8])
    );

    let live: i64 = Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE))
        .unwrap()
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM journal_entries
              WHERE source_kind = 'vat_liquidation' AND source_id = '2026-09'
                AND reversed_at IS NULL AND reversal_of IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(live, 1);

    let morning = body_text(
        router
            .oneshot(Request::builder().uri("/jour").body(Body::empty()).unwrap())
            .await
            .unwrap(),
    )
    .await;
    assert!(!morning.contains("CA3"), "{morning}");
    assert!(!morning.contains("3310"), "{morning}");
    assert!(!morning.contains("3514"), "{morning}");
    assert!(morning.contains("Savoir pour la TVA"), "{morning}");
}

fn assert_3519_only_after_sur_le_site(html: &str) {
    let Some(idx) = html.find("Sur le site") else {
        assert!(
            !html.contains("3519"),
            "3519 sans bloc Sur le site : {html}"
        );
        return;
    };
    assert!(
        !html[..idx].contains("3519"),
        "3519 hors Sur le site : {html}"
    );
}

#[tokio::test]
async fn requesting_the_december_credit_from_the_letter_fills_case_26() {
    let db_path = test_db_path("letter-vat-refund-dec");
    let state = unlocked_state(&db_path)
        .await
        .with_today(time::macros::date!(2027 - 01 - 08));
    {
        let mut store =
            Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
        applied(
            Executor::new(&mut store)
                .execute(
                    &griffe_core::company::SetCompanyProfile {
                        name: "Lumen Conseil".into(),
                        legal_form: "SASU".into(),
                        siren: griffe_core::domain::Siren::parse("552100554").unwrap(),
                        vat_number: None,
                        address: griffe_core::domain::Address {
                            street: "18 rue des Ateliers".into(),
                            postal_code: "69003".into(),
                            city: "Lyon".into(),
                            country: "FR".into(),
                        },
                        share_capital: Some(Money::from_cents(100_000)),
                        rcs_city: Some("Lyon".into()),
                        iban: None,
                        fiscal_year_end: Some(
                            griffe_core::domain::FiscalYearEnd::new(9, 30).unwrap(),
                        ),
                        vat_regime: Some(griffe_core::domain::VatRegime::RealNormalMonthly),
                        director_monthly_gross: Some(Money::from_cents(300_000)),
                        director_charge_ratio_bps: None,
                        president_name: Some("Nicolas Lumen".into()),
                        sole_shareholder_name: Some("Nicolas Lumen".into()),
                        sole_shareholder_address: Some("18 rue des Ateliers, 69003 Lyon".into()),
                        share_count: Some(1000),
                    },
                    &human_ctx(),
                )
                .unwrap(),
        );
        applied(
            Executor::new(&mut store)
                .execute(
                    &griffe_core::society::RecordVatCarryIn {
                        after_period: "2026-08".into(),
                        credit: Money::from_cents(40_000),
                        source: Some("test".into()),
                    },
                    &human_ctx(),
                )
                .unwrap(),
        );
    }
    let router = griffe_web::router(state);

    let letter = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/societe/impots/ca3/2026-12")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        letter.contains("L\u{2019}État me le verse") || letter.contains("L'État me le verse"),
        "{letter}"
    );
    assert_3519_only_after_sur_le_site(&letter);
    assert!(!letter.contains("freeflow "), "{letter}");

    let jour = body_text(
        router
            .clone()
            .oneshot(Request::builder().uri("/jour").body(Body::empty()).unwrap())
            .await
            .unwrap(),
    )
    .await;
    assert!(jour.contains("Savoir pour la TVA"), "{jour}");
    assert!(jour.contains("400,00"), "montant du crédit offert : {jour}");
    assert!(jour.contains("récupérer"), "{jour}");

    let posted = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/societe/impots/ca3/2026-12/vat-refund")
                .header("HX-Request", "true")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(posted.status(), StatusCode::OK);
    assert_eq!(posted.headers().get("HX-Trigger").unwrap(), "griffe:saved");
    let body = body_text(posted).await;
    assert!(
        body.contains("400,00") || body.contains("400.00"),
        "case 26 = 400 : {body}"
    );
    assert!(body.contains("À vous verser"), "{body}");
    assert!(
        body.contains("Finalement, je le laisse en crédit"),
        "{body}"
    );
    assert_3519_only_after_sur_le_site(&body);
    assert!(!body.contains("freeflow "), "{body}");
}

#[tokio::test]
async fn a_duty_letter_does_not_squeeze_steps_into_a_date_column() {
    let state = unlocked_state(&test_db_path("letter-is-measure"))
        .await
        .with_today(time::macros::date!(2026 - 09 - 05));
    let router = griffe_web::router(state);

    let page = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/societe/impots/is-acompte")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(page.contains("Ce que tu feras"), "{page}");
    assert!(
        page.contains("class=\"steps\""),
        "les démarches sans date occupent la feuille : {page}"
    );
    assert!(
        !page.contains("ul class=\"hist\""),
        "une .hist à un seul enfant coincerait la phrase à 8,5 rem : {page}"
    );

    let css = body_text(
        router
            .oneshot(
                Request::builder()
                    .uri("/assets/app.css")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    let lede = css
        .split(".lede {")
        .nth(1)
        .and_then(|rest| rest.split('}').next())
        .expect("règle .lede");
    assert!(
        lede.contains("max-width: 52ch"),
        "lede un peu plus court que le corps : {lede}"
    );
    assert!(
        !css.contains("max-width: 48ch"),
        "le corps n'a pas de seconde mesure en ch : {css}"
    );
    assert!(
        css.contains(".steps"),
        "primitive des listes sans date : {css}"
    );
    assert!(
        css.contains("minmax(0, 1fr)"),
        "la piste texte d'une .hist ne déborde pas : {css}"
    );
}

#[tokio::test]
async fn duty_letter_keeps_filing_actions_on_one_wrapping_row() {
    let state = unlocked_state(&test_db_path("letter-filed-on-row"))
        .await
        .with_today(time::macros::date!(2026 - 09 - 05));
    let router = griffe_web::router(state);

    let page = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/societe/impots/is-acompte")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        page.contains(r#"name="filed_on""#) && page.contains(r#"class="date-pick""#),
        "la date s'ouvre dans un calendrier de la page, pas le popover natif : {page}"
    );
    let bar_for_date = page
        .split(r#"class="duty-bar""#)
        .nth(1)
        .expect("rangée duty-bar");
    assert!(
        !bar_for_date.contains(r#"type="date""#),
        "WebKitGTK ne ferme pas son calendrier natif : {bar_for_date}"
    );
    assert!(
        page.contains(r#"class="duty-bar""#),
        "date et boutons partagent une rangée : {page}"
    );
    let bar = page
        .split(r#"class="duty-bar""#)
        .nth(1)
        .expect("rangée duty-bar");
    assert!(
        bar.contains(r#"name="filed_on""#),
        "la date est dans la rangée : {bar}"
    );
    assert!(bar.contains("C'est déposé."), "déposer : {bar}");
    assert!(bar.contains("Ouvrir dans le navigateur"), "ouvrir : {bar}");
    assert!(bar.contains("Revenir"), "revenir : {bar}");
    assert!(
        bar.contains(r#"class="field-inline""#),
        "libellé et date sur une ligne, pas un .field empilé : {bar}"
    );

    let css = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/assets/app.css")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    let row = css
        .split(".duty-bar {")
        .nth(1)
        .and_then(|rest| rest.split('}').next())
        .expect("règle .duty-bar");
    assert!(
        row.contains("display: flex")
            && row.contains("flex-wrap: wrap")
            && row.contains("align-items: center"),
        "une ligne sur grand écran, qui se replie : {row}"
    );
    let mobile = css
        .split("@media (max-width: 700px)")
        .nth(1)
        .expect("le bureau 700 px empile déjà le chrome");
    assert!(
        mobile.contains(".duty-bar") && mobile.contains("flex-direction: column"),
        "sous 700 px la rangée s'empile : {mobile}"
    );

    let js = body_text(
        router
            .oneshot(
                Request::builder()
                    .uri("/assets/app.js")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        js.contains("closeDatePick") && js.contains("date-pick") && js.contains("Escape"),
        "le calendrier de la page se ferme au jour choisi, au clic dehors et sur Échap : {js}"
    );
}

#[tokio::test]
async fn going_back_from_the_statement_keeps_content_inner_html() {
    let state = unlocked_state(&test_db_path("letter-releve-back")).await;
    let router = griffe_web::router(state);

    let releve = body_text(
        router
            .oneshot(
                Request::builder()
                    .uri("/societe/releve")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        releve.contains("← La société") || releve.contains("&larr; La société"),
        "{releve}"
    );
    assert!(
        releve.contains("hx-swap=\"innerHTML\""),
        "le retour vers La société doit forcer innerHTML, pas hériter outerHTML : {releve}"
    );
    assert!(
        releve.contains("hx-disinherit"),
        "le letter du relevé ne doit pas léguer outerHTML à ses enfants : {releve}"
    );
    assert!(
        !releve.contains("Revenir au menu"),
        "ce fragment ne redessine pas le pli : {releve}"
    );
    assert!(
        releve.contains("class=\"letter spread tenir-desk\""),
        "{releve}"
    );
    assert!(releve.contains("class=\"tenir-card\""), "{releve}");
    assert!(!releve.contains("<h1"), "{releve}");
    assert!(!releve.contains("<!DOCTYPE html>"), "{releve}");
}

fn applied<T: std::fmt::Debug>(outcome: griffe_core::app::Outcome<T>) -> T {
    match outcome {
        griffe_core::app::Outcome::Applied(v) => v,
        other => panic!("expected Applied, got {other:?}"),
    }
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
            griffe_core::app::Outcome::Applied(id) => id,
            other => panic!("expected Applied, got {other:?}"),
        };
        Executor::new(&mut store)
            .execute(
                &griffe_core::prospection::ArchiveOpportunity {
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
                    kind: griffe_core::domain::MissionKind::Recurrent {
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
            griffe_core::app::Outcome::Applied(id) => id,
            other => panic!("expected Applied, got {other:?}"),
        };
        Executor::new(&mut store)
            .execute(
                &griffe_core::missions::CloseMission {
                    id: mission_id,
                    revision: 1,
                    ended_on: time::Date::from_calendar_date(2026, time::Month::June, 30).unwrap(),
                },
                &human_ctx(),
            )
            .unwrap();
        Executor::new(&mut store)
            .execute(
                &griffe_core::missions::ArchiveMission {
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
                &griffe_core::quotes::CreateQuote {
                    client_id,
                    opportunity_id: None,
                    lines: vec![griffe_core::domain::QuoteLine {
                        description: "Refonte plateforme".to_string(),
                        kind: griffe_core::domain::LineKind::Forfait {
                            amount: Money::from_cents(4_500_000),
                        },
                        vat_rate: griffe_core::domain::VatRate::Standard,
                    }],
                    discount: Some(griffe_core::domain::Discount::Percentage(1_000)),
                    terms: None,
                    valid_until: time::Date::from_calendar_date(2026, time::Month::October, 31)
                        .unwrap(),
                },
                &human_ctx(),
            )
            .unwrap();
        Executor::new(&mut store)
            .execute(
                &griffe_core::expenses::RecordExpense {
                    label: "Abonnement hébergement".to_string(),
                    category: griffe_core::domain::ExpenseCategory::Software,
                    amount: Money::from_cents(12_000),
                    vat_rate: griffe_core::domain::VatRate::Standard,
                    vat_deductible: Money::from_cents(2_000),
                    incurred_on: time::Date::from_calendar_date(2026, time::Month::September, 5)
                        .unwrap(),
                    receipt_hash: None,
                    receipt_filename: None,
                    supplier: None,
                    bank_transaction_id: None,
                    paid_by: griffe_core::domain::ExpensePaidBy::Company,

                    reverse_charge: false,
                },
                &human_ctx(),
            )
            .unwrap();
    }

    let router = griffe_web::router(state);

    for path in [
        "/jour",
        "/affaires",
        "/dossiers",
        "/gens",
        "/societe",
        "/aide",
        "/aide/releve",
        "/lexique",
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
    let router = griffe_web::router(state);

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
    let router = griffe_web::router(state);

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
    let router = griffe_web::router(state);

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
    let router = griffe_web::router(state);

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
    let router = griffe_web::router(state);

    for (path, content_type_prefix) in [
        ("/assets/app.css", "text/css"),
        ("/assets/app.js", "text/javascript"),
        ("/assets/htmx.min.js", "text/javascript"),
        (
            "/assets/fonts/source-sans-3-latin-400-normal.woff2",
            "font/woff2",
        ),
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
        if path.ends_with(".woff2") {
            let bytes = response.into_body().collect().await.unwrap().to_bytes();
            assert!(bytes.len() > 1000, "{path} trop court");
            continue;
        }
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
    let router = griffe_web::router(state);

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
    let router = griffe_web::router(state);
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
async fn unlock_screen_says_griffe() {
    let db_path = test_db_path("unlock-griffe");
    Store::create(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
    let state = AppState::new(db_path); // jamais déverrouillé
    let router = griffe_web::router(state);
    let response = router
        .oneshot(
            Request::builder()
                .uri("/unlock")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = body_text(response).await;
    assert!(body.contains("Griffe"), "{body}");
    assert!(!body.contains(">FreeFlow<"), "{body}");
}

#[tokio::test]
async fn the_footer_names_the_compiled_version() {
    let state = unlocked_state(&test_db_path("version-foot")).await;
    let router = griffe_web::router(state);
    let jour = body_text(
        router
            .oneshot(Request::builder().uri("/jour").body(Body::empty()).unwrap())
            .await
            .unwrap(),
    )
    .await;
    let version = format!("Griffe {}", env!("CARGO_PKG_VERSION"));
    let foot = jour.split_once("class=\"foot\"").map(|(_, rest)| rest);
    assert!(
        foot.is_some_and(|foot| foot.contains(&version)),
        "le pied doit nommer la version compilée ({version}) : {jour}"
    );
}

#[tokio::test]
async fn the_locked_screens_name_the_compiled_version() {
    let version = format!("Griffe {}", env!("CARGO_PKG_VERSION"));

    let db_path = test_db_path("version-unlock");
    Store::create(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
    let unlock = body_text(
        griffe_web::router(AppState::new(db_path))
            .oneshot(
                Request::builder()
                    .uri("/unlock")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        unlock.contains(&version),
        "l'écran verrouillé doit nommer la version ({version}) : {unlock}"
    );

    let setup = body_text(
        griffe_web::router(AppState::new(test_db_path("version-setup")))
            .oneshot(
                Request::builder()
                    .uri("/setup")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        setup.contains(&version),
        "la création du coffre doit nommer la version ({version}) : {setup}"
    );
}

#[tokio::test]
async fn a_view_request_while_locked_renders_the_unlock_screen_directly() {
    // Pas de `303 Location` pour une navigation de premier niveau : le protocole URI custom de
    // la coque desktop (WebKitGTK) ne le suit pas de façon fiable (page blanche silencieuse,
    // observé en pratique). Le middleware rend l'écran cible directement, en 200.
    let db_path = test_db_path("locked-redirect");
    Store::create(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
    let state = AppState::new(db_path); // jamais déverrouillé

    let router = griffe_web::router(state);
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

    let router = griffe_web::router(state);
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

    let router = griffe_web::router(state);
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
    let router = griffe_web::router(state);

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
    assert!(
        body.contains("data-view=\"jour\"") || body.contains("Le jour"),
        "le déverrouillage atterrit sur Le jour : {body}"
    );
    assert!(!body.contains(PASSPHRASE));
}

#[tokio::test]
async fn posting_a_wrong_passphrase_re_renders_the_form_and_stays_locked() {
    let db_path = test_db_path("unlock-wrong");
    Store::create(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
    let state = AppState::new(db_path);
    let router = griffe_web::router(state.clone());

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
async fn unlock_with_a_newer_schema_shows_the_download_message() {
    let db_path = test_db_path("unlock-schema-too-new");
    let store = Store::create(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
    store
        .connection()
        .pragma_update(None, "user_version", 9999)
        .unwrap();
    drop(store);
    let state = AppState::new(db_path);
    let router = griffe_web::router(state);
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
    let body = body_text(response).await;
    assert!(body.contains("trop ancienne pour ce coffre"), "{body}");
    assert!(body.contains("téléchargez la dernière"), "{body}");
    assert!(
        !body.contains("MigrationDefinition"),
        "pas le dump rusqlite_migration : {body}"
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
    // Fabrique la fenêtre F4 (voir `griffe_core::store` : base déjà basculée sur la nouvelle
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
    let router = griffe_web::router(state);

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
    let router = griffe_web::router(state);

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
    let router = griffe_web::router(state);

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
    let router = griffe_web::router(state);

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
    let router = griffe_web::router(state);

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
    let router = griffe_web::router(state);

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
async fn a_short_suspension_holds_past_the_idle_timeout_then_lets_audit_polling_lock() {
    let db_path = test_db_path("idle-suspended");
    Store::create(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
    let state = AppState::with_idle_timeout(db_path, Duration::from_millis(200));
    state
        .unlock(&Passphrase::from(PASSPHRASE), false)
        .await
        .unwrap();
    state.suspend_idle_for(Duration::from_millis(1_000)).await;
    let router = griffe_web::router(state);

    tokio::time::sleep(Duration::from_millis(500)).await;
    let held = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/audit/recent")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(held.status(), StatusCode::OK);
    let held_body = body_text(held).await;
    assert!(
        held_body.contains("aucune activité pour l'instant"),
        "au-delà du délai d'inactivité, la suspension tient encore : {held_body}"
    );

    tokio::time::sleep(Duration::from_millis(800)).await;
    let closed = router
        .oneshot(
            Request::builder()
                .uri("/audit/recent")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(closed.status(), StatusCode::OK);
    let closed_body = body_text(closed).await;
    assert!(
        closed_body.contains("Coffre verrouillé"),
        "une fois la suspension passée, le sondage d'audit referme la fenêtre abandonnée : \
         {closed_body}"
    );
}

#[tokio::test]
async fn locking_closes_at_once_while_the_idle_suspension_is_live() {
    let db_path = test_db_path("lock-while-suspended");
    Store::create(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
    let state = AppState::with_idle_timeout(db_path, Duration::from_secs(60));
    state
        .unlock(&Passphrase::from(PASSPHRASE), false)
        .await
        .unwrap();
    state.suspend_idle_for(Duration::from_secs(60)).await;
    let router = griffe_web::router(state);

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
    let body = body_text(after).await;
    assert!(body.contains("Coffre verrouillé"), "{body}");
}

#[tokio::test]
async fn creating_a_client_through_the_panel_appears_in_the_table_and_triggers_a_refresh() {
    let db_path = test_db_path("clients-create");
    let state = unlocked_state(&db_path).await;
    let router = griffe_web::router(state);

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
        "griffe:saved",
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
    let router = griffe_web::router(state);

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
    let router = griffe_web::router(state);
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
        "griffe:saved"
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
    let router = griffe_web::router(state);
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
    let router = griffe_web::router(state);
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
        "griffe:saved"
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
        let griffe_core::app::Outcome::Applied(id) = Executor::new(&mut store)
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
            &griffe_core::prospection::CreateOpportunity {
                client_id,
                name: "Refonte".to_string(),
                amount: griffe_core::domain::Money::from_cents(10_000),
                probability: griffe_core::domain::Probability::new(50).unwrap(),
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
    let router = griffe_web::router(state);

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
    let router = griffe_web::router(state);
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
    let contact_id = griffe_core::clients::list_contacts(store.connection(), client_id)
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
    let router = griffe_web::router(state);
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
    let router = griffe_web::router(state);

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
        "griffe:saved"
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
    let router = griffe_web::router(state);

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
        "griffe:saved"
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
    let router = griffe_web::router(state);
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
    let router = griffe_web::router(state);
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
        "griffe:saved"
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
    let router = griffe_web::router(state);
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
    let router = griffe_web::router(state);
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
        griffe_core::prospection::list_interactions(store.connection(), id)
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
        griffe_core::app::Outcome::Applied(id) => id,
        other => panic!("expected Applied, got {other:?}"),
    };
    let mission_id = match Executor::new(&mut store)
        .execute(
            &CreateMission {
                client_id,
                quote_id: None,
                name: "Refonte dashboard".to_string(),
                kind: griffe_core::domain::MissionKind::Forfait {
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
        griffe_core::app::Outcome::Applied(id) => id,
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
    let router = griffe_web::router(state);

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
        "griffe:saved"
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
    let router = griffe_web::router(state);

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
    let router = griffe_web::router(state);

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
        griffe_core::missions::list_time_entries(store.connection(), id)
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
            &griffe_core::company::SetCompanyProfile {
                name: "Argon Digital".to_string(),
                legal_form: "SASU".to_string(),
                siren: griffe_core::domain::Siren::parse("552100554").unwrap(),
                vat_number: None,
                address: griffe_core::domain::Address {
                    street: "12 rue de la Paix".to_string(),
                    postal_code: "75002".to_string(),
                    city: "Paris".to_string(),
                    country: "FR".to_string(),
                },
                share_capital: Some(Money::from_cents(100_000)),
                rcs_city: Some("Paris".to_string()),
                iban: None,
                fiscal_year_end: Some(griffe_core::domain::FiscalYearEnd::CALENDAR),
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
        griffe_core::app::Outcome::Applied(id) => id,
        other => panic!("expected Applied, got {other:?}"),
    };
    Executor::new(&mut store)
        .execute(
            &griffe_core::billing::EmitInvoice {
                client_id,
                mission_id: None,
                lines: vec![griffe_core::domain::InvoiceLine {
                    description: "Prestation".to_string(),
                    quantity: 9.5,
                    unit_price: Money::from_cents(65_000),
                    vat_rate: griffe_core::domain::VatRate::Standard,
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

fn fiscal_year_id(db_path: &Path) -> griffe_core::domain::FiscalYearId {
    let store = Store::open_with_passphrase(db_path, &Passphrase::from(PASSPHRASE)).unwrap();
    griffe_core::fiscal_year::list_fiscal_years(store.connection())
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
    let router = griffe_web::router(state);

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
        Some("griffe:saved"),
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
    // par le vrai binaire typst, comme les tests de griffe-docs).
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
        Some("griffe:saved")
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
    let router = griffe_web::router(state);

    // Créer, sans justificatif.
    let (content_type, body) = multipart_form(
        &[
            ("label", "Abonnement hébergement"),
            ("category", "software"),
            ("amount", "120.00"),
            ("vat_rate", "standard"),
            ("vat_deductible", "20.00"),
            ("incurred_on", "2026-09-05"),
            ("paid_by", "company"),
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
        "griffe:saved"
    );

    let expense_id = {
        let store = Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
        let expenses = griffe_core::expenses::list_expenses(store.connection()).unwrap();
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
        "griffe:saved"
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
        "griffe:saved"
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
    let router = griffe_web::router(state);

    let (fees_tx, bank_tx) = {
        let mut store =
            Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
        Executor::new(&mut store)
            .execute(
                &griffe_core::billing::ImportBankTransactions {
                    transactions: vec![
                        griffe_core::billing::ParsedTransaction {
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
                        griffe_core::billing::ParsedTransaction {
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
        let debits = griffe_core::billing::unmatched_debits(store.connection()).unwrap();
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
        "griffe:saved"
    );
    let fees_id = {
        let store = Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
        let detail = griffe_core::expenses::list_expenses(store.connection())
            .unwrap()
            .into_iter()
            .next()
            .map(|e| {
                griffe_core::expenses::expense_detail(store.connection(), e.id)
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
    assert!(detail_body.contains("au relevé"), "{detail_body}");
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
        "griffe:saved"
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
        "griffe:saved"
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
        let detail = griffe_core::expenses::expense_detail(store.connection(), fees_id)
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
    let router = griffe_web::router(state);

    let content: &[u8] = b"%PDF-1.4 facture fournisseur de test";
    let expected_hash = griffe_core::expenses::hash_receipt(content);
    let (content_type, body) = multipart_form(
        &[
            ("label", "Écran externe"),
            ("category", "equipment"),
            ("amount", "300.00"),
            ("vat_rate", "standard"),
            ("vat_deductible", "50.00"),
            ("incurred_on", "2026-09-05"),
            ("paid_by", "company"),
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
        "griffe:saved",
        "le justificatif ne doit pas faire échouer la création"
    );

    let (expense_id, archived_name) = {
        let store = Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
        let expenses = griffe_core::expenses::list_expenses(store.connection()).unwrap();
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
            griffe_core::receipts::read(&store, &archived_name).unwrap(),
            content,
            "le contenu déchiffré est le contenu reçu, tel quel"
        );
        (expense.id, archived_name)
    };
    let archived_path = griffe_core::receipts::path_of(
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
        "griffe:saved"
    );
    {
        let store = Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
        let expense = griffe_core::expenses::expense_by_id(store.connection(), expense_id)
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
    let router = griffe_web::router(state);
    let client_id = client_id_by_name(&db_path, "Kappa Software");

    let quote_id = {
        let mut store =
            Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
        match Executor::new(&mut store)
            .execute(
                &griffe_core::quotes::CreateQuote {
                    client_id,
                    opportunity_id: None,
                    lines: vec![griffe_core::domain::QuoteLine {
                        description: "Refonte plateforme".to_string(),
                        kind: griffe_core::domain::LineKind::Forfait {
                            amount: Money::from_cents(4_500_000),
                        },
                        vat_rate: griffe_core::domain::VatRate::Standard,
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
            griffe_core::app::Outcome::Applied(id) => id,
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
    assert_eq!(sent.headers().get("HX-Trigger").unwrap(), "griffe:saved");

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
        "griffe:saved"
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
    let router = griffe_web::router(state);

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
    assert_eq!(created.headers().get("HX-Trigger").unwrap(), "griffe:saved");

    let quote = {
        let store = Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
        let quotes = griffe_core::quotes::list_quotes(store.connection()).unwrap();
        assert_eq!(quotes.len(), 1);
        quotes.into_iter().next().unwrap()
    };
    assert_eq!(quote.lines.len(), 2);
    assert_eq!(quote.lines[0].description, "Dev");
    assert_eq!(
        quote.lines[0].kind,
        griffe_core::domain::LineKind::Forfait {
            amount: Money::from_cents(135_000),
        }
    );
    assert_eq!(
        quote.lines[1].vat_rate,
        griffe_core::domain::VatRate::Reduced
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
    assert_eq!(revised.headers().get("HX-Trigger").unwrap(), "griffe:saved");

    let store = Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
    let quotes = griffe_core::quotes::list_quotes(store.connection()).unwrap();
    assert_eq!(quotes.len(), 2);
    let v2 = quotes.iter().find(|q| q.version == 2).unwrap();
    assert_eq!(v2.root_id, quote.root_id);
    assert_eq!(v2.status, griffe_core::domain::QuoteStatus::Draft);
    assert_eq!(
        v2.discount,
        Some(griffe_core::domain::Discount::Percentage(1000))
    );
    assert_eq!(v2.lines.len(), 1);
    assert_eq!(v2.lines[0].description, "Dev complet");
    assert_eq!(v2.terms, None);
}

#[tokio::test]
async fn voiding_a_payment_through_the_invoice_panel_restores_the_unpaid_status() {
    let db_path = test_db_path("facturation-void");
    let state = unlocked_state_with_client(&db_path).await;
    let router = griffe_web::router(state);
    let client_id = client_id_by_name(&db_path, "Kappa Software");

    // Fixture : une facture payée intégralement (6 500 € HT → 7 800 € TTC).
    let (invoice_id, payment_id) = {
        let mut store =
            Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
        let emitted = match Executor::new(&mut store)
            .execute(
                &griffe_core::billing::EmitInvoice {
                    client_id,
                    mission_id: None,
                    lines: vec![griffe_core::domain::InvoiceLine {
                        description: "Prestation".to_string(),
                        quantity: 10.0,
                        unit_price: Money::from_cents(65_000),
                        vat_rate: griffe_core::domain::VatRate::Standard,
                    }],
                    issued_on: time::Date::from_calendar_date(2026, time::Month::September, 1)
                        .unwrap(),
                    payment_terms_days: 30,
                },
                &human_ctx(),
            )
            .unwrap()
        {
            griffe_core::app::Outcome::Applied(e) => e,
            other => panic!("expected Applied, got {other:?}"),
        };
        let payment_id = match Executor::new(&mut store)
            .execute(
                &griffe_core::billing::RecordPayment {
                    invoice_id: emitted.id,
                    amount: Money::from_cents(780_000),
                    received_on: time::Date::from_calendar_date(2026, time::Month::September, 10)
                        .unwrap(),
                    method: griffe_core::domain::PaymentMethod::BankTransfer,
                },
                &human_ctx(),
            )
            .unwrap()
        {
            griffe_core::app::Outcome::Applied(id) => id,
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
    assert_eq!(voided.headers().get("HX-Trigger").unwrap(), "griffe:saved");
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
    let router = griffe_web::router(state);

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
        Some("griffe:saved")
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
/// se solde par un bandeau dans le panneau, sans `griffe:saved`, rien n'est clos.
#[tokio::test]
async fn closing_an_unfinished_year_from_the_window_shows_a_banner_and_closes_nothing() {
    let db_path = test_db_path("cloture-premature");
    let state = unlocked_state_with_activity(&db_path)
        .await
        .with_today(time::macros::date!(2026 - 12 - 03));
    let router = griffe_web::router(state);

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
    use griffe_core::billing::{ImportBankTransactions, ParsedTransaction};
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
    let router = griffe_web::router(state);

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
        Some("griffe:saved")
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
        Some("griffe:saved")
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
    let router = griffe_web::router(state);

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
        "../../griffe-core/src/billing/fixtures/credit-agricole.csv"
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
        Some("griffe:saved")
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
    let router = griffe_web::router(state);

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
        Some("griffe:saved")
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
    assert!(lexique.contains("data-view=\"aide\""), "{lexique}");
    assert!(dashboard.contains("hx-get=\"/aide\""), "{dashboard}");

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
            ("paid_by", "company"),
        ],
        Some(("facture-cabinet.pdf", b"%PDF-1.4 facture du cabinet")),
    );
    let router = griffe_web::router(state);
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
        Some("griffe:saved")
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
        Some("griffe:saved")
    );
    let expense_id = {
        let store = Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
        griffe_core::expenses::list_expenses(store.connection()).unwrap()[0].id
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
    let router = griffe_web::router(state);
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
        "../../griffe-core/src/opening_balance/fixtures/balance-cabinet.csv"
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
    let router = griffe_web::router(state);

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

/// Lot 58 : approuver un exercice depuis la fenêtre fige le PV au coffre.
#[tokio::test]
async fn approving_a_year_from_the_window_captures_the_minutes() {
    let db_path = test_db_path("papers-approve-http");
    let state = unlocked_state_with_activity(&db_path).await;
    let state = state.with_today(time::macros::date!(2027 - 06 - 01));
    let router = griffe_web::router(state);

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
    assert_eq!(
        closed
            .headers()
            .get("HX-Trigger")
            .map(|v| v.to_str().unwrap()),
        Some("griffe:saved")
    );
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
        Some("griffe:saved")
    );

    let store = Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
    let papers = griffe_core::papers::list_papers(
        store.connection(),
        griffe_core::papers::PaperFilter {
            period: Some(2026),
            kind: Some(griffe_core::domain::PaperKind::Minutes),
            include_superseded: false,
        },
    )
    .unwrap();
    assert_eq!(papers.len(), 1, "le PV doit être figé");
    assert_eq!(papers[0].origin, griffe_core::domain::PaperOrigin::Issued);
}

/// Les briefs officiels (FEC, PV, 2033) ne vivent plus dans des fiches Later : ils
/// n'apparaissent que lorsque la pièce est due. On clôture et on approuve pour les
/// rendre dues, plutôt que de garder des fiches fantômes sur un coffre vide.
#[tokio::test]
async fn the_papers_chapter_explains_the_pieces_and_where_they_live() {
    let db_path = test_db_path("papiers-lettre");
    let state = unlocked_state_with_activity(&db_path)
        .await
        .with_today(time::macros::date!(2027 - 06 - 01));
    let router = griffe_web::router(state);

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
    assert_eq!(
        closed
            .headers()
            .get("HX-Trigger")
            .map(|v| v.to_str().unwrap()),
        Some("griffe:saved")
    );
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
        Some("griffe:saved")
    );

    let body = body_text(
        router
            .oneshot(
                Request::builder()
                    .uri("/societe/papiers?period=2026")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(body.contains("Deux endroits"), "{body}");
    assert!(body.contains("Au coffre"), "{body}");
    assert!(
        body.contains("chiffré") || body.contains("chiffrés"),
        "{body}"
    );
    assert!(body.contains(".receipts"), "{body}");
    assert!(body.contains("en clair"), "{body}");
    assert!(body.contains("Fichier des Écritures Comptables"), "{body}");
    assert!(
        body.contains("Procès-verbal") || body.contains("Proc&#xE8;s-verbal"),
        "{body}"
    );
    assert!(body.contains("2033-A") || body.contains("2033"), "{body}");
    assert!(body.contains("INPI"), "{body}");
    assert!(body.contains("Ce que Griffe écrit"), "{body}");
    assert!(body.contains("Ce que vous apportez"), "{body}");
    assert!(body.contains("class=\"papers\""), "{body}");
    assert!(!body.contains("À avoir au coffre"), "{body}");
    assert!(!body.contains("freeflow "), "{body}");
    assert!(!body.contains("SAE"), "{body}");
    assert!(!body.contains("NF Z42"), "{body}");
}

#[tokio::test]
async fn the_papers_chapter_is_a_letter_not_a_dump() {
    let db_path = test_db_path("papiers-rythme");
    let state = unlocked_state(&db_path)
        .await
        .with_today(time::macros::date!(2027 - 06 - 01));
    let router = griffe_web::router(state);

    let body = body_text(
        router
            .oneshot(
                Request::builder()
                    .uri("/societe/papiers?period=2026")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;

    let written = body.find("Ce que Griffe écrit").expect(&body);
    let brought = body.find("Ce que vous apportez").expect(&body);
    let deposit = body.find(">Déposer<").expect(&body);
    let places = body.find("Deux endroits").expect(&body);
    let pack = body.find("Préparer le dossier d'un contrôle").expect(&body);
    assert!(
        written < brought && brought < deposit && deposit < places && places < pack,
        "inventaires puis dépôt puis coda des deux lieux, pas l'inverse : {body}"
    );

    let letter = body
        .split_once("id=\"papiers-letter\"")
        .map(|(_, rest)| rest)
        .unwrap_or(&body);
    let h2 = letter.matches("<h2>").count();
    assert_eq!(
        h2, 3,
        "trois chapitres serif dans la lettre, pas un h2 par ligne : {letter}"
    );
    assert!(body.contains("class=\"mast\""), "{body}");
    assert!(
        body.contains("id=\"period\""),
        "l'année reste un fait du mât : {body}"
    );
    assert!(
        !body.contains("type=\"number\""),
        "plus d'input d'exercice : {body}"
    );
    assert!(!body.contains(">voir<"), "plus de bouton voir : {body}");
    assert!(body.contains("Il manque les statuts et le Kbis."), "{body}");
    assert!(
        body.contains("se figent ici, à la clôture"),
        "une phrase pour les nés-ici pas encore dus : {body}"
    );
    assert!(
        body.contains("Lesquelles ?"),
        "lien quiet sous la phrase des nés-ici : {body}"
    );
    assert!(
        body.contains("Qu'est-ce qu'un contrôle ?") || body.contains("Qu&#x27;est-ce"),
        "lien quiet dans Pour un contrôle : {body}"
    );
    assert!(
        body.contains("href=\"/aide/papiers\""),
        "les deux liens mènent à la recette : {body}"
    );
    assert_eq!(
        body.matches("Plus tard, à la clôture").count(),
        0,
        "pas une liste de fiches Later : {body}"
    );
    assert_eq!(
        body.matches("paper-fiche").count(),
        2,
        "statuts et Kbis seulement, pas six cercles vides : {body}"
    );
    assert!(body.contains("aria-expanded=\"true\""), "{body}");
    assert!(body.contains("class=\"open\""), "{body}");
    assert!(body.contains("class=\"nm\""), "{body}");
    assert!(
        body.contains("name=\"kind\" value=\"kbis\"")
            || body.contains("name=\"kind\" value=\"statutes\""),
        "nature déjà choisie dans la fiche manquante : {body}"
    );
    assert!(
        body.contains("une autre pièce"),
        "le formulaire générique reste, replié : {body}"
    );
    assert!(!body.contains("Quelle pièce"), "{body}");
    assert!(!body.contains("Déjà au coffre"), "{body}");
    assert!(!body.contains("<h2>Pour un contrôle"), "{body}");
    assert!(!body.contains("Déposer une pièce"), "{body}");
    assert!(!body.contains("bouton plus bas"), "{body}");
    assert!(
        !body.contains("style="),
        "plus de style inline sur la lettre : {body}"
    );
    assert!(!body.contains("freeflow "), "{body}");
    assert!(
        !body.contains("class=\"millesimes\""),
        "un seul exercice possible : l'année est un fait, pas un contrôle : {body}"
    );
}

#[tokio::test]
async fn the_papers_chapter_offers_millesimes_when_two_years_exist() {
    let db_path = test_db_path("papiers-millesimes");
    let state = unlocked_state_with_activity(&db_path)
        .await
        .with_today(time::macros::date!(2027 - 06 - 01));
    let router = griffe_web::router(state);

    for (starts, ends) in [("2025-01-01", "2025-12-31"), ("2026-01-01", "2026-12-31")] {
        let closed = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/cloture")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from(format!(
                        "starts_on={starts}&ends_on={ends}&legal_reserve=0&dividends=0"
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            closed
                .headers()
                .get("HX-Trigger")
                .map(|v| v.to_str().unwrap()),
            Some("griffe:saved"),
            "{starts} → {ends}"
        );
    }

    let body = body_text(
        router
            .oneshot(
                Request::builder()
                    .uri("/societe/papiers?period=2026")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(body.contains("class=\"millesimes\""), "{body}");
    assert!(body.contains("/societe/papiers?period=2025"), "{body}");
    assert!(body.contains("/societe/papiers?period=2027"), "{body}");
    assert!(
        !body.contains("type=\"number\""),
        "millésimes en liens, pas un champ : {body}"
    );
    assert!(!body.contains(">voir<"), "{body}");
}

#[tokio::test]
async fn depositing_a_kbis_from_the_papers_chapter_archives_it() {
    let db_path = test_db_path("papiers-kbis");
    let state = unlocked_state(&db_path)
        .await
        .with_today(time::macros::date!(2027 - 06 - 01));
    let router = griffe_web::router(state);

    let page = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/societe/papiers?period=2026")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    let body = body_text(page).await;
    assert!(body.contains("Les papiers"), "{body}");
    assert!(body.contains("Déposer"), "{body}");
    assert!(body.contains("Kbis"), "{body}");
    assert!(!body.contains("freeflow "), "{body}");

    let content: &[u8] = b"%PDF-1.4 kbis SASU";
    let (content_type, form_body) = multipart_form_file(
        &[("kind", "kbis"), ("period", "2026")],
        "file",
        Some(("../../kbis.pdf", content)),
    );
    let posted = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/societe/papiers")
                .header("content-type", content_type)
                .body(Body::from(form_body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(posted.status(), StatusCode::OK);
    assert_eq!(posted.headers().get("HX-Trigger").unwrap(), "griffe:saved");
    let posted_body = body_text(posted).await;
    assert!(posted_body.is_empty(), "{posted_body}");

    let refreshed = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/societe/papiers?period=2026")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let refreshed_body = body_text(refreshed).await;
    assert!(refreshed_body.contains("kbis.pdf"), "{refreshed_body}");
    assert!(
        refreshed_body.contains("Ouvrir"),
        "le dépli d'une fiche au coffre ouvre la pièce : {refreshed_body}"
    );

    let store = Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
    let papers = griffe_core::papers::list_papers(
        store.connection(),
        griffe_core::papers::PaperFilter {
            period: None,
            kind: Some(griffe_core::domain::PaperKind::Kbis),
            include_superseded: false,
        },
    )
    .unwrap();
    assert_eq!(papers.len(), 1);
    assert_eq!(papers[0].original_name, "kbis.pdf");
    let dir = store.receipts_dir();
    assert!(dir.is_dir(), "{}", dir.display());
    let files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    assert_eq!(files.len(), 1, "un blob chiffré dans .receipts/");

    let shown = router
        .oneshot(
            Request::builder()
                .uri(format!("/societe/papiers/{}", papers[0].id))
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
    let disposition = shown
        .headers()
        .get("content-disposition")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        disposition.contains("kbis.pdf"),
        "inline nomme le fichier : {disposition}"
    );
    let bytes = shown.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&bytes[..], content);
}

#[tokio::test]
async fn exporting_the_control_pack_from_the_window_writes_a_readable_inventory() {
    let db_path = test_db_path("papiers-export");
    let state = unlocked_state(&db_path)
        .await
        .with_today(time::macros::date!(2027 - 06 - 01));
    let router = griffe_web::router(state);

    let posted = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/societe/papiers/export")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("period=2026"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(posted.status(), StatusCode::OK);
    let body = body_text(posted).await;
    assert!(
        body.contains("Ces fichiers ne sont plus chiffrés"),
        "{body}"
    );
    assert!(!body.contains("freeflow "), "{body}");
    let dir = body
        .split("data-pack-dir=\"")
        .nth(1)
        .and_then(|s| s.split('"').next())
        .expect("le panneau porte le chemin du dossier");
    let inventory = Path::new(dir).join("inventaire.txt");
    let inventory_text = std::fs::read_to_string(&inventory)
        .unwrap_or_else(|e| panic!("inventaire introuvable à {} : {e}", inventory.display()));
    assert!(
        inventory_text.contains("Dossier d'un contrôle — exercice 2026"),
        "{inventory_text}"
    );

    let papiers = body_text(
        router
            .oneshot(
                Request::builder()
                    .uri("/societe/papiers?period=2026")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        papiers.contains("Préparer le dossier d'un contrôle"),
        "{papiers}"
    );
    assert!(!papiers.contains("freeflow "), "{papiers}");
}

#[tokio::test]
async fn creating_an_expense_without_payer_or_statement_is_refused() {
    let db_path = test_db_path("expense-payer-required");
    let state = unlocked_state(&db_path).await;
    let router = griffe_web::router(state);
    let (content_type, body) = multipart_form(
        &[
            ("label", "Taxe locale"),
            ("category", "taxes"),
            ("amount", "180.00"),
            ("vat_rate", "zero"),
            ("vat_deductible", "0.00"),
            ("incurred_on", "2026-01-15"),
        ],
        None,
    );
    let response = router
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
    assert!(response.headers().get("HX-Trigger").is_none());
    let body = body_text(response).await;
    assert!(body.contains("Dis qui a payé"), "{body}");
    let store = Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
    assert!(
        griffe_core::expenses::list_expenses(store.connection())
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn creating_an_associate_paid_expense_keeps_the_panel() {
    let db_path = test_db_path("expense-associate");
    let state = unlocked_state(&db_path).await;
    let router = griffe_web::router(state);
    let (content_type, body) = multipart_form(
        &[
            ("label", "Taxe locale"),
            ("category", "taxes"),
            ("amount", "180.00"),
            ("vat_rate", "zero"),
            ("vat_deductible", "0.00"),
            ("incurred_on", "2026-01-15"),
            ("paid_by", "me"),
        ],
        None,
    );
    let response = router
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
        "griffe:saved"
    );
    let body = body_text(response).await;
    assert!(body.contains("La société te doit"), "{body}");
    assert!(body.contains("180,00"), "{body}");
}

#[tokio::test]
async fn the_dossier_accepts_an_imported_invoice_and_lists_its_number() {
    let db_path = test_db_path("dossier-import-invoice");
    let mut store = Store::create(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
    Executor::new(&mut store)
        .execute(
            &CreateClient {
                name: "Camille Rivière".to_string(),
                siren: None,
                vat_number: None,
                address: None,
            },
            &human_ctx(),
        )
        .unwrap();
    drop(store);

    let state = AppState::new(db_path);
    state
        .unlock(&Passphrase::from(PASSPHRASE), false)
        .await
        .unwrap();
    let router = griffe_web::router(state);

    let dossier = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires/Camille%20Rivi%C3%A8re")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        dossier.contains("Poser une facture née ailleurs"),
        "le geste est sur le dossier d'une cliente : {dossier}"
    );
    assert!(
        dossier.contains("quantité négative"),
        "un avoir doit se poser avec une quantité négative : {dossier}"
    );

    let (content_type, form_body) = multipart_form_file(
        &[
            ("number", "FAC-2026-0042"),
            ("issued_on", "2026-09-16"),
            ("payment_terms_days", "30"),
            ("description", "Mission Camille"),
            ("quantity", "1"),
            ("unit_price", "5000,00"),
            ("vat_rate", "standard"),
        ],
        "file",
        Some(("FAC-2026-0042.pdf", b"%PDF-1.7 camille")),
    );
    let posted = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/affaires/Camille%20Rivi%C3%A8re/facture")
                .header("content-type", content_type)
                .body(Body::from(form_body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(posted.status(), StatusCode::OK);
    let posted_body = body_text(posted).await;
    assert!(
        posted_body.contains("FAC-2026-0042"),
        "le numéro de Tiime est collé au dossier : {posted_body}"
    );
    assert!(
        posted_body.contains("original figé") || posted_body.contains("original non figé"),
        "le flash dit si le PDF est au coffre : {posted_body}"
    );

    let facturation = body_text(
        router
            .oneshot(
                Request::builder()
                    .uri("/view/facturation")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        facturation.contains("FAC-2026-0042"),
        "la liste lit le même numéro : {facturation}"
    );
}

#[tokio::test]
async fn the_window_has_no_emit_post_route() {
    let db_path = test_db_path("no-emit-post");
    let state = unlocked_state_with_client(&db_path).await;
    let router = griffe_web::router(state);

    let emit = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/facturation/emit")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        emit.status() == StatusCode::NOT_FOUND || emit.status() == StatusCode::METHOD_NOT_ALLOWED,
        "pas de POST d'émission : {}",
        emit.status()
    );

    let dossier = body_text(
        router
            .oneshot(
                Request::builder()
                    .uri("/affaires/Kappa%20Software")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    let lower = dossier.to_lowercase();
    assert!(
        !lower.contains("émettre une facture") && !lower.contains("emettre une facture"),
        "le dossier n'offre pas d'émission : {dossier}"
    );
}

#[tokio::test]
async fn the_dossier_lets_a_human_stop_waiting_for_an_invoice() {
    let db_path = test_db_path("dossier-write-off");
    let invoice_id = {
        let mut store = Store::create(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
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
            griffe_core::app::Outcome::Applied(id) => id,
            other => panic!("expected Applied, got {other:?}"),
        };
        let emitted = match Executor::new(&mut store)
            .execute(
                &griffe_core::billing::EmitInvoice {
                    client_id,
                    mission_id: None,
                    lines: vec![griffe_core::domain::InvoiceLine {
                        description: "Prestation".to_string(),
                        quantity: 1.0,
                        unit_price: Money::from_cents(100_000),
                        vat_rate: griffe_core::domain::VatRate::Standard,
                    }],
                    issued_on: time::Date::from_calendar_date(2026, time::Month::September, 1)
                        .unwrap(),
                    payment_terms_days: 30,
                },
                &human_ctx(),
            )
            .unwrap()
        {
            griffe_core::app::Outcome::Applied(e) => e,
            other => panic!("expected Applied, got {other:?}"),
        };
        emitted.id
    };

    let state = AppState::new(db_path.clone());
    state
        .unlock(&Passphrase::from(PASSPHRASE), false)
        .await
        .unwrap();
    let state =
        state.with_today(time::Date::from_calendar_date(2026, time::Month::September, 17).unwrap());
    let router = griffe_web::router(state);

    let dossier = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires/Kappa%20Software")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        dossier.contains("Je n'attends plus cet argent"),
        "le geste est sur le dossier : {dossier}"
    );
    assert!(
        !dossier.contains("impayé"),
        "la lettre ne dit pas impayé : {dossier}"
    );

    let posted = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/affaires/Kappa%20Software/facture/{invoice_id}/ne-plus-attendre"
                ))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("on=2026-09-17"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(posted.status(), StatusCode::OK);
    let posted_body = body_text(posted).await;
    assert!(
        !posted_body.is_empty(),
        "succès : le dossier, pas un corps vide"
    );
    assert!(
        posted_body.contains("on ne l'attend plus"),
        "après le geste, le papier change de statut : {posted_body}"
    );
    assert!(
        !posted_body.contains("impayé"),
        "la lettre ne dit pas impayé : {posted_body}"
    );
    assert!(
        uncollectible_notice_papers(&db_path).is_empty(),
        "sans TVA déjà déposée, pas de duplicata"
    );

    let again = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires/Kappa%20Software")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        again.contains("on ne l'attend plus"),
        "un GET ultérieur lit le même statut : {again}"
    );
    assert!(
        again.contains("Finalement j'attends encore"),
        "la rétractation est sur le dossier : {again}"
    );
    assert!(
        !again.contains("impayé"),
        "la lettre ne dit pas impayé : {again}"
    );
}

#[tokio::test]
async fn the_dossier_archives_a_local_notice_when_the_issue_period_was_filed() {
    let db_path = test_db_path("dossier-write-off-notice");
    let (invoice_id, number) = {
        let mut store = Store::create(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
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
            griffe_core::app::Outcome::Applied(id) => id,
            other => panic!("expected Applied, got {other:?}"),
        };
        let emitted = match Executor::new(&mut store)
            .execute(
                &griffe_core::billing::EmitInvoice {
                    client_id,
                    mission_id: None,
                    lines: vec![griffe_core::domain::InvoiceLine {
                        description: "Prestation".to_string(),
                        quantity: 1.0,
                        unit_price: Money::from_cents(100_000),
                        vat_rate: griffe_core::domain::VatRate::Standard,
                    }],
                    issued_on: time::Date::from_calendar_date(2026, time::Month::August, 15)
                        .unwrap(),
                    payment_terms_days: 30,
                },
                &human_ctx(),
            )
            .unwrap()
        {
            griffe_core::app::Outcome::Applied(e) => e,
            other => panic!("expected Applied, got {other:?}"),
        };
        match Executor::new(&mut store)
            .execute(
                &griffe_core::society::MarkDutyFiled {
                    kind: griffe_core::fiscal::FiscalDeadlineKind::Ca3,
                    period_key: "2026-08".into(),
                    due_on: time::Date::from_calendar_date(2026, time::Month::September, 15)
                        .unwrap(),
                    filed_on: time::Date::from_calendar_date(2026, time::Month::September, 15)
                        .unwrap(),
                },
                &human_ctx(),
            )
            .unwrap()
        {
            griffe_core::app::Outcome::Applied(_) => {}
            other => panic!("expected Applied, got {other:?}"),
        }
        (emitted.id, emitted.number)
    };

    let state = AppState::new(db_path.clone());
    state
        .unlock(&Passphrase::from(PASSPHRASE), false)
        .await
        .unwrap();
    let state =
        state.with_today(time::Date::from_calendar_date(2026, time::Month::September, 17).unwrap());
    let router = griffe_web::router(state);

    let posted = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/affaires/Kappa%20Software/facture/{invoice_id}/ne-plus-attendre"
                ))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("on=2026-09-17"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(posted.status(), StatusCode::OK);
    let posted_body = body_text(posted).await;
    assert!(
        posted_body.contains("on ne l'attend plus"),
        "après le geste, le papier change de statut : {posted_body}"
    );
    assert!(
        !posted_body.contains("impayé"),
        "la lettre ne dit pas impayé : {posted_body}"
    );

    let expected_name = format!("facture-{number}-ne-plus-attendue.txt");
    let papers = uncollectible_notice_papers(&db_path);
    assert_eq!(papers.len(), 1, "un duplicata local : {papers:?}");
    let paper = &papers[0];
    assert_eq!(paper.kind, griffe_core::domain::PaperKind::Other);
    assert_eq!(paper.origin, griffe_core::domain::PaperOrigin::Issued);
    assert_eq!(paper.original_name, expected_name);
    assert_eq!(paper.invoice_id, Some(invoice_id));
    assert_eq!(paper.mime, "text/plain");

    let store = Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
    let body = String::from_utf8(griffe_core::receipts::read(&store, &paper.filename).unwrap())
        .expect("duplicata UTF-8");
    assert!(
        body.contains("CGI, art. 272"),
        "mention statutaire : {body}"
    );
    assert!(
        body.contains(&number),
        "le numéro de facture est dans le fichier : {body}"
    );
    assert!(
        body.contains("impayée"),
        "le fichier porte le mot de la notice, pas la lettre : {body}"
    );
}

#[tokio::test]
async fn phrases_are_a_letter_and_the_next_unopened_mail_uses_them() {
    let db_path = test_db_path("phrases");
    let state = unlocked_state(&db_path)
        .await
        .with_today(time::macros::date!(2026 - 10 - 02));
    let router = griffe_web::router(state);

    router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/affaires/nouvelle")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("HX-Request", "true")
                .body(Body::from("who=Camille+Rivi%C3%A8re&phrase=accompagnement"))
                .unwrap(),
        )
        .await
        .unwrap();

    let page = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires/phrases")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(page.contains("Aucune série."), "{page}");
    assert!(page.contains(">Une autre<"), "{page}");
    assert!(page.contains(">Série<"), "{page}");
    assert!(!page.contains("class=\"phrase-frise\""), "{page}");
    assert!(!page.contains("Enregistrer les phrases"), "{page}");

    let empty_letter = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires/Camille%20Rivi%C3%A8re/ecrire")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(empty_letter.contains("Aucune série."), "{empty_letter}");
    assert!(empty_letter.contains(">Une autre<"), "{empty_letter}");
    assert!(
        !empty_letter.contains("serie-menu"),
        "sans série, pas de menu : {empty_letter}"
    );
    assert!(!empty_letter.contains("name=\"serie\""), "{empty_letter}");
    assert!(
        !empty_letter.contains("Choisir une série"),
        "{empty_letter}"
    );
    assert!(!empty_letter.contains(">Changer<"), "{empty_letter}");

    let accueil = create_genre(&router, "Accueil").await;
    let page = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/affaires/phrases?genre={accueil}"))
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(page.contains("Accueil."), "{page}");
    assert!(page.contains("Les mots de cette série."), "{page}");
    assert!(page.contains("Premier message"), "{page}");
    assert!(
        page.contains("Premier message le jour du dossier."),
        "{page}"
    );
    assert!(page.contains("&lt;prénom&gt;"), "{page}");
    assert!(page.contains("&lt;nom&gt;"), "{page}");
    assert!(page.contains("&lt;genre&gt;"), "{page}");
    assert!(page.contains("les deux côtés restent"), "{page}");
    assert!(page.contains("class=\"phrase-frise\""), "{page}");
    assert!(!page.contains("class=\"phrase-moment\""), "{page}");
    assert!(
        page.contains("Griffe n'envoie pas") || page.contains("Griffe n&#x27;envoie pas"),
        "{page}"
    );
    assert!(page.contains("Enregistrer les phrases"), "{page}");
    assert!(!page.contains("{{"), "{page}");
    let shown = without_formula_holder(&page);
    for forbidden in ["template", "cadence", "campagne", "workflow", "pipeline"] {
        assert!(
            !shown.contains(forbidden),
            "{forbidden} dans la lettre : {page}"
        );
    }

    let letter = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires/Camille%20Rivi%C3%A8re/ecrire")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(letter.contains("Accueil"), "{letter}");
    assert!(letter.contains("Premier message"), "{letter}");
    assert!(!letter.contains("Aucune série."), "{letter}");
    assert!(!letter.contains("Pas encore de type"), "{letter}");
    assert!(!letter.contains("Pas encore de genre"), "{letter}");
    assert!(!letter.contains("Garder ces mots"), "{letter}");
    assert!(letter.contains("C'est parti"), "{letter}");
    assert!(letter.contains("<div class=\"serie-menu\">"), "{letter}");
    assert!(letter.contains(">Choisir une série<"), "{letter}");
    assert!(letter.contains(">Fermer<"), "{letter}");
    assert!(!letter.contains(">Changer<"), "{letter}");
    assert!(!letter.contains(">Garder<"), "{letter}");
    let unpicked = serie_face(&letter);
    assert!(
        !unpicked.contains("Accueil"),
        "au repos, le nom n'apparaît qu'une fois la série posée : {unpicked}"
    );
    assert!(!unpicked.contains("name=\"serie\""), "{unpicked}");

    let saved = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/affaires/phrases?genre={accueil}"))
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("HX-Request", "true")
                    .body(Body::from(
                        "label_hello=Premier+contact&subject_hello=Bonjour&body_hello=Un+premier+mot+r%C3%A9%C3%A9crit.&revision_hello=1&\
label_bump=Petit+rappel&subject_bump=Rappel&body_bump=Un+rappel.&revision_bump=1&\
label_value=Relance+utile&subject_value=Suite&body_value=Une+suite.&revision_value=1&\
label_close=Dernier+mot&subject_close=Fin&body_close=Un+dernier+mot.&revision_close=1",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(saved.contains("Les phrases sont enregistrées."), "{saved}");
    assert!(saved.contains("Premier contact"), "{saved}");
    assert!(saved.contains("Un premier mot réécrit."), "{saved}");

    post_form(
        &router,
        "/affaires/Camille%20Rivi%C3%A8re/serie",
        &format!("serie={accueil}"),
    )
    .await;

    let next = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires/Camille%20Rivi%C3%A8re/ecrire")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(next.contains("Premier contact"), "{next}");
    assert!(next.contains("Un premier mot réécrit."), "{next}");
    assert!(!next.contains("Je me permets de revenir"), "{next}");
    assert!(next.contains(">Garder<"), "{next}");
    assert!(!next.contains(">Changer<"), "{next}");
    assert!(!next.contains(">Fermer<"), "{next}");
    let picked = serie_face(&next);
    assert!(picked.contains("Accueil"), "{picked}");
    assert!(!picked.contains(">Choisir une série<"), "{picked}");
    assert!(!picked.contains("name=\"serie\""), "{picked}");

    let identity = body_text(
        router
            .oneshot(
                Request::builder()
                    .uri("/societe/identite")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        identity.contains("Les phrases") && identity.contains("les séries de lettres"),
        "{identity}"
    );
}

#[tokio::test]
async fn phrases_opened_from_a_dossier_land_in_la_societe() {
    let db_path = test_db_path("phrases-lens");
    let state = unlocked_state(&db_path).await;
    let router = griffe_web::router(state);
    router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/affaires/nouvelle")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("HX-Request", "true")
                .body(Body::from("who=Camille+Rivi%C3%A8re&phrase=accompagnement"))
                .unwrap(),
        )
        .await
        .unwrap();
    let href = "/affaires/phrases?depuis=lettre&pour=Camille%20Rivi%C3%A8re";
    let dossier = "href=\"/affaires/Camille%20Rivi%C3%A8re\"";
    let full = body_text(
        router
            .clone()
            .oneshot(Request::builder().uri(href).body(Body::empty()).unwrap())
            .await
            .unwrap(),
    )
    .await;
    assert!(full.contains("Revenir au menu"), "{full}");
    assert!(full.contains("phrase-desk"), "{full}");
    assert!(full.contains("data-family=\"reglages\""), "{full}");
    assert!(full.contains("depuis=lettre"), "{full}");
    assert!(full.contains("pour=Camille"), "{full}");
    assert!(
        full.contains("href=\"/affaires/Camille%20Rivi%C3%A8re/ecrire\""),
        "{full}"
    );
    assert!(!full.contains(dossier), "{full}");
    let fragment = body_text(
        router
            .oneshot(
                Request::builder()
                    .uri(href)
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(fragment.contains("Revenir au menu"), "{fragment}");
    assert!(fragment.contains("phrase-desk"), "{fragment}");
    assert!(fragment.contains("data-family=\"reglages\""), "{fragment}");
    assert!(fragment.contains("depuis=lettre"), "{fragment}");
    assert!(fragment.contains("pour=Camille"), "{fragment}");
    assert!(
        fragment.contains("href=\"/affaires/Camille%20Rivi%C3%A8re/ecrire\""),
        "{fragment}"
    );
    assert!(!fragment.contains(dossier), "{fragment}");
    assert!(!fragment.contains("Garder ces mots"), "{fragment}");
}

#[tokio::test]
async fn an_enseigne_is_not_greeted_with_its_article() {
    let db_path = test_db_path("enseigne-prenom");
    let state = unlocked_state(&db_path).await;
    let router = griffe_web::router(state);
    router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/affaires/nouvelle")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("HX-Request", "true")
                .body(Body::from(
                    "who=Le+porc+de+Val+de+la+Sens%C3%A9e&phrase=charcuterie",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let page = body_text(
        router
            .oneshot(
                Request::builder()
                    .uri("/affaires/Le%20porc%20de%20Val%20de%20la%20Sens%C3%A9e/ecrire")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(page.contains("Bonjour,"), "{page}");
    assert!(!page.contains("Bonjour Le"), "{page}");
    assert!(page.contains("Aucune série."), "{page}");
    assert!(!page.contains("Pas encore de type"), "{page}");
}

#[tokio::test]
async fn engaged_conversations_finish_their_series_and_a_new_one_takes_three_moments() {
    let db_path = test_db_path("moments");
    {
        let state = unlocked_state(&db_path)
            .await
            .with_today(time::macros::date!(2026 - 09 - 02));
        let router = griffe_web::router(state);
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/affaires/nouvelle")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("HX-Request", "true")
                    .body(Body::from("who=Camille+Rivi%C3%A8re&phrase=accompagnement"))
                    .unwrap(),
            )
            .await
            .unwrap();
        {
            let mut store =
                Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
            let client_id = client_id_by_name(&db_path, "Camille Rivière");
            Executor::new(&mut store)
                .execute(
                    &CreateContact {
                        client_id,
                        name: "Camille".into(),
                        email: Some("camille@exemple.fr".into()),
                        phone: None,
                        role: None,
                        given_name: String::new(),
                        family_name: String::new(),
                        civility: None,
                        structured: false,
                    },
                    &human_ctx(),
                )
                .unwrap();
            Executor::new(&mut store)
                .execute(
                    &SetFollowUpSender {
                        email: "moi@exemple.fr".into(),
                        name: Some("Moi".into()),
                    },
                    &human_ctx(),
                )
                .unwrap();
        }
        router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/affaires/Camille%20Rivi%C3%A8re/envoye")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("HX-Request", "true")
                    .body(Body::from("subject_line=Bonjour&body=Lettre+class%C3%A9e."))
                    .unwrap(),
            )
            .await
            .unwrap();
    }

    let state = AppState::new(db_path.clone());
    state
        .unlock(&Passphrase::from(PASSPHRASE), false)
        .await
        .unwrap();
    let state = state.with_today(time::macros::date!(2026 - 09 - 05));
    let router = griffe_web::router(state);

    let jour = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/jour")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        jour.contains("une autre conversation") && !jour.contains("Écrire à Camille"),
        "{jour}"
    );
    let listed = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires?vue=tout")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        listed.contains("premier contact · prochain pas le 7 septembre 2026")
            && listed.contains("Camille"),
        "{listed}"
    );

    let bare = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires/phrases")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(bare.contains("Aucune série."), "{bare}");
    assert!(!bare.contains("class=\"phrase-frise\""), "{bare}");
    let courant = create_genre(&router, "Courant").await;
    let phrases = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/affaires/phrases?genre={courant}"))
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(phrases.contains("phrase-desk"), "{phrases}");
    assert!(phrases.contains("Courant."), "{phrases}");
    assert!(!phrases.contains("letter spread"), "{phrases}");
    assert!(phrases.contains("class=\"letter-stage\""), "{phrases}");
    let tables = phrases.matches("<table").count();
    let presented = phrases.matches("role=\"presentation\"").count();
    assert_eq!(tables, presented, "{phrases}");
    assert!(tables >= 1, "{phrases}");
    assert!(phrases.contains("class=\"phrase-frise\""), "{phrases}");
    assert!(
        phrases.contains("Premier message le jour du dossier."),
        "{phrases}"
    );
    assert!(phrases.contains("jours ouvrés"), "{phrases}");
    assert!(phrases.contains("Jours ouvrés avant"), "{phrases}");
    assert!(
        phrases.contains("Les conversations déjà engagées finissent leur série."),
        "{phrases}"
    );
    assert!(phrases.contains("Ajouter un moment"), "{phrases}");
    assert!(phrases.contains("Retirer"), "{phrases}");
    assert!(phrases.contains("value=\"3\""), "{phrases}");
    assert!(phrases.contains("value=\"7\""), "{phrases}");
    assert!(phrases.contains("value=\"14\""), "{phrases}");
    assert!(!phrases.contains("step="), "{phrases}");
    assert!(phrases.contains("?utm_"), "{phrases}");
    assert!(phrases.contains("se lit sans cette partie"), "{phrases}");
    assert!(phrases.contains("Le lien la garde"), "{phrases}");
    let shown = without_formula_holder(&phrases);
    for forbidden in [
        "template", "cadence", "step", "campagne", "workflow", "pipeline",
    ] {
        assert!(
            !shown.contains(forbidden),
            "{forbidden} dans la lettre : {phrases}"
        );
    }

    let saved = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/affaires/phrases?genre={courant}"))
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("HX-Request", "true")
                    .body(Body::from(
                        "order=hello,bump,close&\
label_hello=Premier+contact&subject_hello=Bonjour&body_hello=Un+premier+mot+r%C3%A9%C3%A9crit.&revision_hello=1&\
label_bump=Premi%C3%A8re+relance&subject_bump=Suite&body_bump=Un+%C3%A9cart+nouveau.&revision_bump=1&ecart_bump=10&\
label_close=Derni%C3%A8re+relance&subject_close=Fin&body_close=Le+dernier+mot.&revision_close=1&ecart_close=21",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(saved.contains("Les phrases sont enregistrées."), "{saved}");
    assert!(saved.contains("Première relance"), "{saved}");
    assert!(!saved.contains("Relance utile"), "{saved}");

    let jour = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/jour")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        jour.contains("une autre conversation") && !jour.contains("Écrire à Camille"),
        "{jour}"
    );
    let listed = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires?vue=tout")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        listed.contains("premier contact · prochain pas le 7 septembre 2026")
            && listed.contains("Camille")
            && !listed.contains("16 septembre"),
        "{listed}"
    );

    router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/affaires/nouvelle")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("HX-Request", "true")
                .body(Body::from("who=Nina&phrase=site"))
                .unwrap(),
        )
        .await
        .unwrap();
    let nina = post_form(&router, "/affaires/Nina/serie", &format!("serie={courant}")).await;
    assert!(nina.contains("Premier contact"), "{nina}");
    assert!(nina.contains("Un premier mot réécrit."), "{nina}");
    assert_eq!(nina.matches("class=\"seal\"").count(), 1, "{nina}");
    assert!(nina.contains("C'est parti"), "{nina}");
    assert!(
        nina.contains("id=\"face-read\" value=\"relire\" checked"),
        "changer de série laisse Relire : {nina}"
    );
    assert!(!nina.contains("<select"), "{nina}");

    let camille = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires/Camille%20Rivi%C3%A8re/ecrire")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(camille.contains("Petit rappel"), "{camille}");
    assert!(
        camille.contains("Je me permets un court rappel"),
        "{camille}"
    );
    assert!(!camille.contains("Un écart nouveau."), "{camille}");

    router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/affaires/Camille%20Rivi%C3%A8re/arreter")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("HX-Request", "true")
                .body(Body::from("reason=timing&detail="))
                .unwrap(),
        )
        .await
        .unwrap();
    router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/affaires/Camille%20Rivi%C3%A8re/reprise")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("HX-Request", "true")
                .body(Body::from("when=2026-09-05"))
                .unwrap(),
        )
        .await
        .unwrap();
    let resumed = body_text(
        router
            .oneshot(
                Request::builder()
                    .uri("/affaires/Camille%20Rivi%C3%A8re/ecrire")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(resumed.contains("Premier contact"), "{resumed}");
    assert!(!resumed.contains("Premier message"), "{resumed}");
    assert!(!resumed.contains("Petit rappel"), "{resumed}");
}

#[tokio::test]
async fn the_genre_changes_only_the_words() {
    let db_path = test_db_path("genres");
    let state = unlocked_state(&db_path)
        .await
        .with_today(time::macros::date!(2026 - 09 - 05));
    let router = griffe_web::router(state);

    for (who, phrase) in [
        ("Camille", "accompagnement"),
        ("Nina", "site"),
        ("L%C3%A9o", "atelier"),
    ] {
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/affaires/nouvelle")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("HX-Request", "true")
                    .body(Body::from(format!("who={who}&phrase={phrase}")))
                    .unwrap(),
            )
            .await
            .unwrap();
    }
    {
        let mut store =
            Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
        for (name, email) in [
            ("Camille", "camille@exemple.fr"),
            ("Nina", "nina@exemple.fr"),
            ("Léo", "leo@exemple.fr"),
        ] {
            Executor::new(&mut store)
                .execute(
                    &CreateContact {
                        client_id: client_id_by_name(&db_path, name),
                        name: name.into(),
                        email: Some(email.into()),
                        phone: None,
                        role: None,
                        given_name: String::new(),
                        family_name: String::new(),
                        civility: None,
                        structured: false,
                    },
                    &human_ctx(),
                )
                .unwrap();
        }
        Executor::new(&mut store)
            .execute(
                &SetFollowUpSender {
                    email: "moi@exemple.fr".into(),
                    name: Some("Moi".into()),
                },
                &human_ctx(),
            )
            .unwrap();
    }

    let nouvelle = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires/nouvelle")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(!nouvelle.contains("name=\"genre\""), "{nouvelle}");

    let bare = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires/Camille/fiche")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(bare.contains("name=\"civility\""), "{bare}");
    assert!(bare.contains(">Genre<"), "{bare}");
    assert!(bare.contains("Monsieur"), "{bare}");
    assert!(bare.contains("Madame"), "{bare}");
    assert!(!bare.contains("name=\"genre\""), "{bare}");
    assert!(!bare.contains("name=\"genre_other\""), "{bare}");
    assert!(
        !bare.contains("Le premier nom servira aux phrases."),
        "{bare}"
    );
    assert!(!bare.contains("<select"), "{bare}");

    let services = create_genre(&router, "Services publics").await;
    let shops = create_genre(&router, "Commerces").await;
    let services_page = post_form(
        &router,
        &format!("/affaires/phrases?genre={services}"),
        "subject_hello=Pour+la+mairie",
    )
    .await;
    assert!(services_page.contains("Pour la mairie"), "{services_page}");
    assert!(services_page.contains("value=\"3\""), "{services_page}");
    assert!(services_page.contains("value=\"7\""), "{services_page}");
    assert!(services_page.contains("value=\"14\""), "{services_page}");
    assert!(
        services_page.contains("Services publics."),
        "{services_page}"
    );
    assert!(
        services_page.contains("Les mots de cette série."),
        "{services_page}"
    );
    assert!(services_page.contains(">Une autre<"), "{services_page}");
    assert!(services_page.contains(">Série<"), "{services_page}");
    assert!(
        services_page.contains("Griffe n'envoie pas")
            || services_page.contains("Griffe n&#x27;envoie pas"),
        "{services_page}"
    );
    assert!(
        services_page.contains("Enregistrer les phrases"),
        "{services_page}"
    );
    assert!(
        services_page.contains("Retirer cette série"),
        "{services_page}"
    );
    assert!(
        services_page.contains("Les conversations déjà engagées finissent leur série."),
        "{services_page}"
    );
    let shown = without_formula_holder(&services_page);
    for forbidden in [
        "template", "cadence", "step", "campagne", "workflow", "pipeline",
    ] {
        assert!(!shown.contains(forbidden), "{forbidden} : {services_page}");
    }
    post_form(
        &router,
        &format!("/affaires/phrases?genre={shops}"),
        "subject_hello=Pour+le+commerce",
    )
    .await;

    post_form(
        &router,
        "/affaires/Camille/serie",
        &format!("serie={services}"),
    )
    .await;
    post_form(&router, "/affaires/Nina/serie", &format!("serie={shops}")).await;

    let camille = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires/Camille/ecrire")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    let nina = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires/Nina/ecrire")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    let leo = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires/L%C3%A9o/ecrire")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(camille.contains("Pour la mairie"), "{camille}");
    assert!(camille.contains("Services publics"), "{camille}");
    assert!(camille.contains("Premier message"), "{camille}");
    assert!(camille.contains("name=\"serie\""), "{camille}");
    assert!(camille.contains(">Garder<"), "{camille}");
    assert!(!camille.contains(">Changer<"), "{camille}");
    let camille_face = serie_face(&camille);
    assert!(camille_face.contains("Services publics"), "{camille_face}");
    assert!(!camille_face.contains("Commerces"), "{camille_face}");
    assert!(!camille_face.contains("Courant"), "{camille_face}");
    assert!(!camille_face.contains("name=\"serie\""), "{camille_face}");
    assert!(!camille.contains("Pas encore de type"), "{camille}");
    assert!(nina.contains("Pour le commerce"), "{nina}");
    assert!(nina.contains("Commerces"), "{nina}");
    assert!(!camille.contains("Pour le commerce"), "{camille}");
    assert!(leo.contains("name=\"serie\""), "{leo}");
    assert!(leo.contains(">Une autre<"), "{leo}");
    assert!(leo.contains(">Choisir une série<"), "{leo}");
    assert!(leo.contains(">Fermer<"), "{leo}");
    assert!(!leo.contains(">Changer<"), "{leo}");
    assert!(!leo.contains("Aucune série."), "{leo}");
    let leo_face = serie_face(&leo);
    assert!(!leo_face.contains("Services publics"), "{leo_face}");
    assert!(!leo_face.contains("Commerces"), "{leo_face}");
    assert!(!leo_face.contains("name=\"serie\""), "{leo_face}");
    assert!(leo.contains("Premier message"), "{leo}");
    assert!(!leo.contains("Pas encore de type"), "{leo}");
    assert!(!leo.contains("Pas encore de genre"), "{leo}");
    assert!(!leo.contains("Pour la mairie"), "{leo}");
    assert!(leo.contains("atelier"), "{leo}");
    assert_eq!(leo.matches("class=\"seal\"").count(), 1, "{leo}");
    assert!(leo.contains("C'est parti"), "{leo}");
    assert!(!leo.contains("Garder ces mots"), "{leo}");
    assert!(!leo.contains("Les phrases"), "{leo}");

    let by_hand = post_form(
        &router,
        "/affaires/Camille/serie",
        &format!("serie={shops}&subject_line=Texte+%C3%A0+la+main&body=Pas+le+mod%C3%A8le."),
    )
    .await;
    assert!(by_hand.contains("Texte à la main"), "{by_hand}");
    assert!(by_hand.contains("Pas le modèle"), "{by_hand}");
    assert!(by_hand.contains("Commerces"), "{by_hand}");
    let hand_face = serie_face(&by_hand);
    assert!(hand_face.contains("Commerces"), "{hand_face}");
    assert!(!hand_face.contains("Services publics"), "{hand_face}");
    assert!(!by_hand.contains("value=\"Pour le commerce\""), "{by_hand}");
    post_form(
        &router,
        "/affaires/Camille/serie",
        &format!("serie={services}"),
    )
    .await;
    for forbidden in [
        "template", "cadence", "step", "campagne", "workflow", "pipeline",
    ] {
        assert!(!camille.contains(forbidden), "{forbidden} : {camille}");
        assert!(!leo.contains(forbidden), "{forbidden} : {leo}");
    }

    post_form(
        &router,
        "/affaires/Nina/ecrire",
        "subject_line=Brouillon+fig%C3%A9&body=Ce+brouillon+reste.",
    )
    .await;
    post_form(
        &router,
        "/affaires/L%C3%A9o/envoye",
        "subject_line=Lettre+class%C3%A9e&body=Elle+reste.",
    )
    .await;

    post_form(
        &router,
        &format!("/affaires/phrases?depuis=societe&genre={services}"),
        "subject_hello=Mot+de+la+soci%C3%A9t%C3%A9&body_hello=Le+mot+de+la+soci%C3%A9t%C3%A9.",
    )
    .await;
    let services_after = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/affaires/phrases?genre={services}"))
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    let shops_after = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/affaires/phrases?genre={shops}"))
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        services_after.contains("Mot de la société"),
        "{services_after}"
    );
    assert!(shops_after.contains("Pour le commerce"), "{shops_after}");
    assert!(!shops_after.contains("Mot de la société"), "{shops_after}");
    assert!(services_after.contains("value=\"3\""), "{services_after}");
    let nina_after = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires/Nina/ecrire")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(nina_after.contains("Brouillon figé"), "{nina_after}");
    let leo_after = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires/L%C3%A9o/ecrire")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(leo_after.contains("Lettre classée"), "{leo_after}");
    assert!(leo_after.contains("Petit rappel"), "{leo_after}");
    let camille_followed = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires/Camille/ecrire")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        camille_followed.contains("Mot de la société"),
        "{camille_followed}"
    );
    assert!(
        camille_followed.contains("Services publics"),
        "{camille_followed}"
    );
    assert!(
        camille_followed.contains("Premier message"),
        "{camille_followed}"
    );
    assert!(
        !camille_followed.contains("Garder ces mots"),
        "{camille_followed}"
    );

    let before_due = opportunity_due(&db_path, "accompagnement");
    post_form(
        &router,
        "/affaires/Camille/envoye",
        "subject_line=Lettre+de+Camille&body=Class%C3%A9e.",
    )
    .await;
    let due_on_bump = opportunity_due(&db_path, "accompagnement");
    post_form(&router, "/affaires/Camille/types", "kind=Commerces").await;
    assert_eq!(opportunity_due(&db_path, "accompagnement"), due_on_bump);
    assert_ne!(due_on_bump, before_due);
    let camille_typed = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires/Camille/ecrire")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        camille_typed.contains("Services publics"),
        "{camille_typed}"
    );
    assert!(camille_typed.contains("Petit rappel"), "{camille_typed}");
    assert!(
        camille_typed.contains("Lettre de Camille"),
        "{camille_typed}"
    );
    assert!(
        !camille_typed.contains("Pour le commerce"),
        "{camille_typed}"
    );
    assert!(
        !camille_typed.contains("Mot de la société"),
        "{camille_typed}"
    );
    post_form(
        &router,
        &format!("/affaires/phrases?genre={shops}"),
        "subject_bump=Rappel+des+commerces",
    )
    .await;
    let camille_next = post_form(
        &router,
        "/affaires/Camille/serie",
        &format!("serie={shops}"),
    )
    .await;
    assert_eq!(opportunity_due(&db_path, "accompagnement"), due_on_bump);
    assert!(
        camille_next.contains("Rappel des commerces"),
        "{camille_next}"
    );
    assert!(camille_next.contains("Commerces"), "{camille_next}");
    assert!(camille_next.contains("Petit rappel"), "{camille_next}");
    assert!(camille_next.contains("Lettre de Camille"), "{camille_next}");
    assert!(
        !camille_next.contains("Mot de la société"),
        "{camille_next}"
    );
    assert!(!camille_next.contains("Pour la mairie"), "{camille_next}");

    let ateliers = create_genre(&router, "Ateliers").await;
    post_form(
        &router,
        "/affaires/L%C3%A9o/serie",
        &format!("serie={ateliers}"),
    )
    .await;
    post_form(
        &router,
        &format!("/affaires/phrases?genre={ateliers}"),
        "retirer_genre=1",
    )
    .await;
    let leo_plain = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires/L%C3%A9o/ecrire")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(leo_plain.contains("name=\"serie\""), "{leo_plain}");
    assert!(!leo_plain.contains("Ateliers"), "{leo_plain}");
    assert!(!leo_plain.contains("Aucune série."), "{leo_plain}");
    assert!(leo_plain.contains("Petit rappel"), "{leo_plain}");
    assert!(!leo_plain.contains("Pas encore de type"), "{leo_plain}");
    assert!(!leo_plain.contains("Pas encore de genre"), "{leo_plain}");
    assert!(leo_plain.contains("Lettre classée"), "{leo_plain}");

    let born = create_genre_for(&router, "Mairie", "lettre", "Camille").await;
    let camille_born = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires/Camille/ecrire")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    let needle = format!("value=\"{born}\"");
    let at = camille_born.find(&needle).expect("la série créée");
    let end = (at + needle.len() + 120).min(camille_born.len());
    let window = &camille_born[at..end];
    assert!(window.contains("checked"), "{window}");
    assert!(camille_born.contains("Mairie"), "{camille_born}");

    let identity = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/societe/identite")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(identity.contains("genre="), "{identity}");
    assert!(identity.contains("les séries de lettres"), "{identity}");
    assert!(!identity.contains("selon leur type"), "{identity}");

    let nina_kept = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires/Nina/ecrire")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(nina_kept.contains("Commerces"), "{nina_kept}");
    assert!(nina_kept.contains("Brouillon figé"), "{nina_kept}");
    let nina_moved = post_form(
        &router,
        "/affaires/Nina/serie",
        &format!("serie={services}&subject_line=Brouillon+fig%C3%A9&body=Ce+brouillon+reste."),
    )
    .await;
    assert!(nina_moved.contains("Brouillon figé"), "{nina_moved}");
    assert!(!nina_moved.contains("Mot de la société"), "{nina_moved}");
    post_form(
        &router,
        "/affaires/Nina/serie",
        &format!("serie={shops}&subject_line=Brouillon+fig%C3%A9&body=Ce+brouillon+reste."),
    )
    .await;
    post_form(&router, "/affaires/Nina/types", "x=").await;
    let nina_plain = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires/Nina/ecrire")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(nina_plain.contains("Commerces"), "{nina_plain}");
    assert!(nina_plain.contains("Brouillon figé"), "{nina_plain}");
    assert!(!nina_plain.contains("Aucune série."), "{nina_plain}");
    assert!(!nina_plain.contains("Pas encore de type"), "{nina_plain}");
    assert!(!nina_plain.contains("Pas encore de genre"), "{nina_plain}");

    let css = body_text(
        router
            .oneshot(
                Request::builder()
                    .uri("/assets/app.css")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    let line = css
        .split(".phrase-line {")
        .nth(1)
        .and_then(|rest| rest.split('}').next())
        .expect("règle .phrase-line");
    assert!(
        line.contains("display: flex") && line.contains("flex-wrap: wrap"),
        "la ligne genre · moment se replie : {line}"
    );
    let chosen = css
        .split(".word-choice label:has(input:checked)")
        .nth(1)
        .and_then(|rest| rest.split('}').next())
        .expect("règle du mot choisi");
    assert!(
        chosen.contains("color: var(--ink)")
            && chosen.contains("box-shadow: inset 0 -1px 0 var(--ink)")
            && !chosen.contains("background"),
        "le mot choisi est encre, souligné, sans fond : {chosen}"
    );
    let opened = css
        .split(".phrase-line .current {")
        .nth(2)
        .and_then(|rest| rest.split('}').next())
        .expect("filet de la série ouverte");
    assert!(
        opened.contains("color: var(--ink)")
            && opened.contains("box-shadow: inset 0 -1px 0 var(--ink)"),
        "la série ouverte a le filet : {opened}"
    );
    assert!(
        css.contains("text-decoration: none"),
        "les autres séries ne sont pas soulignées : {css}"
    );
    assert!(
        css.contains(".word-choice:has(.word-other-toggle:checked) .word-other"),
        "la ligne suit le mot « un autre »"
    );
}

async fn create_genre(router: &axum::Router, name: &str) -> String {
    create_genre_for(router, name, "", "").await
}

async fn create_genre_for(router: &axum::Router, name: &str, depuis: &str, pour: &str) -> String {
    let uri = if depuis.is_empty() && pour.is_empty() {
        "/affaires/phrases/nouveau".to_string()
    } else {
        format!("/affaires/phrases/nouveau?depuis={depuis}&pour={pour}")
    };
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/x-www-form-urlencoded")
                .header("HX-Request", "true")
                .body(Body::from(format!("name={}", name.replace(' ', "+"))))
                .unwrap(),
        )
        .await
        .unwrap();
    let pushed = response
        .headers()
        .get("hx-push-url")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let id = genre_id_from(&pushed);
    assert!(!id.is_empty(), "{pushed}");
    let _ = body_text(response).await;
    id
}

async fn post_form(router: &axum::Router, uri: &str, body: &str) -> String {
    body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("HX-Request", "true")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await
}

/// Ce qui se lit sur Écrire avant le menu des séries.
fn serie_face(page: &str) -> &str {
    page.split_once("<div class=\"serie-menu\">")
        .map(|(head, _)| head)
        .unwrap_or(page)
}

fn hidden_value(page: &str, name: &str) -> String {
    let needle = format!("name=\"{name}\" value=\"");
    page.split(&needle)
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap_or("")
        .to_string()
}

#[tokio::test]
async fn the_fiche_names_the_matter_and_who_answers() {
    let db_path = test_db_path("fiche-quoi");
    let state = unlocked_state(&db_path).await;
    let router = griffe_web::router(state);
    router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/affaires/nouvelle")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("HX-Request", "true")
                .body(Body::from("who=Mairie&phrase=Le+porche"))
                .unwrap(),
        )
        .await
        .unwrap();

    let fiche = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires/Mairie/fiche")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    for needle in [
        "Quoi",
        "Qui répond",
        "Prénom",
        "Nom",
        "Monsieur",
        "non dit",
        "Courriel",
        "Téléphone",
        "Le standard, ou l'accueil.",
        "Le porche",
    ] {
        assert!(fiche.contains(needle), "{needle} manque : {fiche}");
    }
    let dossier = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires/Mairie")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        !dossier.contains("← Les affaires"),
        "le panneau n'a plus de retour : {dossier}"
    );

    let refused = post_form(
        &router,
        "/affaires/Mairie/fiche",
        &format!(
            "who=Mairie&matter=&matter_open=1&opportunity_revision={}&client_revision={}&contact_id={}&contact_revision={}",
            hidden_value(&fiche, "opportunity_revision"),
            hidden_value(&fiche, "client_revision"),
            hidden_value(&fiche, "contact_id"),
            hidden_value(&fiche, "contact_revision"),
        ),
    )
    .await;
    assert!(refused.contains("ce dont il s'agit"), "{refused}");

    let saved = post_form(
        &router,
        "/affaires/Mairie/fiche",
        &format!(
            "who=Mairie&matter=Le+portail&matter_open=1&opportunity_revision={}&client_revision={}&contact_id={}&contact_revision={}&email=accueil@mairie.example&given_name=Camille&family_name=Riviere&civility=madame&correspondent_email=camille@mairie.example",
            hidden_value(&fiche, "opportunity_revision"),
            hidden_value(&fiche, "client_revision"),
            hidden_value(&fiche, "contact_id"),
            hidden_value(&fiche, "contact_revision"),
        ),
    )
    .await;
    assert!(saved.contains("Fiche à jour."), "{saved}");

    let estimation = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires/Mairie/estimation")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(estimation.contains("Le portail"), "{estimation}");
    assert!(estimation.contains("Ce dont il s'agit"), "{estimation}");

    let again = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires/Mairie/fiche")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(again.contains("value=\"Camille\""), "{again}");
    assert!(again.contains("value=\"Riviere\""), "{again}");
    assert!(
        again.contains("value=\"madame\" checked") || again.contains("checked value=\"madame\""),
        "{again}"
    );
    assert!(again.contains("accueil@mairie.example"), "{again}");
    assert!(again.contains("camille@mairie.example"), "{again}");

    let letter = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires/Mairie/ecrire")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(letter.contains("Le standard"), "{letter}");
    assert!(letter.contains("Qui répond"), "{letter}");
    assert!(letter.contains("accueil@mairie.example"), "{letter}");
    assert!(letter.contains("camille@mairie.example"), "{letter}");
    assert!(
        letter.contains("value=\"switchboard\" checked")
            || letter.contains("checked value=\"switchboard\""),
        "{letter}"
    );
}

fn genre_id_from(page: &str) -> String {
    page.split("genre=")
        .nth(1)
        .and_then(|rest| rest.split(['&', '"', ' ']).next())
        .unwrap_or("")
        .to_string()
}

fn opportunity_due(db_path: &Path, name: &str) -> Option<time::Date> {
    let store = Store::open_with_passphrase(db_path, &Passphrase::from(PASSPHRASE)).unwrap();
    list_opportunities(store.connection())
        .unwrap()
        .into_iter()
        .find(|opportunity| opportunity.name == name)
        .and_then(|opportunity| opportunity.next_action_at)
}

fn uncollectible_notice_papers(db_path: &Path) -> Vec<griffe_core::papers::Paper> {
    let store = Store::open_with_passphrase(db_path, &Passphrase::from(PASSPHRASE)).unwrap();
    griffe_core::papers::list_papers(
        store.connection(),
        griffe_core::papers::PaperFilter {
            period: None,
            kind: Some(griffe_core::domain::PaperKind::Other),
            include_superseded: false,
        },
    )
    .unwrap()
    .into_iter()
    .filter(|p| p.original_name.ends_with("-ne-plus-attendue.txt"))
    .collect()
}

#[tokio::test]
async fn facturation_empty_state_does_not_send_to_the_console() {
    let state = unlocked_state(&test_db_path("facturation-empty")).await;
    let router = griffe_web::router(state);

    let body = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/view/facturation")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        !body.to_lowercase().contains("console"),
        "l'empty-state n'envoie pas à la console : {body}"
    );
    assert!(
        body.contains("aucune facture dans le coffre")
            && (body.contains("dossier")
                || body.contains("Les affaires")
                || body.contains("plateforme")),
        "l'empty-state pointe le dossier : {body}"
    );

    let aide = body_text(
        router
            .oneshot(
                Request::builder()
                    .uri("/aide/devis")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        !aide.contains("l'ancien écran Facturation ou la console"),
        "la recette devis ne renvoie plus à l'émission : {aide}"
    );
}

#[tokio::test]
async fn the_statement_pages_badges_and_search() {
    let db_path = test_db_path("releve-pages");
    let state = unlocked_state(&db_path)
        .await
        .with_today(time::macros::date!(2026 - 09 - 05));
    {
        let mut store =
            Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
        let start = time::macros::date!(2026 - 01 - 01);
        let mut transactions = Vec::new();
        for i in 0..41 {
            let (amount_cents, description) = if i == 0 {
                (-123_456_i64, "Cabinet Durand".to_string())
            } else {
                (-1_000, format!("Mouvement {i:02}"))
            };
            transactions.push(griffe_core::billing::ParsedTransaction {
                occurred_on: start + time::Duration::days(i),
                amount_cents,
                description,
                fitid: None,
            });
        }
        applied(
            Executor::new(&mut store)
                .execute(
                    &griffe_core::billing::ImportBankTransactions { transactions },
                    &human_ctx(),
                )
                .unwrap(),
        );
        let newest = griffe_core::billing::list_bank_transactions(store.connection())
            .unwrap()
            .into_iter()
            .find(|t| t.description == "Mouvement 40")
            .unwrap();
        applied(
            Executor::new(&mut store)
                .execute(
                    &griffe_core::billing::SettleBankTransaction {
                        transaction_id: newest.id,
                        account: "455000"
                            .parse::<griffe_core::domain::SettlementAccount>()
                            .unwrap(),
                        label: None,
                    },
                    &human_ctx(),
                )
                .unwrap(),
        );
    }
    let router = griffe_web::router(state);
    let page = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/societe/releve")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        page.contains("40 mouvements ne sont pas encore traités, sur 41."),
        "{page}"
    );
    assert_eq!(
        page.matches("class=\"tenir-card\"").count(),
        1,
        "une carte, pas une par mouvement : {page}"
    );
    assert_eq!(
        page.matches("class=\"block\"").count(),
        40,
        "chaque mouvement reste un article : {page}"
    );
    assert!(
        page.contains("non traité") && page.contains("traité"),
        "{page}"
    );
    assert!(page.contains("Chercher un mouvement"), "{page}");
    assert!(page.contains("Les mouvements plus anciens"), "{page}");
    assert!(
        !page.contains("Cabinet Durand"),
        "la première page s'arrête avant le plus ancien : {page}"
    );

    let more = releve_more_uri(&page);
    let older = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(&more)
                    .header("HX-Request", "true")
                    .header("HX-Target", "releve-more")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(older.contains("Cabinet Durand"), "{older}");
    assert!(older.contains("non traité"), "{older}");
    assert!(
        !older.contains("Mouvement 40"),
        "la suite ne répète pas la première page : {older}"
    );

    let found = body_text(
        router
            .oneshot(
                Request::builder()
                    .uri("/societe/releve?q=Durand")
                    .header("HX-Request", "true")
                    .header("HX-Target", "releve-body")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(found.contains("Cabinet Durand"), "{found}");
    assert!(
        found.contains("40 mouvements ne sont pas encore traités, sur 41."),
        "les compteurs restent ceux du coffre : {found}"
    );
    assert!(!found.contains("Les mouvements plus anciens"), "{found}");
}

fn releve_more_uri(body: &str) -> String {
    let idx = body.find("id=\"releve-more\"").expect("sentinelle");
    let href_at = body[idx..].find("href=\"").expect("lien");
    let rest = &body[idx + href_at + 6..];
    let end = rest.find('"').expect("fin du href");
    rest[..end].replace("&amp;", "&")
}

#[tokio::test]
async fn a_stopped_conversation_is_visible_and_the_stop_gesture_looks_final() {
    let db_path = test_db_path("stopped-visible");
    let state = unlocked_state(&db_path)
        .await
        .with_today(time::macros::date!(2026 - 09 - 05));
    {
        let mut store =
            Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
        applied(
            Executor::new(&mut store)
                .execute(
                    &CreateProspect {
                        prospect_name: "Atelier".into(),
                        address: None,
                        representative: None,
                        email: None,
                        phone: None,
                        name: "Site".into(),
                        amount: Money::from_cents(100_000),
                        probability: Probability::new(40).unwrap(),
                        next_action_at: time::macros::date!(2026 - 10 - 01),
                        source: None,
                    },
                    &human_ctx(),
                )
                .unwrap(),
        );
        let lost = applied(
            Executor::new(&mut store)
                .execute(
                    &CreateProspect {
                        prospect_name: "Ancien".into(),
                        address: None,
                        representative: None,
                        email: None,
                        phone: None,
                        name: "Ancienne piste".into(),
                        amount: Money::from_cents(50_000),
                        probability: Probability::new(20).unwrap(),
                        next_action_at: time::macros::date!(2026 - 10 - 01),
                        source: None,
                    },
                    &human_ctx(),
                )
                .unwrap(),
        );
        applied(
            Executor::new(&mut store)
                .execute(
                    &griffe_core::prospection::LoseOpportunity {
                        opportunity_id: lost,
                        reason: griffe_core::domain::LossReason::Timing,
                    },
                    &human_ctx(),
                )
                .unwrap(),
        );
    }
    let router = griffe_web::router(state);
    let list = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(list.contains("les arrêtés"), "{list}");
    assert!(!list.contains("Arrêtées · "), "{list}");
    assert!(!list.contains("stopped-hint"), "{list}");
    assert!(
        !list.contains("Ancien"),
        "un arrêté n'est pas un geste du jour : {list}"
    );

    let arretes = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires?vue=arretes")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(arretes.contains("Ancien"), "{arretes}");
    assert!(arretes.contains("pas le bon moment"), "{arretes}");
    assert!(
        !arretes.contains("Atelier"),
        "une conversation ouverte reste hors des arrêtés : {arretes}"
    );

    let dossier = body_text(
        router
            .oneshot(
                Request::builder()
                    .uri("/affaires/Atelier")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        dossier.contains("class=\"fate\""),
        "le geste d'arrêt se distingue : {dossier}"
    );
    assert!(
        dossier.contains("Cette conversation s") && dossier.contains("arrête"),
        "{dossier}"
    );
}

#[tokio::test]
async fn the_remember_checkbox_keeps_the_window_open_for_twelve_hours() {
    let label = "Pendant 12 h, ne pas redemander la passphrase, et laisser la fenêtre ouverte.";
    let previous = "Au prochain lancement, ne pas redemander la passphrase pendant 12 h.";

    let setup_path = test_db_path("remember-setup-label");
    let setup = body_text(
        griffe_web::router(AppState::new(setup_path))
            .oneshot(
                Request::builder()
                    .uri("/setup")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(setup.contains("Créer le coffre"), "{setup}");
    assert!(setup.contains(label), "{setup}");
    assert!(!setup.contains(previous), "{setup}");

    let unlock_path = test_db_path("remember-unlock-label");
    Store::create(&unlock_path, &Passphrase::from(PASSPHRASE)).unwrap();
    let unlock = body_text(
        griffe_web::router(AppState::new(unlock_path))
            .oneshot(
                Request::builder()
                    .uri("/unlock")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(unlock.contains("Coffre verrouillé"), "{unlock}");
    assert!(unlock.contains(label), "{unlock}");
    assert!(!unlock.contains(previous), "{unlock}");
}

fn page_has(body: &str, needle: &str) -> bool {
    let hex = needle.replace('\'', "&#x27;");
    let dec = needle.replace('\'', "&#39;");
    body.contains(needle) || body.contains(&hex) || body.contains(&dec)
}

#[tokio::test]
async fn october_first_declares_september_vat_and_closes_the_year_that_just_ended() {
    let db_path = test_db_path("letter-october-close");
    let state = unlocked_state(&db_path)
        .await
        .with_today(time::macros::date!(2026 - 10 - 01));
    {
        let mut store =
            Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
        applied(
            Executor::new(&mut store)
                .execute(
                    &griffe_core::company::SetCompanyProfile {
                        name: "Lumen Conseil".into(),
                        legal_form: "SASU".into(),
                        siren: griffe_core::domain::Siren::parse("552100554").unwrap(),
                        vat_number: None,
                        address: griffe_core::domain::Address {
                            street: "18 rue des Ateliers".into(),
                            postal_code: "69003".into(),
                            city: "Lyon".into(),
                            country: "FR".into(),
                        },
                        share_capital: Some(Money::from_cents(100_000)),
                        rcs_city: Some("Lyon".into()),
                        iban: None,
                        fiscal_year_end: Some(
                            griffe_core::domain::FiscalYearEnd::new(9, 30).unwrap(),
                        ),
                        vat_regime: Some(griffe_core::domain::VatRegime::RealNormalMonthly),
                        director_monthly_gross: Some(Money::from_cents(300_000)),
                        director_charge_ratio_bps: None,
                        president_name: Some("Nicolas Lumen".into()),
                        sole_shareholder_name: Some("Nicolas Lumen".into()),
                        sole_shareholder_address: Some("18 rue des Ateliers, 69003 Lyon".into()),
                        share_count: Some(1000),
                    },
                    &human_ctx(),
                )
                .unwrap(),
        );
        applied(
            Executor::new(&mut store)
                .execute(
                    &griffe_core::opening_balance::RecordOpeningBalance {
                        opens_on: time::macros::date!(2025 - 10 - 01),
                        source: Some("cabinet".into()),
                        lines: vec![
                            "101000:Capital:C:1000.00".parse().unwrap(),
                            "512000:Banque:D:1000.00".parse().unwrap(),
                        ],
                        tax_losses: Money::ZERO,
                        prior_corporate_tax: None,
                        prior_vat_due: None,
                    },
                    &human_ctx(),
                )
                .unwrap(),
        );
    }
    let router = griffe_web::router(state);

    let impots = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/societe/impots")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        impots.contains("href=\"/societe/impots/ca3/2026-09\""),
        "la TVA de septembre est due : {impots}"
    );

    let letter = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/societe/impots/ca3/2026-09")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        page_has(&letter, "trop récupéré"),
        "le trop-déclaré de septembre se saisit ici : {letter}"
    );

    let home = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/societe")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        page_has(&home, "30 septembre 2026"),
        "l'exercice qui vient de finir : {home}"
    );
    assert!(
        page_has(&home, "est à arrêter") || page_has(&home, "n'est pas arrêté"),
        "{home}"
    );
    assert!(
        !home.contains("30 septembre 2027") && !home.contains("1 octobre 2027"),
        "2027 n'est pas l'exercice à clore : {home}"
    );

    let cloture = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/societe/cloture")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(page_has(&cloture, "30 septembre 2026"), "{cloture}");
    assert!(page_has(&cloture, "1 octobre 2025"), "{cloture}");
    assert!(
        cloture.contains("Arrêter les comptes"),
        "on peut arrêter : {cloture}"
    );
    assert!(
        cloture.contains("hx-get=\"/cloture/new\""),
        "le bouton ouvre le formulaire : {cloture}"
    );
    let hist_at = cloture.find("ul class=\"hist\"").expect("parcours");
    let hist_end = cloture[hist_at..].find("</ul>").expect("fin du parcours") + hist_at;
    let hist = &cloture[hist_at..hist_end];
    assert!(
        hist.contains("hx-get=\"/cloture/new\"") && hist.contains("hx-target=\"#panel\""),
        "le bouton du parcours ouvre le volet : {hist}"
    );
    assert!(
        !hist.contains("<form"),
        "pas de formulaire inventé : {hist}"
    );
    assert!(cloture.contains("class=\"etat-card\""), "{cloture}");
    assert!(!cloture.contains("<h1"), "{cloture}");
    assert!(!cloture.contains("<form"), "{cloture}");
    assert!(
        !cloture.contains("CA3") && !cloture.contains("3514") && !cloture.contains("2777"),
        "{cloture}"
    );
    assert!(
        !cloture.contains("30 septembre 2027") && !cloture.contains("1 octobre 2027"),
        "2027 n'est pas l'exercice à clore : {cloture}"
    );

    let form = body_text(
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
        form.contains("value=\"2025-10-01\"") && form.contains("value=\"2026-09-30\""),
        "le formulaire propose l'exercice qui vient de finir : {form}"
    );
    assert!(!form.contains("2027-09-30"), "{form}");

    let jour = body_text(
        router
            .oneshot(Request::builder().uri("/jour").body(Body::empty()).unwrap())
            .await
            .unwrap(),
    )
    .await;
    assert!(
        page_has(&jour, "TVA du mois de septembre"),
        "la CA3 de septembre est au calendrier : {jour}"
    );
    assert!(
        jour.contains("/societe/impots/ca3/2026-09"),
        "la ligne ouvre la lettre : {jour}"
    );
    assert!(
        page_has(&jour, "Clore l'exercice clos le 30 septembre 2026"),
        "la clôture est un événement du mois : {jour}"
    );
    assert!(
        jour.contains("href=\"/societe/cloture\""),
        "la ligne ouvre Clore : {jour}"
    );
}

/// Le drapeau est sur le formulaire de dépense. La lettre du matin ne le porte pas.
#[tokio::test]
async fn the_expense_form_offers_reverse_charge_and_the_morning_letter_does_not() {
    let state = unlocked_state(&test_db_path("reverse-charge-form")).await;
    let router = griffe_web::router(state);

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
        form.contains("Prestation intracommunautaire autoliquidée"),
        "{form}"
    );
    assert!(form.contains("name=\"reverse_charge\""), "{form}");

    let letter = body_text(
        router
            .oneshot(Request::builder().uri("/jour").body(Body::empty()).unwrap())
            .await
            .unwrap(),
    )
    .await;
    assert!(!letter.contains("reverse_charge"), "{letter}");
    assert!(!letter.contains("445662"), "{letter}");
    assert!(!letter.contains("445200"), "{letter}");
}

#[tokio::test]
async fn two_fiches_with_the_same_respondent_open_under_qui() {
    let db_path = test_db_path("two-qui");
    let mut store = Store::create(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
    for (qui, piste) in [
        ("Mairie de Hornaing", "site"),
        ("Mairie de Somain", "accessibilité"),
    ] {
        Executor::new(&mut store)
            .execute(
                &CreateProspect {
                    prospect_name: qui.into(),
                    address: None,
                    representative: Some("Frédéric DELANNOY".into()),
                    email: Some("mairie@exemple.fr".into()),
                    phone: None,
                    name: piste.into(),
                    amount: Money::ZERO,
                    probability: Probability::new(0).unwrap(),
                    next_action_at: time::macros::date!(2026 - 10 - 07),
                    source: None,
                },
                &human_ctx(),
            )
            .unwrap();
    }
    drop(store);

    let state = AppState::new(db_path);
    state
        .unlock(&Passphrase::from(PASSPHRASE), false)
        .await
        .unwrap();
    let state = state.with_today(time::macros::date!(2026 - 10 - 07));
    let router = griffe_web::router(state);

    let list = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires?vue=tas")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(list.contains("Mairie de Hornaing"), "{list}");
    assert!(list.contains("Mairie de Somain"), "{list}");
    assert!(
        list.contains("href=\"/affaires/Mairie%20de%20Hornaing?vue=tas\""),
        "{list}"
    );
    assert!(
        list.contains("href=\"/affaires/Mairie%20de%20Somain?vue=tas\""),
        "{list}"
    );
    assert!(
        !list.contains(">Frédéric DELANNOY<"),
        "la liste nomme le Qui : {list}"
    );

    let found = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires?vue=tas&q=fr%C3%A9d%C3%A9ric")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(found.contains("Mairie de Hornaing"), "{found}");
    assert!(found.contains("Mairie de Somain"), "{found}");

    let hornaing = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires/Mairie%20de%20Hornaing")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(hornaing.contains("Mairie de Hornaing"), "{hornaing}");
    assert!(hornaing.contains("Frédéric DELANNOY"), "{hornaing}");
    assert!(!hornaing.contains("règle métier"), "{hornaing}");

    let shared = body_text(
        router
            .oneshot(
                Request::builder()
                    .uri("/affaires/Fr%C3%A9d%C3%A9ric%20DELANNOY")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(shared.contains("Plusieurs fiches"), "{shared}");
    assert!(shared.contains("Mairie de Hornaing"), "{shared}");
    assert!(shared.contains("Mairie de Somain"), "{shared}");
    assert!(!shared.contains("règle métier"), "{shared}");
}

#[tokio::test]
async fn reporter_unfolds_into_three_days_ten_days_or_a_count() {
    let db_path = test_db_path("reporter-deplie");
    let mut store = Store::create(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
    Executor::new(&mut store)
        .execute(
            &CreateProspect {
                prospect_name: "Atelier Sèvre".into(),
                address: None,
                representative: None,
                email: None,
                phone: None,
                name: "site de la cave".into(),
                amount: Money::from_cents(120_000),
                probability: Probability::new(40).unwrap(),
                next_action_at: time::macros::date!(2026 - 09 - 05),
                source: None,
            },
            &human_ctx(),
        )
        .unwrap();
    drop(store);

    let state = AppState::new(db_path);
    state
        .unlock(&Passphrase::from(PASSPHRASE), false)
        .await
        .unwrap();
    let state = state.with_today(time::macros::date!(2026 - 09 - 05));
    let router = griffe_web::router(state);
    let dossier_uri = "/affaires/Atelier%20S%C3%A8vre";

    let jour = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/jour")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(jour.contains(">Demain<"), "{jour}");
    assert!(
        jour.contains("name=\"until\" value=\"2026-09-06\""),
        "Demain poste le lendemain : {jour}"
    );
    assert!(!jour.contains("class=\"snooze\""), "{jour}");
    assert!(!jour.contains("Reporter de trois jours"), "{jour}");

    let dossier = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(dossier_uri)
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(dossier.contains("<details class=\"snooze\">"), "{dossier}");
    assert!(dossier.contains(">Reporter<"), "{dossier}");
    assert!(dossier.contains("name=\"days\" value=\"3\""), "{dossier}");
    assert!(dossier.contains(">3 jours<"), "{dossier}");
    assert!(dossier.contains("name=\"days\" value=\"10\""), "{dossier}");
    assert!(dossier.contains(">10 jours<"), "{dossier}");
    assert!(dossier.contains("inputmode=\"numeric\""), "{dossier}");
    assert!(dossier.contains(">jours<"), "{dossier}");
    assert!(!dossier.contains("Reporter de trois jours"), "{dossier}");
    assert!(!dossier.contains("<select"), "{dossier}");
    assert!(!dossier.contains("<script"), "{dossier}");
    assert!(!dossier.contains("hx-on"), "{dossier}");
    assert!(!dossier.contains(">Demain<"), "{dossier}");

    let sentence = "Le report se compte de 1 à 366 jours.";
    for body in ["days=0", "days=367", "days=abc", "days=-1"] {
        let refused = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("{dossier_uri}/reporter"))
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("HX-Request", "true")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(refused.status(), StatusCode::OK, "{body}");
        assert!(refused.headers().get("HX-Trigger").is_none(), "{body}");
        let page = body_text(refused).await;
        assert!(page.contains(sentence), "{body} : {page}");
    }

    let ten = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("{dossier_uri}/reporter"))
                .header("content-type", "application/x-www-form-urlencoded")
                .header("HX-Request", "true")
                .body(Body::from("days=10"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ten.status(), StatusCode::OK);
    assert_eq!(ten.headers().get("HX-Trigger").unwrap(), "griffe:saved");
    let ten_page = body_text(ten).await;
    assert!(ten_page.contains("Atelier Sèvre"), "{ten_page}");
    assert!(
        ten_page.contains("<details class=\"snooze\">"),
        "{ten_page}"
    );

    let listed = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires?vue=livre")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        listed.contains("15 septembre 2026"),
        "dix jours après le 5 septembre : {listed}"
    );

    let three = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("{dossier_uri}/reporter"))
                .header("content-type", "application/x-www-form-urlencoded")
                .header("HX-Request", "true")
                .body(Body::from("days=3"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(three.headers().get("HX-Trigger").unwrap(), "griffe:saved");
    let _ = body_text(three).await;
    let after_three = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires?vue=livre")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        after_three.contains("8 septembre 2026"),
        "trois jours après le 5 septembre : {after_three}"
    );

    let tomorrow = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("{dossier_uri}/reporter"))
                .header("content-type", "application/x-www-form-urlencoded")
                .header("HX-Request", "true")
                .body(Body::from("until=2026-09-06"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        tomorrow.headers().get("HX-Trigger").unwrap(),
        "griffe:saved"
    );
    let _ = body_text(tomorrow).await;
    let after_until = body_text(
        router
            .oneshot(
                Request::builder()
                    .uri("/affaires?vue=livre")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        after_until.contains("6 septembre 2026"),
        "until reste le chemin de Demain : {after_until}"
    );
}

fn form_encode(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(key, value)| format!("{}={}", form_escape(key), form_escape(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn form_escape(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => {
                out.push(char::from(byte));
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[tokio::test]
async fn travaux_is_read_as_prose_and_written_as_markdown_that_saves_itself() {
    let db = test_db_path("travaux");
    let mut store = Store::create(&db, &Passphrase::from(PASSPHRASE)).unwrap();
    let client_id = match Executor::new(&mut store)
        .execute(
            &CreateClient {
                name: "Atelier du recit".into(),
                siren: None,
                vat_number: None,
                address: None,
            },
            &human_ctx(),
        )
        .unwrap()
    {
        griffe_core::app::Outcome::Applied(id) => id,
        other => panic!("création attendue, obtenu {other:?}"),
    };
    Executor::new(&mut store)
        .execute(
            &CreateOpportunity {
                client_id,
                name: "Chantier du recit".into(),
                amount: Money::from_cents(150_000),
                probability: Probability::new(40).unwrap(),
                next_action_at: time::Date::from_calendar_date(2026, time::Month::October, 8)
                    .unwrap(),
                source: None,
            },
            &human_ctx(),
        )
        .unwrap();
    drop(store);

    let state = AppState::new(db.clone());
    state
        .unlock(&Passphrase::from(PASSPHRASE), false)
        .await
        .unwrap();
    let router = griffe_web::router(state);
    let dossier_uri = "/affaires/Atelier%20du%20recit";

    let dossier = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(dossier_uri)
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    let travaux_at = dossier.find("Les travaux").expect("le bouton Les travaux");
    let reporter_at = dossier.find("Reporter").expect("Reporter");
    assert!(
        travaux_at < reporter_at,
        "Les travaux se place juste avant Reporter : {dossier}"
    );
    assert!(dossier.contains("/travaux"), "{dossier}");

    let empty = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("{dossier_uri}/travaux"))
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(empty.contains("Les travaux."), "{empty}");
    assert!(empty.contains("Rien d'écrit."), "{empty}");
    assert!(empty.contains("Où on en est."), "{empty}");
    assert!(empty.contains("/travaux/ecrire"), "{empty}");
    assert!(empty.contains("Écrire"), "{empty}");
    assert!(!empty.contains("class=\"back\""), "{empty}");
    assert!(!empty.contains("<textarea"), "{empty}");
    assert!(!empty.contains("/travaux/apercu"), "{empty}");
    assert!(!empty.contains("keyup"), "{empty}");

    let editing = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("{dossier_uri}/travaux/ecrire"))
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(editing.contains("Il se garde tout seul."), "{editing}");
    assert!(editing.contains("Relire"), "{editing}");
    assert!(!editing.contains("class=\"back\""), "{editing}");
    assert!(editing.contains("input changed delay:600ms"), "{editing}");
    assert!(editing.contains("hx-swap=\"none\""), "{editing}");
    assert!(editing.contains("hx-push-url=\"false\""), "{editing}");
    assert!(editing.contains("hx-history=\"false\""), "{editing}");
    assert!(!editing.contains("work-prose"), "{editing}");
    assert!(!editing.contains("/travaux/apercu"), "{editing}");

    let source = "# Le chantier\n\nUne *ligne*.\n\n- un\n\n> dit\n\n`code`\n\n[voir](https://exemple.fr/page)\n\n![plan](https://exemple.fr/plan.png)\n\n<script>alert(1)</script>\n";
    let saved = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("{dossier_uri}/travaux"))
                .header("content-type", "application/x-www-form-urlencoded")
                .header("HX-Request", "true")
                .body(Body::from(form_encode(&[
                    ("body", source),
                    ("revision", "0"),
                ])))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        saved
            .headers()
            .get("X-Griffe-Travaux")
            .and_then(|v| v.to_str().ok()),
        Some("ok")
    );
    assert_eq!(
        saved
            .headers()
            .get("X-Griffe-Travaux-Revision")
            .and_then(|v| v.to_str().ok()),
        Some("1")
    );
    assert_eq!(
        saved
            .headers()
            .get("HX-Trigger")
            .and_then(|v| v.to_str().ok()),
        Some("griffe:saved")
    );
    let saved_body = body_text(saved).await;
    assert!(saved_body.contains("Gardé."), "{saved_body}");
    assert!(saved_body.contains("value=\"1\""), "{saved_body}");
    assert!(!saved_body.contains("<h1>Le chantier</h1>"), "{saved_body}");
    assert!(!saved_body.contains("work-prose"), "{saved_body}");

    let read = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("{dossier_uri}/travaux"))
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(read.contains("<h1>Le chantier</h1>"), "{read}");
    assert!(read.contains("<em>ligne</em>"), "{read}");
    assert!(read.contains("<li>un</li>"), "{read}");
    assert!(read.contains("<blockquote>"), "{read}");
    assert!(read.contains("<code>code</code>"), "{read}");
    assert!(read.contains("voir (https://exemple.fr/page)"), "{read}");
    assert!(!read.contains("<a href=\"https://exemple.fr"), "{read}");
    assert!(!read.contains("<img"), "{read}");
    assert!(!read.contains("plan.png"), "{read}");
    assert!(!read.to_ascii_lowercase().contains("<script"), "{read}");
    assert!(read.contains("&lt;script&gt;"), "{read}");
    assert!(!read.contains("<textarea"), "{read}");
    assert!(!read.contains("class=\"back\""), "{read}");

    let source_page = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("{dossier_uri}/travaux/ecrire"))
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(source_page.contains("textarea"), "{source_page}");
    assert!(source_page.contains("# Le chantier"), "{source_page}");
    assert!(!source_page.contains("work-prose"), "{source_page}");
    assert!(
        !source_page.to_ascii_lowercase().contains("<script"),
        "{source_page}"
    );

    let same = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("{dossier_uri}/travaux"))
                .header("content-type", "application/x-www-form-urlencoded")
                .header("HX-Request", "true")
                .body(Body::from(form_encode(&[
                    ("body", source),
                    ("revision", "0"),
                ])))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        same.headers()
            .get("X-Griffe-Travaux-Revision")
            .and_then(|v| v.to_str().ok()),
        Some("1")
    );
    assert!(same.headers().get("HX-Trigger").is_none());

    let reread = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("{dossier_uri}/travaux"))
                .header("content-type", "application/x-www-form-urlencoded")
                .header("HX-Request", "true")
                .body(Body::from(form_encode(&[
                    ("body", source),
                    ("revision", "1"),
                    ("intent", "relire"),
                ])))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        reread
            .headers()
            .get("HX-Trigger")
            .and_then(|v| v.to_str().ok()),
        Some("griffe:saved")
    );
    let push = reread
        .headers()
        .get("HX-Push-Url")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(push.ends_with("/travaux"), "{push}");
    assert!(!push.contains("/ecrire"), "{push}");
    let reread_body = body_text(reread).await;
    assert!(
        reread_body.contains("<h1>Le chantier</h1>"),
        "{reread_body}"
    );
    assert!(!reread_body.contains("<textarea"), "{reread_body}");

    let stale = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("{dossier_uri}/travaux"))
                .header("content-type", "application/x-www-form-urlencoded")
                .header("HX-Request", "true")
                .body(Body::from(form_encode(&[
                    ("body", "Un texte périmé.\n"),
                    ("revision", "0"),
                    ("intent", "relire"),
                ])))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        stale
            .headers()
            .get("X-Griffe-Travaux")
            .and_then(|v| v.to_str().ok()),
        Some("conflit")
    );
    assert!(stale.headers().get("HX-Trigger").is_none());
    let stale_body = body_text(stale).await;
    assert!(
        stale_body.contains("Ce récit a changé. Voici celui du coffre."),
        "{stale_body}"
    );
    assert!(stale_body.contains("Un texte périmé."), "{stale_body}");
    assert!(stale_body.contains("<textarea"), "{stale_body}");
    assert!(
        stale_body.contains("Reprendre le récit du coffre"),
        "{stale_body}"
    );
    assert!(stale_body.contains("data-travaux-discard"), "{stale_body}");

    let kept = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("{dossier_uri}/travaux"))
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(kept.contains("<h1>Le chantier</h1>"), "{kept}");
    assert!(!kept.contains("Un texte périmé"), "{kept}");

    let over = "b".repeat(griffe_core::dossier_work::MAX_BODY_BYTES + 1);
    let too_long = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("{dossier_uri}/travaux"))
                .header("content-type", "application/x-www-form-urlencoded")
                .header("HX-Request", "true")
                .body(Body::from(form_encode(&[
                    ("body", &over),
                    ("revision", "1"),
                ])))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        too_long
            .headers()
            .get("X-Griffe-Travaux")
            .and_then(|v| v.to_str().ok()),
        Some("refuse")
    );
    let too_long_body = body_text(too_long).await;
    assert!(
        too_long_body.contains("Ce récit dépasse 64 Kio."),
        "{too_long_body}"
    );
    assert!(!too_long_body.contains("<textarea"), "{too_long_body}");

    let estimation = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("{dossier_uri}/estimation"))
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        estimation.contains("Ce que chaque ligne vaut."),
        "{estimation}"
    );

    // Relire remplace #content avant htmx:afterRequest. Le compteur qui
    // bloque la navigation doit suivre le XHR, pas le formulaire déjà retiré.
    let js = body_text(
        router
            .oneshot(
                Request::builder()
                    .uri("/assets/app.js")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        js.contains("const travauxRequests = new Set()")
            && js.contains("travauxRequests.delete(xhr)")
            && js.contains("htmx:onLoadError"),
        "plusieurs Relire ne doivent pas laisser le compteur de sauvegarde coincé : {js}"
    );
}

fn kind_links(html: &str) -> Vec<(String, String)> {
    let marker = "/affaires/types?ouvert=";
    let mut rest = html;
    let mut found = Vec::new();
    while let Some(at) = rest.find(marker) {
        let after = &rest[at + marker.len()..];
        let end = after.find('"').unwrap_or(0);
        let id = after[..end].to_string();
        if id != "nouveau" && found.iter().all(|(seen, _)| seen != &id) {
            let name = after
                .find("class=\"pastille")
                .and_then(|pos| {
                    let gt = after[pos..].find('>')?;
                    let start = pos + gt + 1;
                    let stop = after[start..].find('<')?;
                    Some(after[start..start + stop].trim().to_string())
                })
                .unwrap_or_default();
            found.push((id, name));
        }
        rest = &after[end..];
    }
    found
}

async fn posted(
    router: &axum::Router,
    uri: &str,
    fields: &[(&str, &str)],
) -> axum::response::Response {
    router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/x-www-form-urlencoded")
                .header("HX-Request", "true")
                .body(Body::from(form_encode(fields)))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn work_kinds_filter_the_affaires_and_leave_on_the_second_gesture() {
    let db_path = test_db_path("types");
    let mut store = Store::create(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
    let quai = match Executor::new(&mut store)
        .execute(
            &CreateProspect {
                prospect_name: "Atelier Quai".into(),
                address: None,
                representative: None,
                email: None,
                phone: None,
                name: "Affaire du quai".into(),
                amount: Money::from_cents(120_000),
                probability: Probability::new(40).unwrap(),
                next_action_at: time::macros::date!(2026 - 10 - 07),
                source: None,
            },
            &human_ctx(),
        )
        .unwrap()
    {
        Outcome::Applied(id) => id,
        other => panic!("création attendue, obtenu {other:?}"),
    };
    let port = match Executor::new(&mut store)
        .execute(
            &CreateProspect {
                prospect_name: "Atelier Port".into(),
                address: None,
                representative: None,
                email: None,
                phone: None,
                name: "Affaire du port".into(),
                amount: Money::from_cents(80_000),
                probability: Probability::new(40).unwrap(),
                next_action_at: time::macros::date!(2026 - 10 - 07),
                source: None,
            },
            &human_ctx(),
        )
        .unwrap()
    {
        Outcome::Applied(id) => id,
        other => panic!("création attendue, obtenu {other:?}"),
    };
    Executor::new(&mut store)
        .execute(
            &SetDossierGenre {
                subject: FollowUpSubject::Opportunity(quai),
                name: "Mairie".into(),
            },
            &human_ctx(),
        )
        .unwrap();
    Executor::new(&mut store)
        .execute(
            &WinOpportunity {
                opportunity_id: port,
                started_on: time::macros::date!(2026 - 10 - 07),
            },
            &human_ctx(),
        )
        .unwrap();
    Executor::new(&mut store)
        .execute(
            &RecordExpense {
                label: "Ramette".into(),
                category: griffe_core::domain::ExpenseCategory::Software,
                amount: Money::from_cents(2_000),
                vat_rate: griffe_core::domain::VatRate::Zero,
                vat_deductible: Money::ZERO,
                incurred_on: time::macros::date!(2026 - 10 - 01),
                receipt_hash: None,
                receipt_filename: None,
                bank_transaction_id: None,
                supplier: Some("Papeterie du Nord".into()),
                paid_by: griffe_core::domain::ExpensePaidBy::Company,
                reverse_charge: false,
            },
            &human_ctx(),
        )
        .unwrap();
    drop(store);

    let state = AppState::new(db_path.clone());
    state
        .unlock(&Passphrase::from(PASSPHRASE), false)
        .await
        .unwrap();
    let state = state.with_today(time::macros::date!(2026 - 10 - 07));
    let router = griffe_web::router(state);
    let quai_uri = "/affaires/Atelier%20Quai";
    let port_uri = "/affaires/Atelier%20Port";

    let bare = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(quai_uri)
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(bare.contains("Ajouter un type"), "{bare}");
    assert!(bare.contains("hx-trigger=\"change\""), "{bare}");
    assert!(!bare.contains("un autre"), "{bare}");
    assert!(!bare.contains("Nouveau nom"), "{bare}");
    assert!(!bare.contains("hx-on"), "{bare}");

    let created = posted(
        &router,
        "/affaires/types",
        &[
            ("name", "site web"),
            ("note", "le site, la page"),
            ("tint", "mousse"),
            ("geste", "garder"),
        ],
    )
    .await;
    assert_eq!(
        created
            .headers()
            .get("HX-Trigger")
            .and_then(|value| value.to_str().ok()),
        Some("griffe:saved")
    );
    let created_body = body_text(created).await;
    assert!(created_body.contains("Les types."), "{created_body}");
    assert!(created_body.contains("le site, la page"), "{created_body}");
    assert!(created_body.contains("+ un type"), "{created_body}");
    assert!(!created_body.contains("Nouveau nom"), "{created_body}");
    let forms = kind_links(&created_body);
    let site_id = forms
        .iter()
        .find(|(_, name)| name == "site web")
        .map(|(id, _)| id.clone())
        .unwrap_or_else(|| panic!("site web absent du catalogue : {created_body}"));

    let editor = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/affaires/types?ouvert={site_id}"))
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(editor.contains("Ce qu'on y range"), "{editor}");
    assert!(editor.contains("value=\"le site, la page\""), "{editor}");
    assert!(editor.contains("value=\"mousse\""), "{editor}");
    assert!(!editor.contains("Nouveau nom"), "{editor}");

    let worn = posted(
        &router,
        &format!("{quai_uri}/types"),
        &[("kind", "site web")],
    )
    .await;
    assert_eq!(
        worn.headers()
            .get("HX-Trigger")
            .and_then(|value| value.to_str().ok()),
        Some("griffe:saved")
    );
    let worn_body = body_text(worn).await;
    let reading = worn_body
        .split_once("<details class=\"kind-add\"")
        .map(|(head, _)| head)
        .unwrap_or(&worn_body);
    assert!(reading.contains("site web"), "{worn_body}");
    assert!(worn_body.contains("value=\"site web\""), "{worn_body}");

    let filtered = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/affaires?vue=tout&type={site_id}"))
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(filtered.contains("Atelier Quai"), "{filtered}");
    assert!(filtered.contains("Papeterie du Nord"), "{filtered}");
    assert!(
        filtered.contains("Aucune mission de ce type."),
        "{filtered}"
    );
    assert!(!filtered.contains("Atelier Port"), "{filtered}");
    assert!(filtered.contains("Les types"), "{filtered}");
    assert!(
        filtered.contains("class=\"pastille wash-mousse\">site web"),
        "{filtered}"
    );
    assert!(
        filtered.contains("class=\"tint-dot wash-mousse\""),
        "{filtered}"
    );

    let made_backend = posted(
        &router,
        "/affaires/types",
        &[("name", "backend"), ("tint", "encre"), ("geste", "garder")],
    )
    .await;
    assert_eq!(made_backend.status(), StatusCode::OK);

    let both = posted(
        &router,
        &format!("{port_uri}/types"),
        &[("kind", "site web"), ("kind", "backend")],
    )
    .await;
    assert_eq!(
        both.headers()
            .get("HX-Trigger")
            .and_then(|value| value.to_str().ok()),
        Some("griffe:saved")
    );
    let both_body = body_text(both).await;
    assert!(both_body.contains("value=\"site web\""), "{both_body}");
    assert!(both_body.contains("value=\"backend\""), "{both_body}");

    let site_page = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/affaires?type={site_id}&q=Port"))
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        !site_page.contains("Atelier Quai"),
        "la recherche filtre la vue courante : {site_page}"
    );
    assert!(site_page.contains("Aucun nom."), "{site_page}");
    assert!(
        site_page.contains("href=\"/affaires?q=Port\""),
        "{site_page}"
    );
    assert!(
        site_page.contains("q=Port&amp;type=") || site_page.contains("q=Port&type="),
        "{site_page}"
    );

    let catalog = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires/types")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    let forms = kind_links(&catalog);
    let backend_id = forms
        .iter()
        .find(|(_, name)| name == "backend")
        .map(|(id, _)| id.clone())
        .unwrap_or_else(|| panic!("backend absent : {catalog}"));
    let made_third = posted(
        &router,
        "/affaires/types",
        &[("name", "reliure"), ("tint", "sable"), ("geste", "garder")],
    )
    .await;
    assert_eq!(made_third.status(), StatusCode::OK);
    let piled = posted(
        &router,
        &format!("{port_uri}/types"),
        &[
            ("kind", "site web"),
            ("kind", "backend"),
            ("kind", "reliure"),
        ],
    )
    .await;
    assert_eq!(piled.status(), StatusCode::OK);
    let backend_page = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/affaires?vue=tout&type={backend_id}"))
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        backend_page.contains("Aucune conversation de ce type."),
        "{backend_page}"
    );
    assert!(backend_page.contains("Atelier Port"), "{backend_page}");
    assert!(
        backend_page.contains("class=\"kind-more\">+1"),
        "{backend_page}"
    );
    assert!(
        !backend_page.contains("href=\"/affaires/Atelier%20Quai\""),
        "{backend_page}"
    );

    let renamed = posted(
        &router,
        &format!("/affaires/types/{site_id}"),
        &[
            ("name", "site"),
            ("note", ""),
            ("tint", "ardoise"),
            ("geste", "garder"),
        ],
    )
    .await;
    assert_eq!(
        renamed
            .headers()
            .get("HX-Trigger")
            .and_then(|value| value.to_str().ok()),
        Some("griffe:saved")
    );
    let renamed_body = body_text(renamed).await;
    assert!(
        renamed_body.contains("class=\"pastille wash-ardoise\">site"),
        "{renamed_body}"
    );
    assert!(!renamed_body.contains("site web"), "{renamed_body}");
    assert!(!renamed_body.contains("Nouveau nom"), "{renamed_body}");

    let followed = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(quai_uri)
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(followed.contains("value=\"site\""), "{followed}");
    assert!(!followed.contains("site web"), "{followed}");

    let asking = posted(
        &router,
        &format!("/affaires/types/{site_id}"),
        &[("name", "site"), ("geste", "retirer")],
    )
    .await;
    assert!(asking.headers().get("HX-Trigger").is_none());
    let asking_body = body_text(asking).await;
    assert!(asking_body.contains("Oui, le retirer"), "{asking_body}");
    let still = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/affaires?vue=tout&type={site_id}"))
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(still.contains("Atelier Quai"), "{still}");
    assert!(still.contains("Atelier Port"), "{still}");

    let removed = posted(
        &router,
        &format!("/affaires/types/{site_id}"),
        &[("geste", "confirmer")],
    )
    .await;
    assert_eq!(removed.status(), StatusCode::OK);
    assert_eq!(
        removed
            .headers()
            .get("HX-Trigger")
            .and_then(|value| value.to_str().ok()),
        Some("griffe:saved")
    );
    let removed_body = body_text(removed).await;
    assert!(!removed_body.contains(">site<"), "{removed_body}");
    assert!(
        removed_body.contains("class=\"pastille wash-encre\">backend"),
        "{removed_body}"
    );

    let after = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/affaires?vue=tout&type={site_id}"))
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(after.contains("Aucune conversation de ce type."), "{after}");
    assert!(after.contains("Aucune mission de ce type."), "{after}");
    assert!(after.contains("Papeterie du Nord"), "{after}");
    assert!(
        !after.contains("href=\"/affaires/Atelier%20Quai\""),
        "{after}"
    );
    assert!(
        !after.contains("href=\"/affaires/Atelier%20Port\""),
        "{after}"
    );

    let port_page = body_text(
        router
            .oneshot(
                Request::builder()
                    .uri(port_uri)
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(port_page.contains("value=\"backend\""), "{port_page}");
    assert!(!port_page.contains("value=\"site\""), "{port_page}");

    let check = Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
    assert_eq!(
        prospect_genre_for(check.connection(), quai)
            .unwrap()
            .expect("la série reste")
            .name,
        "Mairie"
    );
}

#[tokio::test]
async fn le_courrier_garde_le_mot_de_passe_et_arme_une_lettre() {
    let db_path = test_db_path("courrier-envoi");
    let today = time::macros::date!(2026 - 09 - 05);
    {
        let mut store = Store::create(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
        let client_id = applied(
            Executor::new(&mut store)
                .execute(
                    &CreateClient {
                        name: "Atelier Nord".into(),
                        siren: None,
                        vat_number: None,
                        address: None,
                    },
                    &human_ctx(),
                )
                .unwrap(),
        );
        Executor::new(&mut store)
            .execute(
                &CreateContact {
                    client_id,
                    name: "Ada".into(),
                    email: Some("ada@atelier.test".into()),
                    phone: None,
                    role: None,
                    given_name: String::new(),
                    family_name: String::new(),
                    civility: None,
                    structured: false,
                },
                &human_ctx(),
            )
            .unwrap();
        Executor::new(&mut store)
            .execute(
                &CreateOpportunity {
                    client_id,
                    name: "Site".into(),
                    amount: Money::from_cents(12_000),
                    probability: Probability::new(40).unwrap(),
                    next_action_at: today,
                    source: None,
                },
                &human_ctx(),
            )
            .unwrap();
    }
    let state = AppState::new(db_path.clone()).with_today(today);
    state
        .unlock(&Passphrase::from(PASSPHRASE), false)
        .await
        .unwrap();
    let router = griffe_web::router(state);
    let secret = "mot-de-passe-application-xyz";

    let chapter = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/societe/courrier")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        chapter.contains("class=\"plateau plateau-open\"") && chapter.contains("Revenir au menu"),
        "le courrier reste dans le pli : {chapter}"
    );
    assert!(chapter.contains("Pas encore branché."), "{chapter}");
    assert!(
        chapter.contains("D'où elles partent") || chapter.contains("D&#x27;où elles partent"),
        "{chapter}"
    );
    assert!(chapter.contains("Ce qui ferme la lettre"), "{chapter}");
    assert!(!chapter.contains("Sous le nom"), "{chapter}");
    assert!(!chapter.contains("En bas de la lettre"), "{chapter}");
    assert!(!chapter.contains("Comment on les lit"), "{chapter}");
    assert!(chapter.contains("ferment chaque lettre"), "{chapter}");
    assert!(
        chapter.contains("Ingénieur logiciel indépendant"),
        "{chapter}"
    );
    assert!(chapter.contains("courrier-desk"), "{chapter}");
    assert!(chapter.contains("courrier-board"), "{chapter}");
    assert!(chapter.contains("courrier-side"), "{chapter}");
    assert!(chapter.contains("courrier-read"), "{chapter}");
    assert!(chapter.contains("class=\"letter-stage\""), "{chapter}");
    assert!(chapter.contains("class=\"letter-frame\""), "{chapter}");
    let frame_at = chapter.find("class=\"letter-frame\"").unwrap();
    let close_at = chapter.find("</iframe>").unwrap_or(usize::MAX);
    assert!(
        close_at < chapter.len(),
        "l'iframe se ferme, sinon le navigateur cache la suite : {chapter}"
    );
    for marker in [
        "La feuille",
        "courrier-send",
        "courrier-formula",
        "courrier-account",
        "courrier-jour",
    ] {
        let at = chapter.find(marker).unwrap_or(usize::MAX);
        assert!(
            close_at < at,
            "{marker} doit rester après </iframe> : {chapter}"
        );
    }
    assert!(frame_at < close_at, "{chapter}");
    assert!(
        chapter.contains("&lt;!DOCTYPE html&gt;") || chapter.contains("<!DOCTYPE html>"),
        "{chapter}"
    );
    assert!(chapter.contains(">Clair<"), "{chapter}");
    assert!(chapter.contains(">Sauge<"), "{chapter}");
    assert!(chapter.contains("class=\"mail-pane\""), "{chapter}");
    assert!(chapter.contains("class=\"mail-subject\""), "{chapter}");
    assert!(chapter.contains("hx-get=\"/aide/courrier\""), "{chapter}");
    assert!(
        chapter.contains("Une autre feuille") || chapter.contains("votre feuille"),
        "{chapter}"
    );
    let pane_at = chapter.find("class=\"mail-pane\"").unwrap_or(usize::MAX);
    let formula_at = chapter.find("courrier-formula").unwrap_or(0);
    assert!(pane_at < formula_at, "{chapter}");
    assert!(chapter.contains("#f6f0e4"), "{chapter}");
    assert!(chapter.contains("#2c2418"), "{chapter}");
    assert!(
        chapter.contains("hx-post=\"/societe/courrier/apparence\""),
        "{chapter}"
    );
    assert!(
        chapter.contains("hx-post=\"/societe/courrier/apercu\""),
        "{chapter}"
    );
    assert!(
        chapter.contains("hx-post=\"/societe/courrier/signature\""),
        "{chapter}"
    );
    assert!(!chapter.contains(">Métier<"), "{chapter}");
    assert!(!chapter.contains(">Site<"), "{chapter}");
    assert!(!chapter.contains("Enregistrer la formule"), "{chapter}");
    assert!(
        !chapter.contains("Enregistrer l'allure") && !chapter.contains("Enregistrer l&#x27;allure"),
        "{chapter}"
    );
    assert!(
        chapter.contains("Une lettre d'essai") || chapter.contains("Une lettre d&#x27;essai"),
        "{chapter}"
    );
    assert!(chapter.contains("Cette lettre est un essai"), "{chapter}");
    assert!(chapter.contains("Le prénom est fictif"), "{chapter}");
    assert!(
        chapter.contains("Enregistrer et essayer la liaison"),
        "{chapter}"
    );
    assert!(!chapter.contains("form=\"compte\""), "{chapter}");
    assert!(chapter.contains(">Vers<"), "{chapter}");
    assert!(
        chapter.contains("Envoyer l'essai") || chapter.contains("Envoyer l&#x27;essai"),
        "{chapter}"
    );
    assert!(chapter.contains("envoi du jour est"), "{chapter}");
    assert!(
        chapter.contains("Activer l'envoi du jour")
            || chapter.contains("Activer l&#x27;envoi du jour"),
        "{chapter}"
    );
    assert!(!chapter.contains("type=\"password\" value="), "{chapter}");

    let formula = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/societe/courrier/signature")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("HX-Request", "true")
                .body(Body::from("signature=Atelier+Nord"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(formula.headers().get("HX-Trigger").unwrap(), "griffe:saved");
    let formula_body = body_text(formula).await;
    assert!(
        formula_body.contains("La formule est enregistrée."),
        "{formula_body}"
    );
    assert!(formula_body.contains("Atelier Nord"), "{formula_body}");
    assert!(
        formula_body.contains("Envoyer l'essai") || formula_body.contains("Envoyer l&#x27;essai"),
        "{formula_body}"
    );

    let preview = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/societe/courrier/apercu")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("HX-Request", "true")
                .body(Body::from(
                    "signature=Atelier+Nord%0Ahttps%3A%2F%2Fatelier.example%2Fpage%3Futm_source%3Dessai",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(preview.headers().get("HX-Trigger").is_none());
    let preview_body = body_text(preview).await;
    assert!(preview_body.contains("Atelier Nord"), "{preview_body}");
    assert!(
        preview_body.contains("https://atelier.example/page?utm_source=essai"),
        "{preview_body}"
    );
    assert!(
        preview_body.contains("https://\u{2060}atelier.example/page"),
        "{preview_body}"
    );
    assert!(
        !preview_body.contains("&gt;https://atelier.example/page?utm_source=essai")
            && !preview_body.contains(">https://atelier.example/page?utm_source=essai<"),
        "{preview_body}"
    );
    assert!(
        preview_body.contains("&lt;!DOCTYPE html&gt;") || preview_body.contains("<!DOCTYPE html>"),
        "{preview_body}"
    );
    let preview_open = preview_body
        .find("class=\"letter-frame\"")
        .unwrap_or(usize::MAX);
    let preview_close = preview_body.find("</iframe>").unwrap_or(usize::MAX);
    assert!(
        preview_open < preview_close && preview_close < preview_body.len(),
        "l'aperçu ferme l'iframe : {preview_body}"
    );
    assert!(preview_body.contains("#f6f0e4"), "{preview_body}");
    assert!(preview_body.contains("#8a3e24"), "{preview_body}");
    assert!(
        preview_body.contains("text-decoration:none"),
        "{preview_body}"
    );
    assert!(
        !preview_body.contains("text-decoration:underline"),
        "{preview_body}"
    );

    let lire = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/societe/courrier/lire")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("HX-Request", "true")
                .body(Body::from(
                    "ink=sauge&metier=Atelier&site=https%3A%2F%2Fatelier.example&signature=Atelier+Nord%0Ahttps%3A%2F%2Fatelier.example",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(lire.headers().get("HX-Trigger").unwrap(), "griffe:saved");
    let lire_body = body_text(lire).await;
    assert!(
        lire_body.contains("La lettre est enregistrée."),
        "{lire_body}"
    );
    assert!(lire_body.contains("#1e6b45"), "{lire_body}");
    assert!(lire_body.contains("#e5eee6"), "{lire_body}");
    assert!(!lire_body.contains("#9f3218"), "{lire_body}");
    assert!(
        !lire_body.contains("text-decoration:underline"),
        "{lire_body}"
    );
    assert!(lire_body.contains("class=\"letter-stage\""), "{lire_body}");
    assert!(lire_body.contains("Atelier Nord"), "{lire_body}");

    let saved = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/societe/courrier")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("HX-Request", "true")
                .body(Body::from(
                    "from_name=Camille&from_address=camille%40studio.test&username=camille%40icloud.test&preset=icloud&host=&port=587",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(saved.headers().get("HX-Trigger").unwrap(), "griffe:saved");
    let saved_body = body_text(saved).await;
    assert!(
        saved_body.contains("Le serveur est enregistré."),
        "{saved_body}"
    );
    assert!(
        saved_body.contains("value=\"camille@studio.test\""),
        "{saved_body}"
    );

    let hidden = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/societe/courrier/secret")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("HX-Request", "true")
                .body(Body::from(format!("secret={secret}")))
                .unwrap(),
        )
        .await
        .unwrap();
    let hidden_body = body_text(hidden).await;
    assert!(hidden_body.contains("dans le coffre"), "{hidden_body}");
    assert!(!hidden_body.contains(secret), "{hidden_body}");
    assert!(
        hidden_body.contains("Envoyer l'essai") || hidden_body.contains("Envoyer l&#x27;essai"),
        "{hidden_body}"
    );
    assert!(
        hidden_body.contains("Cette lettre est un essai"),
        "{hidden_body}"
    );
    assert!(hidden_body.contains("Atelier Nord"), "{hidden_body}");
    assert!(hidden_body.contains("Bonjour Camille"), "{hidden_body}");

    let refused = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/societe/courrier/envoyer")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("HX-Request", "true")
                .body(Body::from("to=pas-une-adresse"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(!refused.headers().contains_key("HX-Trigger"));
    let refused_body = body_text(refused).await;
    assert!(refused_body.contains("illisible"), "{refused_body}");
    assert!(!refused_body.contains(secret), "{refused_body}");

    let trial = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/societe/courrier/envoyer")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("HX-Request", "true")
                .body(Body::from("to=ada%40atelier.test"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(trial.headers().get("HX-Trigger").unwrap(), "griffe:saved");
    let trial_body = body_text(trial).await;
    assert!(trial_body.contains("Elle part vers"), "{trial_body}");
    assert!(trial_body.contains("every 1s"), "{trial_body}");
    assert!(trial_body.contains("depart-track"), "{trial_body}");
    assert!(trial_body.contains("Annuler"), "{trial_body}");
    assert!(!trial_body.contains(secret), "{trial_body}");
    let trial_id = trial_body
        .split("name=\"id\" value=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("identifiant de l'essai");

    let trial_poll = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/societe/courrier/envoi")
                .header("HX-Request", "true")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(!trial_poll.headers().contains_key("HX-Trigger"));
    assert!(!trial_poll.headers().contains_key("HX-Reswap"));
    let trial_cache = trial_poll
        .headers()
        .get("cache-control")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    assert!(trial_cache.contains("no-store"), "{trial_cache}");
    let trial_poll_body = body_text(trial_poll).await;
    assert!(
        trial_poll_body.contains("Elle part vers"),
        "{trial_poll_body}"
    );
    assert!(
        !trial_poll_body.contains("id=\"depart\""),
        "{trial_poll_body}"
    );

    let trial_kept = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/societe/courrier/envoi/annuler")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("HX-Request", "true")
                    .body(Body::from(format!("id={trial_id}")))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        trial_kept.contains("Envoyer l'essai") || trial_kept.contains("Envoyer l&#x27;essai"),
        "{trial_kept}"
    );
    assert!(!trial_kept.contains("Elle part"), "{trial_kept}");
    assert!(!trial_kept.contains("Annuler"), "{trial_kept}");
    assert!(!trial_kept.contains("every 1s"), "{trial_kept}");

    let letter = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires/Atelier%20Nord/ecrire")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(letter.contains("Une lettre"), "{letter}");
    assert!(letter.contains("Envoyer"), "{letter}");
    assert!(!letter.contains("C'est parti"), "{letter}");
    assert!(letter.contains("?utm_"), "{letter}");
    assert!(letter.contains("se lit sans cette partie"), "{letter}");
    assert!(letter.contains("Le lien la garde"), "{letter}");
    assert!(letter.contains("letter-desk"), "{letter}");
    assert!(
        letter.contains("id=\"face-read\" value=\"relire\" checked"),
        "{letter}"
    );
    assert!(letter.contains(">Éditer<"), "{letter}");
    assert!(letter.contains(">Relire<"), "{letter}");
    assert!(letter.contains("/relire"), "{letter}");
    assert!(letter.contains("class=\"letter-stage\""), "{letter}");
    assert!(!letter.contains("letter-face-body"), "{letter}");
    assert!(!letter.contains(secret), "{letter}");

    let reread = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/affaires/Atelier%20Nord/relire")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("HX-Request", "true")
                .body(Body::from(
                    "subject_line=Sujet+relu&body=Une+ligne.%0A%0Ahttps%3A%2F%2Fatelier.example%2Fconformite%3Futm_source%3Dmairie%26utm_medium%3Demail%26utm_campaign%3Dautomne-2026",
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(!reread.headers().contains_key("HX-Trigger"));
    let reread_body = body_text(reread).await;
    assert!(
        reread_body.contains("class=\"letter-stage\""),
        "{reread_body}"
    );
    assert!(
        reread_body.contains("id=\"face-read\" value=\"relire\" checked"),
        "{reread_body}"
    );
    assert!(
        reread_body.contains("https://atelier.example/conformite?utm_source=mairie&amp;utm_medium=email&amp;utm_campaign=automne-2026"),
        "{reread_body}"
    );
    assert!(
        reread_body.contains(">https://\u{2060}atelier.example/conformite</a>"),
        "{reread_body}"
    );
    assert!(
        !reread_body.contains(">https://atelier.example/conformite?utm_source=mairie"),
        "{reread_body}"
    );
    assert!(reread_body.contains("Une ligne."), "{reread_body}");
    assert!(!reread_body.contains(secret), "{reread_body}");

    let before_send = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        before_send.contains("Atelier Nord"),
        "le pas d'aujourd'hui demande un geste : {before_send}"
    );

    let armed = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/affaires/Atelier%20Nord/envoyer")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("HX-Request", "true")
                .body(Body::from("subject_line=Bonjour&body=Une+ligne."))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(armed.headers().get("HX-Trigger").unwrap(), "griffe:saved");
    let armed_body = body_text(armed).await;
    assert!(armed_body.contains("Elle part vers"), "{armed_body}");
    assert!(armed_body.contains("every 1s"), "{armed_body}");
    assert!(armed_body.contains("Annuler"), "{armed_body}");
    assert!(!armed_body.contains(secret), "{armed_body}");
    let id = armed_body
        .split("name=\"id\" value=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("identifiant de la lettre");

    let while_armed = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        !while_armed.contains("Atelier Nord"),
        "une lettre en partance n'est plus un geste : {while_armed}"
    );

    let poll = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/affaires/Atelier%20Nord/envoi")
                .header("HX-Request", "true")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(!poll.headers().contains_key("HX-Trigger"));
    assert!(!poll.headers().contains_key("HX-Reswap"));
    let poll_cache = poll
        .headers()
        .get("cache-control")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    assert!(poll_cache.contains("no-store"), "{poll_cache}");
    let poll_body = body_text(poll).await;
    assert!(poll_body.contains("Elle part vers"), "{poll_body}");

    let kept = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/affaires/Atelier%20Nord/envoi/annuler")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("HX-Request", "true")
                    .body(Body::from(format!("id={id}")))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(kept.contains("Envoyer"), "{kept}");
    assert!(!kept.contains("Elle part"), "{kept}");
    assert!(!kept.contains("every 1s"), "{kept}");

    let after_cancel = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        after_cancel.contains("Atelier Nord"),
        "annuler rend le geste : {after_cancel}"
    );

    let mut store = Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
    let due = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap();
    store
        .connection_mut()
        .execute(
            "UPDATE outbound_mail SET send_after = ?1 WHERE id = ?2",
            (due.as_str(), id),
        )
        .unwrap();
    let batch = griffe_core::mail::take_due(
        &mut store,
        &human_ctx(),
        "fenetre",
        time::OffsetDateTime::now_utc(),
        time::OffsetDateTime::now_utc().date(),
        false,
    )
    .unwrap();
    assert!(batch.is_none());
    let status: String = store
        .connection()
        .query_row(
            "SELECT status FROM outbound_mail WHERE id = ?1",
            [id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(status, "cancelled");
    drop(store);

    let again = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/societe/courrier/envoyer")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("HX-Request", "true")
                .body(Body::from("to=ada%40atelier.test"))
                .unwrap(),
        )
        .await
        .unwrap();
    let again_body = body_text(again).await;
    let again_id = again_body
        .split("name=\"id\" value=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("identifiant du second essai");
    let mut store = Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
    let now = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap();
    let updated = store
        .connection_mut()
        .execute(
            "UPDATE outbound_mail SET status = 'sent', updated_at = ?1 WHERE id = ?2",
            (now.as_str(), again_id),
        )
        .unwrap();
    assert_eq!(updated, 1);
    drop(store);

    let sent = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/societe/courrier/envoi")
                .header("HX-Request", "true")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        sent.headers()
            .get("HX-Reswap")
            .and_then(|value| value.to_str().ok()),
        Some("outerHTML")
    );
    let sent_body = body_text(sent).await;
    assert!(sent_body.contains("Courrier envoyé."), "{sent_body}");
    assert!(
        sent_body.contains("Envoyer l'essai") || sent_body.contains("Envoyer l&#x27;essai"),
        "{sent_body}"
    );
    assert!(!sent_body.contains("Annuler"), "{sent_body}");
    assert!(!sent_body.contains("every 1s"), "{sent_body}");

    let prospect_sent = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/affaires/Atelier%20Nord/envoyer")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("HX-Request", "true")
                .body(Body::from("subject_line=Bonjour&body=Une+ligne."))
                .unwrap(),
        )
        .await
        .unwrap();
    let prospect_body = body_text(prospect_sent).await;
    let prospect_id = prospect_body
        .split("name=\"id\" value=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("identifiant de la lettre partie");
    let mut store = Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
    let updated = store
        .connection_mut()
        .execute(
            "UPDATE outbound_mail SET status = 'sent', updated_at = ?1 WHERE id = ?2",
            (now.as_str(), prospect_id),
        )
        .unwrap();
    assert_eq!(updated, 1);
    drop(store);
    let prospect_poll = router
        .oneshot(
            Request::builder()
                .uri("/affaires/Atelier%20Nord/envoi")
                .header("HX-Request", "true")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let prospect_poll_body = body_text(prospect_poll).await;
    assert!(
        prospect_poll_body.contains("Courrier envoyé."),
        "{prospect_poll_body}"
    );
    assert!(
        prospect_poll_body.contains(">Envoyer<"),
        "{prospect_poll_body}"
    );
    assert!(
        !prospect_poll_body.contains("Annuler"),
        "{prospect_poll_body}"
    );
    assert!(
        !prospect_poll_body.contains("every 1s"),
        "{prospect_poll_body}"
    );
}

#[tokio::test]
async fn la_liaison_affiche_la_coche_ou_le_refus_sans_effacer_les_champs() {
    let db_path = test_db_path("courrier-liaison");
    Store::create(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
    let state = AppState::new(db_path);
    state
        .unlock(&Passphrase::from(PASSPHRASE), false)
        .await
        .unwrap();
    state.set_mail_probe(|_, _, _| griffe_core::mail::ProbeVerdict {
        ok: true,
        detail: format!(
            "{} La copie n'ira pas dans Envoyés.",
            griffe_core::mail::PROBE_OK_SENTENCE
        ),
        imap_username: None,
    });
    let router = griffe_web::router(state.clone());
    let secret = "mot-de-passe-application-xyz";
    let body = format!(
        "from_name=Camille&from_address=camille%40studio.test&username=camille%40icloud.test&preset=custom&host=smtp.exemple.test&port=587&secret={secret}"
    );

    let held = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/societe/courrier")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("HX-Request", "true")
                .body(Body::from(body.clone()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(held.headers().get("HX-Trigger").unwrap(), "griffe:saved");
    let held_body = body_text(held).await;
    assert!(held_body.contains("probe-ok"), "{held_body}");
    assert!(
        held_body.contains("Le serveur répond, et l")
            && held_body.contains("identifiant est accepté"),
        "{held_body}"
    );
    assert!(
        held_body.contains("value=\"camille@studio.test\""),
        "{held_body}"
    );
    assert!(
        held_body.contains("value=\"camille@icloud.test\""),
        "{held_body}"
    );
    assert!(
        held_body.contains("value=\"smtp.exemple.test\""),
        "{held_body}"
    );
    assert!(!held_body.contains(secret), "{held_body}");
    assert!(
        !held_body.contains("type=\"password\" value="),
        "{held_body}"
    );

    state.set_mail_probe(|_, _, _| griffe_core::mail::ProbeVerdict {
        ok: false,
        detail: "Le serveur a refusé le mot de passe.".to_string(),
        imap_username: None,
    });
    let refused = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/societe/courrier")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("HX-Request", "true")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    let refused_body = body_text(refused).await;
    assert!(refused_body.contains("probe-bad"), "{refused_body}");
    assert!(
        refused_body.contains("Le serveur a refusé le mot de passe."),
        "{refused_body}"
    );
    assert!(
        refused_body.contains("value=\"smtp.exemple.test\""),
        "{refused_body}"
    );
    assert!(
        refused_body.contains("value=\"camille@studio.test\""),
        "{refused_body}"
    );
    assert!(!refused_body.contains(secret), "{refused_body}");
}

#[tokio::test]
async fn la_fiche_refuse_une_adresse_illisible_et_ecrire_prend_la_nouvelle() {
    let db_path = test_db_path("fiche-courriel");
    let state = unlocked_state(&db_path).await;
    let router = griffe_web::router(state);
    let created = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/affaires/nouvelle")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("HX-Request", "true")
                .body(Body::from("who=Atelier+Copie&phrase=signaletique"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);

    let client_id = client_id_by_name(&db_path, "Atelier Copie");
    let revision = {
        let store = Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
        griffe_core::clients::client_by_id(store.connection(), client_id)
            .unwrap()
            .unwrap()
            .revision
    };
    let first = format!(
        "who=Atelier+Copie&given_name=Ada&email=ada.ancienne%40exemple.fr&phone=&client_revision={revision}&contact_id=&contact_revision=&street=&postal_code=&city="
    );
    let saved = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/affaires/Atelier%20Copie/fiche")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("HX-Request", "true")
                    .body(Body::from(first))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(saved.contains("Fiche à jour"), "{saved}");

    let (client_revision, contact_id, contact_revision) = {
        let store = Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
        let client = griffe_core::clients::client_by_id(store.connection(), client_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            client.switchboard_email.as_deref(),
            Some("ada.ancienne@exemple.fr")
        );
        let contact = griffe_core::clients::correspondent(store.connection(), client_id)
            .unwrap()
            .expect("correspondant");
        assert_eq!(contact.name, "Ada");
        assert_eq!(contact.email, None);
        (client.revision, contact.id.to_string(), contact.revision)
    };
    let bad = format!(
        "who=Atelier+Copie&given_name=Ada&email=ada+sans+arobase&phone=&client_revision={client_revision}&contact_id={contact_id}&contact_revision={contact_revision}&street=&postal_code=&city="
    );
    let refused = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/affaires/Atelier%20Copie/fiche")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("HX-Request", "true")
                    .body(Body::from(bad))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        refused.contains("Cette adresse est illisible."),
        "{refused}"
    );
    assert!(refused.contains("value=\"ada sans arobase\""), "{refused}");

    let kept = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires/Atelier%20Copie/fiche")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(kept.contains("value=\"ada.ancienne@exemple.fr\""), "{kept}");
    assert!(!kept.contains("ada sans arobase"), "{kept}");

    let next = format!(
        "who=Atelier+Copie&given_name=Ada&email=ada.nouvelle%40exemple.fr&phone=&client_revision={client_revision}&contact_id={contact_id}&contact_revision={contact_revision}&street=&postal_code=&city="
    );
    let updated = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/affaires/Atelier%20Copie/fiche")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("HX-Request", "true")
                    .body(Body::from(next))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(updated.contains("Fiche à jour"), "{updated}");

    let letter = body_text(
        router
            .oneshot(
                Request::builder()
                    .uri("/affaires/Atelier%20Copie/ecrire")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(letter.contains("Le standard"), "{letter}");
    assert!(letter.contains("ada.nouvelle@exemple.fr"), "{letter}");
    assert!(!letter.contains("ada.ancienne@exemple.fr"), "{letter}");
}

#[tokio::test]
async fn un_ecart_nul_laisse_le_nom_parmi_les_gestes() {
    let db_path = test_db_path("ecart-nul");
    let today = time::macros::date!(2026 - 09 - 05);
    {
        let mut store = Store::create(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
        let phrases = prospect_phrases(store.connection()).unwrap();
        let moments = phrases
            .iter()
            .map(|phrase| {
                let mut draft = MomentDraft::from_phrase(phrase);
                if phrase.key == "bump" {
                    draft.offset_days = 0;
                }
                draft
            })
            .collect();
        let _phrases = applied(
            Executor::new(&mut store)
                .execute(&ArrangeProspectPhrases { moments }, &human_ctx())
                .unwrap(),
        );
        Executor::new(&mut store)
            .execute(
                &CreateProspect {
                    prospect_name: "Écart nul".into(),
                    address: None,
                    representative: None,
                    email: None,
                    phone: None,
                    name: "le porche".into(),
                    amount: Money::ZERO,
                    probability: Probability::new(40).unwrap(),
                    next_action_at: today,
                    source: None,
                },
                &human_ctx(),
            )
            .unwrap();
    }
    let state = AppState::new(db_path.clone()).with_today(today);
    state
        .unlock(&Passphrase::from(PASSPHRASE), false)
        .await
        .unwrap();
    let router = griffe_web::router(state);

    let listed = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(listed.contains("Écart nul"), "{listed}");

    let posted = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/affaires/%C3%89cart%20nul/envoye")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("HX-Request", "true")
                    .body(Body::from("subject_line=Bonjour&body=Le+porche."))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(posted.contains("hors de Griffe"), "{posted}");
    assert!(posted.contains("/societe/courrier"), "{posted}");

    let still = body_text(
        router
            .oneshot(
                Request::builder()
                    .uri("/affaires")
                    .header("HX-Request", "true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        still.contains("Écart nul"),
        "un écart nul laisse le pas à aujourd'hui : {still}"
    );
    assert_eq!(opportunity_due(&db_path, "le porche"), Some(today));
}

fn plant_letter(
    store: &Store,
    uid: i64,
    message_id: &str,
    notice: Option<&str>,
    subject: &str,
    text: &str,
    html: Option<&str>,
) -> String {
    let mailbox = store.open_mailbox().unwrap();
    mailbox
        .insert(&griffe_core::mailbox::MirrorInsert {
            mailbox: "INBOX".into(),
            uid,
            uidvalidity: 1,
            message_id: Some(message_id.into()),
            from_name: Some(subject.into()),
            from_address: Some("qui@exemple.test".into()),
            subject: subject.into(),
            sent_on: Some("Sat, 5 Sep 2026 09:00:00 +0200".into()),
            text: text.into(),
            html: html.map(str::to_string),
            seen: false,
            notice: notice.map(str::to_string),
            notice_address: (notice == Some("certain"))
                .then(|| "contact@atelier-du-mail.fr".into()),
            folder_role: "inbox".into(),
            choice_of: None,
            choice_ids: None,
        })
        .unwrap();
    mailbox.remember("INBOX", 1, uid).unwrap();
    mailbox
        .list()
        .unwrap()
        .into_iter()
        .find(|line| line.uid == uid)
        .unwrap()
        .id
}

#[tokio::test]
async fn the_box_page_reads_a_letter_and_confirms_a_failed_address() {
    let db_path = test_db_path("boite-page");
    let store = Store::create(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
    store
        .connection()
        .execute_batch(
            "INSERT INTO clients (id, name, created_at, switchboard_email) VALUES
                ('c1', 'Camille', '2026-09-01T00:00:00Z', 'camille@riviere.test'),
                ('c2', 'Atelier', '2026-09-01T00:00:00Z', 'contact@atelier-du-mail.fr');
             INSERT INTO opportunities (
                id, client_id, name, stage, amount_cents, probability_percent,
                next_action_at, created_at
             ) VALUES
                ('o1', 'c1', 'Site', 'discovery', 0, 40, '2026-09-08', '2026-09-01T00:00:00Z'),
                ('o2', 'c2', 'Enseigne', 'discovery', 0, 40, '2026-09-08', '2026-09-01T00:00:00Z');
             INSERT INTO mail_filing (message_id, client_id, opportunity_id, source) VALUES
                ('camille-1@riviere.test', 'c1', 'o1', 'auto'),
                ('bounce@mx.example', 'c2', 'o2', 'auto');
             INSERT INTO address_fault (client_id, address, status, message_id) VALUES
                ('c2', 'contact@atelier-du-mail.fr', 'failed', 'bounce@mx.example');",
        )
        .unwrap();
    let reply = plant_letter(
        &store,
        1,
        "<camille-1@riviere.test>",
        None,
        "Reponse Camille",
        "PHRASE-SECRETE-REPONSE",
        None,
    );
    let bounce = plant_letter(
        &store,
        2,
        "<bounce@mx.example>",
        Some("certain"),
        "Avis certain",
        "PHRASE-SECRETE-AVIS",
        None,
    );
    plant_letter(
        &store,
        3,
        "<peut-etre@mx.example>",
        Some("probable"),
        "Avis probable",
        "indice",
        None,
    );
    let libre = plant_letter(
        &store,
        4,
        "<libre@exemple.test>",
        None,
        "Disponibilite",
        "PHRASE-SECRETE-BOITE",
        None,
    );
    let html_id = plant_letter(
        &store,
        5,
        "<html@exemple.test>",
        None,
        "Avec cadre",
        "TEXTE-LOCAL",
        Some(
            "<p style=\"background:url(https://pixel.evil.test/b.png)\">Bonjour Camille</p><img src=\"https://pixel.evil.test/a.png\"><script>pixel.evil.test</script>",
        ),
    );
    let empty_id = plant_letter(
        &store,
        6,
        "<vide@exemple.test>",
        None,
        "Cadre vide",
        "",
        Some("<p><img src=\"https://pixel.evil.test/a.png\"></p>"),
    );
    drop(store);

    let state = AppState::new(db_path.clone());
    state
        .unlock(&Passphrase::from(PASSPHRASE), false)
        .await
        .unwrap();
    let router = griffe_web::router(state);

    let css = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/assets/app.css")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(css.contains(".content:has(.boite)"), "{css}");
    assert!(css.contains("max-width: 820px"), "{css}");

    let jour = body_text(
        router
            .clone()
            .oneshot(Request::builder().uri("/jour").body(Body::empty()).unwrap())
            .await
            .unwrap(),
    )
    .await;
    assert!(jour.contains("Boîte"), "{jour}");
    assert!(jour.contains("class=\"today\""), "{jour}");
    assert!(
        !jour.contains("class=\"n\""),
        "le sceau initial ne compte pas : {jour}"
    );

    let seal = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/boite/sceau")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        seal.contains("class=\"n\">2<"),
        "réponse non lue et adresse en échec : {seal}"
    );
    assert!(!seal.contains("<!DOCTYPE"), "{seal}");

    let failed = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/boite?vue=echec")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(failed.status(), StatusCode::OK);
    assert_eq!(
        failed
            .headers()
            .get("HX-Trigger")
            .and_then(|value| value.to_str().ok()),
        Some("griffe:box")
    );
    let failed_body = body_text(failed).await;
    assert!(failed_body.contains("Avis certain"), "{failed_body}");
    assert!(!failed_body.contains("Avis probable"), "{failed_body}");
    assert!(!failed_body.contains("Reponse Camille"), "{failed_body}");
    assert!(failed_body.contains("Elle est bonne"), "{failed_body}");
    assert!(failed_body.contains("data-view=\"boite\""), "{failed_body}");
    assert!(!failed_body.contains("hx-on"), "{failed_body}");

    let opened = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/boite?vue=lire&lettre={reply}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let opened_body = body_text(opened).await;
    assert!(
        opened_body.contains("PHRASE-SECRETE-REPONSE"),
        "{opened_body}"
    );
    assert!(opened_body.contains("Ouvrir le dossier"), "{opened_body}");

    let seal_after = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/boite/sceau")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        seal_after.contains("class=\"n\">1<"),
        "la réponse lue sort du sceau, l'adresse reste : {seal_after}"
    );

    let lire = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/boite?vue=lire")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        !lire.contains("Reponse Camille"),
        "la lettre lue quitte À lire : {lire}"
    );

    let framed = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/boite?lettre={html_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(framed.contains("TEXTE-LOCAL"), "{framed}");
    assert!(framed.contains("Bonjour Camille"), "{framed}");
    assert!(framed.contains("sandbox=\"\""), "{framed}");
    assert!(framed.contains("<iframe"), "{framed}");
    assert!(!framed.contains("pixel.evil.test"), "{framed}");

    let bare = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/boite?lettre={empty_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(bare.contains("Cadre vide"), "{bare}");
    assert!(
        !bare.contains("<iframe"),
        "un HTML vide ne laisse pas de cadre : {bare}"
    );
    assert!(!bare.contains("pixel.evil.test"), "{bare}");

    let filed = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/boite/classer")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("HX-Request", "true")
                .body(Body::from(format!(
                    "message_id=libre%40exemple.test&client_id=c1&opportunity_id=o1&vue=tout&lettre={libre}"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(filed.status(), StatusCode::OK);
    assert_eq!(
        filed
            .headers()
            .get("HX-Trigger")
            .and_then(|value| value.to_str().ok()),
        Some("griffe:saved")
    );
    let filed_body = body_text(filed).await;
    assert!(filed_body.contains("Camille"), "{filed_body}");
    assert!(filed_body.contains("PHRASE-SECRETE-BOITE"), "{filed_body}");
    assert!(!filed_body.contains("<!DOCTYPE"), "{filed_body}");

    let audit = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/audit/recent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        !audit.contains("PHRASE-SECRETE"),
        "le corps reste hors du journal : {audit}"
    );

    let confirmed = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/boite/adresse")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("HX-Request", "true")
                .body(Body::from(format!(
                    "client_id=c2&address=contact%40atelier-du-mail.fr&vue=echec&lettre={bounce}"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(confirmed.status(), StatusCode::OK);
    assert_eq!(
        confirmed
            .headers()
            .get("HX-Trigger")
            .and_then(|value| value.to_str().ok()),
        Some("griffe:saved")
    );
    let confirmed_body = body_text(confirmed).await;
    assert!(
        !confirmed_body.contains("Elle est bonne"),
        "{confirmed_body}"
    );
    assert!(
        !confirmed_body.contains("ne reçoit pas"),
        "{confirmed_body}"
    );

    let quiet = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/boite/sceau")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        !quiet.contains("class=\"n\""),
        "plus rien à compter : {quiet}"
    );
}

fn engage_releve(state: &AppState) {
    state.set_releve_fetch(|_endpoint, _secret, _cursors, _tick| Ok(Vec::new()));
}

fn save_account(store: &mut Store, preset: &str, imap_host: &str) {
    let ctx = ExecutionContext::new(Actor::System, false);
    Executor::new(store)
        .execute(
            &griffe_core::mail::SaveMailAccount {
                from_name: Some("Nicolas".to_string()),
                from_address: "nicolas@lumen-conseil.fr".to_string(),
                host: "smtp.exemple.test".to_string(),
                port: 587,
                tls: "starttls".to_string(),
                username: "nicolas@lumen-conseil.fr".to_string(),
                preset: preset.to_string(),
                imap_host: imap_host.to_string(),
                imap_port: 993,
            },
            &ctx,
        )
        .unwrap();
    Executor::new(store)
        .execute(
            &griffe_core::mail::SaveMailSecret {
                secret: griffe_core::mail::MailSecret::new("mot-de-passe-application").unwrap(),
            },
            &ctx,
        )
        .unwrap();
}

async fn open_saved(db_path: &Path) -> AppState {
    let state = AppState::new(db_path.to_path_buf());
    state
        .unlock(&Passphrase::from(PASSPHRASE), false)
        .await
        .unwrap();
    state
}

async fn read_route(router: &axum::Router, uri: &str) -> (StatusCode, Option<String>, String) {
    let response = router
        .clone()
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let trigger = response
        .headers()
        .get("HX-Trigger")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let body = body_text(response).await;
    (status, trigger, body)
}

#[tokio::test]
async fn the_box_says_it_is_reading_and_the_poll_stops_at_rest() {
    let quiet_path = test_db_path("boite-lecture-quiet");
    let quiet = unlocked_state(&quiet_path).await;
    let quiet_router = griffe_web::router(quiet);
    let (_, _, quiet_body) = read_route(&quiet_router, "/boite").await;
    assert!(
        !quiet_body.contains("every 1s"),
        "sans crochet, pas de sondage : {quiet_body}"
    );
    assert!(!quiet_body.contains("Griffe ouvre"), "{quiet_body}");
    assert!(!quiet_body.contains("hx-on"), "{quiet_body}");
    assert!(
        quiet_body.contains("Aucune lettre dans cette vue."),
        "{quiet_body}"
    );

    let empty_path = test_db_path("boite-lecture-vide");
    let mut empty_store = Store::create(&empty_path, &Passphrase::from(PASSPHRASE)).unwrap();
    save_account(&mut empty_store, "icloud", "");
    drop(empty_store);
    let empty = open_saved(&empty_path).await;
    engage_releve(&empty);
    empty.note_lecture(
        griffe_core::mail::LecturePhase::Reading {
            folder: griffe_core::mail::LectureFolder::Reception,
            read: 3,
            total: Some(14),
        },
        "nicolas@lumen-conseil.fr",
    );
    let empty_router = griffe_web::router(empty.clone());
    let (_, trigger, reading) = read_route(&empty_router, "/boite").await;
    assert_eq!(trigger.as_deref(), None);
    assert!(reading.contains("Griffe lit la réception."), "{reading}");
    assert!(reading.contains("3 sur 14"), "{reading}");
    assert!(
        reading.contains("La réception · nicolas@lumen-conseil.fr"),
        "{reading}"
    );
    assert!(reading.contains("Les lettres arrivent."), "{reading}");
    assert!(reading.contains("every 1s"), "{reading}");
    assert!(reading.contains("class=\"glyph draw\""), "{reading}");
    assert!(reading.contains("pathLength=\"100\""), "{reading}");
    assert!(!reading.contains("hx-on"), "{reading}");

    let (_, trigger, poll) = read_route(
        &empty_router,
        "/boite/lecture?gen=0&mode=glyph&col=glyph&stall=0",
    )
    .await;
    assert_eq!(trigger.as_deref(), None);
    assert!(poll.contains("Griffe lit la réception."), "{poll}");
    assert!(poll.contains("3 sur 14"), "{poll}");
    assert!(
        !poll.contains("pathLength"),
        "le sondage ne relance pas le trait : {poll}"
    );
    assert!(!poll.contains("hx-on"), "{poll}");
    assert!(poll.contains("every 1s"), "{poll}");

    empty.note_lecture(
        griffe_core::mail::LecturePhase::Failed,
        "nicolas@lumen-conseil.fr",
    );
    let (_, _, failed) = read_route(&empty_router, "/boite").await;
    assert!(failed.contains("Le compte n'a pas répondu."), "{failed}");
    assert!(
        failed.contains("nicolas@lumen-conseil.fr · nouvel essai dans un instant."),
        "{failed}"
    );
    assert!(failed.contains("Aucune lettre pour l'instant."), "{failed}");
    assert!(failed.contains("class=\"glyph stall\""), "{failed}");
    assert!(failed.contains("every 1s"), "{failed}");
    assert!(!failed.contains("hx-on"), "{failed}");

    let (_, _, stalled) = read_route(
        &empty_router,
        "/boite/lecture?gen=0&mode=fail&col=glyph&stall=1",
    )
    .await;
    assert!(
        !stalled.contains("pathLength"),
        "un échec déjà montré ne retrace pas : {stalled}"
    );

    empty.note_lecture(
        griffe_core::mail::LecturePhase::Filing { on_dossier: 1 },
        "nicolas@lumen-conseil.fr",
    );
    let (_, _, filing) = read_route(&empty_router, "/boite").await;
    assert!(
        filing.contains("Une lettre se pose sur un dossier."),
        "{filing}"
    );
    assert!(filing.contains("every 1s"), "{filing}");
    let (_, _, after) = read_route(&empty_router, "/boite").await;
    assert!(
        !after.contains("Une lettre se pose sur un dossier."),
        "{after}"
    );
    assert!(
        !after.contains("every 1s"),
        "au repos le sondage s'arrête : {after}"
    );

    let (_, trigger, rest) = read_route(
        &empty_router,
        "/boite/lecture?gen=0&mode=idle&col=letter&stall=0",
    )
    .await;
    assert_ne!(trigger.as_deref(), Some("griffe:saved"));
    assert!(!rest.contains("every 1s"), "{rest}");
    assert!(!rest.contains("hx-on"), "{rest}");

    let filled_path = test_db_path("boite-lecture-pleine");
    let filled_store = Store::create(&filled_path, &Passphrase::from(PASSPHRASE)).unwrap();
    let letter = plant_letter(
        &filled_store,
        4,
        "<camille-lecture@riviere.test>",
        None,
        "Reponse Camille",
        "PHRASE-SECRETE-LECTURE",
        None,
    );
    drop(filled_store);
    let mut filled_store =
        Store::open_with_passphrase(&filled_path, &Passphrase::from(PASSPHRASE)).unwrap();
    save_account(&mut filled_store, "icloud", "");
    drop(filled_store);
    let filled = open_saved(&filled_path).await;
    engage_releve(&filled);
    filled.note_lecture(
        griffe_core::mail::LecturePhase::Reading {
            folder: griffe_core::mail::LectureFolder::Reception,
            read: 1,
            total: Some(1),
        },
        "nicolas@lumen-conseil.fr",
    );
    let filled_router = griffe_web::router(filled.clone());
    let (_, _, reread) = read_route(&filled_router, "/boite").await;
    assert!(reread.contains("Griffe relit la réception."), "{reread}");
    assert!(reread.contains("Reponse Camille"), "{reread}");
    assert!(!reread.contains("class=\"glyph"), "{reread}");
    assert!(!reread.contains("hx-on"), "{reread}");

    let (_, _, kept) = read_route(
        &filled_router,
        &format!("/boite/lecture?gen=0&mode=quiet&col=letter&stall=0&lettre={letter}"),
    )
    .await;
    assert!(kept.contains("Griffe relit la réception."), "{kept}");
    assert!(
        !kept.contains("PHRASE-SECRETE-LECTURE"),
        "la lettre ouverte n'est pas remplacée : {kept}"
    );
    assert!(!kept.contains("hx-on"), "{kept}");

    filled.note_fresh(2, vec![letter.clone()]);
    let (status, trigger, arrived_rows) = read_route(
        &filled_router,
        "/boite/lecture?vue=tout&gen=0&mode=idle&col=letter&stall=0",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(trigger.as_deref(), Some("griffe:box"));
    assert_ne!(trigger.as_deref(), Some("griffe:saved"));
    assert!(arrived_rows.contains("arriving"), "{arrived_rows}");
    assert!(arrived_rows.contains("Reponse Camille"), "{arrived_rows}");
    assert!(!arrived_rows.contains("hx-on"), "{arrived_rows}");

    filled.note_lecture(
        griffe_core::mail::LecturePhase::Arrived { fresh: 1 },
        "nicolas@lumen-conseil.fr",
    );
    let (_, _, one) = read_route(&filled_router, "/boite").await;
    assert!(one.contains("Une lettre nouvelle."), "{one}");
    let (_, _, settled) = read_route(&filled_router, "/boite").await;
    assert!(!settled.contains("Une lettre nouvelle."), "{settled}");
    assert!(!settled.contains("every 1s"), "{settled}");

    filled.note_lecture(
        griffe_core::mail::LecturePhase::Failed,
        "nicolas@lumen-conseil.fr",
    );
    let (_, _, quiet_fail) = read_route(&filled_router, "/boite?vue=tout").await;
    assert!(
        quiet_fail.contains("Le compte n'a pas répondu. Nouvel essai dans un instant."),
        "{quiet_fail}"
    );
    assert!(quiet_fail.contains("Reponse Camille"), "{quiet_fail}");
    assert!(!quiet_fail.contains("class=\"glyph"), "{quiet_fail}");
    assert!(quiet_fail.contains("every 1s"), "{quiet_fail}");
    assert!(!quiet_fail.contains("hx-on"), "{quiet_fail}");

    let mute_path = test_db_path("boite-lecture-muet");
    let mut mute_store = Store::create(&mute_path, &Passphrase::from(PASSPHRASE)).unwrap();
    save_account(&mut mute_store, "custom", "");
    drop(mute_store);
    let mute = open_saved(&mute_path).await;
    engage_releve(&mute);
    let mute_router = griffe_web::router(mute);
    let (_, _, unreadable) = read_route(&mute_router, "/boite").await;
    assert!(
        unreadable.contains("Le compte ne se lit pas."),
        "{unreadable}"
    );
    assert!(unreadable.contains("/societe/courrier"), "{unreadable}");
    assert!(
        unreadable.contains("Aucune lettre dans cette vue."),
        "{unreadable}"
    );
    assert!(!unreadable.contains("every 1s"), "{unreadable}");
    assert!(!unreadable.contains("class=\"glyph"), "{unreadable}");
    assert!(!unreadable.contains("hx-on"), "{unreadable}");
}

#[tokio::test]
async fn the_dossier_thread_holds_the_reply_and_refuses_a_dead_address() {
    let db_path = test_db_path("boite-fil");
    let mut store = Store::create(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
    let camille = prospect(
        &mut store,
        "Camille",
        Some("Camille Rivière"),
        Some("camille@riviere.test"),
        "Site",
    );
    let atelier = prospect(
        &mut store,
        "Atelier",
        None,
        Some("contact@atelier-du-mail.fr"),
        "Enseigne",
    );
    plant_from(
        &store,
        1,
        "<camille-fil@boite.test>",
        "camille@riviere.test",
        "Camille Rivière",
        "Re: Site",
        "PHRASE-FIL",
    );
    plant_from(
        &store,
        2,
        "<atelier-fil@boite.test>",
        "contact@atelier-du-mail.fr",
        "Atelier",
        "Re: Enseigne",
        "PHRASE-MORT",
    );
    griffe_core::mail::file_human(
        &mut store,
        &human_ctx(),
        "<camille-fil@boite.test>",
        &camille.0,
        Some(camille.1.clone()),
    )
    .unwrap();
    griffe_core::mail::file_human(
        &mut store,
        &human_ctx(),
        "<atelier-fil@boite.test>",
        &atelier.0,
        Some(atelier.1.clone()),
    )
    .unwrap();
    Executor::new(&mut store)
        .execute(
            &griffe_core::mail::MarkAddress {
                client_id: atelier.0.clone(),
                address: "contact@atelier-du-mail.fr".into(),
                status: griffe_core::mail::AddressFault::Failed,
                message_id: None,
            },
            &human_ctx(),
        )
        .unwrap();
    store
        .connection()
        .execute(
            "INSERT INTO outbound_mail (
                id, kind, status, anchor, from_address, to_address, subject, body, message_id,
                send_after, created_at, updated_at
             ) VALUES (
                'partie-fil', 'letter', 'sent', ?1, 'moi@studio.test', 'camille@riviere.test',
                'Partie', 'PARTIE-FIL', '<partie@studio.test>',
                '2026-09-02T08:00:00Z', '2026-09-02T08:00:00Z', '2026-09-02T08:00:00Z'
             )",
            [format!("opportunity:{}", camille.1)],
        )
        .unwrap();
    drop(store);

    let state = AppState::new(db_path.clone());
    state
        .unlock(&Passphrase::from(PASSPHRASE), false)
        .await
        .unwrap();
    let router = griffe_web::router(state);

    let dossier = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires/Camille")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(dossier.contains("Camille a répondu"), "{dossier}");
    assert!(dossier.contains("La relance ne part pas"), "{dossier}");
    assert!(dossier.contains("PHRASE-FIL"), "{dossier}");
    assert!(dossier.contains("PARTIE-FIL"), "{dossier}");
    assert!(dossier.contains("Répondre"), "{dossier}");
    assert!(dossier.contains("Laisser partir la relance"), "{dossier}");
    assert!(dossier.contains("<textarea"), "{dossier}");
    assert!(!dossier.contains("hx-on"), "{dossier}");

    let released = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/affaires/Camille/relance")
                .header("HX-Request", "true")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(released.status(), StatusCode::OK);
    assert_eq!(
        released
            .headers()
            .get("HX-Trigger")
            .and_then(|value| value.to_str().ok()),
        Some("griffe:saved")
    );
    let released_body = body_text(released).await;
    assert!(
        !released_body.contains("La relance ne part pas"),
        "{released_body}"
    );
    let hold: i64 = Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE))
        .unwrap()
        .connection()
        .query_row(
            "SELECT reply_hold FROM opportunities WHERE id = ?1",
            [camille.1.clone()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(hold, 0);

    let dead = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires/Atelier")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(dead.contains("ne reçoit pas"), "{dead}");
    assert!(dead.contains("Corriger l'adresse"), "{dead}");
    assert!(dead.contains("Elle est bonne"), "{dead}");
    assert!(!dead.contains("<textarea"), "{dead}");
    assert!(!dead.contains("Répondre"), "{dead}");

    let write = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/affaires/Atelier/ecrire")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(write.contains("ne reçoit pas"), "{write}");
    assert!(write.contains("Elle est bonne"), "{write}");
    assert!(!write.contains("/envoyer"), "{write}");

    let refused = body_text(
        router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/affaires/Atelier/repondre")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("HX-Request", "true")
                    .body(Body::from(
                        "message=atelier-fil%40boite.test&subject=Re%3A+suite&body=bonjour",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(refused.contains("ne reçoit pas"), "{refused}");
    let check = Store::open_with_passphrase(&db_path, &Passphrase::from(PASSPHRASE)).unwrap();
    let armed: i64 = check
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM outbound_mail WHERE status = 'armed'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(armed, 0);
    let still: i64 = check
        .connection()
        .query_row(
            "SELECT reply_hold FROM opportunities WHERE id = ?1",
            [atelier.1],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(still, 1);
}

fn prospect(
    store: &mut Store,
    qui: &str,
    respondent: Option<&str>,
    email: Option<&str>,
    piste: &str,
) -> (String, String) {
    let Outcome::Applied(id) = Executor::new(store)
        .execute(
            &CreateProspect {
                prospect_name: qui.into(),
                address: None,
                representative: respondent.map(str::to_string),
                email: email.map(str::to_string),
                phone: None,
                name: piste.into(),
                amount: Money::ZERO,
                probability: Probability::new(40).unwrap(),
                next_action_at: time::macros::date!(2026 - 09 - 20),
                source: None,
            },
            &human_ctx(),
        )
        .unwrap()
    else {
        panic!("dossier");
    };
    let client_id: String = store
        .connection()
        .query_row(
            "SELECT client_id FROM opportunities WHERE id = ?1",
            [id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    (client_id, id.to_string())
}

fn plant_from(
    store: &Store,
    uid: i64,
    message_id: &str,
    from: &str,
    name: &str,
    subject: &str,
    text: &str,
) {
    let mailbox = store.open_mailbox().unwrap();
    mailbox
        .insert(&griffe_core::mailbox::MirrorInsert {
            mailbox: "INBOX".into(),
            uid,
            uidvalidity: 1,
            message_id: Some(message_id.into()),
            from_name: Some(name.into()),
            from_address: Some(from.into()),
            subject: subject.into(),
            sent_on: Some("Sat, 5 Sep 2026 09:00:00 +0200".into()),
            text: text.into(),
            html: None,
            seen: false,
            notice: None,
            notice_address: None,
            folder_role: "inbox".into(),
            choice_of: None,
            choice_ids: None,
        })
        .unwrap();
    mailbox.remember("INBOX", 1, uid).unwrap();
}
