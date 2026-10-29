//! Lecture du catalogue. L'adaptateur écrit le fichier.

use rusqlite::Connection;

use crate::app::AppError;
use crate::follow_up::{phrases_for_genre, prospect_genres, prospect_phrases};

use super::document::{CadenceMoment, ExchangeDocument, SeriesFile, SeriesMoment};

/// Séries et cadence, dans la forme du document. Les dossiers restent vides.
///
/// # Errors
///
/// Lecture impossible.
pub fn series_catalog(conn: &Connection) -> Result<ExchangeDocument, AppError> {
    let phrases = prospect_phrases(conn)?;
    let cadence = if phrases.is_empty() {
        None
    } else {
        Some(
            phrases
                .iter()
                .map(|phrase| CadenceMoment {
                    cle: phrase.key.clone(),
                    libelle: phrase.label.clone(),
                    ecart_ouvres: phrase.offset_days,
                    sujet: Some(phrase.subject.clone()),
                    corps: Some(phrase.body.clone()),
                })
                .collect(),
        )
    };
    let mut series = Vec::new();
    for genre in prospect_genres(conn)? {
        let moments = phrases_for_genre(conn, &genre.id)?
            .into_iter()
            .map(|phrase| SeriesMoment {
                cle: phrase.key,
                sujet: phrase.subject,
                corps: phrase.body,
            })
            .collect();
        series.push(SeriesFile {
            nom: genre.name,
            reprendre: false,
            moments,
        });
    }
    Ok(ExchangeDocument {
        version: 1,
        stop: false,
        dossiers: Vec::new(),
        series,
        cadence,
    })
}
