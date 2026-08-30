//! Missions polymorphes (régie / forfait / récurrent). La génération d'échéancier et les
//! calculs de TJM effectif sont portés par la couche applicative (lot 4).

use serde::{Deserialize, Serialize};
use time::Date;

use super::ids::{ClientId, MissionId, QuoteId};
use super::money::Money;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MissionKind {
    Regie { daily_rate: Money },
    Forfait { budget: Money },
    Recurrent { monthly_amount: Money },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Milestone {
    pub label: String,
    /// Part du budget total en dix-millièmes (`10_000` = 100 %).
    pub share_bps: u32,
    pub due_on: Option<Date>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Mission {
    pub id: MissionId,
    pub client_id: ClientId,
    pub quote_id: Option<QuoteId>,
    pub name: String,
    pub kind: MissionKind,
    pub milestones: Vec<Milestone>,
    pub started_on: Date,
    pub ended_on: Option<Date>,
}
