//! Compte-rendu d'un contrôle à blanc ou d'une application. Même forme des deux côtés.

use serde::{Deserialize, Serialize};

use crate::domain::{ClientId, OpportunityId};

/// Effet d'un dossier, d'une série ou de la cadence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExchangeStatus {
    Cree,
    Revise,
    Inchange,
    Incomplet,
}

/// Un dossier du fichier, après lecture du coffre.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DossierOutcome {
    pub origine: String,
    pub qui: String,
    pub statut: ExchangeStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raison: Option<String>,
    /// Champs du fichier laissés de côté parce que Griffe les tenait déjà.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gardes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fiche: Option<ClientId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub piste: Option<OpportunityId>,
}

impl DossierOutcome {
    pub(crate) fn incomplet(origine: &str, qui: &str, raison: impl Into<String>) -> Self {
        Self {
            origine: origine.to_string(),
            qui: qui.to_string(),
            statut: ExchangeStatus::Incomplet,
            raison: Some(raison.into()),
            gardes: Vec::new(),
            fiche: None,
            piste: None,
        }
    }
}

/// Une série du fichier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeriesOutcome {
    pub nom: String,
    pub statut: ExchangeStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raison: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gardes: Vec<String>,
}

/// La cadence, seulement si le fichier la porte.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CadenceOutcome {
    pub statut: ExchangeStatus,
    /// Vrai quand l'ordre, le nombre ou un écart change : les conversations ouvertes
    /// gardent la série d'avant.
    pub epingle: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raison: Option<String>,
}

/// Le lot entier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExchangeReport {
    pub dossiers: Vec<DossierOutcome>,
    pub series: Vec<SeriesOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cadence: Option<CadenceOutcome>,
}
