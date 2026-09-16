//! Qui déclenche une commande — détermine la politique de confirmation et alimente le journal
//! d'audit.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Actor {
    /// Un humain agissant via la CLI ou la GUI : taper la commande ou cliquer le bouton EST la
    /// confirmation, aucune étape supplémentaire n'est requise.
    Human,
    /// Un agent LLM, identifié par sa session MCP. Les commandes marquées
    /// [`crate::app::Command::requires_confirmation`] ne s'appliquent jamais directement pour
    /// cet acteur : elles créent une [`crate::app::PendingAction`] à la place.
    Agent { session: String },
    /// Une action automatique du système (ex. facturation récurrente planifiée), hors
    /// périmètre du lot 2 mais prévue dès maintenant dans le modèle d'acteur.
    System,
}

impl Actor {
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::Agent { .. } => "agent",
            Self::System => "system",
        }
    }

    #[must_use]
    pub fn session(&self) -> Option<&str> {
        match self {
            Self::Agent { session } => Some(session.as_str()),
            Self::Human | Self::System => None,
        }
    }
}
