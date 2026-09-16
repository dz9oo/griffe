//! Requêtes de lecture des devis — comblées au lot 21 : depuis le lot 6, un devis créé restait
//! illisible autrement que par les tests du module (aucune query publique), alors même que son
//! contenu est immuable par trigger depuis sa création. Rien à filtrer ici : un devis n'est ni
//! archivable ni supprimable (voir `0005_quotes_up.sql`), son cycle de vie est entièrement porté
//! par `status` — la liste inclusive est donc la seule liste.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::app::AppError;
use crate::domain::{Quote, QuoteId, QuoteLine};

use super::row;

/// Un devis (une version précise) par id, lignes comprises.
///
/// # Errors
pub fn quote_by_id(conn: &Connection, id: QuoteId) -> Result<Option<Quote>, AppError> {
    row::quote_by_id(conn, id)
}

/// Tous les devis, toutes versions confondues, les plus récents d'abord — avec leurs lignes,
/// chargées en **deux requêtes au total** (jamais une par devis), même discipline que
/// `missions::row::missions_matching`.
///
/// # Errors
pub fn list_quotes(conn: &Connection) -> Result<Vec<Quote>, AppError> {
    let mut stmt = conn.prepare("SELECT * FROM quotes ORDER BY created_at DESC")?;
    let mut quotes = stmt
        .query_map([], row::row_to_quote_without_lines)?
        .collect::<Result<Vec<_>, _>>()?;

    let mut lines_stmt = conn.prepare(
        "SELECT quote_id, description, kind, vat_rate FROM quote_lines
          ORDER BY quote_id, position ASC",
    )?;
    let all_lines: Vec<(String, QuoteLine)> = lines_stmt
        .query_map([], |line_row| {
            let quote_id: String = line_row.get("quote_id")?;
            let kind_json: String = line_row.get("kind")?;
            let vat_rate: String = line_row.get("vat_rate")?;
            Ok((
                quote_id,
                QuoteLine {
                    description: line_row.get("description")?,
                    kind: serde_json::from_str(&kind_json).map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            0,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })?,
                    vat_rate: vat_rate.parse().map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            0,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })?,
                },
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    for quote in &mut quotes {
        let id_str = quote.id.to_string();
        quote.lines = all_lines
            .iter()
            .filter(|(qid, _)| *qid == id_str)
            .map(|(_, line)| line.clone())
            .collect();
    }
    Ok(quotes)
}

/// Ce qui descend d'un devis — le même exercice que `client_references` (lot 15), mais un devis
/// étant immuable et indélébile par trigger, ce dénombrement ne garde aucune suppression : il
/// n'existe que pour qu'une façade puisse afficher la lignée (« ce devis a produit telle
/// mission ») et le nombre de versions partageant la même racine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuoteReferences {
    /// Missions issues de l'acceptation de cette version précise.
    pub missions: i64,
    /// Nombre total de versions partageant la même racine (`root_id`), celle-ci comprise.
    pub versions: i64,
}

/// # Errors
pub fn quote_references(conn: &Connection, id: QuoteId) -> Result<QuoteReferences, AppError> {
    let missions = conn.query_row(
        "SELECT count(*) FROM missions WHERE quote_id = ?1",
        [id.to_string()],
        |row| row.get(0),
    )?;
    let versions = conn.query_row(
        "SELECT count(*) FROM quotes
          WHERE root_id = (SELECT root_id FROM quotes WHERE id = ?1)",
        [id.to_string()],
        |row| row.get(0),
    )?;
    Ok(QuoteReferences { missions, versions })
}
