//! Outils `clients.*` — miroir de `freeflow client ...` (CLI). Toute référence à un client
//! (`client`, `id`) accepte un UUID complet, un préfixe d'UUID, ou un nom (voir
//! `griffe_core::reference`) : un agent n'a pas à retenir un identifiant pour agir sur un
//! client qu'il vient de lister.

use griffe_core::app::Executor;
use griffe_core::clients::{
    self, ClientFilter, client_by_id, client_references, contact_by_id, list_clients_with,
    list_contacts,
};
use griffe_core::domain::{Address, ContactId, Siren, VatNumber};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::server::FreeflowServer;
use crate::support::{err_text, ok_json, ok_or_return, outcome_json, resolve_client};

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
    /// N'écrit rien, montre ce qui serait fait (défaut : faux).
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ClientRefArgs {
    /// Référence du client : UUID, préfixe d'UUID, ou nom (voir `clients.list`).
    client: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ListClientsArgs {
    /// Inclut les clients archivés si vrai (défaut : faux, seuls les clients actifs).
    #[serde(default)]
    archived: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct UpdateClientArgs {
    /// Référence du client à modifier.
    client: String,
    /// Chaque champ fourni remplace la valeur actuelle ; un champ omis est conservé tel quel.
    name: Option<String>,
    siren: Option<String>,
    vat_number: Option<String>,
    street: Option<String>,
    postal_code: Option<String>,
    city: Option<String>,
    country: Option<String>,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ClientRefMutationArgs {
    client: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct CreateContactArgs {
    /// Référence du client auquel rattacher ce contact.
    client: String,
    name: String,
    email: Option<String>,
    phone: Option<String>,
    role: Option<String>,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ListContactsArgs {
    client: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct UpdateContactArgs {
    /// Identifiant du contact (UUID) — voir `clients.contacts.list`.
    id: String,
    name: Option<String>,
    email: Option<String>,
    phone: Option<String>,
    role: Option<String>,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ContactIdArgs {
    id: String,
    #[serde(default)]
    dry_run: bool,
}

/// Combine les quatre champs d'adresse optionnels — tous fournis, aucun, ou l'adresse actuelle
/// en repli si aucun n'est fourni. Une combinaison partielle est un usage incorrect de l'outil.
fn merge_address(
    street: Option<String>,
    postal_code: Option<String>,
    city: Option<String>,
    country: Option<String>,
    fallback: Option<Address>,
) -> Result<Option<Address>, CallToolResult> {
    match (street, postal_code, city, country) {
        (Some(street), Some(postal_code), Some(city), Some(country)) => Ok(Some(Address {
            street,
            postal_code,
            city,
            country,
        })),
        (None, None, None, None) => Ok(fallback),
        _ => Err(err_text(
            "street, postal_code, city et country doivent être fournis ensemble, ou omis ensemble",
        )),
    }
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
        let address =
            match merge_address(args.street, args.postal_code, args.city, args.country, None) {
                Ok(a) => a,
                Err(result) => return result,
            };
        let cmd = clients::CreateClient {
            name: args.name,
            siren,
            vat_number,
            address,
        };
        let mut store = self.store.lock().await;
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Affiche un client.
    #[tool(
        name = "clients.show",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn clients_show(&self, Parameters(args): Parameters<ClientRefArgs>) -> CallToolResult {
        let store = self.store.lock().await;
        let id = ok_or_return!("client", resolve_client(&store, &args.client));
        match client_by_id(store.connection(), id) {
            Ok(client) => ok_json(client),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Liste les clients actifs (`archived: true` pour inclure les archivés).
    #[tool(
        name = "clients.list",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn clients_list(&self, Parameters(args): Parameters<ListClientsArgs>) -> CallToolResult {
        let store = self.store.lock().await;
        let filter = if args.archived {
            ClientFilter::All
        } else {
            ClientFilter::ActiveOnly
        };
        match list_clients_with(store.connection(), filter) {
            Ok(clients) => ok_json(clients),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Ce qui référence encore un client (opportunités, devis, missions, factures) — à
    /// consulter avant `clients.delete`, qui refuse tant que ce total n'est pas nul.
    #[tool(
        name = "clients.references",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn clients_references(
        &self,
        Parameters(args): Parameters<ClientRefArgs>,
    ) -> CallToolResult {
        let store = self.store.lock().await;
        let id = ok_or_return!("client", resolve_client(&store, &args.client));
        match client_references(store.connection(), id) {
            Ok(refs) => ok_json(refs),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Modifie un client existant — seuls les champs fournis changent, le reste est conservé.
    #[tool(
        name = "clients.update",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn clients_update(
        &self,
        Parameters(args): Parameters<UpdateClientArgs>,
    ) -> CallToolResult {
        let siren = match &args.siren {
            Some(s) => Some(ok_or_return!("siren", Siren::parse(s))),
            None => None,
        };
        let vat_number = match &args.vat_number {
            Some(s) => Some(ok_or_return!("vat_number", VatNumber::parse(s))),
            None => None,
        };
        let mut store = self.store.lock().await;
        let id = ok_or_return!("client", resolve_client(&store, &args.client));
        let current = match client_by_id(store.connection(), id) {
            Ok(Some(c)) => c,
            Ok(None) => return err_text(format!("client introuvable : {id}")),
            Err(e) => return err_text(e.to_string()),
        };
        let address = match merge_address(
            args.street,
            args.postal_code,
            args.city,
            args.country,
            current.address,
        ) {
            Ok(a) => a,
            Err(result) => return result,
        };
        let cmd = clients::UpdateClient {
            id,
            revision: current.revision,
            name: args.name.unwrap_or(current.name),
            siren: siren.or(current.siren),
            vat_number: vat_number.or(current.vat_number),
            address,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Retire un client des listes actives sans le supprimer.
    #[tool(
        name = "clients.archive",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn clients_archive(
        &self,
        Parameters(args): Parameters<ClientRefMutationArgs>,
    ) -> CallToolResult {
        let mut store = self.store.lock().await;
        let id = ok_or_return!("client", resolve_client(&store, &args.client));
        let current = match client_by_id(store.connection(), id) {
            Ok(Some(c)) => c,
            Ok(None) => return err_text(format!("client introuvable : {id}")),
            Err(e) => return err_text(e.to_string()),
        };
        let cmd = clients::ArchiveClient {
            id,
            revision: current.revision,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Réintègre un client archivé dans les listes actives.
    #[tool(
        name = "clients.unarchive",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn clients_unarchive(
        &self,
        Parameters(args): Parameters<ClientRefMutationArgs>,
    ) -> CallToolResult {
        let mut store = self.store.lock().await;
        let id = ok_or_return!("client", resolve_client(&store, &args.client));
        let current = match client_by_id(store.connection(), id) {
            Ok(Some(c)) => c,
            Ok(None) => return err_text(format!("client introuvable : {id}")),
            Err(e) => return err_text(e.to_string()),
        };
        let cmd = clients::UnarchiveClient {
            id,
            revision: current.revision,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Supprime un client pour de bon — refusé s'il est encore référencé par une opportunité,
    /// un devis, une mission ou une facture (voir `clients.references`, ou archivez-le à la
    /// place). Effet destructeur : déclenché par un agent, cette commande attend toujours une
    /// confirmation humaine (`pending.list`, puis confirmation au terminal ou dans la fenêtre —
    /// jamais par l'agent lui-même).
    #[tool(
        name = "clients.delete",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false
        )
    )]
    async fn clients_delete(
        &self,
        Parameters(args): Parameters<ClientRefMutationArgs>,
    ) -> CallToolResult {
        let mut store = self.store.lock().await;
        let id = ok_or_return!("client", resolve_client(&store, &args.client));
        let current = match client_by_id(store.connection(), id) {
            Ok(Some(c)) => c,
            Ok(None) => return err_text(format!("client introuvable : {id}")),
            Err(e) => return err_text(e.to_string()),
        };
        let cmd = clients::DeleteClient {
            id,
            revision: current.revision,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Ajoute un contact à un client.
    #[tool(
        name = "clients.contacts.create",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn clients_contacts_create(
        &self,
        Parameters(args): Parameters<CreateContactArgs>,
    ) -> CallToolResult {
        let mut store = self.store.lock().await;
        let client_id = ok_or_return!("client", resolve_client(&store, &args.client));
        let cmd = clients::CreateContact {
            client_id,
            name: args.name,
            email: args.email,
            phone: args.phone,
            role: args.role,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Liste les contacts d'un client.
    #[tool(
        name = "clients.contacts.list",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn clients_contacts_list(
        &self,
        Parameters(args): Parameters<ListContactsArgs>,
    ) -> CallToolResult {
        let store = self.store.lock().await;
        let client_id = ok_or_return!("client", resolve_client(&store, &args.client));
        match list_contacts(store.connection(), client_id) {
            Ok(contacts) => ok_json(contacts),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Modifie un contact existant — seuls les champs fournis changent.
    #[tool(
        name = "clients.contacts.update",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn clients_contacts_update(
        &self,
        Parameters(args): Parameters<UpdateContactArgs>,
    ) -> CallToolResult {
        let id: ContactId = ok_or_return!("id", args.id.parse());
        let mut store = self.store.lock().await;
        let current = match contact_by_id(store.connection(), id) {
            Ok(Some(c)) => c,
            Ok(None) => return err_text(format!("contact introuvable : {id}")),
            Err(e) => return err_text(e.to_string()),
        };
        let cmd = clients::UpdateContact {
            id,
            revision: current.revision,
            name: args.name.unwrap_or(current.name),
            email: args.email.or(current.email),
            phone: args.phone.or(current.phone),
            role: args.role.or(current.role),
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Supprime un contact.
    #[tool(
        name = "clients.contacts.delete",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false
        )
    )]
    async fn clients_contacts_delete(
        &self,
        Parameters(args): Parameters<ContactIdArgs>,
    ) -> CallToolResult {
        let id: ContactId = ok_or_return!("id", args.id.parse());
        let mut store = self.store.lock().await;
        let current = match contact_by_id(store.connection(), id) {
            Ok(Some(c)) => c,
            Ok(None) => return err_text(format!("contact introuvable : {id}")),
            Err(e) => return err_text(e.to_string()),
        };
        let cmd = clients::DeleteContact {
            id,
            revision: current.revision,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }
}
