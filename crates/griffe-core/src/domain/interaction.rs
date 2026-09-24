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
    /// Visioconférence — distincte d'une rencontre physique (`Meeting`) et d'un appel (`Call`).
    Visio,
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
            Self::Visio => "visio",
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
            "visio" => Ok(Self::Visio),
            "note" => Ok(Self::Note),
            other => Err(UnknownInteractionKind(other.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::InteractionKind;

    #[test]
    fn visio_round_trips_through_the_stored_label() {
        let kind: InteractionKind = "visio".parse().unwrap();
        assert_eq!(kind, InteractionKind::Visio);
        assert_eq!(kind.as_str(), "visio");
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Interaction {
    pub id: InteractionId,
    pub opportunity_id: OpportunityId,
    pub kind: InteractionKind,
    pub note: String,
    #[serde(with = "crate::domain::serde_date::datetime")]
    pub occurred_at: OffsetDateTime,
    /// Révision optimiste (lot 16) — voir `crate::app::revision`.
    pub revision: i64,
}
