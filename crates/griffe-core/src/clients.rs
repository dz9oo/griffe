//! Clients et leurs contacts — référencés par la prospection, les missions, la facturation et
//! les devis, mais dont la création n'appartenait à aucun de ces lots (découvert au lot 7, sans
//! ce module aucune donnée réelle ne pouvait exister).
//!
//! Lot 15 : un client est mutable (`UpdateClient`), archivable (`ArchiveClient`/
//! `UnarchiveClient` — pour les listes actives, sans casser les factures/devis passés qui le
//! référencent) et supprimable *seulement* s'il n'est référencé nulle part
//! (`client_references`). Chaque mutation porte une `revision` lue au préalable : une écriture
//! concurrente (GUI, CLI, serveur MCP écrivant simultanément dans le même coffre) se traduit par
//! `AppError::Conflict`, jamais par un écrasement silencieux.

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::app::{AppError, Command};
use crate::domain::{Address, Client, ClientId, Contact, ContactId, Siren, VatNumber};

fn conv_err(e: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ClientError {
    #[error("client introuvable : {0}")]
    NotFound(ClientId),

    #[error("contact introuvable : {0}")]
    ContactNotFound(ContactId),

    #[error("suppression impossible : {0}")]
    HasReferences(String),
}

impl From<ClientError> for AppError {
    fn from(e: ClientError) -> Self {
        Self::Domain(e.to_string())
    }
}

// ---------------------------------------------------------------------------------------------
// Client — création, lecture, modification, archivage, suppression
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateClient {
    pub name: String,
    pub siren: Option<Siren>,
    pub vat_number: Option<VatNumber>,
    pub address: Option<Address>,
}

impl Command for CreateClient {
    type Output = ClientId;
    const NAME: &'static str = "clients.create_client";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let client = Client {
            id: ClientId::new(),
            name: self.name.clone(),
            siren: self.siren,
            vat_number: self.vat_number.clone(),
            address: self.address.clone(),
            created_at: OffsetDateTime::now_utc(),
            revision: 1,
            archived_at: None,
        };
        insert_client(conn, &client)?;
        Ok(client.id)
    }
}

/// État complet d'un client — pas de patch : la commande porte toujours la valeur nouvelle
/// entière (y compris les champs inchangés), pour que l'événement d'audit reste une image
/// complète plutôt qu'un delta qu'il faudrait recomposer pour comprendre l'état résultant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateClient {
    pub id: ClientId,
    /// Révision lue avant modification — voir le commentaire de module.
    pub revision: i64,
    pub name: String,
    pub siren: Option<Siren>,
    pub vat_number: Option<VatNumber>,
    pub address: Option<Address>,
}

impl Command for UpdateClient {
    /// La révision résultante — la façade appelante s'en sert pour continuer d'éditer sans
    /// recharger la fiche.
    type Output = i64;
    const NAME: &'static str = "clients.update_client";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let new_revision = require_client_revision(conn, self.id, self.revision)?;
        conn.execute(
            "UPDATE clients SET name = ?1, siren = ?2, vat_number = ?3, address_street = ?4,
                address_postal_code = ?5, address_city = ?6, address_country = ?7, revision = ?8
             WHERE id = ?9 AND revision = ?10",
            params![
                self.name,
                self.siren.map(|s| s.to_string()),
                self.vat_number
                    .as_ref()
                    .map(std::string::ToString::to_string),
                self.address.as_ref().map(|a| a.street.clone()),
                self.address.as_ref().map(|a| a.postal_code.clone()),
                self.address.as_ref().map(|a| a.city.clone()),
                self.address.as_ref().map(|a| a.country.clone()),
                new_revision,
                self.id.to_string(),
                self.revision,
            ],
        )?;
        Ok(new_revision)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveClient {
    pub id: ClientId,
    pub revision: i64,
}

impl Command for ArchiveClient {
    type Output = i64;
    const NAME: &'static str = "clients.archive_client";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        set_client_archived(conn, self.id, self.revision, true)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnarchiveClient {
    pub id: ClientId,
    pub revision: i64,
}

impl Command for UnarchiveClient {
    type Output = i64;
    const NAME: &'static str = "clients.unarchive_client";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        set_client_archived(conn, self.id, self.revision, false)
    }
}

