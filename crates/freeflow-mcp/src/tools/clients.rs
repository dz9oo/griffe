//! Outils `clients.*` — miroir de `freeflow client ...` (CLI, lot 7).

use freeflow_core::app::Executor;
use freeflow_core::clients::{self, client_by_id, list_clients};
use freeflow_core::domain::{Address, ClientId, Siren, VatNumber};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::server::FreeflowServer;
use crate::support::{err_text, ok_json, ok_or_return, outcome_json};

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct CreateClientArgs {
    /// Raison sociale du client.
    name: String,
    /// SIREN à 9 chiffres, si connu.
    siren: Option<String>,
    /// Numéro de TVA intracommunautaire, si connu.
    vat_number: Option<String>,
    /// Adresse : les quatre champs (`street`, `postal_code`, `city`, `country`) doivent être
    /// fournis ensemble, ou omis ensemble.
    street: Option<String>,
    postal_code: Option<String>,
    city: Option<String>,
    /// Code pays ISO 3166-1 alpha-2, ex. `FR`.
    country: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ClientIdArgs {
    /// Identifiant du client (UUID).
    id: String,
}

#[tool_router(router = clients_router, vis = "pub(crate)")]
impl FreeflowServer {
    /// Crée un nouveau client.
    #[tool(
        name = "clients.create",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn clients_create(
        &self,
        Parameters(args): Parameters<CreateClientArgs>,
    ) -> CallToolResult {
        let siren = match &args.siren {
            Some(s) => Some(ok_or_return!("siren", Siren::parse(s))),
            None => None,
        };
        let vat_number = match &args.vat_number {
            Some(s) => Some(ok_or_return!("vat_number", VatNumber::parse(s))),
            None => None,
        };
        let address = match (args.street, args.postal_code, args.city, args.country) {
            (Some(street), Some(postal_code), Some(city), Some(country)) => Some(Address {
                street,
                postal_code,
                city,
                country,
            }),
            (None, None, None, None) => None,
            _ => {
                return err_text(
                    "street, postal_code, city et country doivent être fournis ensemble, ou omis ensemble",
                );
            }
        };
        let cmd = clients::CreateClient {
            name: args.name,
            siren,
            vat_number,
            address,
        };
        let mut store = self.store.lock().await;
        match Executor::new(&mut store).execute(&cmd, &self.ctx()) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Affiche un client par son identifiant.
    #[tool(
        name = "clients.show",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn clients_show(&self, Parameters(args): Parameters<ClientIdArgs>) -> CallToolResult {
        let id: ClientId = ok_or_return!("id", args.id.parse());
        let store = self.store.lock().await;
        match client_by_id(store.connection(), id) {
            Ok(client) => ok_json(client),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Liste tous les clients.
    #[tool(
        name = "clients.list",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn clients_list(&self) -> CallToolResult {
        let store = self.store.lock().await;
        match list_clients(store.connection()) {
            Ok(clients) => ok_json(clients),
            Err(e) => err_text(e.to_string()),
        }
    }
}
