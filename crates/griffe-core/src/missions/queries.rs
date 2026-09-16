//! Requêtes missions : TJM effectif et capacité mensuelle globale (tous clients confondus).

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::app::AppError;
use crate::domain::{self, Mission, MissionId, MissionKind, Money, Month, TimeEntry, TimeEntryId};

use super::error::MissionsError;
use super::row;

/// Chiffre d'affaires porté par une mission, indépendamment du temps déjà consommé :
/// - régie : TJM contractuel × jours facturables déjà saisis (le CA n'existe qu'au fil de l'eau) ;
/// - forfait : le budget vendu dans son intégralité (fixe par définition) ;
/// - récurrent : le montant mensuel (un seul mois à la fois — cumuler sur plusieurs mois
///   suppose de savoir combien de mois facturés, hors périmètre d'un calcul instantané).
fn mission_revenue(mission: &Mission, billable_days: f64) -> Money {
    match &mission.kind {
        MissionKind::Regie { daily_rate } => daily_rate.multiply_by_days(billable_days),
        MissionKind::Forfait { budget } => *budget,
        MissionKind::Recurrent { monthly_amount } => *monthly_amount,
    }
}

/// Deux axes indépendants, plutôt qu'une énumération de leurs combinaisons : terminée/en cours
/// et archivée/non — même patron que `crate::prospection::OpportunityFilter`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MissionFilter {
    /// `false` : exclut les missions déjà closes (`ended_on` renseigné).
    pub include_ended: bool,
    /// `false` : exclut les missions archivées.
    pub include_archived: bool,
}

impl MissionFilter {
    /// En cours, non archivées — l'écran missions et le prévisionnel de trésorerie.
    pub const ACTIVE: Self = Self {
        include_ended: false,
        include_archived: false,
    };
    /// Tout, sans exception — la liste inclusive dont dépend la résolution de référence.
    pub const ALL: Self = Self {
        include_ended: true,
        include_archived: true,
    };
}

/// Liste filtrée pour un écran de navigation — voir [`MissionFilter`]. Charge les jalons de
/// chaque mission (deux requêtes au total, jamais une par mission).
///
/// # Errors
pub fn list_missions_with(
    conn: &Connection,
    filter: MissionFilter,
) -> Result<Vec<Mission>, AppError> {
    row::missions_matching(
        conn,
        "SELECT * FROM missions
          WHERE (?1 OR ended_on IS NULL)
            AND (?2 OR archived_at IS NULL)
          ORDER BY started_on DESC",
        params![filter.include_ended, filter.include_archived],
    )
}

/// Missions actives (ni closes, ni archivées), les plus récemment démarrées d'abord — alimente
/// l'écran missions de la GUI et le prévisionnel de trésorerie.
///
/// # Errors
pub fn list_active_missions(conn: &Connection) -> Result<Vec<Mission>, AppError> {
    list_missions_with(conn, MissionFilter::ACTIVE)
}

/// Toutes les missions, closes et archivées comprises — utilisée pour résoudre un nom (y compris
/// celui d'une mission archivée référencée par une facture passée) plutôt que pour peupler un
/// écran de navigation. Miroir exact de `crate::clients::list_clients`.
///
/// # Errors
pub fn list_missions(conn: &Connection) -> Result<Vec<Mission>, AppError> {
    list_missions_with(conn, MissionFilter::ALL)
}

/// Une mission par id, avec ses jalons, close/archivée ou non.
///
/// # Errors
pub fn mission_by_id(conn: &Connection, id: MissionId) -> Result<Option<Mission>, AppError> {
    row::mission_by_id(conn, id)
}

/// TJM effectif d'une mission : chiffre d'affaires ÷ jours facturables réellement consommés.
/// `None` si aucun jour facturable n'a encore été saisi (division non pertinente).
///
/// # Errors
pub fn effective_daily_rate(
    conn: &Connection,
    mission_id: MissionId,
) -> Result<Option<Money>, AppError> {
    let mission =
        row::mission_by_id(conn, mission_id)?.ok_or(MissionsError::NotFound(mission_id))?;
    let entries = row::time_entries_for_mission(conn, mission_id)?;
    let billable_days: f64 = entries
        .iter()
        .filter(|e| e.category.is_billable())
        .map(|e| e.days)
        .sum();
    if billable_days <= 0.0 {
        return Ok(None);
    }
    Ok(Some(
        mission_revenue(&mission, billable_days).divide_by_days(billable_days),
    ))
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MonthlyCapacity {
    pub month: Month,
    pub available_business_days: u32,
    pub billable_days: f64,
}

impl MonthlyCapacity {
    #[must_use]
    pub fn utilization_percent(self) -> f64 {
        if self.available_business_days == 0 {
            return 0.0;
        }
        (self.billable_days / f64::from(self.available_business_days)) * 100.0
    }
}

/// Capacité vendue sur `month`, tous clients et toutes missions confondus (temps facturable
/// saisi ÷ jours ouvrés du mois).
///
/// # Errors
pub fn monthly_capacity(conn: &Connection, month: Month) -> Result<MonthlyCapacity, AppError> {
    let entries = row::all_time_entries(conn)?;
    let billable_days: f64 = entries
        .iter()
        .filter(|e| {
            e.category.is_billable()
                && e.worked_on >= month.first_day()
                && e.worked_on <= month.last_day()
        })
        .map(|e| e.days)
        .sum();
    let available_business_days =
        domain::french_business_days_in(month.first_day(), month.last_day());
    Ok(MonthlyCapacity {
        month,
        available_business_days,
        billable_days,
    })
}

/// Ce qui empêche une mission d'être supprimée pour de bon — voir [`mission_references`]. Les
/// factures bloquent parce qu'elles sont immuables par trigger (lot 5). Les saisies de temps
/// bloquent aussi, à la différence des jalons ou des contacts : elles n'existent nulle part
/// ailleurs et alimentent une métrique transversale ([`monthly_capacity`], tous clients
/// confondus) — les supprimer en cascade réécrirait silencieusement un taux d'occupation passé.
/// L'échappatoire reste `ArchiveMission`, ou vider les saisies une à une avant de supprimer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionReferences {
    pub invoices: i64,
    pub time_entries: i64,
}

impl MissionReferences {
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.invoices == 0 && self.time_entries == 0
    }
}

/// Dénombre ce qui référence encore `id`.
///
/// # Errors
pub fn mission_references(conn: &Connection, id: MissionId) -> Result<MissionReferences, AppError> {
    let invoices = conn.query_row(
        "SELECT count(*) FROM invoices WHERE mission_id = ?1",
        [id.to_string()],
        |row| row.get(0),
    )?;
    let time_entries = conn.query_row(
        "SELECT count(*) FROM time_entries WHERE mission_id = ?1",
        [id.to_string()],
        |row| row.get(0),
    )?;
    Ok(MissionReferences {
        invoices,
        time_entries,
    })
}

/// Saisies de temps d'une mission, les plus anciennes d'abord.
///
/// # Errors
pub fn list_time_entries(
    conn: &Connection,
    mission_id: MissionId,
) -> Result<Vec<TimeEntry>, AppError> {
    row::time_entries_for_mission(conn, mission_id)
}

/// Une saisie de temps par id.
///
/// # Errors
pub fn time_entry_by_id(conn: &Connection, id: TimeEntryId) -> Result<Option<TimeEntry>, AppError> {
    row::time_entry_by_id(conn, id)
}