fn set_client_archived(
    conn: &Connection,
    id: ClientId,
    revision: i64,
    archived: bool,
) -> Result<i64, AppError> {
    let new_revision = require_client_revision(conn, id, revision)?;
    let archived_at = archived
        .then(|| OffsetDateTime::now_utc().format(&Rfc3339))
        .transpose()?;
    conn.execute(
        "UPDATE clients SET archived_at = ?1, revision = ?2 WHERE id = ?3 AND revision = ?4",
        params![archived_at, new_revision, id.to_string(), revision],
    )?;
    Ok(new_revision)
}

/// Ce qui empêche un client d'être supprimé pour de bon — voir [`client_references`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientReferences {
    pub opportunities: i64,
    pub quotes: i64,
    pub missions: i64,
    pub invoices: i64,
}

impl ClientReferences {
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.opportunities == 0 && self.quotes == 0 && self.missions == 0 && self.invoices == 0
    }
}

/// Dénombre ce qui référence encore `id` — sert à la fois au refus de [`DeleteClient`] et au
/// panneau de détail d'une façade (afficher *pourquoi* la suppression est bloquée).
///
/// # Errors
pub fn client_references(conn: &Connection, id: ClientId) -> Result<ClientReferences, AppError> {
    let id_str = id.to_string();
    let opportunities = conn.query_row(
        "SELECT count(*) FROM opportunities WHERE client_id = ?1",
        [id_str.as_str()],
        |row| row.get(0),
    )?;
    let quotes = conn.query_row(
        "SELECT count(*) FROM quotes WHERE client_id = ?1",
        [id_str.as_str()],
        |row| row.get(0),
    )?;
    let missions = conn.query_row(
        "SELECT count(*) FROM missions WHERE client_id = ?1",
        [id_str.as_str()],
        |row| row.get(0),
    )?;
    let invoices = conn.query_row(
        "SELECT count(*) FROM invoices WHERE client_id = ?1",
        [id_str.as_str()],
        |row| row.get(0),
    )?;
    Ok(ClientReferences {
        opportunities,
        quotes,
        missions,
        invoices,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteClient {
    pub id: ClientId,
    pub revision: i64,
}

impl Command for DeleteClient {
    type Output = ();
    const NAME: &'static str = "clients.delete_client";

    /// Destructeur réel : un agent MCP ne doit jamais le déclencher sans validation humaine
    /// explicite — voir la politique de confirmation de [`crate::app::Command`].
    fn requires_confirmation(&self) -> bool {
        true
    }

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        // Pas de bump de révision ici : la ligne va disparaître, `require_client_revision`
        // vérifie tout de même l'existence et la fraîcheur de la révision fournie.
        require_client_revision(conn, self.id, self.revision)?;

        let refs = client_references(conn, self.id)?;
        if !refs.is_empty() {
            return Err(ClientError::HasReferences(format!(
                "{} opportunité(s), {} devis, {} mission(s) et {} facture(s) référencent encore \
                 ce client — archivez-le plutôt que de le supprimer",
                refs.opportunities, refs.quotes, refs.missions, refs.invoices
            ))
            .into());
        }

        // Les contacts appartiennent au cycle de vie du client : ils ne comptent pas comme des
        // références qui bloquent la suppression, ils disparaissent avec lui.
        conn.execute(
            "DELETE FROM contacts WHERE client_id = ?1",
            [self.id.to_string()],
        )?;
        conn.execute(
            "DELETE FROM clients WHERE id = ?1 AND revision = ?2",
            params![self.id.to_string(), self.revision],
        )?;
        Ok(())
    }
}

/// Wrapper typé au-dessus du socle partagé `crate::app::revision` (extrait d'ici au lot 16, quand
/// la prospection et les missions en ont eu besoin à leur tour) — porte l'erreur `NotFound` du
/// bon type pour un client.
fn require_client_revision(
    conn: &Connection,
    id: ClientId,
    expected: i64,
) -> Result<i64, AppError> {
    let id_str = id.to_string();
    let current = crate::app::revision::current_revision(conn, "clients", &id_str)?
        .ok_or(ClientError::NotFound(id))?;
    crate::app::revision::require_revision(current, expected, "client", &id_str)
}

fn require_contact_revision(
    conn: &Connection,
    id: ContactId,
    expected: i64,
) -> Result<i64, AppError> {
    let id_str = id.to_string();
    let current = crate::app::revision::current_revision(conn, "contacts", &id_str)?
        .ok_or(ClientError::ContactNotFound(id))?;
    crate::app::revision::require_revision(current, expected, "contact", &id_str)
}

fn insert_client(conn: &Connection, client: &Client) -> Result<(), AppError> {
    insert_client_as(conn, client, false)
}

pub(crate) fn insert_client_as(
    conn: &Connection,
    client: &Client,
    is_prospect: bool,
) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO clients (id, name, siren, vat_number, address_street, address_postal_code, address_city, address_country, created_at, is_prospect)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            client.id.to_string(),
            client.name,
            client.siren.map(|s| s.to_string()),
            client.vat_number.as_ref().map(std::string::ToString::to_string),
            client.address.as_ref().map(|a| a.street.clone()),
            client.address.as_ref().map(|a| a.postal_code.clone()),
            client.address.as_ref().map(|a| a.city.clone()),
            client.address.as_ref().map(|a| a.country.clone()),
            client.created_at.format(&Rfc3339)?,
            i64::from(is_prospect),
        ],
    )?;
    Ok(())
}

