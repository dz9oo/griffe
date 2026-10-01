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
    Address, Client, ClientId, Interaction, InteractionId, InteractionKind, LossReason, Mission,
    MissionId, MissionKind, Money, Opportunity, OpportunityId, OpportunityStage, Probability,
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
    #[serde(with = "crate::domain::serde_date::date")]
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

fn trimmed_opt(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
}

fn upsert_prospect_contact(
    conn: &Connection,
    client_id: ClientId,
    prospect_name: &str,
    representative: Option<&str>,
    email: Option<&str>,
    phone: Option<&str>,
) -> Result<(), AppError> {
    let representative = trimmed_opt(representative);
    let email = trimmed_opt(email);
    let phone = trimmed_opt(phone);
    if representative.is_none() && email.is_none() && phone.is_none() {
        return Ok(());
    }
    let existing = crate::clients::list_contacts(conn, client_id)?;
    if let Some(current) = existing.into_iter().next() {
        crate::clients::UpdateContact {
            id: current.id,
            revision: current.revision,
            name: representative.unwrap_or(current.name),
            email: email.or(current.email),
            phone: phone.or(current.phone),
            role: current.role,
        }
        .apply(conn)?;
    } else {
        crate::clients::CreateContact {
            client_id,
            name: representative.unwrap_or_else(|| prospect_name.to_string()),
            email,
            phone,
            role: None,
        }
        .apply(conn)?;
    }
    Ok(())
}

/// Crée une opportunité en créant au besoin la fiche du prospect. La fiche est une ligne
/// `clients` (`is_prospect = 1`) : devis et facture en ont besoin, mais elle n'apparaît dans
/// l'onglet Clients qu'à la première pièce commerciale. Un nom qui correspond *exactement* à
/// une fiche déjà présente (casse et accents ignorés) s'y rattache, sans la modifier.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateProspect {
    pub prospect_name: String,
    pub address: Option<Address>,
    pub representative: Option<String>,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub name: String,
    pub amount: Money,
    pub probability: Probability,
    #[serde(with = "crate::domain::serde_date::date")]
    pub next_action_at: Date,
    pub source: Option<String>,
}

impl Command for CreateProspect {
    type Output = OpportunityId;
    const NAME: &'static str = "prospection.create_prospect";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let prospect_name = self.prospect_name.trim();
        if prospect_name.is_empty() {
            return Err(ProspectionError::ProspectNameRequired.into());
        }
        let client_id = match crate::reference::resolve_client_exact(conn, prospect_name)? {
            crate::reference::RefMatch::Unique(id) => id,
            crate::reference::RefMatch::Ambiguous(_) => {
                return Err(ProspectionError::AmbiguousProspect(prospect_name.to_string()).into());
            }
            crate::reference::RefMatch::NotFound => {
                let client = Client {
                    id: ClientId::new(),
                    name: prospect_name.to_string(),
                    siren: None,
                    vat_number: None,
                    address: self.address.clone(),
                    created_at: OffsetDateTime::now_utc(),
                    revision: 1,
                    archived_at: None,
                };
                crate::clients::insert_client_as(conn, &client, true)?;
                upsert_prospect_contact(
                    conn,
                    client.id,
                    prospect_name,
                    self.representative.as_deref(),
                    self.email.as_deref(),
                    self.phone.as_deref(),
                )?;
                client.id
            }
        };
        CreateOpportunity {
            client_id,
            name: self.name.clone(),
            amount: self.amount,
            probability: self.probability,
            next_action_at: self.next_action_at,
            source: self.source.clone(),
        }
        .apply(conn)
    }
}

/// Met à jour l'opportunité *et* la fiche prospect (nom, adresse, contact) tant que cette
/// fiche n'est pas encore un client. Après un devis ou une facture, [`UpdateOpportunity`]
/// reste disponible ; la fiche se modifie depuis Clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateProspect {
    pub id: OpportunityId,
    pub revision: i64,
    pub client_revision: i64,
    pub name: String,
    pub amount: Money,
    pub probability: Probability,
    #[serde(with = "crate::domain::serde_date::date::option")]
    pub next_action_at: Option<Date>,
    pub source: Option<String>,
    pub prospect_name: String,
    pub address: Option<Address>,
    pub representative: Option<String>,
    pub email: Option<String>,
    pub phone: Option<String>,
}

