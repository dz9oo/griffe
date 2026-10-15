//! Séries de phrases. Le type d'une fiche ne les choisit pas.

use rusqlite::{Connection, OptionalExtension};

use crate::app::AppError;

use super::row::ProspectGenre;

/// Le genre de phrases de ce nom, sans tenir compte de la casse.
///
/// # Errors
///
/// Lecture SQL impossible.
pub(crate) fn phrase_genre_named(
    conn: &Connection,
    name: &str,
) -> Result<Option<ProspectGenre>, AppError> {
    conn.query_row(
        "SELECT id, name FROM prospect_genres
          WHERE name = ?1 COLLATE NOCASE
          ORDER BY name ASC, id ASC
          LIMIT 1",
        [name],
        |row| {
            Ok(ProspectGenre {
                id: row.get(0)?,
                name: row.get(1)?,
            })
        },
    )
    .optional()
    .map_err(AppError::from)
}
