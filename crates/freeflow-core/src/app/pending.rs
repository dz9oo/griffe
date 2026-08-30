//! Actions en attente : une commande à confirmation proposée par un agent ne s'applique pas
//! tout de suite — elle est déposée ici, consultable et validable depuis la CLI ou la GUI.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PendingActionId(Uuid);

impl PendingActionId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for PendingActionId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for PendingActionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl FromStr for PendingActionId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(Self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PendingActionStatus {
    Pending,
    Confirmed,
    Rejected,
}

impl PendingActionStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Confirmed => "confirmed",
            Self::Rejected => "rejected",
        }
    }
}

/// Une commande proposée par un agent, en attente de validation humaine. Porte assez
/// d'information (nom + JSON de la commande) pour être reconstruite et rejouée par un autre
/// processus (CLI, GUI) que celui qui l'a créée.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingAction {
    pub id: PendingActionId,
    pub actor_session: String,
    pub command_name: String,
    pub command_json: String,
    pub status: PendingActionStatus,
}
