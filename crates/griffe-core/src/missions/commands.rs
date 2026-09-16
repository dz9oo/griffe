//! Commandes missions : création directe, saisie de temps, et — lot 16 — modification,
//! clôture/réouverture, archivage/désarchivage et suppression d'une mission, ainsi que la
//! modification/suppression d'une saisie de temps.
//!
//! [`UpdateMission`] porte l'état complet *descriptif* d'une mission (nom, type, jalons, date de
//! début) mais volontairement pas son état de *cycle de vie* : ni `ended_on` (voir
//! [`CloseMission`]/[`ReopenMission`]), ni la possibilité de re-pointer `quote_id` (la lignée de
//! facturation ne se re-pointe pas — les devis sont déjà immuables par trigger depuis le lot 6 ;
//! le champ reste dans la struct pour que l'image d'audit soit complète, mais sa valeur est
//! gelée par le cœur, pas par une convention répétée dans chaque façade). Les jalons sont
//! remplacés en bloc : `milestones.id` est un entier de substitution sans identité externe
//! stable, jamais référencé ailleurs dans le schéma — c'est la seule lecture cohérente de « état
//! complet, jamais un patch » pour une collection de ce genre.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use time::Date;

use crate::app::{AppError, Command};
use crate::domain::{
    ClientId, Milestone, Mission, MissionId, MissionKind, QuoteId, TimeCategory, TimeEntry,
    TimeEntryId,
};

use super::error::MissionsError;
use super::queries::mission_references;
use super::row;

const MISSION_ENTITY: &str = "mission";
const TIME_ENTRY_ENTITY: &str = "saisie de temps";

fn require_mission_revision(
    conn: &Connection,
    id: MissionId,
    expected: i64,
) -> Result<i64, AppError> {
    let id_str = id.to_string();
    let current = crate::app::revision::current_revision(conn, "missions", &id_str)?
        .ok_or(MissionsError::NotFound(id))?;
    crate::app::revision::require_revision(current, expected, MISSION_ENTITY, &id_str)
}

fn require_time_entry_revision(
    conn: &Connection,
    id: TimeEntryId,
    expected: i64,
) -> Result<i64, AppError> {
    let id_str = id.to_string();
    let current = crate::app::revision::current_revision(conn, "time_entries", &id_str)?
        .ok_or(MissionsError::TimeEntryNotFound(id))?;
    crate::app::revision::require_revision(current, expected, TIME_ENTRY_ENTITY, &id_str)
}

/// Chaque libellé non vide, et la somme des parts au plus égale à 100 % (`10_000` dix-millièmes)
/// — au-delà, l'échéancier de facturation dérivé ([`crate::missions::billing_schedule`])
/// facturerait plus que le budget vendu.
fn validate_milestones(milestones: &[Milestone]) -> Result<(), MissionsError> {
    if milestones.iter().any(|m| m.label.trim().is_empty()) {
        return Err(MissionsError::EmptyMilestoneLabel);
    }
    let total_bps: u32 = milestones.iter().map(|m| m.share_bps).sum();
    if total_bps > 10_000 {
        return Err(MissionsError::MilestonesOverBudget { total_bps });
    }
    Ok(())
}

/// Borne haute d'une saisie de temps unique. Une journée de travail réelle ne dépasse pas
/// quelques jours-homme par saisie ; ce plafond très généreux (un an) n'est là que pour fermer
/// le vecteur de déni de service par lequel un `days` gigantesque faisait déborder le calcul de
/// revenu (`Money::multiply_by_days`) et figeait le rendu du tableau de bord — voir le test
/// `logging_an_absurd_number_of_days_is_rejected`.
const MAX_DAYS_PER_ENTRY: f64 = 366.0;

fn validate_days(days: f64) -> Result<(), MissionsError> {
    if !days.is_finite() || days <= 0.0 {
        return Err(MissionsError::InvalidDays(days));
    }
    if days > MAX_DAYS_PER_ENTRY {
        return Err(MissionsError::DaysOutOfRange {
            days,
            max: MAX_DAYS_PER_ENTRY,
        });
    }
    Ok(())
}

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
    #[serde(with = "crate::domain::serde_date::date")]
    pub started_on: Date,
}