fn row_to_client(row: &Row) -> rusqlite::Result<Client> {
    let id: String = row.get("id")?;
    let siren: Option<String> = row.get("siren")?;
    let vat_number: Option<String> = row.get("vat_number")?;
    let street: Option<String> = row.get("address_street")?;
    let postal_code: Option<String> = row.get("address_postal_code")?;
    let city: Option<String> = row.get("address_city")?;
    let country: Option<String> = row.get("address_country")?;
    let created_at: String = row.get("created_at")?;
    let archived_at: Option<String> = row.get("archived_at")?;

    let address = match (street, postal_code, city, country) {
        (Some(street), Some(postal_code), Some(city), Some(country)) => Some(Address {
            street,
            postal_code,
            city,
            country,
        }),
        _ => None,
    };

    Ok(Client {
        id: id.parse().map_err(conv_err)?,
        name: row.get("name")?,
        siren: siren
            .map(|s| Siren::parse(&s))
            .transpose()
            .map_err(conv_err)?,
        vat_number: vat_number
            .map(|s| VatNumber::parse(&s))
            .transpose()
            .map_err(conv_err)?,
        address,
        created_at: OffsetDateTime::parse(&created_at, &Rfc3339).map_err(conv_err)?,
        revision: row.get("revision")?,
        archived_at: archived_at
            .map(|s| OffsetDateTime::parse(&s, &Rfc3339))
            .transpose()
            .map_err(conv_err)?,
    })
}

/// # Errors
pub fn client_by_id(conn: &Connection, id: ClientId) -> Result<Option<Client>, AppError> {
    conn.query_row(
        "SELECT * FROM clients WHERE id = ?1",
        [id.to_string()],
        row_to_client,
    )
    .optional()
    .map_err(AppError::from)
}

/// Tous les clients, archivés compris — utilisée pour résoudre un nom (y compris celui d'un
/// client archivé référencé par une facture passée) plutôt que pour peupler un écran de
/// navigation. Voir [`list_clients_with`] pour une liste filtrée.
///
/// # Errors
pub fn list_clients(conn: &Connection) -> Result<Vec<Client>, AppError> {
    let mut stmt = conn.prepare("SELECT * FROM clients ORDER BY created_at ASC")?;
    let rows = stmt.query_map([], row_to_client)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientFilter {
    /// Exclut les clients archivés — le défaut pour un écran de navigation. Un prospect
    /// (`is_prospect`) sans devis ni facture en est aussi exclu : il n'est pas encore un client.
    ActiveOnly,
    /// Inclut aussi les clients archivés. Les prospects sans pièce commerciale restent exclus.
    All,
}

/// Condition SQL : fiche créée comme client, ou déjà porteuse d'un devis / d'une facture.
const LISTED_CLIENT_PREDICATE: &str = "(is_prospect = 0
            OR EXISTS (SELECT 1 FROM quotes WHERE quotes.client_id = clients.id)
            OR EXISTS (SELECT 1 FROM invoices WHERE invoices.client_id = clients.id))";

/// Liste filtrée pour un écran de navigation (CLI `client list`, écran `clients` de la GUI) —
/// à la différence de [`list_clients`], qui reste inclusive pour la résolution de références.
///
/// # Errors
pub fn list_clients_with(conn: &Connection, filter: ClientFilter) -> Result<Vec<Client>, AppError> {
    let sql = match filter {
        ClientFilter::ActiveOnly => format!(
            "SELECT * FROM clients WHERE archived_at IS NULL AND {LISTED_CLIENT_PREDICATE} ORDER BY name ASC"
        ),
        ClientFilter::All => {
            format!("SELECT * FROM clients WHERE {LISTED_CLIENT_PREDICATE} ORDER BY name ASC")
        }
    };
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], row_to_client)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
}

