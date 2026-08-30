//! Tests d'intégration du serveur MCP (lot 8) : un vrai client `rmcp`, en mémoire
//! (`tokio::io::duplex`), en face du vrai `FreeflowServer` — pas de mock du protocole MCP, pas
//! de mock de la base (SQLCipher réelle, temporaire).

use std::collections::BTreeSet;
use std::path::PathBuf;

use freeflow_core::store::Store;
use freeflow_mcp::FreeflowServer;
use rmcp::RoleClient;
use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, CallToolResult, ContentBlock};
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
    let store = Store::open_with_passphrase(&test_db_path("list-tools"), "s3cret").unwrap();
    let client = spawn_client(store).await;

    let tools = client.list_tools(None).await.unwrap().tools;
    let names: BTreeSet<String> = tools.iter().map(|t| t.name.to_string()).collect();

    for expected in [
        "clients.create",
        "clients.show",
        "clients.list",
        "prospect.create",
        "prospect.advance",
        "prospect.win",
        "prospect.lose",
        "prospect.log_interaction",
        "prospect.late",
        "prospect.orphans",
        "prospect.pipeline",
        "mission.create",
        "mission.log_time",
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
        "pending.confirm",
        "audit.verify_chain",
    ] {
        assert!(names.contains(expected), "outil manquant : {expected}");
    }

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

    // Les commandes qui déclarent `requires_confirmation()` côté core (lot 2/5) doivent être
    // annoncées comme destructives côté MCP — c'est ce qui doit alerter un agent avant appel.
    for destructive in ["invoice.emit", "invoice.credit_note", "pending.confirm"] {
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
    let store = Store::open_with_passphrase(&db_path, "s3cret").unwrap();
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
    let check = Store::open_with_passphrase(&db_path, "s3cret").unwrap();
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
    let store = Store::open_with_passphrase(&test_db_path("parse-error"), "s3cret").unwrap();
    let client = spawn_client(store).await;

    let result = call(&client, "clients.show", json!({"id": "not-a-uuid"})).await;

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
    let store = Store::open_with_passphrase(&db_path, "s3cret").unwrap();
    let client = spawn_client(store).await;

    let created = call(&client, "clients.create", json!({"name": "Kappa Software"})).await;
    let client_id = json_of(&created)["result"].as_str().unwrap().to_string();

    let opportunity = call(
        &client,
        "prospect.create",
        json!({
            "client_id": client_id,
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
        json!({"id": opportunity_id, "started_on": "2026-09-01"}),
    )
    .await;
    assert_eq!(won.is_error, Some(false));
    let mission_id = json_of(&won)["result"].as_str().unwrap().to_string();

    let logged = call(
        &client,
        "mission.log_time",
        json!({
            "mission_id": mission_id,
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

    let confirmed = call(&client, "pending.confirm", json!({"id": pending_id})).await;
    assert_eq!(confirmed.is_error, Some(false));
    let confirmed_body = json_of(&confirmed);
    assert_eq!(confirmed_body["status"], "applied");
    let invoice_id = confirmed_body["result"]["id"].as_str().unwrap().to_string();

    let chain = call(&client, "invoice.verify_chain", json!(null)).await;
    assert_eq!(json_of(&chain)["status"], "intact");

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
