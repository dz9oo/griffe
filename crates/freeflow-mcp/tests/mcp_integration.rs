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
        "invoice.render",
        "payment.record",
        "bank.import",
        "bank.reconcile",
        "expense.reconcile",
        "pending.list",
        "audit.verify_chain",
        "company.show",
        "company.set_profile",
        "forecast.show",
        "fiscal.calendar",
        "fiscal.deadlines",
        "fiscal.years",
        "fiscal.year_show",
        "fiscal.close_year",
        "fiscal.amend_year",
        "fiscal.approve_year",
        "fiscal.delete_year",
        "fiscal.render_year",
        "fiscal.opening_balance",
        "fiscal.set_opening_balance",
        "fiscal.delete_opening_balance",
        "fiscal.balance_sheet",
        "fec.export",
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
        "company.show",
        "forecast.show",
        "fiscal.calendar",
        "fiscal.deadlines",
        "fiscal.years",
        "fiscal.year_show",
        "fiscal.opening_balance",
        "fiscal.balance_sheet",
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
        "fiscal.delete_year",
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
async fn the_fiscal_calendar_tool_returns_dated_deadlines() {
    let store = Store::create(
        &test_db_path("fiscal-calendar"),
        &Passphrase::from("s3cret"),
    )
    .unwrap();
    let client = spawn_client(store).await;

    let calendar = call(&client, "fiscal.calendar", json!({"today": "2026-04-01"})).await;
    assert_eq!(calendar.is_error, Some(false));
    let rows = json_of(&calendar);
    let rows = rows.as_array().unwrap();
    assert!(
        !rows.is_empty(),
        "même sans profil, le calendrier retombe sur l'année civile et liste des échéances"
    );
    // Chaque échéance porte au moins un type et une date.
    for row in rows {
        assert!(row["kind"].is_string());
        assert!(row["due_on"].is_string());
    }

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

/// Profil minimal (exercice civil) écrit directement sur le `Store` avant de lancer le serveur —
/// l'outil MCP `company.set_profile` existe depuis le lot 25 (et est testé plus bas), mais les
/// scénarios fiscaux n'ont pas à en dépendre : ils testent le calendrier, pas la saisie du profil.
fn set_company_profile(store: &mut Store) {
    let cmd = freeflow_core::company::SetCompanyProfile {
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
        share_capital: None,
        rcs_city: None,
        iban: None,
        fiscal_year_end: Some(freeflow_core::domain::FiscalYearEnd::CALENDAR),
        vat_regime: None,
        director_monthly_gross: None,
        director_charge_ratio_bps: None,
    };
    Executor::new(store)
        .execute(
            &cmd,
            &freeflow_core::app::ExecutionContext::new(freeflow_core::app::Actor::Human, false),
        )
        .unwrap();
}

#[tokio::test]
async fn an_agent_closing_a_fiscal_year_only_deposits_a_pending_action() {
    let db_path = test_db_path("fiscal-close-gate");
    let mut store = Store::create(&db_path, &Passphrase::from("s3cret")).unwrap();
    set_company_profile(&mut store);
    let client = spawn_client(store).await;

    // L'option de report en arrière (lot 32) voyage avec la commande en attente.
    let closed = call(
        &client,
        "fiscal.close_year",
        json!({"period": 2026, "carry_back": true}),
    )
    .await;
    assert_eq!(closed.is_error, Some(false));
    let body = json_of(&closed);
    assert_eq!(body["status"], "pending_confirmation");
    assert!(body["pending_action_id"].as_str().is_some());

    // Rien n'est clos tant qu'un humain n'a pas confirmé — visible sur l'outil de liste comme
    // sur la ressource.
    let years = call(&client, "fiscal.years", json!({})).await;
    assert_eq!(json_of(&years).as_array().unwrap().len(), 0);
    let read = client
        .read_resource(ReadResourceRequestParams::new("freeflow://fiscal-years"))
        .await
        .unwrap();
    let text = match &read.contents[0] {
        rmcp::model::ResourceContents::TextResourceContents { text, .. } => text.clone(),
        other => panic!("expected text contents, got {other:?}"),
    };
    let payload: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(payload.as_array().unwrap().len(), 0);

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn expense_lifecycle_over_mcp() {
    let db_path = test_db_path("expense-lifecycle");
    let store = Store::create(&db_path, &Passphrase::from("s3cret")).unwrap();
    let client = spawn_client(store).await;

    let recorded = call(
        &client,
        "expense.record",
        json!({
            "label": "Abonnement hébergement",
            "category": "software",
            "amount_cents": 12_000,
            "vat_rate": "standard",
            "vat_deductible_cents": 2_000,
            "incurred_on": "2026-09-05",
        }),
    )
    .await;
    assert_eq!(recorded.is_error, Some(false));

    // Mise à jour par référence (préfixe de libellé) : seul le montant change.
    let updated = call(
        &client,
        "expense.update",
        json!({"expense": "abonnement", "amount_cents": 24_000, "vat_deductible_cents": 4_000}),
    )
    .await;
    assert_eq!(updated.is_error, Some(false));

    let shown = call(&client, "expense.show", json!({"expense": "abonnement"})).await;
    let expense = json_of(&shown);
    assert_eq!(expense["amount"], 24_000);
    assert_eq!(expense["revision"], 2);

    // Suppression proposée par l'agent : action en attente, jamais un effet direct.
    let deleted = call(&client, "expense.delete", json!({"expense": "abonnement"})).await;
    let pending_id = json_of(&deleted)["pending_action_id"]
        .as_str()
        .expect("une suppression de dépense proposée par un agent dépose une action en attente")
        .to_string();

    let mut confirming_store =
        Store::open_with_passphrase(&db_path, &Passphrase::from("s3cret")).unwrap();
    let confirmed = Executor::new(&mut confirming_store)
        .confirm::<freeflow_core::expenses::DeleteExpense>(pending_id.parse().unwrap())
        .unwrap();
    assert!(matches!(confirmed, Outcome::Applied(())));

    let empty = call(&client, "expense.list", json!({})).await;
    assert!(json_of(&empty).as_array().unwrap().is_empty());

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn expense_reconciliation_over_mcp_needs_a_human() {
    // Lot 33 : rapprocher une dépense d'un débit du relevé — depuis le débit (`expense.record`
    // avec `bank_transaction_id`) ou après coup (`expense.reconcile`) — est une action de
    // rapprochement bancaire : l'agent la propose, un humain la confirme.
    let db_path = test_db_path("expense-reconciliation");
    let store = Store::create(&db_path, &Passphrase::from("s3cret")).unwrap();
    let client = spawn_client(store).await;

    let imported = call(
        &client,
        "bank.import",
        json!({
            "format": "csv",
            "content": "date;description;montant\n2026-09-07;PRLV CABINET COMPTA;-960.00\n2026-09-09;FRAIS;-12.50\n",
        }),
    )
    .await;
    assert_eq!(imported.is_error, Some(false));
    let listed = call(&client, "bank.list", json!({"unmatched": true})).await;
    let transactions = json_of(&listed);
    assert_eq!(transactions.as_array().unwrap().len(), 2);
    let fees_tx = transactions[1]["id"].as_str().unwrap().to_string();
    let bank_tx = transactions[0]["id"].as_str().unwrap().to_string();

    // Depuis le débit : montant et date repris, action en attente.
    let proposed = call(
        &client,
        "expense.record",
        json!({
            "label": "Expert-comptable",
            "category": "fees",
            "vat_rate": "standard",
            "vat_deductible_cents": 16_000,
            "bank_transaction_id": fees_tx,
        }),
    )
    .await;
    let pending_id = json_of(&proposed)["pending_action_id"]
        .as_str()
        .expect("une dépense rapprochée proposée par un agent dépose une action en attente")
        .to_string();
    assert!(
        json_of(&call(&client, "expense.list", json!({})).await)
            .as_array()
            .unwrap()
            .is_empty()
    );
    let mut confirming_store =
        Store::open_with_passphrase(&db_path, &Passphrase::from("s3cret")).unwrap();
    let confirmed = Executor::new(&mut confirming_store)
        .confirm::<freeflow_core::expenses::RecordExpense>(pending_id.parse().unwrap())
        .unwrap();
    assert!(matches!(confirmed, Outcome::Applied(_)));
    drop(confirming_store);

    let shown = call(&client, "expense.show", json!({"expense": "expert"})).await;
    let expense = json_of(&shown);
    assert_eq!(expense["amount"], 96_000);
    // `time::Date` se sérialise en (année, jour ordinal) : le 7 septembre 2026 est le 250e.
    assert_eq!(expense["incurred_on"], json!([2026, 250]));
    assert_eq!(expense["bank_transaction"]["id"], fees_tx);

    // Sans débit, `amount_cents`/`incurred_on` restent requis — et une dépense ordinaire ne
    // demande pas de confirmation.
    let refused = call(
        &client,
        "expense.record",
        json!({"label": "x", "category": "other", "vat_rate": "zero", "vat_deductible_cents": 0}),
    )
    .await;
    assert_eq!(refused.is_error, Some(true));
    let recorded = call(
        &client,
        "expense.record",
        json!({
            "label": "Frais bancaires",
            "category": "bank_charges",
            "amount_cents": 1_250,
            "vat_rate": "zero",
            "vat_deductible_cents": 0,
            "incurred_on": "2026-09-09",
        }),
    )
    .await;
    assert_eq!(json_of(&recorded)["status"], "applied");

    // Après coup : action en attente ; confirmée, le débit n'est plus à rapprocher.
    let proposed = call(
        &client,
        "expense.reconcile",
        json!({"expense": "frais", "transaction_id": bank_tx}),
    )
    .await;
    let pending_id = json_of(&proposed)["pending_action_id"]
        .as_str()
        .expect("un rapprochement proposé par un agent dépose une action en attente")
        .to_string();
    let mut confirming_store =
        Store::open_with_passphrase(&db_path, &Passphrase::from("s3cret")).unwrap();
    Executor::new(&mut confirming_store)
        .confirm::<freeflow_core::expenses::ReconcileExpense>(pending_id.parse().unwrap())
        .unwrap();
    drop(confirming_store);
    let listed = call(&client, "bank.list", json!({"unmatched": true})).await;
    assert!(json_of(&listed).as_array().unwrap().is_empty());

    // La ressource de détail porte aussi le débit.
    let resource = client
        .read_resource(ReadResourceRequestParams::new("freeflow://expenses/frais"))
        .await
        .unwrap();
    let rmcp::model::ResourceContents::TextResourceContents { text, .. } = &resource.contents[0]
    else {
        panic!("contenu texte attendu")
    };
    let detail: Value = serde_json::from_str(text).unwrap();
    assert_eq!(detail["expense"]["bank_transaction"]["id"], bank_tx);

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn a_quote_is_readable_by_tool_and_resource_after_creation() {
    let store = Store::create(&test_db_path("quote-read"), &Passphrase::from("s3cret")).unwrap();
    let client = spawn_client(store).await;

    call(&client, "clients.create", json!({"name": "Kappa Software"})).await;
    let lines = r#"[{"description":"Refonte plateforme","kind":{"Forfait":{"amount":4500000}},"vat_rate":"Standard"}]"#;
    let created = call(
        &client,
        "quote.create",
        json!({"client": "kappa", "lines_json": lines, "valid_until": "2026-10-31"}),
    )
    .await;
    assert_eq!(created.is_error, Some(false));

    // Lisible par l'outil, adressé par le nom du client porteur…
    let shown = call(&client, "quote.show", json!({"quote": "kappa"})).await;
    let view = json_of(&shown);
    assert_eq!(view["quote"]["status"], "Draft");
    assert_eq!(view["total_net_ht"], 4_500_000);
    assert_eq!(view["references"]["versions"], 1);

    // … et par la ressource de collection.
    let resource = client
        .read_resource(ReadResourceRequestParams::new("freeflow://quotes"))
        .await
        .unwrap();
    let rmcp::model::ResourceContents::TextResourceContents { text, .. } = &resource.contents[0]
    else {
        panic!("contenu texte attendu")
    };
    let quotes: Value = serde_json::from_str(text).unwrap();
    assert_eq!(quotes.as_array().unwrap().len(), 1);

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn a_payment_correction_proposed_by_an_agent_waits_for_a_human() {
    let db_path = test_db_path("payment-void");
    let mut store = Store::create(&db_path, &Passphrase::from("s3cret")).unwrap();
    let human = freeflow_core::app::ExecutionContext::new(freeflow_core::app::Actor::Human, false);

    // Fixture posée en acteur humain, avant de donner le coffre au serveur : une facture payée.
    let client_id = freeflow_core::domain::ClientId::new();
    store
        .connection()
        .execute(
            "INSERT INTO clients (id, name, created_at) \
             VALUES (?1, 'Kappa Software', '2026-01-01T00:00:00Z')",
            [client_id.to_string()],
        )
        .unwrap();
    let Outcome::Applied(emitted) = Executor::new(&mut store)
        .execute(
            &EmitInvoice {
                client_id,
                mission_id: None,
                lines: vec![freeflow_core::domain::InvoiceLine {
                    description: "Prestation".to_string(),
                    quantity: 10.0,
                    unit_price: freeflow_core::domain::Money::from_cents(65_000),
                    vat_rate: freeflow_core::domain::VatRate::Standard,
                }],
                issued_on: time::Date::from_calendar_date(2026, time::Month::September, 1).unwrap(),
                payment_terms_days: 30,
            },
            &human,
        )
        .unwrap()
    else {
        panic!("expected Applied")
    };
    Executor::new(&mut store)
        .execute(
            &RecordPayment {
                invoice_id: emitted.id,
                amount: freeflow_core::domain::Money::from_cents(780_000),
                received_on: time::Date::from_calendar_date(2026, time::Month::September, 10)
                    .unwrap(),
                method: freeflow_core::domain::PaymentMethod::BankTransfer,
            },
            &human,
        )
        .unwrap();

    let client = spawn_client(store).await;

    let listed = call(&client, "payment.list", json!({})).await;
    let payments = json_of(&listed);
    assert_eq!(payments.as_array().unwrap().len(), 1);
    let payment_id = payments[0]["id"].as_str().unwrap().to_string();

    // L'annulation proposée par l'agent reste en attente — et le dry_run (dette remboursée au
    // lot 22) n'écrit rien du tout.
    let dry = call(
        &client,
        "payment.void",
        json!({"payment_id": payment_id, "dry_run": true}),
    )
    .await;
    assert_eq!(json_of(&dry)["status"], "dry_run");

    let voided = call(
        &client,
        "payment.void",
        json!({"payment_id": payment_id, "reason": "saisi en double"}),
    )
    .await;
    let pending_id = json_of(&voided)["pending_action_id"]
        .as_str()
        .expect("une annulation proposée par un agent dépose une action en attente")
        .to_string();

    let still_active = call(&client, "payment.list", json!({})).await;
    assert!(
        json_of(&still_active)[0]["voided_at"].is_null(),
        "rien n'est annulé tant qu'un humain n'a pas confirmé"
    );

    let mut confirming_store =
        Store::open_with_passphrase(&db_path, &Passphrase::from("s3cret")).unwrap();
    let confirmed = Executor::new(&mut confirming_store)
        .confirm::<freeflow_core::billing::VoidPayment>(pending_id.parse().unwrap())
        .unwrap();
    assert!(matches!(confirmed, Outcome::Applied(())));

    let after = call(&client, "payment.list", json!({})).await;
    assert!(
        !json_of(&after)[0]["voided_at"].is_null(),
        "la contre-écriture reste listée, marquée annulée"
    );

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn the_company_profile_roundtrips_through_set_profile_show_and_the_company_resource() {
    let store = Store::create(&test_db_path("company"), &Passphrase::from("s3cret")).unwrap();
    let client = spawn_client(store).await;

    let empty = call(&client, "company.show", json!(null)).await;
    assert_eq!(empty.is_error, Some(false));
    assert!(json_of(&empty).is_null(), "aucun profil au départ");

    let set = call(
        &client,
        "company.set_profile",
        json!({
            "name": "Lumen Conseil",
            "legal_form": "SASU",
            "siren": "552100554",
            "street": "1 rue de la Paix",
            "postal_code": "75002",
            "city": "Paris",
            "country": "FR",
            "share_capital_cents": 100_000,
            "fiscal_year_end": "31/12",
            "vat_regime": "real_normal_quarterly",
        }),
    )
    .await;
    assert_eq!(set.is_error, Some(false));
    assert_eq!(json_of(&set)["status"], "applied");

    let shown = call(&client, "company.show", json!(null)).await;
    let profile = json_of(&shown);
    assert_eq!(profile["name"], "Lumen Conseil");
    assert_eq!(profile["siren"], "552100554");
    // La règle de télédéclaration dérivée : SASU parisienne au SIREN 55… → le 23 du mois, en
    // trimestriel puisque c'est le régime déclaré.
    assert_eq!(profile["vat_filing"]["rule"]["day"], 23);
    assert_eq!(profile["vat_filing"]["scheme"], "ca3_quarterly");

    let read = client
        .read_resource(ReadResourceRequestParams::new("freeflow://company"))
        .await
        .unwrap();
    let text = match &read.contents[0] {
        rmcp::model::ResourceContents::TextResourceContents { text, .. } => text.clone(),
        other => panic!("expected text contents, got {other:?}"),
    };
    let resource: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(resource["name"], "Lumen Conseil");
    assert_eq!(resource["vat_filing"]["scheme"], "ca3_quarterly");

    let bad = call(
        &client,
        "company.set_profile",
        json!({
            "name": "X",
            "legal_form": "SASU",
            "siren": "123",
            "street": "s",
            "postal_code": "p",
            "city": "c",
            "country": "FR",
        }),
    )
    .await;
    assert_eq!(
        bad.is_error,
        Some(true),
        "un SIREN invalide est une erreur d'outil, visible par l'agent"
    );

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn the_forecast_tool_projects_twelve_months() {
    let store = Store::create(&test_db_path("forecast"), &Passphrase::from("s3cret")).unwrap();
    let client = spawn_client(store).await;

    let forecast = call(
        &client,
        "forecast.show",
        json!({"starting_cash_cents": 1_000_000, "today": "2026-09-01"}),
    )
    .await;
    assert_eq!(forecast.is_error, Some(false));
    let body = json_of(&forecast);
    assert_eq!(body["months"].as_array().unwrap().len(), 12);
    assert!(
        body.as_object()
            .unwrap()
            .contains_key("first_shortfall_month"),
        "le premier mois en découvert est toujours annoncé, même null"
    );

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn the_fiscal_deadlines_tool_returns_dated_deadlines() {
    let store = Store::create(&test_db_path("deadlines"), &Passphrase::from("s3cret")).unwrap();
    let client = spawn_client(store).await;

    let deadlines = call(&client, "fiscal.deadlines", json!({"today": "2026-04-01"})).await;
    assert_eq!(deadlines.is_error, Some(false));
    let rows = json_of(&deadlines);
    assert!(!rows.as_array().unwrap().is_empty());
    for row in rows.as_array().unwrap() {
        assert!(row["kind"].is_string());
        assert!(row["due_on"].is_string());
    }

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn a_fiscal_year_can_be_shown_and_amended_but_approval_and_deletion_need_a_human() {
    use freeflow_core::app::{Actor, ExecutionContext};
    use freeflow_core::company::SetCompanyProfile;
    use freeflow_core::domain::{Address, Money, Siren};
    use freeflow_core::fiscal_year::CloseFiscalYear;

    let db_path = test_db_path("year-lifecycle");
    let mut store = Store::create(&db_path, &Passphrase::from("s3cret")).unwrap();

    // Clore l'exercice en humain, hors du canal MCP : `fiscal.close_year` proposé par un agent
    // ne fait que déposer une action en attente (déjà couvert par ailleurs). La clôture exige un
    // profil d'entreprise (le résultat et la réserve légale en dépendent) — posé d'abord.
    let human = ExecutionContext::new(Actor::Human, false);
    Executor::new(&mut store)
        .execute(
            &SetCompanyProfile {
                name: "Lumen Conseil".into(),
                legal_form: "SASU".into(),
                siren: Siren::parse("552100554").unwrap(),
                vat_number: None,
                address: Address {
                    street: "1 rue de la Paix".into(),
                    postal_code: "75002".into(),
                    city: "Paris".into(),
                    country: "FR".into(),
                },
                share_capital: None,
                rcs_city: None,
                iban: None,
                fiscal_year_end: None,
                vat_regime: None,
                director_monthly_gross: None,
                director_charge_ratio_bps: None,
            },
            &human,
        )
        .unwrap();
    let closed = Executor::new(&mut store)
        .execute(
            &CloseFiscalYear {
                starts_on: time::Date::from_calendar_date(2025, time::Month::January, 1).unwrap(),
                ends_on: time::Date::from_calendar_date(2025, time::Month::December, 31).unwrap(),
                legal_reserve: Money::from_cents(0),
                dividends: Money::from_cents(0),
                carry_back: false,
            },
            &human,
        )
        .unwrap();
    assert!(matches!(closed, Outcome::Applied(_)));
    let client = spawn_client(store).await;

    let shown = call(&client, "fiscal.year_show", json!({"period": 2025})).await;
    assert_eq!(shown.is_error, Some(false));
    let record = json_of(&shown);
    assert_eq!(record["ends_on"], "2025-12-31");
    assert!(record["approved_on"].is_null());
    // Lot 32 : le suivi des déficits fait partie de la vue partagée.
    assert_eq!(record["taxable_result_cents"], 0);
    assert_eq!(record["losses_imputed_cents"], 0);
    assert_eq!(record["carry_back_credit_cents"], 0);
    assert_eq!(record["losses_carried_forward_cents"], 0);

    // Lot 31 : balance et bilan dérivés du grand livre — outil et ressource partagent la vue.
    let sheet = call(&client, "fiscal.balance_sheet", json!({"period": 2025})).await;
    assert_eq!(sheet.is_error, Some(false));
    let sheet = json_of(&sheet);
    assert_eq!(sheet["balance_sheet"]["balanced"], true);
    assert_eq!(sheet["ends_on"], "2025-12-31");
    let resource = client
        .read_resource(ReadResourceRequestParams::new(
            "freeflow://balance-sheet/2025",
        ))
        .await
        .unwrap();
    let text = match &resource.contents[0] {
        rmcp::model::ResourceContents::TextResourceContents { text, .. } => text.clone(),
        other => panic!("contenu inattendu : {other:?}"),
    };
    let via_resource: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(via_resource, sheet);

    let amended = call(
        &client,
        "fiscal.amend_year",
        json!({"period": 2025, "legal_reserve_cents": 0}),
    )
    .await;
    assert_eq!(amended.is_error, Some(false));
    assert_eq!(
        json_of(&amended)["status"],
        "applied",
        "réviser une affectation en projet n'exige pas de confirmation"
    );

    let approved = call(
        &client,
        "fiscal.approve_year",
        json!({"period": 2025, "approved_on": "2026-06-30"}),
    )
    .await;
    assert_eq!(json_of(&approved)["status"], "pending_confirmation");

    let deleted = call(&client, "fiscal.delete_year", json!({"period": 2025})).await;
    assert_eq!(json_of(&deleted)["status"], "pending_confirmation");

    let still_there = call(&client, "fiscal.year_show", json!({"period": 2025})).await;
    let record = json_of(&still_there);
    assert!(
        record["approved_on"].is_null(),
        "rien n'est approuvé ni supprimé tant qu'un humain n'a pas confirmé"
    );

    // La liasse (JSON) exerce le rendu de document sans dépendre de `typst` — et le refus
    // d'écraser un fichier existant, la garde propre à l'adaptateur MCP.
    let out = db_path.parent().unwrap().join("liasse-2025.json");
    let rendered = call(
        &client,
        "fiscal.render_year",
        json!({"period": 2025, "doc": "liasse", "out": out.to_str().unwrap()}),
    )
    .await;
    assert_eq!(rendered.is_error, Some(false));
    let liasse: Value = serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    assert!(liasse.is_object());
    let overwrite = call(
        &client,
        "fiscal.render_year",
        json!({"period": 2025, "doc": "liasse", "out": out.to_str().unwrap()}),
    )
    .await;
    assert_eq!(
        overwrite.is_error,
        Some(true),
        "un fichier existant n'est jamais écrasé par l'agent"
    );

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn exporting_the_fec_writes_the_regulatory_file_and_never_overwrites() {
    let db_path = test_db_path("fec-export");
    let mut store = Store::create(&db_path, &Passphrase::from("s3cret")).unwrap();
    let human = freeflow_core::app::ExecutionContext::new(freeflow_core::app::Actor::Human, false);
    Executor::new(&mut store)
        .execute(
            &freeflow_core::company::SetCompanyProfile {
                name: "Lumen Conseil".into(),
                legal_form: "SASU".into(),
                siren: freeflow_core::domain::Siren::parse("552100554").unwrap(),
                vat_number: None,
                address: freeflow_core::domain::Address {
                    street: "1 rue de la Paix".into(),
                    postal_code: "75002".into(),
                    city: "Paris".into(),
                    country: "FR".into(),
                },
                share_capital: None,
                rcs_city: None,
                iban: None,
                fiscal_year_end: None,
                vat_regime: None,
                director_monthly_gross: None,
                director_charge_ratio_bps: None,
            },
            &human,
        )
        .unwrap();
    let client_id = freeflow_core::domain::ClientId::new();
    store
        .connection()
        .execute(
            "INSERT INTO clients (id, name, created_at) \
             VALUES (?1, 'Kappa Software', '2026-01-01T00:00:00Z')",
            [client_id.to_string()],
        )
        .unwrap();
    Executor::new(&mut store)
        .execute(
            &EmitInvoice {
                client_id,
                mission_id: None,
                lines: vec![freeflow_core::domain::InvoiceLine {
                    description: "Prestation".to_string(),
                    quantity: 2.0,
                    unit_price: freeflow_core::domain::Money::from_cents(100_000),
                    vat_rate: freeflow_core::domain::VatRate::Standard,
                }],
                issued_on: time::Date::from_calendar_date(2026, time::Month::March, 10).unwrap(),
                payment_terms_days: 30,
            },
            &human,
        )
        .unwrap();
    let client = spawn_client(store).await;

    // `out` sur un répertoire : le fichier prend son nom réglementaire.
    let dir = db_path.parent().unwrap().to_path_buf();
    let exported = call(
        &client,
        "fec.export",
        json!({"period": 2026, "out": dir.to_str().unwrap()}),
    )
    .await;
    assert_eq!(exported.is_error, Some(false), "{exported:?}");
    let result = json_of(&exported);
    assert_eq!(result["summary"]["file_name"], "552100554FEC20261231.txt");
    // La facture, puis l'IS de clôture en OD (lot 31) : 15 % de 2 000 € = 300 €.
    assert_eq!(result["summary"]["entries"], 2);
    assert_eq!(result["summary"]["total_debit_cents"], 270_000);
    let path = dir.join("552100554FEC20261231.txt");
    assert_eq!(result["path"], path.to_str().unwrap());
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.starts_with("JournalCode|JournalLib|"), "{content}");
    assert!(content.contains("|FA-2026-0001|20260310|"), "{content}");

    // Un fichier existant n'est jamais écrasé par l'agent — la garde propre au canal MCP.
    let again = call(
        &client,
        "fec.export",
        json!({"period": 2026, "out": dir.to_str().unwrap()}),
    )
    .await;
    assert_eq!(again.is_error, Some(true));

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn rendering_an_invoice_writes_a_facturx_pdf() {
    let db_path = test_db_path("invoice-render");
    let store = Store::create(&db_path, &Passphrase::from("s3cret")).unwrap();
    let client = spawn_client(store).await;

    let profile_set = call(
        &client,
        "company.set_profile",
        json!({
            "name": "Lumen Conseil",
            "legal_form": "SASU",
            "siren": "552100554",
            "street": "1 rue de la Paix",
            "postal_code": "75002",
            "city": "Paris",
            "country": "FR",
            "vat_number": "FR96552100554",
        }),
    )
    .await;
    assert_eq!(
        profile_set.is_error,
        Some(false),
        "{:?}",
        profile_set.content
    );
    let created = call(&client, "clients.create", json!({"name": "Kappa Software"})).await;
    let client_id = json_of(&created)["result"].as_str().unwrap().to_string();
    let lines_json = serde_json::to_string(&json!([
        {"description": "Sept.", "quantity": 2.0, "unit_price": 65000, "vat_rate": "Standard"}
    ]))
    .unwrap();
    let emitted = call(
        &client,
        "invoice.emit",
        json!({"client_id": client_id, "lines_json": lines_json, "issued_on": "2026-09-01"}),
    )
    .await;
    let pending_id = json_of(&emitted)["pending_action_id"]
        .as_str()
        .unwrap()
        .to_string();
    let mut confirming_store =
        Store::open_with_passphrase(&db_path, &Passphrase::from("s3cret")).unwrap();
    let confirmed = Executor::new(&mut confirming_store)
        .confirm::<EmitInvoice>(pending_id.parse().unwrap())
        .unwrap();
    let Outcome::Applied(invoice) = confirmed else {
        panic!("expected Applied")
    };

    let out = db_path.parent().unwrap().join("facture.pdf");
    let rendered = call(
        &client,
        "invoice.render",
        json!({"id": invoice.id.to_string(), "out": out.to_str().unwrap()}),
    )
    .await;
    assert_eq!(rendered.is_error, Some(false), "{:?}", rendered.content);
    let bytes = std::fs::read(&out).unwrap();
    assert!(bytes.starts_with(b"%PDF"), "le fichier écrit est un PDF");

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn an_agent_setting_the_opening_balance_only_deposits_a_pending_action() {
    let db_path = test_db_path("opening-gate");
    let mut store = Store::create(&db_path, &Passphrase::from("s3cret")).unwrap();
    set_company_profile(&mut store);
    let client = spawn_client(store).await;

    // Rien d'enregistré : outil et ressource répondent null.
    let none = call(&client, "fiscal.opening_balance", json!({})).await;
    assert_eq!(none.is_error, Some(false));
    assert!(json_of(&none).is_null());

    let set = call(
        &client,
        "fiscal.set_opening_balance",
        json!({
            "opens_on": "2026-01-01",
            "source": "bilan au 31/12/2025",
            "lines": ["101000:Capital social:C:1000.00", "512000:Banque:D:1000.00"],
            "tax_losses_cents": 300_000,
        }),
    )
    .await;
    assert_eq!(set.is_error, Some(false));
    assert_eq!(json_of(&set)["status"], "pending_confirmation");
    let still_none = call(&client, "fiscal.opening_balance", json!({})).await;
    assert!(json_of(&still_none).is_null());
    let read = client
        .read_resource(ReadResourceRequestParams::new("freeflow://opening-balance"))
        .await
        .unwrap();
    let text = match &read.contents[0] {
        rmcp::model::ResourceContents::TextResourceContents { text, .. } => text.clone(),
        other => panic!("expected text contents, got {other:?}"),
    };
    assert_eq!(text.trim(), "null");

    // Une ligne mal formée ou un bilan déséquilibré sont refusés avant tout dépôt d'action.
    let malformed = call(
        &client,
        "fiscal.set_opening_balance",
        json!({"opens_on": "2026-01-01", "lines": ["101000:Capital"]}),
    )
    .await;
    assert_eq!(malformed.is_error, Some(true));
    let deleted = call(&client, "fiscal.delete_opening_balance", json!({})).await;
    assert_eq!(deleted.is_error, Some(true), "rien à supprimer");
}