/// Vrai tant que la fiche est un prospect sans devis ni facture — on peut encore modifier
/// nom, adresse et contact depuis la prospection. Dès qu'un devis ou une facture existe,
/// ou si la fiche a été créée via [`CreateClient`], l'édition se fait depuis Clients.
///
/// # Errors
pub fn prospect_party_is_editable(conn: &Connection, id: ClientId) -> Result<bool, AppError> {
    let id_str = id.to_string();
    let is_prospect: i64 = conn
        .query_row(
            "SELECT is_prospect FROM clients WHERE id = ?1",
            [id_str.as_str()],
            |row| row.get(0),
        )
        .optional()?
        .ok_or(ClientError::NotFound(id))?;
    if is_prospect == 0 {
        return Ok(false);
    }
    let refs = client_references(conn, id)?;
    Ok(refs.quotes == 0 && refs.invoices == 0)
}

// ---------------------------------------------------------------------------------------------
// Contact — sous-entité d'un client, sans existence propre en dehors de lui
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateContact {
    pub client_id: ClientId,
    pub name: String,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub role: Option<String>,
}

impl Command for CreateContact {
    type Output = ContactId;
    const NAME: &'static str = "clients.create_contact";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        // Vérifié explicitement pour échouer proprement plutôt que de heurter la contrainte de
        // clé étrangère avec une erreur SQLite peu lisible.
        client_by_id(conn, self.client_id)?.ok_or(ClientError::NotFound(self.client_id))?;
        let contact = Contact {
            id: ContactId::new(),
            client_id: self.client_id,
            name: self.name.clone(),
            email: self.email.clone(),
            phone: self.phone.clone(),
            role: self.role.clone(),
            revision: 1,
        };
        insert_contact(conn, &contact)?;
        Ok(contact.id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateContact {
    pub id: ContactId,
    pub revision: i64,
    pub name: String,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub role: Option<String>,
}

impl Command for UpdateContact {
    type Output = i64;
    const NAME: &'static str = "clients.update_contact";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let new_revision = require_contact_revision(conn, self.id, self.revision)?;
        conn.execute(
            "UPDATE contacts SET name = ?1, email = ?2, phone = ?3, role = ?4, revision = ?5
             WHERE id = ?6 AND revision = ?7",
            params![
                self.name,
                self.email,
                self.phone,
                self.role,
                new_revision,
                self.id.to_string(),
                self.revision,
            ],
        )?;
        Ok(new_revision)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteContact {
    pub id: ContactId,
    pub revision: i64,
}

impl Command for DeleteContact {
    type Output = ();
    const NAME: &'static str = "clients.delete_contact";

    // Suppression définitive, cohérente avec `DeleteClient` : un agent la propose, un humain la
    // confirme.
    fn requires_confirmation(&self) -> bool {
        true
    }

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        require_contact_revision(conn, self.id, self.revision)?;
        conn.execute(
            "DELETE FROM contacts WHERE id = ?1 AND revision = ?2",
            params![self.id.to_string(), self.revision],
        )?;
        Ok(())
    }
}

pub(crate) fn insert_contact(conn: &Connection, contact: &Contact) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO contacts (id, client_id, name, email, phone, role) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            contact.id.to_string(),
            contact.client_id.to_string(),
            contact.name,
            contact.email,
            contact.phone,
            contact.role,
        ],
    )?;
    Ok(())
}

fn row_to_contact(row: &Row) -> rusqlite::Result<Contact> {
    let id: String = row.get("id")?;
    let client_id: String = row.get("client_id")?;
    Ok(Contact {
        id: id.parse().map_err(conv_err)?,
        client_id: client_id.parse().map_err(conv_err)?,
        name: row.get("name")?,
        email: row.get("email")?,
        phone: row.get("phone")?,
        role: row.get("role")?,
        revision: row.get("revision")?,
    })
}

/// # Errors
pub fn contact_by_id(conn: &Connection, id: ContactId) -> Result<Option<Contact>, AppError> {
    conn.query_row(
        "SELECT * FROM contacts WHERE id = ?1",
        [id.to_string()],
        row_to_contact,
    )
    .optional()
    .map_err(AppError::from)
}

