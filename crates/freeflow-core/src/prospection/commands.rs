//! Commandes de prospection : toute mutation d'opportunité ou d'interaction passe par ici.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use time::{Date, OffsetDateTime};

use crate::app::{AppError, Command};
use crate::domain::{
    ClientId, Interaction, InteractionId, InteractionKind, LossReason, Mission, MissionId,
    MissionKind, Money, Opportunity, OpportunityId, OpportunityStage, Probability,
};

use super::error::ProspectionError;
use super::row;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateOpportunity {
    pub client_id: ClientId,
    pub name: String,
    pub amount: Money,
    pub probability: Probability,
    pub next_action_at: Date,
    pub source: Option<String>,
}

impl Command for CreateOpportunity {
    type Output = OpportunityId;
    const NAME: &'static str = "prospection.create_opportunity";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let opportunity = Opportunity {
            id: OpportunityId::new(),
            client_id: self.client_id,
            name: self.name.clone(),
            stage: OpportunityStage::Qualification,
            amount: self.amount,
            probability: self.probability,
            next_action_at: Some(self.next_action_at),
            source: self.source.clone(),
            loss_reason: None,
            created_at: OffsetDateTime::now_utc(),
        };
        row::insert_opportunity(conn, &opportunity)?;
        Ok(opportunity.id)
    }
}

/// Fait avancer une opportunité vers une autre étape ouverte (`Qualification`, `Discovery`,
/// `Proposal` ou `Negotiation`). Le gain et la perte ont leurs propres commandes
/// ([`WinOpportunity`], [`LoseOpportunity`]) : elles seules peuvent fermer une opportunité, et
/// une opportunité déjà close ne peut plus jamais bouger.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdvanceOpportunity {
    pub opportunity_id: OpportunityId,
    pub to: OpportunityStage,
    /// La prochaine action reste obligatoire à chaque étape ouverte : avancer sans en fournir
    /// une nouvelle n'est pas permis par ce contrat de type.
    pub next_action_at: Date,
}

impl Command for AdvanceOpportunity {
    type Output = ();
    const NAME: &'static str = "prospection.advance_opportunity";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        if self.to.is_closed() {
            return Err(ProspectionError::InvalidTarget(self.to).into());
        }
        let opportunity = row::opportunity_by_id(conn, self.opportunity_id)?
            .ok_or(ProspectionError::NotFound(self.opportunity_id))?;
        if opportunity.stage.is_closed() {
            return Err(ProspectionError::AlreadyClosed(self.opportunity_id).into());
        }
        row::update_stage(
            conn,
            self.opportunity_id,
            self.to,
            Some(self.next_action_at),
            None,
        )?;
        Ok(())
    }
}

/// Gagne une opportunité et crée la mission correspondante : forfait budgété au montant de
/// l'opportunité, sans devis associé (le cas d'un devis formel accepté relève du lot Devis).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WinOpportunity {
    pub opportunity_id: OpportunityId,
    pub started_on: Date,
}

impl Command for WinOpportunity {
    type Output = MissionId;
    const NAME: &'static str = "prospection.win_opportunity";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let opportunity = row::opportunity_by_id(conn, self.opportunity_id)?
            .ok_or(ProspectionError::NotFound(self.opportunity_id))?;
        if opportunity.stage.is_closed() {
            return Err(ProspectionError::AlreadyClosed(self.opportunity_id).into());
        }
        row::update_stage(conn, self.opportunity_id, OpportunityStage::Won, None, None)?;

        let mission = Mission {
            id: MissionId::new(),
            client_id: opportunity.client_id,
            quote_id: None,
            name: opportunity.name.clone(),
            kind: MissionKind::Forfait {
                budget: opportunity.amount,
            },
            milestones: Vec::new(),
            started_on: self.started_on,
            ended_on: None,
        };
        crate::missions::row::insert_mission(conn, &mission)?;
        Ok(mission.id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoseOpportunity {
    pub opportunity_id: OpportunityId,
    pub reason: LossReason,
}

impl Command for LoseOpportunity {
    type Output = ();
    const NAME: &'static str = "prospection.lose_opportunity";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let opportunity = row::opportunity_by_id(conn, self.opportunity_id)?
            .ok_or(ProspectionError::NotFound(self.opportunity_id))?;
        if opportunity.stage.is_closed() {
            return Err(ProspectionError::AlreadyClosed(self.opportunity_id).into());
        }
        row::update_stage(
            conn,
            self.opportunity_id,
            OpportunityStage::Lost,
            None,
            Some(&self.reason),
        )?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogInteraction {
    pub opportunity_id: OpportunityId,
    pub kind: InteractionKind,
    pub note: String,
}

impl Command for LogInteraction {
    type Output = InteractionId;
    const NAME: &'static str = "prospection.log_interaction";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        // Vérifié explicitement pour échouer proprement plutôt que de heurter la contrainte de
        // clé étrangère avec une erreur SQLite peu lisible.
        row::opportunity_by_id(conn, self.opportunity_id)?
            .ok_or(ProspectionError::NotFound(self.opportunity_id))?;
        let interaction = Interaction {
            id: InteractionId::new(),
            opportunity_id: self.opportunity_id,
            kind: self.kind,
            note: self.note.clone(),
            occurred_at: OffsetDateTime::now_utc(),
        };
        row::insert_interaction(conn, &interaction)?;
        Ok(interaction.id)
    }
}
