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

/// Genre d'une personne, tel qu'une lettre peut l'écrire.
///
/// Absent : la fiche ne dit ni Monsieur ni Madame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Civility {
    Monsieur,
    Madame,
}

impl Civility {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Monsieur => "monsieur",
            Self::Madame => "madame",
        }
    }

    /// Mot écrit dans une lettre : « Monsieur » ou « Madame ».
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Monsieur => "Monsieur",
            Self::Madame => "Madame",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "monsieur" => Some(Self::Monsieur),
            "madame" => Some(Self::Madame),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Client {
    pub id: ClientId,
    pub name: String,
    pub siren: Option<Siren>,
    pub vat_number: Option<VatNumber>,
    pub address: Option<Address>,
    /// Courriel du standard, ou de l'accueil. Pas celui d'une personne.
    #[serde(default)]
    pub switchboard_email: Option<String>,
    /// Téléphone du standard, ou de l'accueil.
    #[serde(default)]
    pub switchboard_phone: Option<String>,
    #[serde(with = "crate::domain::serde_date::datetime")]
    pub created_at: OffsetDateTime,
    /// Incrémentée à chaque modification — portée par `UpdateClient`/`ArchiveClient`/
    /// `DeleteClient` pour détecter une écriture concurrente (GUI, CLI et serveur MCP peuvent
    /// écrire simultanément dans le même coffre).
    pub revision: i64,
    /// Un client déjà référencé par une facture, un devis ou une mission ne peut pas être
    /// supprimé (voir les triggers d'immuabilité de la facturation et des devis) : il se
    /// retire des listes actives par archivage plutôt que par suppression.
    #[serde(with = "crate::domain::serde_date::datetime::option")]
    pub archived_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contact {
    pub id: ContactId,
    pub client_id: ClientId,
    pub name: String,
    /// Prénom, séparé du nom. Vide tant que la fiche n'a pas été enregistrée ainsi.
    #[serde(default)]
    pub given_name: String,
    /// Nom de famille, ou enseigne quand il n'y a pas de prénom.
    #[serde(default)]
    pub family_name: String,
    /// Monsieur ou Madame. Absent : la lettre ne choisit pas.
    #[serde(default)]
    pub civility: Option<Civility>,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub role: Option<String>,
    /// Vrai pour le seul contact qui reçoit les lettres de cette fiche.
    #[serde(default)]
    pub correspondent: bool,
    pub revision: i64,
}
