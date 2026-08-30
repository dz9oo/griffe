//! Correspondance ligne SQL <-> types du domaine pour les devis.

use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::app::AppError;
use crate::domain::{self, Discount, Quote, QuoteId, QuoteLine, QuoteStatus};

fn conv_err(e: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
}

pub(super) fn insert_quote(conn: &Connection, quote: &Quote) -> Result<(), AppError> {
    let discount_json = quote
        .discount
        .map(|d| serde_json::to_string(&d))
        .transpose()?;
    conn.execute(
        "INSERT INTO quotes (id, root_id, client_id, opportunity_id, version, status, valid_until, discount_json, terms, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            quote.id.to_string(),
            quote.root_id.to_string(),
            quote.client_id.to_string(),
            quote.opportunity_id.map(|o| o.to_string()),
            quote.version,
            quote.status.as_str(),
            domain::format_date(quote.valid_until),
            discount_json,
            quote.terms,
            quote.created_at.format(&time::format_description::well_known::Rfc3339)?,
        ],
    )?;
    for (position, line) in quote.lines.iter().enumerate() {
        let kind_json = serde_json::to_string(&line.kind)?;
        conn.execute(
            "INSERT INTO quote_lines (quote_id, position, description, kind, vat_rate) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![quote.id.to_string(), i64::try_from(position).unwrap_or(i64::MAX), line.description, kind_json, line.vat_rate.as_str()],
        )?;
    }
    Ok(())
}

fn row_to_quote_without_lines(row: &Row) -> rusqlite::Result<Quote> {
    let id: String = row.get("id")?;
    let root_id: String = row.get("root_id")?;
    let client_id: String = row.get("client_id")?;
    let opportunity_id: Option<String> = row.get("opportunity_id")?;
    let status: String = row.get("status")?;
    let valid_until: String = row.get("valid_until")?;
    let discount_json: Option<String> = row.get("discount_json")?;
    let created_at: String = row.get("created_at")?;

    Ok(Quote {
        id: id.parse().map_err(conv_err)?,
        root_id: root_id.parse().map_err(conv_err)?,
        client_id: client_id.parse().map_err(conv_err)?,
        opportunity_id: opportunity_id
            .map(|s| s.parse())
            .transpose()
            .map_err(conv_err)?,
        version: row.get("version")?,
        status: status.parse::<QuoteStatus>().map_err(conv_err)?,
        lines: Vec::new(),
        discount: discount_json
            .map(|s| serde_json::from_str::<Discount>(&s))
            .transpose()
            .map_err(conv_err)?,
        terms: row.get("terms")?,
        valid_until: domain::parse_date(&valid_until).map_err(conv_err)?,
        created_at: time::OffsetDateTime::parse(
            &created_at,
            &time::format_description::well_known::Rfc3339,
        )
        .map_err(conv_err)?,
    })
}

fn lines_for_quote(conn: &Connection, id: QuoteId) -> Result<Vec<QuoteLine>, AppError> {
    let mut stmt = conn.prepare("SELECT description, kind, vat_rate FROM quote_lines WHERE quote_id = ?1 ORDER BY position ASC")?;
    let rows = stmt.query_map([id.to_string()], |row| {
        let kind_json: String = row.get("kind")?;
        let vat_rate: String = row.get("vat_rate")?;
        Ok(QuoteLine {
            description: row.get("description")?,
            kind: serde_json::from_str(&kind_json).map_err(conv_err)?,
            vat_rate: vat_rate.parse().map_err(conv_err)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
}

pub(super) fn quote_by_id(conn: &Connection, id: QuoteId) -> Result<Option<Quote>, AppError> {
    let Some(mut quote) = conn
        .query_row(
            "SELECT * FROM quotes WHERE id = ?1",
            [id.to_string()],
            row_to_quote_without_lines,
        )
        .optional()?
    else {
        return Ok(None);
    };
    quote.lines = lines_for_quote(conn, id)?;
    Ok(Some(quote))
}

/// Le numéro de la dernière version connue d'un devis (0 si `root_id` n'existe pas encore).
pub(super) fn latest_version(conn: &Connection, root_id: QuoteId) -> Result<u32, AppError> {
    let version: Option<u32> = conn.query_row(
        "SELECT MAX(version) FROM quotes WHERE root_id = ?1",
        [root_id.to_string()],
        |row| row.get(0),
    )?;
    Ok(version.unwrap_or(0))
}

pub(super) fn set_status(
    conn: &Connection,
    id: QuoteId,
    status: QuoteStatus,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE quotes SET status = ?1 WHERE id = ?2",
        params![status.as_str(), id.to_string()],
    )?;
    Ok(())
}

#[cfg(test)]
pub(super) fn seed_client(conn: &Connection) -> crate::domain::ClientId {
    let id = crate::domain::ClientId::new();
    conn.execute("INSERT INTO clients (id, name, created_at) VALUES (?1, 'Argon Digital', '2026-01-01T00:00:00Z')", [id.to_string()]).unwrap();
    id
}
