//! Taux de TVA français applicables aux prestations d'un indépendant.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Un taux de TVA français. `Zero` couvre l'exonération, l'autoliquidation intracommunautaire
/// et l'export : dans les trois cas la ligne apparaît sur la facture sans TVA collectée.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VatRate {
    Standard,
    Intermediate,
    Reduced,
    SuperReduced,
    Zero,
}

#[derive(Debug, Error, PartialEq, Eq)]
#[error("taux de TVA inconnu : {0}")]
pub struct UnknownVatRate(pub String);

impl VatRate {
    /// Taux exprimé en dix-millièmes (`10_000` = 100 %), pour un calcul entier exact via
    /// [`crate::domain::Money::apply_rate_bps`].
    #[must_use]
    pub const fn basis_points(self) -> u32 {
        match self {
            Self::Standard => 2_000,
            Self::Intermediate => 1_000,
            Self::Reduced => 550,
            Self::SuperReduced => 210,
            Self::Zero => 0,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Intermediate => "intermediate",
            Self::Reduced => "reduced",
            Self::SuperReduced => "super_reduced",
            Self::Zero => "zero",
        }
    }

    /// Les cinq taux, dans un ordre stable — utilisé pour construire une ventilation de TVA
    /// déterministe (une ligne par taux réellement présent sur la facture).
    pub const ALL: [Self; 5] = [
        Self::Standard,
        Self::Intermediate,
        Self::Reduced,
        Self::SuperReduced,
        Self::Zero,
    ];
}

impl std::str::FromStr for VatRate {
    type Err = UnknownVatRate;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "standard" => Ok(Self::Standard),
            "intermediate" => Ok(Self::Intermediate),
            "reduced" => Ok(Self::Reduced),
            "super_reduced" => Ok(Self::SuperReduced),
            "zero" => Ok(Self::Zero),
            other => Err(UnknownVatRate(other.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Money;

    #[test]
    fn standard_rate_matches_reference_amount() {
        let ht = Money::from_cents(617_500);
        assert_eq!(
            ht.apply_rate_bps(VatRate::Standard.basis_points()),
            Money::from_cents(123_500)
        );
    }
}