impl Command for CreateMission {
    type Output = MissionId;
    const NAME: &'static str = "missions.create_mission";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        validate_milestones(&self.milestones)?;
        let mission = Mission {
            id: MissionId::new(),
            client_id: self.client_id,
            quote_id: self.quote_id,
            // Une mission créée directement n'a pas de lignée de prospection : ce lien ne naît
            // que d'un gain (`WinOpportunity`) ou d'une acceptation de devis (`AcceptQuote`) —
            // le poser à la main reviendrait à réécrire l'histoire de l'entonnoir.
            opportunity_id: None,
            name: self.name.clone(),
            kind: self.kind.clone(),
            milestones: self.milestones.clone(),
            started_on: self.started_on,
            ended_on: None,
            revision: 1,
            archived_at: None,
        };
        row::insert_mission(conn, &mission)?;
        Ok(mission.id)
    }
}

/// État complet *descriptif* d'une mission — voir le commentaire de module.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateMission {
    pub id: MissionId,
    pub revision: i64,
    pub name: String,
    pub kind: MissionKind,
    pub milestones: Vec<Milestone>,
    #[serde(with = "crate::domain::serde_date::date")]
    pub started_on: Date,
    /// Doit rester égal à la valeur actuelle — voir le commentaire de module.
    pub quote_id: Option<QuoteId>,
}

impl Command for UpdateMission {
    type Output = i64;
    const NAME: &'static str = "missions.update_mission";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        validate_milestones(&self.milestones)?;
        let current = row::mission_by_id(conn, self.id)?.ok_or(MissionsError::NotFound(self.id))?;
        if self.quote_id != current.quote_id {
            return Err(MissionsError::QuoteLinkImmutable(self.id).into());
        }
        let new_revision = require_mission_revision(conn, self.id, self.revision)?;
        row::update_mission_scalars(
            conn,
            self.id,
            &self.name,
            &self.kind,
            self.started_on,
            new_revision,
            self.revision,
        )?;
        row::replace_milestones(conn, self.id, &self.milestones)?;
        Ok(new_revision)
    }
}

/// Clôt une mission : fait métier daté, réversible ([`ReopenMission`]) — distinct de
/// [`ArchiveMission`], qui est un classement sans date. Voir le commentaire de module de
/// `crate::domain::Mission`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloseMission {
    pub id: MissionId,
    pub revision: i64,
    #[serde(with = "crate::domain::serde_date::date")]
    pub ended_on: Date,
}

impl Command for CloseMission {
    type Output = i64;
    const NAME: &'static str = "missions.close_mission";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let current = row::mission_by_id(conn, self.id)?.ok_or(MissionsError::NotFound(self.id))?;
        if current.ended_on.is_some() {
            return Err(MissionsError::AlreadyClosed(self.id).into());
        }
        if self.ended_on < current.started_on {
            return Err(MissionsError::EndsBeforeStart {
                mission: self.id,
                started_on: current.started_on,
                ended_on: self.ended_on,
            }
            .into());
        }
        let new_revision = require_mission_revision(conn, self.id, self.revision)?;
        row::set_mission_ended(
            conn,
            self.id,
            Some(self.ended_on),
            new_revision,
            self.revision,
        )?;
        Ok(new_revision)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReopenMission {
    pub id: MissionId,
    pub revision: i64,
}

impl Command for ReopenMission {
    type Output = i64;
    const NAME: &'static str = "missions.reopen_mission";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let current = row::mission_by_id(conn, self.id)?.ok_or(MissionsError::NotFound(self.id))?;
        if current.ended_on.is_none() {
            return Err(MissionsError::NotClosed(self.id).into());
        }
        let new_revision = require_mission_revision(conn, self.id, self.revision)?;
        row::set_mission_ended(conn, self.id, None, new_revision, self.revision)?;
        Ok(new_revision)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveMission {
    pub id: MissionId,
    pub revision: i64,
}

impl Command for ArchiveMission {
    type Output = i64;
    const NAME: &'static str = "missions.archive_mission";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        set_mission_archived(conn, self.id, self.revision, true)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnarchiveMission {
    pub id: MissionId,
    pub revision: i64,
}

impl Command for UnarchiveMission {
    type Output = i64;
    const NAME: &'static str = "missions.unarchive_mission";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        set_mission_archived(conn, self.id, self.revision, false)
    }
}

fn set_mission_archived(
    conn: &Connection,
    id: MissionId,
    revision: i64,
    archived: bool,
) -> Result<i64, AppError> {
    let new_revision = require_mission_revision(conn, id, revision)?;
    let archived_at = archived
        .then(|| {
            time::OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339)
        })
        .transpose()?;
    row::set_mission_archived(conn, id, archived_at.as_deref(), new_revision, revision)?;
    Ok(new_revision)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteMission {
    pub id: MissionId,
    pub revision: i64,
}

impl Command for DeleteMission {
    type Output = ();
    const NAME: &'static str = "missions.delete_mission";

