//! Journal des dépôts de démarches d'État. Un fait humain, pas une preuve d'administration.

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::Date;

use crate::app::{AppError, Command};
use crate::domain::{DutyFilingId, format_date, parse_date};
use crate::fiscal::FiscalDeadlineKind;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DutyFilingError {
    #[error("aucun dépôt ne correspond à cette démarche")]
    NotFound,
}

impl From<DutyFilingError> for AppError {
    fn from(e: DutyFilingError) -> Self {
        Self::Domain(e.to_string())
    }
}

/// Un dépôt enregistré : `filed_on` est le jour où l'humain l'a marqué, pas une date d'administration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DutyFiling {
    pub id: DutyFilingId,
    pub kind: FiscalDeadlineKind,
    pub period_key: String,
    #[serde(with = "crate::domain::serde_date::date")]
    pub due_on: Date,
    #[serde(with = "crate::domain::serde_date::date")]
    pub filed_on: Date,
}

/// Marque une occurrence comme déposée. Idempotent sur `(kind, period_key)`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarkDutyFiled {
    pub kind: FiscalDeadlineKind,
    pub period_key: String,
    #[serde(with = "crate::domain::serde_date::date")]
    pub due_on: Date,
    #[serde(with = "crate::domain::serde_date::date")]
    pub filed_on: Date,
}

impl Command for MarkDutyFiled {
    type Output = DutyFiling;
    const NAME: &'static str = "society.mark_duty_filed";

    fn requires_confirmation(&self) -> bool {
        true
    }

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        if let Some(existing) = filing_for(conn, self.kind, &self.period_key)? {
            return Ok(existing);
        }
        let filing = DutyFiling {
            id: DutyFilingId::new(),
            kind: self.kind,
            period_key: self.period_key.clone(),
            due_on: self.due_on,
            filed_on: self.filed_on,
        };
        conn.execute(
            "INSERT INTO duty_filings (id, kind, period_key, due_on, filed_on)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                filing.id.to_string(),
                filing.kind.as_str(),
                filing.period_key,
                format_date(filing.due_on),
                format_date(filing.filed_on),
            ],
        )?;
        Ok(filing)
    }
}

/// Marque toutes les occurrences antérieures à `before` (le début d'usage du coffre) comme
/// déposées à leur échéance — le rattrapage d'un coffre ouvert en cours d'année.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarkCatchUpFiled {
    #[serde(with = "crate::domain::serde_date::date")]
    pub before: Date,
}

impl Command for MarkCatchUpFiled {
    type Output = u32;
    const NAME: &'static str = "society.mark_catch_up_filed";

    fn requires_confirmation(&self) -> bool {
        true
    }

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let duties = super::society_duties(conn, self.before)?;
        let mut n = 0_u32;
        for d in duties {
            if d.filed_on.is_some() || d.due_on >= self.before {
                continue;
            }
            if filing_for(conn, d.kind, &d.period_key)?.is_some() {
                continue;
            }
            conn.execute(
                "INSERT INTO duty_filings (id, kind, period_key, due_on, filed_on)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    crate::domain::DutyFilingId::new().to_string(),
                    d.kind.as_str(),
                    d.period_key,
                    format_date(d.due_on),
                    format_date(d.due_on),
                ],
            )?;
            n += 1;
        }
        Ok(n)
    }
}

/// Retire un dépôt. Sans effet de bord hors de cette table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetractDutyFiled {
    pub kind: FiscalDeadlineKind,
    pub period_key: String,
}

impl Command for RetractDutyFiled {
    type Output = ();
    const NAME: &'static str = "society.retract_duty_filed";

    fn requires_confirmation(&self) -> bool {
        true
    }

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let n = conn.execute(
            "DELETE FROM duty_filings WHERE kind = ?1 AND period_key = ?2",
            params![self.kind.as_str(), self.period_key],
        )?;
        if n == 0 {
            return Err(DutyFilingError::NotFound.into());
        }
        Ok(())
    }
}

/// # Errors
pub fn filing_for(
    conn: &Connection,
    kind: FiscalDeadlineKind,
    period_key: &str,
) -> Result<Option<DutyFiling>, AppError> {
    conn.query_row(
        "SELECT id, kind, period_key, due_on, filed_on
         FROM duty_filings WHERE kind = ?1 AND period_key = ?2",
        params![kind.as_str(), period_key],
        row_to_filing,
    )
    .optional()
    .map_err(AppError::from)
}

/// # Errors
pub fn list_filings(conn: &Connection) -> Result<Vec<DutyFiling>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, kind, period_key, due_on, filed_on
         FROM duty_filings ORDER BY due_on ASC, kind ASC",
    )?;
    let rows = stmt.query_map([], row_to_filing)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
}

fn row_to_filing(row: &rusqlite::Row<'_>) -> rusqlite::Result<DutyFiling> {
    let id: String = row.get("id")?;
    let kind: String = row.get("kind")?;
    let due_on: String = row.get("due_on")?;
    let filed_on: String = row.get("filed_on")?;
    Ok(DutyFiling {
        id: id.parse().map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })?,
        kind: FiscalDeadlineKind::parse(&kind).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(
                1,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("démarche inconnue : {kind}"),
                )),
            )
        })?,
        period_key: row.get("period_key")?,
        due_on: parse_date(&due_on).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(e))
        })?,
        filed_on: parse_date(&filed_on).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, Box::new(e))
        })?,
    })
}
