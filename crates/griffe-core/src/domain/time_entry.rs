//! Saisie de temps. Le calcul du TJM effectif et du taux d'occupation (lot 4) distingue le
//! temps facturable du reste.

use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::Date;

use super::ids::{MissionId, TimeEntryId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TimeCategory {
    Billable,
    PreSales,
    Admin,
    Training,
}

#[derive(Debug, Error, PartialEq, Eq)]
#[error("catégorie de temps inconnue : {0}")]
pub struct UnknownTimeCategory(pub String);

impl TimeCategory {
    #[must_use]
    pub const fn is_billable(self) -> bool {
        matches!(self, Self::Billable)
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Billable => "billable",
            Self::PreSales => "pre_sales",
            Self::Admin => "admin",
            Self::Training => "training",
        }
    }
}

impl std::str::FromStr for TimeCategory {
    type Err = UnknownTimeCategory;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "billable" => Ok(Self::Billable),
            "pre_sales" => Ok(Self::PreSales),
            "admin" => Ok(Self::Admin),
            "training" => Ok(Self::Training),
            other => Err(UnknownTimeCategory(other.to_string())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimeEntry {
    pub id: TimeEntryId,
    pub mission_id: MissionId,
    #[serde(with = "crate::domain::serde_date::date")]
    pub worked_on: Date,
    /// Fraction de jour (ex. `0.5` pour une demi-journée).
    pub days: f64,
    pub category: TimeCategory,
    pub note: Option<String>,
    /// Révision optimiste (lot 16) — voir `crate::app::revision`.
    pub revision: i64,
}
