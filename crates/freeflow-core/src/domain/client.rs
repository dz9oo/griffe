//! Clients et contacts.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use super::ids::{ClientId, ContactId};
use super::siren::{Siren, VatNumber};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Address {
    pub street: String,
    pub postal_code: String,
    pub city: String,
    /// Code pays ISO 3166-1 alpha-2 (ex. `"FR"`).
    pub country: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Client {
    pub id: ClientId,
    pub name: String,
    pub siren: Option<Siren>,
    pub vat_number: Option<VatNumber>,
    pub address: Option<Address>,
    pub created_at: OffsetDateTime,
    /// Incrémentée à chaque modification — portée par `UpdateClient`/`ArchiveClient`/
    /// `DeleteClient` pour détecter une écriture concurrente (GUI, CLI et serveur MCP peuvent
    /// écrire simultanément dans le même coffre).
    pub revision: i64,
    /// Un client déjà référencé par une facture, un devis ou une mission ne peut pas être
    /// supprimé (voir les triggers d'immuabilité de la facturation et des devis) : il se
    /// retire des listes actives par archivage plutôt que par suppression.
    pub archived_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contact {
    pub id: ContactId,
    pub client_id: ClientId,
    pub name: String,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub role: Option<String>,
    pub revision: i64,
}
