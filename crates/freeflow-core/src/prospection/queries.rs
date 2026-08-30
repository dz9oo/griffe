//! Requêtes de prospection : pipeline pondéré, opportunités en retard, répartition par étape.
//!
//! Le taux de conversion par étape et la durée de cycle moyenne, mentionnés dans le plan
//! initial, exigeraient un historique des transitions que le schéma actuel ne conserve pas
//! (seule l'étape courante est stockée). Ajouter cet historique reste possible plus tard sans
//! rupture ; en attendant, mieux vaut ne rien construire de superficiel sur des données qu'on
//! n'a pas.

use rusqlite::Connection;
use time::Date;

use crate::app::AppError;
use crate::domain::{Money, Opportunity, OpportunityStage};

use super::row::row_to_opportunity;

/// Opportunités ouvertes (ni gagnées ni perdues) — le pipeline affiché tel quel par l'écran
/// prospection de la GUI (lot 9).
///
/// # Errors
pub fn list_open_opportunities(conn: &Connection) -> Result<Vec<Opportunity>, AppError> {
    let mut stmt =
        conn.prepare("SELECT * FROM opportunities WHERE stage NOT IN ('won', 'lost')")?;
    let rows = stmt.query_map([], row_to_opportunity)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
}

/// Opportunités ouvertes dont la date de prochaine action est strictement antérieure à
/// `today`.
///
/// # Errors
pub fn late_actions(conn: &Connection, today: Date) -> Result<Vec<Opportunity>, AppError> {
    let opportunities = list_open_opportunities(conn)?;
    Ok(opportunities
        .into_iter()
        .filter(|o| o.next_action_at.is_some_and(|d| d < today))
        .collect())
}

/// Opportunités ouvertes sans date de prochaine action — ne devrait normalement jamais se
/// produire puisque les commandes l'exigent à la création comme à chaque avancée, mais sert de
/// filet de sécurité observable (import de données, migration, bug futur).
///
/// # Errors
pub fn without_next_action(conn: &Connection) -> Result<Vec<Opportunity>, AppError> {
    let opportunities = list_open_opportunities(conn)?;
    Ok(opportunities
        .into_iter()
        .filter(|o| o.next_action_at.is_none())
        .collect())
}

/// Somme des montants des opportunités ouvertes, pondérés par leur probabilité — le pipeline
/// pondéré affiché au dashboard.
///
/// # Errors
pub fn weighted_pipeline(conn: &Connection) -> Result<Money, AppError> {
    let opportunities = list_open_opportunities(conn)?;
    Ok(opportunities
        .iter()
        .map(|o| o.probability.weighted(o.amount))
        .sum())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StageSummary {
    pub stage: OpportunityStage,
    pub count: u32,
    pub weighted_amount: Money,
}

/// Répartition des opportunités ouvertes par étape — alimente le kanban et l'entonnoir du
/// dashboard.
///
/// # Errors
pub fn pipeline_by_stage(conn: &Connection) -> Result<Vec<StageSummary>, AppError> {
    let opportunities = list_open_opportunities(conn)?;
    let stages = [
        OpportunityStage::Qualification,
        OpportunityStage::Discovery,
        OpportunityStage::Proposal,
        OpportunityStage::Negotiation,
    ];
    Ok(stages
        .into_iter()
        .map(|stage| {
            let matching: Vec<&Opportunity> =
                opportunities.iter().filter(|o| o.stage == stage).collect();
            StageSummary {
                stage,
                count: u32::try_from(matching.len()).unwrap_or(u32::MAX),
                weighted_amount: matching
                    .iter()
                    .map(|o| o.probability.weighted(o.amount))
                    .sum(),
            }
        })
        .collect())
}