/// # Errors
pub fn list_contacts(conn: &Connection, client_id: ClientId) -> Result<Vec<Contact>, AppError> {
    let mut stmt = conn.prepare("SELECT * FROM contacts WHERE client_id = ?1 ORDER BY name ASC")?;
    let rows = stmt.query_map([client_id.to_string()], row_to_contact)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Actor, ExecutionContext, Executor, Outcome};
    use crate::store::{Passphrase, Store};

    fn test_store(label: &str) -> Store {
        let dir = std::env::temp_dir().join(format!(
            "griffe-clients-test-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ));
        Store::create(&dir.join("vault.db"), &Passphrase::from("s3cret")).unwrap()
    }

    fn human_ctx() -> ExecutionContext {
        ExecutionContext::new(Actor::Human, false)
    }

    fn create(store: &mut Store, name: &str) -> ClientId {
        let cmd = CreateClient {
            name: name.to_string(),
            siren: None,
            vat_number: None,
            address: None,
        };
        let Outcome::Applied(id) = Executor::new(store).execute(&cmd, &human_ctx()).unwrap() else {
            panic!("expected Applied")
        };
        id
    }

    #[test]
    fn creating_a_client_makes_it_immediately_fetchable_and_listed() {
        let mut store = test_store("create-fetch");
        let id = create(&mut store, "Argon Digital");

        let fetched = client_by_id(store.connection(), id).unwrap().unwrap();
        assert_eq!(fetched.name, "Argon Digital");
        assert_eq!(fetched.revision, 1);
        assert_eq!(fetched.archived_at, None);

        let listed = list_clients(store.connection()).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, id);
    }

    #[test]
    fn updating_a_client_changes_its_fields_and_bumps_the_revision() {
        let mut store = test_store("update");
        let id = create(&mut store, "Argon Digital");

        let cmd = UpdateClient {
            id,
            revision: 1,
            name: "Argon Digital SASU".to_string(),
            siren: None,
            vat_number: None,
            address: None,
        };
        let Outcome::Applied(new_revision) = Executor::new(&mut store)
            .execute(&cmd, &human_ctx())
            .unwrap()
        else {
            panic!("expected Applied")
        };
        assert_eq!(new_revision, 2);

        let fetched = client_by_id(store.connection(), id).unwrap().unwrap();
        assert_eq!(fetched.name, "Argon Digital SASU");
        assert_eq!(fetched.revision, 2);
    }

    #[test]
    fn updating_with_a_stale_revision_is_a_conflict_and_writes_nothing() {
        let mut store = test_store("stale-update");
        let id = create(&mut store, "Argon Digital");

        let cmd = UpdateClient {
            id,
            revision: 999,
            name: "Nom incorrect".to_string(),
            siren: None,
            vat_number: None,
            address: None,
        };
        let err = Executor::new(&mut store)
            .execute(&cmd, &human_ctx())
            .unwrap_err();
        assert!(matches!(
            err,
            AppError::Conflict {
                entity: "client",
                ..
            }
        ));

        let fetched = client_by_id(store.connection(), id).unwrap().unwrap();
        assert_eq!(fetched.name, "Argon Digital", "aucune écriture sur conflit");
        assert_eq!(fetched.revision, 1);
    }

    #[test]
    fn updating_an_unknown_client_is_not_found() {
        let mut store = test_store("update-unknown");
        let cmd = UpdateClient {
            id: ClientId::new(),
            revision: 1,
            name: "Fantôme".to_string(),
            siren: None,
            vat_number: None,
            address: None,
        };
        let err = Executor::new(&mut store)
            .execute(&cmd, &human_ctx())
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("introuvable")));
    }

    #[test]
    fn archiving_removes_a_client_from_the_active_list_but_not_from_the_full_list() {
        let mut store = test_store("archive");
        let id = create(&mut store, "Argon Digital");

        let cmd = ArchiveClient { id, revision: 1 };
        Executor::new(&mut store)
            .execute(&cmd, &human_ctx())
            .unwrap();

        let active = list_clients_with(store.connection(), ClientFilter::ActiveOnly).unwrap();
        assert!(active.is_empty());

        let all = list_clients_with(store.connection(), ClientFilter::All).unwrap();
        assert_eq!(all.len(), 1);

        let full = list_clients(store.connection()).unwrap();
        assert_eq!(
            full.len(),
            1,
            "list_clients reste inclusive pour la résolution de références"
        );

        let fetched = client_by_id(store.connection(), id).unwrap().unwrap();
        assert!(fetched.archived_at.is_some());
    }

    #[test]
    fn unarchiving_brings_a_client_back_to_the_active_list() {
        let mut store = test_store("unarchive");
        let id = create(&mut store, "Argon Digital");
        Executor::new(&mut store)
            .execute(&ArchiveClient { id, revision: 1 }, &human_ctx())
            .unwrap();
        Executor::new(&mut store)
            .execute(&UnarchiveClient { id, revision: 2 }, &human_ctx())
            .unwrap();

        let active = list_clients_with(store.connection(), ClientFilter::ActiveOnly).unwrap();
        assert_eq!(active.len(), 1);
        let fetched = client_by_id(store.connection(), id).unwrap().unwrap();
        assert_eq!(fetched.archived_at, None);
        assert_eq!(fetched.revision, 3);
    }

    #[test]
    fn deleting_an_unreferenced_client_removes_it_and_its_contacts() {
        let mut store = test_store("delete");
        let id = create(&mut store, "Argon Digital");
        Executor::new(&mut store)
            .execute(
                &CreateContact {
                    client_id: id,
                    name: "Alex".to_string(),
                    email: None,
                    phone: None,
                    role: None,
                },
                &human_ctx(),
            )
            .unwrap();

        Executor::new(&mut store)
            .execute(&DeleteClient { id, revision: 1 }, &human_ctx())
            .unwrap();

        assert_eq!(client_by_id(store.connection(), id).unwrap(), None);
        assert_eq!(list_contacts(store.connection(), id).unwrap(), Vec::new());
    }

    #[test]
    fn deleting_a_referenced_client_is_refused_and_names_what_blocks_it() {
        let mut store = test_store("delete-referenced");
        let id = create(&mut store, "Argon Digital");
        Executor::new(&mut store)
            .execute(
                &crate::prospection::CreateOpportunity {
                    client_id: id,
                    name: "Refonte site".to_string(),
                    amount: crate::domain::Money::from_cents(500_000),
                    probability: crate::domain::Probability::new(50).unwrap(),
                    next_action_at: time::Date::from_calendar_date(2026, time::Month::October, 1)
                        .unwrap(),
                    source: None,
                },
                &human_ctx(),
            )
            .unwrap();

        let err = Executor::new(&mut store)
            .execute(&DeleteClient { id, revision: 1 }, &human_ctx())
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("1 opportunité")));

        assert!(client_by_id(store.connection(), id).unwrap().is_some());
    }

    #[test]
    fn contact_lifecycle_create_update_delete() {
        let mut store = test_store("contact-lifecycle");
        let client_id = create(&mut store, "Argon Digital");

        let Outcome::Applied(contact_id) = Executor::new(&mut store)
            .execute(
                &CreateContact {
                    client_id,
                    name: "Alex Martin".to_string(),
                    email: Some("alex@argon.example".to_string()),
                    phone: None,
                    role: Some("DAF".to_string()),
                },
                &human_ctx(),
            )
            .unwrap()
        else {
            panic!("expected Applied")
        };

        let listed = list_contacts(store.connection(), client_id).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].revision, 1);

        Executor::new(&mut store)
            .execute(
                &UpdateContact {
                    id: contact_id,
                    revision: 1,
                    name: "Alex Martin".to_string(),
                    email: Some("alex.martin@argon.example".to_string()),
                    phone: None,
                    role: Some("DAF".to_string()),
                },
                &human_ctx(),
            )
            .unwrap();

        let updated = contact_by_id(store.connection(), contact_id)
            .unwrap()
            .unwrap();
        assert_eq!(updated.email.as_deref(), Some("alex.martin@argon.example"));
        assert_eq!(updated.revision, 2);

        Executor::new(&mut store)
            .execute(
                &DeleteContact {
                    id: contact_id,
                    revision: 2,
                },
                &human_ctx(),
            )
            .unwrap();
        assert_eq!(contact_by_id(store.connection(), contact_id).unwrap(), None);
    }

    #[test]
    fn creating_a_contact_for_an_unknown_client_fails() {
        let mut store = test_store("contact-unknown-client");
        let err = Executor::new(&mut store)
            .execute(
                &CreateContact {
                    client_id: ClientId::new(),
                    name: "Alex".to_string(),
                    email: None,
                    phone: None,
                    role: None,
                },
                &human_ctx(),
            )
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("introuvable")));
    }
}
