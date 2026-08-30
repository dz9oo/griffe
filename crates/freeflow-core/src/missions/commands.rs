//! Commandes missions : création directe (hors gain d'opportunité) et saisie de temps.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use time::Date;

use crate::app::{AppError, Command};
use crate::domain::{
    ClientId, Milestone, Mission, MissionId, MissionKind, QuoteId, TimeCategory, TimeEntry,
    TimeEntryId,
};

use super::error::MissionsError;
use super::row;

/// Crée une mission directement (sans passer par le gain d'une opportunité, cf.
/// [`crate::prospection::WinOpportunity`]) — le cas d'un client existant pour qui aucune
/// prospection formelle n'a été tracée.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateMission {
    pub client_id: ClientId,
    pub quote_id: Option<QuoteId>,
    pub name: String,
    pub kind: MissionKind,
    pub milestones: Vec<Milestone>,
    pub started_on: Date,
}

impl Command for CreateMission {
    type Output = MissionId;
    const NAME: &'static str = "missions.create_mission";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let mission = Mission {
            id: MissionId::new(),
            client_id: self.client_id,
            quote_id: self.quote_id,
            name: self.name.clone(),
            kind: self.kind.clone(),
            milestones: self.milestones.clone(),
            started_on: self.started_on,
            ended_on: None,
        };
        row::insert_mission(conn, &mission)?;
        Ok(mission.id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogTime {
    pub mission_id: MissionId,
    pub worked_on: Date,
    /// Fraction de jour (ex. `0.5` pour une demi-journée).
    pub days: f64,
    pub category: TimeCategory,
    pub note: Option<String>,
}

impl Command for LogTime {
    type Output = TimeEntryId;
    const NAME: &'static str = "missions.log_time";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        row::mission_by_id(conn, self.mission_id)?
            .ok_or(MissionsError::NotFound(self.mission_id))?;
        let entry = TimeEntry {
            id: TimeEntryId::new(),
            mission_id: self.mission_id,
            worked_on: self.worked_on,
            days: self.days,
            category: self.category,
            note: self.note.clone(),
        };
        row::insert_time_entry(conn, &entry)?;
        Ok(entry.id)
    }
}
