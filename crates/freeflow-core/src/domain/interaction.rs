//! Journal d'interactions avec un prospect (appels, emails, réunions, notes).

use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::OffsetDateTime;

use super::ids::{InteractionId, OpportunityId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum InteractionKind {
    Call,
    Email,
    Meeting,
    Note,
}

#[derive(Debug, Error, PartialEq, Eq)]
#[error("type d'interaction inconnu : {0}")]
pub struct UnknownInteractionKind(pub String);

impl InteractionKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Call => "call",
            Self::Email => "email",
            Self::Meeting => "meeting",
            Self::Note => "note",
        }
    }
}

impl std::str::FromStr for InteractionKind {
    type Err = UnknownInteractionKind;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "call" => Ok(Self::Call),
            "email" => Ok(Self::Email),
            "meeting" => Ok(Self::Meeting),
            "note" => Ok(Self::Note),
            other => Err(UnknownInteractionKind(other.to_string())),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Interaction {
    pub id: InteractionId,
    pub opportunity_id: OpportunityId,
    pub kind: InteractionKind,
    pub note: String,
    pub occurred_at: OffsetDateTime,
}
