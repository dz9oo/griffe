//! Commandes de prospection : toute mutation d'opportunité ou d'interaction passe par ici.
//!
//! Lot 16 : une opportunité est mutable ([`UpdateOpportunity`]), archivable
//! ([`ArchiveOpportunity`]/[`UnarchiveOpportunity`] — un axe *distinct* de `stage = won|lost`,
//! voir le commentaire de [`crate::domain::Opportunity::archived_at`]) et supprimable seulement
//! si aucun devis ne la référence ([`super::queries::opportunity_references`]). `UpdateOpportunity`
//! ne porte volontairement ni `stage` ni `loss_reason` : la doctrine « état complet, jamais un
//! patch » du lot 15 s'applique à l'état *descriptif* d'une entité, pas à son état de *cycle de
//! vie* — `AdvanceOpportunity`/`WinOpportunity`/`LoseOpportunity` gardent leurs gardes dédiées
//! (`Win` crée une mission, `Lose` exige un motif structuré), et un patch d'état complet qui
//! pourrait aussi les court-circuiter romprait ces gardes. Pour la même raison,
//! `UpdateOpportunity` refuse une opportunité déjà close : elle a déjà produit une mission dont
//! le budget a *copié* son montant — corriger après coup se fait sur la mission, l'objet vivant.
//!
//! `AdvanceOpportunity`/`WinOpportunity`/`LoseOpportunity` ne portent pas de champ `revision` :
//! elles relisent déjà l'opportunité dans la même transaction `IMMEDIATE` que leur écriture et
//! refusent sur `stage.is_closed()`, un verrou sémantique plus fort qu'un verrou optimiste pour
//! ce cas précis ; leur ajouter une révision ne fermerait donc aucune fenêtre de conflit réelle
//! (la façade la relirait de toute façon dans la même invocation) et casserait leur contrat
//! CLI/MCP pour rien. Leur écriture (`row::update_stage`) bumpe néanmoins la révision : sans ça,
//! un panneau d'édition ouvert avant une transition garderait une révision qui *paraît* fraîche
//! et écraserait silencieusement le `next_action_at` que la transition vient de poser.

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::{Date, OffsetDateTime};

use crate::app::{AppError, Command};
use crate::domain::{
    ClientId, Interaction, InteractionId, InteractionKind, LossReason, Mission, MissionId,
    MissionKind, Money, Opportunity, OpportunityId, OpportunityStage, Probability,
};

use super::error::ProspectionError;
use super::queries::opportunity_references;
use super::row;

/// Libellé porté par `AppError::Conflict` pour une opportunité — voir `crate::app::revision`.
const OPPORTUNITY_ENTITY: &str = "opportunité";
const INTERACTION_ENTITY: &str = "interaction";

fn require_opportunity_revision(
    conn: &Connection,
    id: OpportunityId,
    expected: i64,
) -> Result<i64, AppError> {
    let id_str = id.to_string();
    let current = crate::app::revision::current_revision(conn, "opportunities", &id_str)?
        .ok_or(ProspectionError::NotFound(id))?;
    crate::app::revision::require_revision(current, expected, OPPORTUNITY_ENTITY, &id_str)
}

fn require_interaction_revision(
    conn: &Connection,
    id: InteractionId,
    expected: i64,
) -> Result<i64, AppError> {
    let id_str = id.to_string();
    let current = crate::app::revision::current_revision(conn, "interactions", &id_str)?
        .ok_or(ProspectionError::InteractionNotFound(id))?;
    crate::app::revision::require_revision(current, expected, INTERACTION_ENTITY, &id_str)
}

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
            revision: 1,
            archived_at: None,
        };
        row::insert_opportunity(conn, &opportunity)?;
        Ok(opportunity.id)
    }
}

/// État complet *descriptif* d'une opportunité — pas de patch, et pas de cycle de vie (voir le
/// commentaire de module). Refusée sur une opportunité close.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateOpportunity {
    pub id: OpportunityId,
    /// Révision lue avant modification.
    pub revision: i64,
    pub name: String,
    pub amount: Money,
    pub probability: Probability,
    pub next_action_at: Option<Date>,
    pub source: Option<String>,
}

