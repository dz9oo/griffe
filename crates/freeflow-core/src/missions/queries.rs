//! Requêtes missions : TJM effectif et capacité mensuelle globale (tous clients confondus).

use rusqlite::Connection;

use crate::app::AppError;
use crate::domain::{self, Mission, MissionKind, Money, Month};

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

/// Missions actives (non closes), les plus récemment démarrées d'abord — alimente l'écran
/// missions de la GUI (lot 9).
///
/// # Errors
pub fn list_active_missions(conn: &Connection) -> Result<Vec<Mission>, AppError> {
    let missions = row::all_missions(conn)?;
    Ok(missions
        .into_iter()
        .filter(|m| m.ended_on.is_none())
        .collect())
}

/// TJM effectif d'une mission : chiffre d'affaires ÷ jours facturables réellement consommés.
/// `None` si aucun jour facturable n'a encore été saisi (division non pertinente).
///
/// # Errors
pub fn effective_daily_rate(
    conn: &Connection,
    mission_id: crate::domain::MissionId,
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
    let revenue = mission_revenue(&mission, billable_days);
    Ok(Some(revenue.divide_by_days(billable_days)))
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MonthlyCapacity {
    pub month: Month,
    pub available_business_days: u32,
    pub billable_days: f64,
}

impl MonthlyCapacity {
    /// Taux d'occupation, en pourcentage des jours ouvrés disponibles ce mois-là.
    #[must_use]
    pub fn utilization_percent(self) -> f64 {
        if self.available_business_days == 0 {
            0.0
        } else {
            self.billable_days / f64::from(self.available_business_days) * 100.0
        }
    }
}

/// Capacité vendue pour un mois donné : jours facturables saisis tous clients confondus,
/// rapportés aux jours ouvrés français disponibles ce mois-là — la mesure qui alimente
/// l'alerte de sous-remplissage du dashboard.
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
