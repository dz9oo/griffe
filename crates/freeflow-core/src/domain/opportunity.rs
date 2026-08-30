//! Opportunités de prospection. Les transitions d'étape et les requêtes (pipeline pondéré,
//! retards) sont portées par la couche applicative — ce module ne fixe que la forme des données.

use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::{Date, OffsetDateTime};

use super::ids::{ClientId, OpportunityId};
use super::money::Money;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OpportunityStage {
    Qualification,
    Discovery,
    Proposal,
    Negotiation,
    Won,
    Lost,
}

#[derive(Debug, Error, PartialEq, Eq)]
#[error("étape d'opportunité inconnue : {0}")]
pub struct UnknownStage(pub String);

impl OpportunityStage {
    #[must_use]
    pub const fn is_closed(self) -> bool {
        matches!(self, Self::Won | Self::Lost)
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Qualification => "qualification",
            Self::Discovery => "discovery",
            Self::Proposal => "proposal",
            Self::Negotiation => "negotiation",
            Self::Won => "won",
            Self::Lost => "lost",
        }
    }
}

impl std::str::FromStr for OpportunityStage {
    type Err = UnknownStage;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "qualification" => Ok(Self::Qualification),
            "discovery" => Ok(Self::Discovery),
            "proposal" => Ok(Self::Proposal),
            "negotiation" => Ok(Self::Negotiation),
            "won" => Ok(Self::Won),
            "lost" => Ok(Self::Lost),
            other => Err(UnknownStage(other.to_string())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LossReason {
    Budget,
    Timing,
    Competitor,
    NoResponse,
    ScopeMismatch,
    Other(String),
}

#[derive(Debug, Error, PartialEq, Eq)]
#[error("une probabilité doit être comprise entre 0 et 100 (reçu {0})")]
pub struct ProbabilityError(pub u8);

/// Une probabilité de gain, en pourcentage entier (0 à 100).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Probability(u8);

impl Probability {
    /// # Errors
    ///
    /// Retourne une erreur si `percent` dépasse 100.
    pub const fn new(percent: u8) -> Result<Self, ProbabilityError> {
        if percent > 100 {
            return Err(ProbabilityError(percent));
        }
        Ok(Self(percent))
    }

    #[must_use]
    pub const fn percent(self) -> u8 {
        self.0
    }

    /// Pondère un montant par cette probabilité — c'est le calcul du pipeline pondéré.
    #[must_use]
    pub fn weighted(self, amount: Money) -> Money {
        amount.apply_rate_bps(u32::from(self.0) * 100)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Opportunity {
    pub id: OpportunityId,
    pub client_id: ClientId,
    pub name: String,
    pub stage: OpportunityStage,
    pub amount: Money,
    pub probability: Probability,
    /// Date de la prochaine action de relance. Obligatoire sur une opportunité active — la
    /// couche applicative (lot 3) refuse de créer ou faire avancer une opportunité sans elle.
    pub next_action_at: Option<Date>,
    pub source: Option<String>,
    pub loss_reason: Option<LossReason>,
    pub created_at: OffsetDateTime,
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn probability_above_100_is_rejected() {
        assert_eq!(Probability::new(101), Err(ProbabilityError(101)));
    }

    #[test]
    fn weighted_pipeline_reference_case() {
        // Cas Kappa Software de la maquette : 78 000 € à 40 % -> 31 200 €.
        let amount = Money::from_cents(7_800_000);
        let probability = Probability::new(40).unwrap();
        assert_eq!(probability.weighted(amount), Money::from_cents(3_120_000));
    }

    proptest! {
        #[test]
        fn weighted_amount_never_exceeds_the_original(cents in 0i64..1_000_000_000, percent in 0u8..=100) {
            let amount = Money::from_cents(cents);
            let probability = Probability::new(percent).unwrap();
            prop_assert!(probability.weighted(amount).cents() <= amount.cents());
        }

        #[test]
        fn full_probability_is_the_identity(cents in 0i64..1_000_000_000) {
            let amount = Money::from_cents(cents);
            let probability = Probability::new(100).unwrap();
            prop_assert_eq!(probability.weighted(amount), amount);
        }
    }
}
