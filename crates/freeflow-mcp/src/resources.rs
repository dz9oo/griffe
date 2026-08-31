//! Ressources MCP `freeflow://clients` (les clients actifs) et `freeflow://clients/{référence}`
//! (fiche d'un client, contacts et références comprises) — la doctrine annoncée dans
//! `CLAUDE.md` (« ressources = requêtes ») n'avait jusqu'ici aucune ressource réelle. Une
//! lecture de ressource coûte moins de jetons à un agent qu'un appel d'outil de liste, et ne
//! laisse aucune trace dans le journal d'audit (une lecture n'en est pas une, comme toute
//! `Query` — voir `freeflow_core::app`).

use freeflow_core::clients::{
    ClientFilter, client_by_id, client_references, list_clients_with, list_contacts,
};
use freeflow_core::store::Store;
use rmcp::model::{
    ErrorData as McpError, ListResourceTemplatesResult, ListResourcesResult, ReadResourceResult,
    Resource, ResourceContents, ResourceTemplate,
};
use serde_json::json;

const CLIENTS_COLLECTION_URI: &str = "freeflow://clients";
const CLIENT_DETAIL_PREFIX: &str = "freeflow://clients/";

pub(crate) fn list() -> ListResourcesResult {
    ListResourcesResult::with_all_items(vec![
        Resource::new(CLIENTS_COLLECTION_URI, "clients")
            .with_description(
                "Clients actifs, en JSON — voir l'outil clients.list pour inclure les archivés.",
            )
            .with_mime_type("application/json"),
    ])
}

pub(crate) fn list_templates() -> ListResourceTemplatesResult {
    ListResourceTemplatesResult::with_all_items(vec![
        ResourceTemplate::new(format!("{CLIENT_DETAIL_PREFIX}{{reference}}"), "client")
            .with_description(
                "Fiche d'un client (UUID, préfixe d'UUID, ou nom — voir freeflow_core::reference), \
                 avec ses contacts et ce qui le référence.",
            )
            .with_mime_type("application/json"),
    ])
}

fn json_contents(uri: &str, value: impl serde::Serialize) -> Result<ReadResourceResult, McpError> {
    let text = serde_json::to_string_pretty(&value)
        .map_err(|e| McpError::resource_not_found(e.to_string(), None))?;
    Ok(ReadResourceResult::new(vec![
        ResourceContents::text(text, uri).with_mime_type("application/json"),
    ]))
}

pub(crate) fn read(store: &Store, uri: &str) -> Result<ReadResourceResult, McpError> {
    if uri == CLIENTS_COLLECTION_URI {
        let clients = list_clients_with(store.connection(), ClientFilter::ActiveOnly)
            .map_err(|e| McpError::resource_not_found(e.to_string(), None))?;
        return json_contents(uri, clients);
    }

    if let Some(reference) = uri.strip_prefix(CLIENT_DETAIL_PREFIX) {
        let id = crate::support::resolve_client(store, reference)
            .map_err(|e| McpError::resource_not_found(e, None))?;
        let client = client_by_id(store.connection(), id)
            .map_err(|e| McpError::resource_not_found(e.to_string(), None))?
            .ok_or_else(|| {
                McpError::resource_not_found(format!("client introuvable : {id}"), None)
            })?;
        let contacts = list_contacts(store.connection(), id)
            .map_err(|e| McpError::resource_not_found(e.to_string(), None))?;
        let references = client_references(store.connection(), id)
            .map_err(|e| McpError::resource_not_found(e.to_string(), None))?;
        return json_contents(
            uri,
            json!({ "client": client, "contacts": contacts, "references": references }),
        );
    }

    Err(McpError::resource_not_found(
        format!("ressource inconnue : {uri}"),
        None,
    ))
}