    /// Destructeur réel : un agent MCP ne doit jamais le déclencher sans validation humaine
    /// explicite.
    fn requires_confirmation(&self) -> bool {
        true
    }

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        require_mission_revision(conn, self.id, self.revision)?;

        let refs = mission_references(conn, self.id)?;
        if !refs.is_empty() {
            return Err(MissionsError::HasReferences(format!(
                "{} facture(s) et {} saisie(s) de temps référencent encore cette mission — \
                 archivez-la, ou videz d'abord ses saisies de temps, plutôt que de la supprimer",
                refs.invoices, refs.time_entries
            ))
            .into());
        }

        // Les jalons appartiennent au cycle de vie de la mission : ils ne comptent pas comme des
        // références qui bloquent la suppression, ils disparaissent avec elle.
        row::delete_mission(conn, self.id, self.revision)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogTime {
    pub mission_id: MissionId,
    #[serde(with = "crate::domain::serde_date::date")]
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
        validate_days(self.days)?;
        row::mission_by_id(conn, self.mission_id)?
            .ok_or(MissionsError::NotFound(self.mission_id))?;
        let entry = TimeEntry {
            id: TimeEntryId::new(),
            mission_id: self.mission_id,
            worked_on: self.worked_on,
            days: self.days,
            category: self.category,
            note: self.note.clone(),
            revision: 1,
        };
        row::insert_time_entry(conn, &entry)?;
        Ok(entry.id)
    }
}

/// État complet d'une saisie de temps — pas de `mission_id` : déplacer une saisie vers une autre
/// mission déplacerait silencieusement du chiffre d'affaires entre clients, le même raisonnement
/// que `UpdateMission` gelant `quote_id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateTimeEntry {
    pub id: TimeEntryId,
    pub revision: i64,
    #[serde(with = "crate::domain::serde_date::date")]
    pub worked_on: Date,
    pub days: f64,
    pub category: TimeCategory,
    pub note: Option<String>,
}

impl Command for UpdateTimeEntry {
    type Output = i64;
    const NAME: &'static str = "missions.update_time_entry";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        validate_days(self.days)?;
        let new_revision = require_time_entry_revision(conn, self.id, self.revision)?;
        row::update_time_entry(
            conn,
            self.id,
            self.worked_on,
            self.days,
            self.category,
            self.note.as_deref(),
            new_revision,
            self.revision,
        )?;
        Ok(new_revision)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteTimeEntry {
    pub id: TimeEntryId,
    pub revision: i64,
}

impl Command for DeleteTimeEntry {
    type Output = ();
    const NAME: &'static str = "missions.delete_time_entry";

    // Suppression définitive d'une saisie qui alimente une métrique transversale (taux
    // d'occupation) : un agent la propose, un humain la confirme.
    fn requires_confirmation(&self) -> bool {
        true
    }

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        require_time_entry_revision(conn, self.id, self.revision)?;
        row::delete_time_entry(conn, self.id, self.revision)?;
        Ok(())
    }
}
