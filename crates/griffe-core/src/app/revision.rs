//! Garde-fou d'écriture concurrente partagé par tous les modules d'entités mutables (clients,
//! prospection, missions) — extrait au lot 16 depuis `clients.rs`, où il vivait en privé avant
//! que la prospection et les missions n'en aient besoin à leur tour. Une préoccupation de couche
//! applicative, pas de domaine (elle ne modélise rien de métier) ni de `store` (elle ne parle pas
//! du fichier-coffre, mais de la sémantique d'une ligne d'entité) — au même titre que
//! `AppError::Conflict`, qui vit déjà dans `app/error.rs`.
//!
//! Chaque module d'entité garde un petit wrapper typé au-dessus de [`require_revision`] (voir
//! `clients::require_client_revision`, `prospection::require_opportunity_revision`, etc.) plutôt
//! que d'appeler directement les fonctions d'ici : c'est ce wrapper qui porte l'erreur
//! `NotFound` du bon type pour son entité.

use rusqlite::{Connection, OptionalExtension};

use super::AppError;

/// Lit la révision actuelle de `id` dans `table`, sans la vérifier ni l'écrire.
pub(crate) fn current_revision(
    conn: &Connection,
    table: &str,
    id: &str,
) -> Result<Option<i64>, AppError> {
    let sql = format!("SELECT revision FROM {table} WHERE id = ?1");
    conn.query_row(&sql, [id], |row| row.get(0))
        .optional()
        .map_err(AppError::from)
}

/// Vérifie que `current == expected`, et renvoie la révision suivante — sans l'écrire : c'est à
/// l'appelant de le faire dans le même `UPDATE`/`DELETE` que sa propre mutation, pour que la
/// vérification et l'écriture restent une seule opération atomique côté SQLite (la transaction
/// `IMMEDIATE` de l'exécuteur fait le reste).
pub(crate) fn require_revision(
    current: i64,
    expected: i64,
    entity: &'static str,
    id: &str,
) -> Result<i64, AppError> {
    if current != expected {
        return Err(AppError::Conflict {
            entity,
            id: id.to_string(),
        });
    }
    Ok(expected + 1)
}
