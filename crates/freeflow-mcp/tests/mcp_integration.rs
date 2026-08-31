//! Tests d'intégration du serveur MCP (lot 8) : un vrai client `rmcp`, en mémoire
//! (`tokio::io::duplex`), en face du vrai `FreeflowServer` — pas de mock du protocole MCP, pas
//! de mock de la base (SQLCipher réelle, temporaire).

use std::collections::BTreeSet;
use std::path::PathBuf;

use freeflow_core::app::{Executor, Outcome};
use freeflow_core::billing::{EmitInvoice, RecordPayment};
use freeflow_core::clients::DeleteContact;
use freeflow_core::store::{Passphrase, Store};
use freeflow_mcp::FreeflowServer;
use rmcp::RoleClient;
use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, CallToolResult, ContentBlock, ReadResourceRequestParams};
use rmcp::service::RunningService;
use serde_json::{Value, json};

fn test_db_path(label: &str) -> PathBuf {
    std::env::temp_dir()
        .join(format!(
            "freeflow-mcp-test-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ))
        .join("vault.db")
}

async fn spawn_client(store: Store) -> RunningService<RoleClient, ()> {
    let (server_io, client_io) = tokio::io::duplex(256 * 1024);
    tokio::spawn(async move {
        let service = FreeflowServer::new(store)
            .serve(server_io)
            .await
            .expect("le serveur MCP doit s'initialiser");
        let _ = service.waiting().await;
    });
    ().serve(client_io)
        .await
        .expect("le client MCP doit s'initialiser")
}

fn tool_text(result: &CallToolResult) -> &str {
    result
        .content
        .iter()
        .find_map(|block| match block {
            ContentBlock::Text(t) => Some(t.text.as_str()),
            _ => None,
        })
        .expect("chaque réponse d'outil FreeFlow porte un bloc de texte JSON")
}

fn json_of(result: &CallToolResult) -> Value {
    serde_json::from_str(tool_text(result)).expect("le contenu texte doit être du JSON valide")
}

async fn call(
    client: &RunningService<RoleClient, ()>,
    name: &str,
    arguments: Value,
) -> CallToolResult {
    let mut params = CallToolRequestParams::new(name.to_string());
    if let Value::Object(map) = arguments {
        params = params.with_arguments(map);
    }
    client
        .call_tool(params)
        .await
        .expect("l'appel d'outil ne doit pas échouer au niveau transport")
}

#[tokio::test]
async fn lists_every_domain_tool_with_correct_annotations() {
    let store = Store::create(&test_db_path("list-tools"), &Passphrase::from("s3cret")).unwrap();
    let client = spawn_client(store).await;

    let tools = client.list_tools(None).await.unwrap().tools;
    let names: BTreeSet<String> = tools.iter().map(|t| t.name.to_string()).collect();

    for expected in [
        "clients.create",
        "clients.show",
        "clients.list",
        "clients.references",
        "clients.update",
        "clients.archive",
        "clients.unarchive",
        "clients.delete",
        "clients.contacts.create",
        "clients.contacts.list",
        "clients.contacts.update",
        "clients.contacts.delete",
        "prospect.create",
        "prospect.show",
        "prospect.list",
        "prospect.references",
        "prospect.update",
        "prospect.archive",
        "prospect.unarchive",
        "prospect.delete",
        "prospect.advance",
        "prospect.win",
        "prospect.lose",
        "prospect.log_interaction",
        "prospect.interactions.list",
        "prospect.interactions.update",
        "prospect.interactions.delete",
        "prospect.late",
        "prospect.orphans",
        "prospect.pipeline",
        "mission.create",
        "mission.show",
        "mission.list",
        "mission.references",
        "mission.schedule",
        "mission.update",
        "mission.close",
        "mission.reopen",
        "mission.archive",
        "mission.unarchive",
        "mission.delete",
        "mission.log_time",
        "mission.time.list",
        "mission.time.update",
        "mission.time.delete",
        "mission.rate",
        "mission.capacity",
        "quote.create",
        "quote.revise",
        "quote.send",
        "quote.decline",
        "quote.accept",
        "invoice.emit",
        "invoice.credit_note",
        "invoice.verify_chain",
        "invoice.aged_balance",
        "payment.record",
        "bank.import",
        "bank.reconcile",
        "pending.list",
        "audit.verify_chain",
    ] {
        assert!(names.contains(expected), "outil manquant : {expected}");
    }

    // Retiré volontairement (lot 15) : rien côté MCP ne distingue un appel d'outil émis par
    // l'agent lui-même d'une confirmation humaine réelle — voir `tools/pending.rs`.
    assert!(
        !names.contains("pending.confirm"),
        "pending.confirm ne doit plus être exposé côté MCP : un agent ne doit jamais pouvoir \
         confirmer sa propre action en attente"
    );

    let by_name = |n: &str| tools.iter().find(|t| t.name == n).unwrap();

    // Une requête pure doit être annoncée `read_only_hint = true` — c'est le signal spec-correct
    // qu'un client MCP utilise pour juger qu'un appel est sans effet de bord.
    for read_only in [
        "clients.list",
        "prospect.pipeline",
        "invoice.aged_balance",
        "pending.list",
    ] {
        assert_eq!(
            by_name(read_only)
                .annotations
                .as_ref()
                .and_then(|a| a.read_only_hint),
            Some(true),
            "{read_only} devrait être annoté read_only_hint = true"
        );
    }

    // Les commandes qui déclarent `requires_confirmation()` côté core (lot 2/5/15) doivent être
    // annoncées comme destructives côté MCP — c'est ce qui doit alerter un agent avant appel.
    for destructive in [
        "invoice.emit",
        "invoice.credit_note",
        "clients.delete",
        "prospect.delete",
        "prospect.interactions.delete",
        "mission.delete",
        "mission.time.delete",
    ] {
        let ann = by_name(destructive).annotations.as_ref().unwrap();
        assert_eq!(ann.read_only_hint, Some(false));
        assert_eq!(ann.destructive_hint, Some(true));
    }

    insta::assert_json_snapshot!(json!({
        "tool_names": names,
        "invoice_emit_input_schema": by_name("invoice.emit").input_schema,
    }));

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn an_agent_emitting_an_invoice_only_deposits_a_pending_action_no_invoice_row_is_written() {
    let db_path = test_db_path("confirm-gate");
    let store = Store::create(&db_path, &Passphrase::from("s3cret")).unwrap();
    let client = spawn_client(store).await;

    let created = call(&client, "clients.create", json!({"name": "Kappa Software"})).await;
    assert_eq!(created.is_error, Some(false));
    let client_id = json_of(&created)["result"].as_str().unwrap().to_string();

    let lines_json = serde_json::to_string(&json!([
        {"description": "Sept.", "quantity": 9.5, "unit_price": 65000, "vat_rate": "Standard"}
    ]))
    .unwrap();
    let emitted = call(
        &client,
        "invoice.emit",
        json!({"client_id": client_id, "lines_json": lines_json, "issued_on": "2026-09-01"}),
    )
    .await;
    assert_eq!(emitted.is_error, Some(false));
    let body = json_of(&emitted);
    assert_eq!(body["status"], "pending_confirmation");
    assert!(body["pending_action_id"].as_str().is_some());

    // Preuve directe en base, pas seulement sur la réponse de l'outil : ouvrir une seconde
    // connexion sur le même coffre (comme le ferait un autre process) et compter les factures.
    let check = Store::open_with_passphrase(&db_path, &Passphrase::from("s3cret")).unwrap();
    let invoice_count: i64 = check
        .connection()
        .query_row("SELECT count(*) FROM invoices", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        invoice_count, 0,
        "aucune facture ne doit exister tant qu'un humain n'a pas confirmé"
    );

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn a_field_that_fails_to_parse_is_a_tool_level_error_not_a_protocol_error() {
    let store = Store::create(&test_db_path("parse-error"), &Passphrase::from("s3cret")).unwrap();
    let client = spawn_client(store).await;

    // `clients.contacts.delete` parse encore un `id` brut (un contact n'a pas de résolution par
    // nom, contrairement à un client depuis le lot 15) — le cas visé par ce test.
    let result = call(
        &client,
        "clients.contacts.delete",
        json!({"id": "not-a-uuid"}),
    )
    .await;

    assert_eq!(result.is_error, Some(true));
    assert!(
        tool_text(&result).contains("id"),
        "le message d'erreur doit rester lisible par l'agent : {}",
        tool_text(&result)
    );

    client.cancel().await.unwrap();
}

/// Parcours complet, sur base réelle, à travers la plupart des outils exposés : la preuve que
/// l'adaptateur MCP orchestre correctement la même couche applicative que la CLI (lot 7).
#[tokio::test]
async fn golden_path_from_prospection_to_paid_invoice_over_mcp() {
    let db_path = test_db_path("golden-path");
    let store = Store::create(&db_path, &Passphrase::from("s3cret")).unwrap();
    let client = spawn_client(store).await;

    let created = call(&client, "clients.create", json!({"name": "Kappa Software"})).await;
    let client_id = json_of(&created)["result"].as_str().unwrap().to_string();

    let opportunity = call(
        &client,
        "prospect.create",
        json!({
            "client": client_id,
            "name": "Refonte plateforme",
            "amount_cents": 7_800_000,
            "probability_percent": 40,
            "next_action": "2026-09-02",
        }),
    )
    .await;
    assert_eq!(opportunity.is_error, Some(false));
    let opportunity_id = json_of(&opportunity)["result"]
        .as_str()
        .unwrap()
        .to_string();

    let won = call(
        &client,
        "prospect.win",
        json!({"opportunity": opportunity_id, "started_on": "2026-09-01"}),
    )
    .await;
    assert_eq!(won.is_error, Some(false));
    let mission_id = json_of(&won)["result"].as_str().unwrap().to_string();

    let logged = call(
        &client,
        "mission.log_time",
        json!({
            "mission": mission_id,
            "worked_on": "2026-09-15",
            "days": 9.5,
            "category": "billable",
        }),
    )
    .await;
    assert_eq!(logged.is_error, Some(false));

    let lines_json = serde_json::to_string(&json!([
        {"description": "Sept.", "quantity": 9.5, "unit_price": 65000, "vat_rate": "Standard"}
    ]))
    .unwrap();
    let emitted = call(
        &client,
        "invoice.emit",
        json!({
            "client_id": client_id,
            "mission_id": mission_id,
            "lines_json": lines_json,
            "issued_on": "2026-09-30",
        }),
    )
    .await;
    let pending_id = json_of(&emitted)["pending_action_id"]
        .as_str()
        .unwrap()
        .to_string();

    let pending_list = call(&client, "pending.list", json!(null)).await;
    let pending_actions = json_of(&pending_list);
    assert!(
        pending_actions
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["id"] == pending_id),
        "l'action déposée par l'agent doit apparaître dans pending.list"
    );

    // La confirmation elle-même n'est volontairement pas un outil MCP (lot 15, voir
    // `tools/pending.rs`) : c'est ici un vrai geste humain hors du canal de l'agent, une
    // seconde connexion sur le même coffre — exactement ce que fait `freeflow confirm <id>`
    // dans un terminal séparé pendant que le serveur MCP tourne.
    let mut confirming_store =
        Store::open_with_passphrase(&db_path, &Passphrase::from("s3cret")).unwrap();
    let confirm_outcome = Executor::new(&mut confirming_store)
        .confirm::<EmitInvoice>(pending_id.parse().unwrap())
        .unwrap();
    let Outcome::Applied(emitted_invoice) = confirm_outcome else {
        panic!("expected Applied, got {confirm_outcome:?}")
    };
    let invoice_id = emitted_invoice.id.to_string();

    let chain = call(&client, "invoice.verify_chain", json!(null)).await;
    assert_eq!(json_of(&chain)["status"], "intact");

    // `payment.record` a un effet comptable sensible : proposé par l'agent, il ne s'applique pas
    // directement mais dépose une action en attente qu'un humain confirme — même rail que
    // `invoice.emit` ci-dessus.
    let paid = call(
        &client,
        "payment.record",
        json!({
            "invoice_id": invoice_id,
            "amount_cents": 741_000,
            "received_on": "2026-10-05",
            "method": "bank_transfer",
        }),
    )
    .await;
    assert_eq!(paid.is_error, Some(false));
    let payment_pending_id = json_of(&paid)["pending_action_id"]
        .as_str()
        .expect("payment.record proposé par un agent dépose une action en attente")
        .to_string();
    let confirm_payment = Executor::new(&mut confirming_store)
        .confirm::<RecordPayment>(payment_pending_id.parse().unwrap())
        .unwrap();
    assert!(
        matches!(confirm_payment, Outcome::Applied(_)),
        "la confirmation humaine applique l'encaissement"
    );

    let aged = call(
        &client,
        "invoice.aged_balance",
        json!({"today": "2026-10-06"}),
    )
    .await;
    let aged_rows = json_of(&aged);
    assert!(
        aged_rows.as_array().unwrap().is_empty(),
        "la facture est payée intégralement : elle ne doit plus apparaître dans la balance âgée"
    );

    let audit = call(&client, "audit.verify_chain", json!(null)).await;
    assert_eq!(json_of(&audit)["status"], "intact");

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn updating_a_client_by_name_changes_only_the_provided_fields() {
    let store =
        Store::create(&test_db_path("update-by-name"), &Passphrase::from("s3cret")).unwrap();
    let client = spawn_client(store).await;

    call(&client, "clients.create", json!({"name": "Kappa Software"})).await;

    let updated = call(
        &client,
        "clients.update",
        json!({"client": "Kappa Software", "siren": "552100554"}),
    )
    .await;
    assert_eq!(updated.is_error, Some(false));

    let shown = call(&client, "clients.show", json!({"client": "kappa"})).await;
    let body = json_of(&shown);
    assert_eq!(
        body["name"], "Kappa Software",
        "nom conservé, non fourni à clients.update"
    );
    assert_eq!(body["siren"], "552100554");
    assert_eq!(body["revision"], 2);

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn archiving_a_client_removes_it_from_the_default_list() {
    let store = Store::create(&test_db_path("archive"), &Passphrase::from("s3cret")).unwrap();
    let client = spawn_client(store).await;

    call(&client, "clients.create", json!({"name": "Kappa Software"})).await;
    let archived = call(
        &client,
        "clients.archive",
        json!({"client": "Kappa Software"}),
    )
    .await;
    assert_eq!(archived.is_error, Some(false));

    let active = call(&client, "clients.list", json!({})).await;
    assert!(json_of(&active).as_array().unwrap().is_empty());

    let all = call(&client, "clients.list", json!({"archived": true})).await;
    assert_eq!(json_of(&all).as_array().unwrap().len(), 1);

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn deleting_a_client_still_referenced_by_an_opportunity_is_refused_at_confirmation() {
    // `clients.delete` a `requires_confirmation() == true` : pour un acteur agent, l'exécuteur
    // dépose une action en attente *avant* même d'appeler `DeleteClient::apply` — le refus lié
    // aux références (l'opportunité ci-dessous) ne peut donc se manifester qu'au moment où un
    // humain confirme réellement, jamais dans la réponse immédiate de l'outil. Même schéma que
    // `EmitInvoice`/`IssueCreditNote` (voir `an_agent_emitting_an_invoice_only_deposits_a_pending_action…`).
    let db_path = test_db_path("delete-referenced");
    let store = Store::create(&db_path, &Passphrase::from("s3cret")).unwrap();
    let client = spawn_client(store).await;

    let created = call(&client, "clients.create", json!({"name": "Kappa Software"})).await;
    let client_id = json_of(&created)["result"].as_str().unwrap().to_string();
    call(
        &client,
        "prospect.create",
        json!({
            "client": client_id,
            "name": "Refonte",
            "amount_cents": 10_000,
            "probability_percent": 50,
            "next_action": "2026-09-02",
        }),
    )
    .await;

    let deleted = call(&client, "clients.delete", json!({"client": client_id})).await;
    assert_eq!(deleted.is_error, Some(false));
    let body = json_of(&deleted);
    assert_eq!(body["status"], "pending_confirmation");
    let pending_id: freeflow_core::app::PendingActionId =
        body["pending_action_id"].as_str().unwrap().parse().unwrap();

    let mut confirming_store =
        Store::open_with_passphrase(&db_path, &Passphrase::from("s3cret")).unwrap();
    let err = Executor::new(&mut confirming_store)
        .confirm::<freeflow_core::clients::DeleteClient>(pending_id)
        .unwrap_err();
    assert!(
        err.to_string().contains("1 opportunité"),
        "le refus doit nommer ce qui bloque : {err}"
    );

    let shown = call(&client, "clients.show", json!({"client": client_id})).await;
    assert_eq!(shown.is_error, Some(false), "le client existe toujours");

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn an_ambiguous_client_reference_is_a_tool_level_error_listing_the_candidates() {
    let store = Store::create(&test_db_path("ambiguous"), &Passphrase::from("s3cret")).unwrap();
    let client = spawn_client(store).await;

    call(&client, "clients.create", json!({"name": "Argon Digital"})).await;
    call(&client, "clients.create", json!({"name": "Argon Studio"})).await;

    let result = call(&client, "clients.show", json!({"client": "argon"})).await;
    assert_eq!(result.is_error, Some(true));
    let text = tool_text(&result);
    assert!(text.contains("Argon Digital"));
    assert!(text.contains("Argon Studio"));

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn contact_lifecycle_over_mcp() {
    let db_path = test_db_path("contact-lifecycle");
    let store = Store::create(&db_path, &Passphrase::from("s3cret")).unwrap();
    let client = spawn_client(store).await;

    call(&client, "clients.create", json!({"name": "Kappa Software"})).await;

    let added = call(
        &client,
        "clients.contacts.create",
        json!({"client": "Kappa Software", "name": "Alex Martin", "email": "alex@kappa.example"}),
    )
    .await;
    assert_eq!(added.is_error, Some(false));
    let contact_id = json_of(&added)["result"].as_str().unwrap().to_string();

    let listed = call(
        &client,
        "clients.contacts.list",
        json!({"client": "Kappa Software"}),
    )
    .await;
    assert_eq!(json_of(&listed).as_array().unwrap().len(), 1);

    let updated = call(
        &client,
        "clients.contacts.update",
        json!({"id": contact_id, "role": "DAF"}),
    )
    .await;
    assert_eq!(updated.is_error, Some(false));

    // Suppression définitive proposée par l'agent : elle dépose une action en attente plutôt que
    // de s'appliquer, comme toute suppression (cohérent avec `clients.delete`). Un humain la
    // confirme par une seconde connexion — le geste `freeflow confirm <id>`.
    let deleted = call(
        &client,
        "clients.contacts.delete",
        json!({"id": contact_id}),
    )
    .await;
    assert_eq!(deleted.is_error, Some(false));
    let delete_pending_id = json_of(&deleted)["pending_action_id"]
        .as_str()
        .expect("une suppression de contact proposée par un agent dépose une action en attente")
        .to_string();

    let not_yet_empty = call(
        &client,
        "clients.contacts.list",
        json!({"client": "Kappa Software"}),
    )
    .await;
    assert_eq!(
        json_of(&not_yet_empty).as_array().unwrap().len(),
        1,
        "tant que l'humain n'a pas confirmé, le contact existe toujours"
    );

    let mut confirming_store =
        Store::open_with_passphrase(&db_path, &Passphrase::from("s3cret")).unwrap();
    let confirmed = Executor::new(&mut confirming_store)
        .confirm::<DeleteContact>(delete_pending_id.parse().unwrap())
        .unwrap();
    assert!(matches!(confirmed, Outcome::Applied(())));

    let empty = call(
        &client,
        "clients.contacts.list",
        json!({"client": "Kappa Software"}),
    )
    .await;
    assert!(json_of(&empty).as_array().unwrap().is_empty());

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn a_dry_run_update_writes_nothing() {
    let store = Store::create(&test_db_path("dry-run"), &Passphrase::from("s3cret")).unwrap();
    let client = spawn_client(store).await;

    call(&client, "clients.create", json!({"name": "Kappa Software"})).await;
    let dry = call(
        &client,
        "clients.update",
        json!({"client": "Kappa Software", "name": "Kappa Software SASU", "dry_run": true}),
    )
    .await;
    assert_eq!(dry.is_error, Some(false));
    assert_eq!(json_of(&dry)["status"], "dry_run");

    let shown = call(&client, "clients.show", json!({"client": "Kappa Software"})).await;
    assert_eq!(
        json_of(&shown)["name"],
        "Kappa Software",
        "dry-run n'écrit rien"
    );

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn reading_the_clients_collection_resource_lists_active_clients() {
    let store = Store::create(
        &test_db_path("resource-collection"),
        &Passphrase::from("s3cret"),
    )
    .unwrap();
    let client = spawn_client(store).await;

    call(&client, "clients.create", json!({"name": "Kappa Software"})).await;

    let resources = client.list_resources(None).await.unwrap().resources;
    assert!(resources.iter().any(|r| r.uri == "freeflow://clients"));

    let read = client
        .read_resource(ReadResourceRequestParams::new("freeflow://clients"))
        .await
        .unwrap();
    let text = match &read.contents[0] {
        rmcp::model::ResourceContents::TextResourceContents { text, .. } => text.clone(),
        other => panic!("expected text contents, got {other:?}"),
    };
    let clients: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(clients.as_array().unwrap().len(), 1);
    assert_eq!(clients[0]["name"], "Kappa Software");

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn reading_a_client_detail_resource_by_name_includes_contacts_and_references() {
    let store = Store::create(
        &test_db_path("resource-detail"),
        &Passphrase::from("s3cret"),
    )
    .unwrap();
    let client = spawn_client(store).await;

    call(&client, "clients.create", json!({"name": "Kappa Software"})).await;
    call(
        &client,
        "clients.contacts.create",
        json!({"client": "Kappa Software", "name": "Alex Martin"}),
    )
    .await;

    let read = client
        .read_resource(ReadResourceRequestParams::new(
            "freeflow://clients/Kappa Software",
        ))
        .await
        .unwrap();
    let text = match &read.contents[0] {
        rmcp::model::ResourceContents::TextResourceContents { text, .. } => text.clone(),
        other => panic!("expected text contents, got {other:?}"),
    };
    let payload: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(payload["client"]["name"], "Kappa Software");
    assert_eq!(payload["contacts"].as_array().unwrap().len(), 1);
    assert_eq!(payload["references"]["opportunities"], 0);

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn mission_create_dry_run_writes_nothing() {
    // Preuve que la dette remboursée par le lot 16 (`dry_run` sur les outils préexistants
    // `mission.create`/`prospect.*`) fonctionne réellement, pas seulement déclarée dans le schéma.
    let db_path = test_db_path("mission-dry-run");
    let store = Store::create(&db_path, &Passphrase::from("s3cret")).unwrap();
    let client = spawn_client(store).await;

    let created = call(&client, "clients.create", json!({"name": "Kappa Software"})).await;
    let client_id = json_of(&created)["result"].as_str().unwrap().to_string();

    let result = call(
        &client,
        "mission.create",
        json!({
            "client": client_id,
            "name": "Refonte",
            "kind": "forfait",
            "budget_cents": 100_000,
            "started_on": "2026-09-01",
            "dry_run": true,
        }),
    )
    .await;
    assert_eq!(result.is_error, Some(false));
    assert_eq!(json_of(&result)["status"], "dry_run");

    let check = Store::open_with_passphrase(&db_path, &Passphrase::from("s3cret")).unwrap();
    let count: i64 = check
        .connection()
        .query_row("SELECT count(*) FROM missions", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0, "un dry-run ne doit rien écrire");

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn prospect_update_accepts_a_reference_instead_of_a_uuid() {
    let db_path = test_db_path("prospect-update-by-name");
    let store = Store::create(&db_path, &Passphrase::from("s3cret")).unwrap();
    let client = spawn_client(store).await;

    call(&client, "clients.create", json!({"name": "Kappa Software"})).await;
    call(
        &client,
        "prospect.create",
        json!({
            "client": "Kappa Software",
            "name": "Refonte",
            "amount_cents": 100_000,
            "probability_percent": 50,
            "next_action": "2026-09-02",
        }),
    )
    .await;

    let updated = call(
        &client,
        "prospect.update",
        json!({"opportunity": "Refonte", "name": "Refonte v2"}),
    )
    .await;
    assert_eq!(updated.is_error, Some(false));

    let shown = call(
        &client,
        "prospect.show",
        json!({"opportunity": "Refonte v2"}),
    )
    .await;
    assert_eq!(json_of(&shown)["name"], "Refonte v2");

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn prospect_delete_from_an_agent_creates_a_pending_action_instead_of_deleting() {
    let db_path = test_db_path("prospect-delete-confirm");
    let store = Store::create(&db_path, &Passphrase::from("s3cret")).unwrap();
    let client = spawn_client(store).await;

    call(&client, "clients.create", json!({"name": "Kappa Software"})).await;
    call(
        &client,
        "prospect.create",
        json!({
            "client": "Kappa Software",
            "name": "Refonte",
            "amount_cents": 100_000,
            "probability_percent": 50,
            "next_action": "2026-09-02",
        }),
    )
    .await;

    let deleted = call(
        &client,
        "prospect.delete",
        json!({"opportunity": "Refonte"}),
    )
    .await;
    assert_eq!(deleted.is_error, Some(false));
    assert_eq!(json_of(&deleted)["status"], "pending_confirmation");

    let check = Store::open_with_passphrase(&db_path, &Passphrase::from("s3cret")).unwrap();
    let count: i64 = check
        .connection()
        .query_row("SELECT count(*) FROM opportunities", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        count, 1,
        "l'opportunité doit toujours exister tant qu'aucun humain n'a confirmé"
    );

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn reading_the_missions_resource_returns_the_active_missions() {
    let db_path = test_db_path("missions-resource");
    let store = Store::create(&db_path, &Passphrase::from("s3cret")).unwrap();
    let client = spawn_client(store).await;

    call(&client, "clients.create", json!({"name": "Kappa Software"})).await;
    call(
        &client,
        "mission.create",
        json!({
            "client": "Kappa Software",
            "name": "Refonte",
            "kind": "forfait",
            "budget_cents": 100_000,
            "started_on": "2026-09-01",
        }),
    )
    .await;

    let resources = client.list_resources(None).await.unwrap().resources;
    assert!(resources.iter().any(|r| r.uri == "freeflow://missions"));

    let read = client
        .read_resource(ReadResourceRequestParams::new("freeflow://missions"))
        .await
        .unwrap();
    let text = match &read.contents[0] {
        rmcp::model::ResourceContents::TextResourceContents { text, .. } => text.clone(),
        other => panic!("expected text contents, got {other:?}"),
    };
    let missions: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(missions.as_array().unwrap().len(), 1);
    assert_eq!(missions[0]["name"], "Refonte");

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn reading_an_opportunity_by_name_returns_its_interactions_and_references() {
    let db_path = test_db_path("opportunity-resource");
    let store = Store::create(&db_path, &Passphrase::from("s3cret")).unwrap();
    let client = spawn_client(store).await;

    call(&client, "clients.create", json!({"name": "Kappa Software"})).await;
    call(
        &client,
        "prospect.create",
        json!({
            "client": "Kappa Software",
            "name": "Refonte",
            "amount_cents": 100_000,
            "probability_percent": 50,
            "next_action": "2026-09-02",
        }),
    )
    .await;
    call(
        &client,
        "prospect.log_interaction",
        json!({"opportunity": "Refonte", "kind": "call", "note": "premier contact"}),
    )
    .await;

    let read = client
        .read_resource(ReadResourceRequestParams::new(
            "freeflow://opportunities/Refonte",
        ))
        .await
        .unwrap();
    let text = match &read.contents[0] {
        rmcp::model::ResourceContents::TextResourceContents { text, .. } => text.clone(),
        other => panic!("expected text contents, got {other:?}"),
    };
    let payload: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(payload["opportunity"]["name"], "Refonte");
    assert_eq!(payload["interactions"].as_array().unwrap().len(), 1);
    assert_eq!(payload["references"]["quotes"], 0);

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn reading_an_unknown_resource_uri_is_a_protocol_level_error() {
    let store = Store::create(
        &test_db_path("resource-unknown"),
        &Passphrase::from("s3cret"),
    )
    .unwrap();
    let client = spawn_client(store).await;

    let result = client
        .read_resource(ReadResourceRequestParams::new("freeflow://unknown"))
        .await;
    assert!(result.is_err());

    client.cancel().await.unwrap();
}
