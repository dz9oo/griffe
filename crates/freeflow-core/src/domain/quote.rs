//! Devis. La conversion en mission (lot 6) dérive l'échéancier de facturation de ces lignes.

use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::{Date, OffsetDateTime};

use super::ids::{ClientId, OpportunityId, QuoteId};
use super::money::Money;
use super::vat::VatRate;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QuoteStatus {
    Draft,
    Sent,
    Accepted,
    Declined,
    Expired,
}

#[derive(Debug, Error, PartialEq, Eq)]
#[error("statut de devis inconnu : {0}")]
pub struct UnknownQuoteStatus(pub String);

impl QuoteStatus {
    #[must_use]
    pub const fn is_closed(self) -> bool {
        matches!(self, Self::Accepted | Self::Declined | Self::Expired)
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Sent => "sent",
            Self::Accepted => "accepted",
            Self::Declined => "declined",
            Self::Expired => "expired",
        }
    }
}

impl std::str::FromStr for QuoteStatus {
    type Err = UnknownQuoteStatus;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "draft" => Ok(Self::Draft),
            "sent" => Ok(Self::Sent),
            "accepted" => Ok(Self::Accepted),
            "declined" => Ok(Self::Declined),
            "expired" => Ok(Self::Expired),
            other => Err(UnknownQuoteStatus(other.to_string())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LineKind {
    Regie { daily_rate: Money, days: f64 },
    Forfait { amount: Money },
    Recurrent { monthly_amount: Money, months: u32 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuoteLine {
    pub description: String,
    pub kind: LineKind,
    pub vat_rate: VatRate,
}

/// Une remise globale sur un devis, allouée proportionnellement sur les lignes (méthode du
/// plus fort reste via [`super::Money::allocate_proportionally`]) — jamais appliquée ligne par
/// ligne, pour ne jamais perdre ni créer un centime.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Discount {
    /// En dix-millièmes (`10_000` = 100 %).
    Percentage(u32),
    FixedAmount(Money),
}

/// Une version de devis. Le contenu (lignes, remise, validité, conditions) est immuable dès la
/// création — pour réviser un devis, on crée une nouvelle version portant le même `root_id`,
/// jamais une modification en place. Seul `status` évolue, via des transitions contrôlées par
/// la couche applicative (lot 6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Quote {
    pub id: QuoteId,
    /// Id de la toute première version de ce devis — vaut `id` pour la version 1 elle-même.
    /// Permet de retrouver toutes les versions d'un même devis par une simple égalité.
    pub root_id: QuoteId,
    pub client_id: ClientId,
    pub opportunity_id: Option<OpportunityId>,
    pub version: u32,
    pub status: QuoteStatus,
    pub lines: Vec<QuoteLine>,
    pub discount: Option<Discount>,
    pub terms: Option<String>,
    pub valid_until: Date,
    pub created_at: OffsetDateTime,
}