impl Command for UpdateOpportunity {
    /// La révision résultante.
    type Output = i64;
    const NAME: &'static str = "prospection.update_opportunity";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let opportunity =
            row::opportunity_by_id(conn, self.id)?.ok_or(ProspectionError::NotFound(self.id))?;
        if opportunity.stage.is_closed() {
            return Err(ProspectionError::AlreadyClosed(self.id).into());
        }
        let new_revision = require_opportunity_revision(conn, self.id, self.revision)?;
        conn.execute(
            "UPDATE opportunities
                SET name = ?1, amount_cents = ?2, probability_percent = ?3, next_action_at = ?4,
                    source = ?5, revision = ?6
              WHERE id = ?7 AND revision = ?8",
            params![
                self.name,
                self.amount.cents(),
                i64::from(self.probability.percent()),
                self.next_action_at.map(crate::domain::format_date),
                self.source,
                new_revision,
                self.id.to_string(),
                self.revision,
            ],
        )?;
        Ok(new_revision)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveOpportunity {
    pub id: OpportunityId,
    pub revision: i64,
}

impl Command for ArchiveOpportunity {
    type Output = i64;
    const NAME: &'static str = "prospection.archive_opportunity";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        set_opportunity_archived(conn, self.id, self.revision, true)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnarchiveOpportunity {
    pub id: OpportunityId,
    pub revision: i64,
}

impl Command for UnarchiveOpportunity {
    type Output = i64;
    const NAME: &'static str = "prospection.unarchive_opportunity";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        set_opportunity_archived(conn, self.id, self.revision, false)
    }
}

fn set_opportunity_archived(
    conn: &Connection,
    id: OpportunityId,
    revision: i64,
    archived: bool,
) -> Result<i64, AppError> {
    let new_revision = require_opportunity_revision(conn, id, revision)?;
    let archived_at = archived
        .then(|| OffsetDateTime::now_utc().format(&Rfc3339))
        .transpose()?;
    conn.execute(
        "UPDATE opportunities SET archived_at = ?1, revision = ?2 WHERE id = ?3 AND revision = ?4",
        params![archived_at, new_revision, id.to_string(), revision],
    )?;
    Ok(new_revision)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteOpportunity {
    pub id: OpportunityId,
    pub revision: i64,
}

impl Command for DeleteOpportunity {
    type Output = ();
    const NAME: &'static str = "prospection.delete_opportunity";

    /// Destructeur réel : un agent MCP ne doit jamais le déclencher sans validation humaine
    /// explicite.
    fn requires_confirmation(&self) -> bool {
        true
    }

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        require_opportunity_revision(conn, self.id, self.revision)?;

        let refs = opportunity_references(conn, self.id)?;
        if !refs.is_empty() {
            return Err(ProspectionError::HasReferences(format!(
                "{} devis référence(nt) encore cette opportunité — archivez-la plutôt que de la \
                 supprimer",
                refs.quotes
            ))
            .into());
        }

        // Les interactions appartiennent au cycle de vie de l'opportunité : elles ne comptent pas
        // comme des références qui bloquent la suppression, elles disparaissent avec elle.
        conn.execute(
            "DELETE FROM interactions WHERE opportunity_id = ?1",
            [self.id.to_string()],
        )?;
        conn.execute(
            "DELETE FROM opportunities WHERE id = ?1 AND revision = ?2",
            params![self.id.to_string(), self.revision],
        )?;
        Ok(())
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
            revision: 1,
            archived_at: None,
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
    /// `None` → horodatée à l'instant présent. Permet de journaliser un échange survenu plus tôt
    /// (« j'ai oublié de noter l'appel de mardi ») — sans ce champ, seule `UpdateInteraction`
    /// pourrait dater une interaction, ce que `LogInteraction` ne pourrait alors jamais faire
    /// rétroactivement.
    #[serde(default)]
    pub occurred_at: Option<OffsetDateTime>,
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
            occurred_at: self.occurred_at.unwrap_or_else(OffsetDateTime::now_utc),
            revision: 1,
        };
        row::insert_interaction(conn, &interaction)?;
        Ok(interaction.id)
    }
}

/// État complet d'une interaction — pas de patch, même doctrine que [`UpdateOpportunity`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateInteraction {
    pub id: InteractionId,
    pub revision: i64,
    pub kind: InteractionKind,
    pub note: String,
    pub occurred_at: OffsetDateTime,
}

impl Command for UpdateInteraction {
    type Output = i64;
    const NAME: &'static str = "prospection.update_interaction";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let new_revision = require_interaction_revision(conn, self.id, self.revision)?;
        conn.execute(
            "UPDATE interactions SET kind = ?1, note = ?2, occurred_at = ?3, revision = ?4
              WHERE id = ?5 AND revision = ?6",
            params![
                self.kind.as_str(),
                self.note,
                self.occurred_at.format(&Rfc3339)?,
                new_revision,
                self.id.to_string(),
                self.revision,
            ],
        )?;
        Ok(new_revision)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteInteraction {
    pub id: InteractionId,
    pub revision: i64,
}

impl Command for DeleteInteraction {
    type Output = ();
    const NAME: &'static str = "prospection.delete_interaction";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        require_interaction_revision(conn, self.id, self.revision)?;
        conn.execute(
            "DELETE FROM interactions WHERE id = ?1 AND revision = ?2",
            params![self.id.to_string(), self.revision],
        )?;
        Ok(())
    }
}
