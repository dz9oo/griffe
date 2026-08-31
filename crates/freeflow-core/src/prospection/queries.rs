//! Requêtes de prospection : pipeline pondéré, opportunités en retard, répartition par étape.
//!
//! Le taux de conversion par étape et la durée de cycle moyenne, mentionnés dans le plan
//! initial, exigeraient un historique des transitions que le schéma actuel ne conserve pas
//! (seule l'étape courante est stockée). Ajouter cet historique reste possible plus tard sans
//! rupture ; en attendant, mieux vaut ne rien construire de superficiel sur des données qu'on
//! n'a pas.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use time::Date;

use crate::app::AppError;
use crate::domain::{
    Interaction, InteractionId, Money, Opportunity, OpportunityId, OpportunityStage,
};

use super::row::{self, row_to_opportunity};

/// Deux axes indépendants, plutôt qu'une énumération de leurs combinaisons (qui grossirait en
/// produit cartésien à mesure que les axes s'accumulent) : ouverte/close et archivée/non.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpportunityFilter {
    /// `false` : exclut les opportunités déjà gagnées ou perdues.
    pub include_closed: bool,
    /// `false` : exclut les opportunités archivées.
    pub include_archived: bool,
}

impl OpportunityFilter {
    /// Le pipeline : ouvertes, non archivées — dashboard et écran prospection par défaut.
    pub const OPEN: Self = Self {
        include_closed: false,
        include_archived: false,
    };
    /// Tout, sans exception — la liste inclusive dont dépend la résolution de référence (voir
    /// [`crate::reference::resolve_opportunity`]) : il faut pouvoir retrouver le nom d'une
    /// opportunité même gagnée, perdue, ou archivée.
    pub const ALL: Self = Self {
        include_closed: true,
        include_archived: true,
    };
}

/// Liste filtrée pour un écran de navigation (CLI `prospect list`, écran `prospection` de la
/// GUI) — voir [`OpportunityFilter`].
///
/// # Errors
pub fn list_opportunities_with(
    conn: &Connection,
    filter: OpportunityFilter,
) -> Result<Vec<Opportunity>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT * FROM opportunities
          WHERE (?1 OR stage NOT IN ('won', 'lost'))
            AND (?2 OR archived_at IS NULL)
          ORDER BY created_at ASC",
    )?;
    let rows = stmt.query_map(
        [filter.include_closed, filter.include_archived],
        row_to_opportunity,
    )?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
}

/// Opportunités ouvertes et non archivées — le pipeline affiché tel quel par l'écran prospection
/// de la GUI et par le dashboard. Une opportunité archivée n'y figure plus : c'est l'objet même
/// de l'archivage (elle est devenue sans objet, ni gagnée ni perdue).
///
/// # Errors
pub fn list_open_opportunities(conn: &Connection) -> Result<Vec<Opportunity>, AppError> {
    list_opportunities_with(conn, OpportunityFilter::OPEN)
}

/// Toutes les opportunités, closes et archivées comprises — utilisée pour résoudre un nom (y
/// compris celui d'une opportunité gagnée ou archivée référencée par une facture ou un devis
/// passés) plutôt que pour peupler un écran de navigation. Voir [`list_opportunities_with`] pour
/// une liste filtrée. Miroir exact de `crate::clients::list_clients`.
///
/// # Errors
pub fn list_opportunities(conn: &Connection) -> Result<Vec<Opportunity>, AppError> {
    list_opportunities_with(conn, OpportunityFilter::ALL)
}

/// Une opportunité par id, toutes closes/archivées confondues.
///
/// # Errors
pub fn opportunity_by_id(
    conn: &Connection,
    id: OpportunityId,
) -> Result<Option<Opportunity>, AppError> {
    row::opportunity_by_id(conn, id)
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

/// Ce qui empêche une opportunité d'être supprimée pour de bon — voir [`opportunity_references`].
/// `quotes.opportunity_id` est la seule clé étrangère du schéma pointant vers `opportunities`, et
/// un devis est immuable par trigger (lot 6) : une opportunité devisée n'est donc jamais
/// supprimable, seulement archivable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpportunityReferences {
    pub quotes: i64,
}

impl OpportunityReferences {
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.quotes == 0
    }
}

/// Dénombre ce qui référence encore `id` — sert à la fois au refus de `DeleteOpportunity` et à
/// une façade qui veut afficher *pourquoi* la suppression est bloquée.
///
/// # Errors
pub fn opportunity_references(
    conn: &Connection,
    id: OpportunityId,
) -> Result<OpportunityReferences, AppError> {
    let quotes = conn.query_row(
        "SELECT count(*) FROM quotes WHERE opportunity_id = ?1",
        [id.to_string()],
        |row| row.get(0),
    )?;
    Ok(OpportunityReferences { quotes })
}

/// Interactions d'une opportunité, les plus anciennes d'abord.
///
/// # Errors
pub fn list_interactions(
    conn: &Connection,
    opportunity_id: OpportunityId,
) -> Result<Vec<Interaction>, AppError> {
    row::list_interactions(conn, opportunity_id)
}

/// Une interaction par id.
///
/// # Errors
pub fn interaction_by_id(
    conn: &Connection,
    id: InteractionId,
) -> Result<Option<Interaction>, AppError> {
    row::interaction_by_id(conn, id)
}