impl Command for UpdateProspect {
    type Output = i64;
    const NAME: &'static str = "prospection.update_prospect";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let opportunity =
            row::opportunity_by_id(conn, self.id)?.ok_or(ProspectionError::NotFound(self.id))?;
        if opportunity.stage.is_closed() {
            return Err(ProspectionError::AlreadyClosed(self.id).into());
        }
        if !crate::clients::prospect_party_is_editable(conn, opportunity.client_id)? {
            return Err(ProspectionError::AlreadyAClient.into());
        }
        let prospect_name = self.prospect_name.trim();
        if prospect_name.is_empty() {
            return Err(ProspectionError::ProspectNameRequired.into());
        }
        let current = crate::clients::client_by_id(conn, opportunity.client_id)?
            .ok_or(crate::clients::ClientError::NotFound(opportunity.client_id))?;
        crate::clients::UpdateClient {
            id: current.id,
            revision: self.client_revision,
            name: prospect_name.to_string(),
            siren: current.siren,
            vat_number: current.vat_number,
            address: self.address.clone(),
        }
        .apply(conn)?;
        upsert_prospect_contact(
            conn,
            current.id,
            prospect_name,
            self.representative.as_deref(),
            self.email.as_deref(),
            self.phone.as_deref(),
        )?;
        UpdateOpportunity {
            id: self.id,
            revision: self.revision,
            name: self.name.clone(),
            amount: self.amount,
            probability: self.probability,
            next_action_at: self.next_action_at,
            source: self.source.clone(),
        }
        .apply(conn)
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
    #[serde(with = "crate::domain::serde_date::date::option")]
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
                "{} devis et {} mission(s) référencent encore cette opportunité — archivez-la \
                 plutôt que de la supprimer",
                refs.quotes, refs.missions
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
    #[serde(with = "crate::domain::serde_date::date")]
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
    #[serde(with = "crate::domain::serde_date::date")]
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
            opportunity_id: Some(self.opportunity_id),
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

/// Rouvre une conversation arrêtée. Les lettres, les rencontres et le montant restent sur
/// la même opportunité. La cadence de relance repart, sans effacer l'historique.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReopenOpportunity {
    pub opportunity_id: OpportunityId,
    #[serde(with = "crate::domain::serde_date::date")]
    pub next_action_at: Date,
}

impl Command for ReopenOpportunity {
    type Output = ();
    const NAME: &'static str = "prospection.reopen_opportunity";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let opportunity = row::opportunity_by_id(conn, self.opportunity_id)?
            .ok_or(ProspectionError::NotFound(self.opportunity_id))?;
        if opportunity.stage != OpportunityStage::Lost {
            return Err(ProspectionError::NotLost(self.opportunity_id).into());
        }
        let note = opportunity
            .loss_reason
            .as_ref()
            .map_or_else(|| "Conversation reprise.".into(), loss_history_note);
        LogInteraction {
            opportunity_id: self.opportunity_id,
            kind: crate::domain::InteractionKind::Note,
            note,
            occurred_at: None,
        }
        .apply(conn)?;
        row::update_stage(
            conn,
            self.opportunity_id,
            OpportunityStage::Discovery,
            Some(self.next_action_at),
            None,
        )?;
        crate::follow_up::open_cycle(conn, self.opportunity_id, self.next_action_at)?;
        Ok(())
    }
}

/// Remplace les lignes de travaux d'une estimation et aligne le montant de l'opportunité
/// sur leur somme.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetEstimation {
    pub id: OpportunityId,
    pub revision: i64,
    pub name: String,
    pub lines: Vec<EstimationLineInput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EstimationLineInput {
    pub label: String,
    pub amount: Money,
}

impl Command for SetEstimation {
    type Output = i64;
    const NAME: &'static str = "prospection.set_estimation";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let opportunity =
            row::opportunity_by_id(conn, self.id)?.ok_or(ProspectionError::NotFound(self.id))?;
        if opportunity.stage.is_closed() {
            return Err(ProspectionError::AlreadyClosed(self.id).into());
        }
        if self.lines.is_empty() {
            return Err(ProspectionError::EstimationRequired.into());
        }
        let mut total = Money::ZERO;
        for line in &self.lines {
            if line.label.trim().is_empty() || line.amount.cents() <= 0 {
                return Err(ProspectionError::EstimationLineInvalid.into());
            }
            total = total
                .checked_add(line.amount)
                .ok_or(ProspectionError::EstimationLineInvalid)?;
        }
        let new_revision = require_opportunity_revision(conn, self.id, self.revision)?;
        conn.execute(
            "DELETE FROM estimation_lines WHERE opportunity_id = ?1",
            [self.id.to_string()],
        )?;
        for (position, line) in self.lines.iter().enumerate() {
            conn.execute(
                "INSERT INTO estimation_lines (opportunity_id, position, label, amount_cents)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    self.id.to_string(),
                    i64::try_from(position).unwrap_or(i64::MAX),
                    line.label.trim(),
                    line.amount.cents(),
                ],
            )?;
        }
        conn.execute(
            "UPDATE opportunities
                SET name = ?1, amount_cents = ?2, revision = ?3
              WHERE id = ?4 AND revision = ?5",
            params![
                self.name.trim(),
                total.cents(),
                new_revision,
                self.id.to_string(),
                self.revision,
            ],
        )?;
        Ok(new_revision)
    }
}

fn loss_history_note(reason: &crate::domain::LossReason) -> String {
    let why = match reason {
        crate::domain::LossReason::Budget => "le budget ne suivait pas",
        crate::domain::LossReason::Timing => "pas le bon moment",
        crate::domain::LossReason::Competitor => "quelqu'un d'autre a été choisi",
        crate::domain::LossReason::NoResponse => "pas de réponse",
        crate::domain::LossReason::ScopeMismatch => "ce n'était pas le bon sujet",
        crate::domain::LossReason::Other(text) => text.as_str(),
    };
    format!("Arrêtée : {why}. Conversation reprise.")
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
    #[serde(with = "crate::domain::serde_date::datetime::option")]
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
    #[serde(with = "crate::domain::serde_date::datetime")]
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

    // Suppression définitive, cohérente avec les autres suppressions : un agent la propose, un
    // humain la confirme.
    fn requires_confirmation(&self) -> bool {
        true
    }

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        require_interaction_revision(conn, self.id, self.revision)?;
        conn.execute(
            "DELETE FROM interactions WHERE id = ?1 AND revision = ?2",
            params![self.id.to_string(), self.revision],
        )?;
        Ok(())
    }
}
