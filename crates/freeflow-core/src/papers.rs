//! Coffre documentaire (lot 57) : index SQL des pièces, primitive d'archivage.
//!
//! Les octets vivent dans `<coffre>.receipts/` via [`crate::receipts`] — même crypto que les
//! justificatifs. Une [`Command`] n'écrit que l'index (`&Connection`) ; l'IO fichier est faite
//! par [`archive_paper`] *avant* [`ArchivePaper`], jamais dans `apply`.

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::format_description::well_known::Rfc3339;
use time::{Date, OffsetDateTime};

use crate::app::{AppError, Command, ExecutionContext, Executor, Outcome};
use crate::domain::{
    ClientId, ExpenseId, FiscalYearEnd, FiscalYearId, InvoiceId, PaperId, PaperKind, PaperOrigin,
    format_date, parse_date, retained_until,
};
use crate::expenses::hash_receipt;
use crate::receipts::{self, ReceiptError};
use crate::store::Store;

const PAPER_SELECT: &str = "SELECT id, kind, origin, original_name, filename, content_hash, mime,
        byte_size, period, issued_on, client_id, invoice_id, expense_id, fiscal_year_id,
        note, superseded_by, captured_at
     FROM papers";

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PapersError {
    #[error("pièce introuvable : {0}")]
    NotFound(PaperId),

    #[error(
        "le délai de conservation n'est pas échu{}",
        match .until {
            Some(d) => format!(" (à conserver jusqu'au {d})"),
            None => String::new(),
        }
    )]
    RetentionNotElapsed { until: Option<Date> },

    #[error(
        "un original né dans FreeFlow ne se remplace pas : le premier figé gagne — déposez une \
         copie de travail, ou laissez l'original"
    )]
    IssuedImmutable,

    #[error("{0}")]
    Receipt(String),
}

impl From<PapersError> for AppError {
    fn from(e: PapersError) -> Self {
        Self::Domain(e.to_string())
    }
}

impl From<ReceiptError> for PapersError {
    fn from(e: ReceiptError) -> Self {
        Self::Receipt(e.to_string())
    }
}

/// Une pièce indexée au coffre.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Paper {
    pub id: PaperId,
    pub kind: PaperKind,
    pub origin: PaperOrigin,
    pub original_name: String,
    pub filename: String,
    pub content_hash: String,
    pub mime: String,
    pub byte_size: i64,
    pub period: Option<i32>,
    #[serde(with = "crate::domain::serde_date::date::option")]
    pub issued_on: Option<Date>,
    pub client_id: Option<ClientId>,
    pub invoice_id: Option<InvoiceId>,
    pub expense_id: Option<ExpenseId>,
    pub fiscal_year_id: Option<FiscalYearId>,
    pub note: Option<String>,
    pub superseded_by: Option<PaperId>,
    #[serde(with = "crate::domain::serde_date::datetime")]
    pub captured_at: OffsetDateTime,
}

/// Filtre de [`list_papers`]. Par défaut : pièces actives (non remplacées), toutes natures.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PaperFilter {
    pub period: Option<i32>,
    pub kind: Option<PaperKind>,
    pub include_superseded: bool,
}

impl PaperFilter {
    pub const ACTIVE: Self = Self {
        period: None,
        kind: None,
        include_superseded: false,
    };
}

/// Spécification d'une pièce à archiver, avant que l'IO n'ait produit hash / nom / taille.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewPaper {
    pub kind: PaperKind,
    pub origin: PaperOrigin,
    pub original_name: String,
    pub mime: String,
    pub period: Option<i32>,
    #[serde(with = "crate::domain::serde_date::date::option")]
    pub issued_on: Option<Date>,
    pub client_id: Option<ClientId>,
    pub invoice_id: Option<InvoiceId>,
    pub expense_id: Option<ExpenseId>,
    pub fiscal_year_id: Option<FiscalYearId>,
    pub note: Option<String>,
    pub idempotency_key: Option<String>,
}

/// Indexe une pièce dont les octets sont déjà (ou seront) dans `<coffre>.receipts/`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchivePaper {
    pub kind: PaperKind,
    pub origin: PaperOrigin,
    pub original_name: String,
    pub filename: String,
    pub content_hash: String,
    pub mime: String,
    pub byte_size: i64,
    pub period: Option<i32>,
    #[serde(with = "crate::domain::serde_date::date::option")]
    pub issued_on: Option<Date>,
    pub client_id: Option<ClientId>,
    pub invoice_id: Option<InvoiceId>,
    pub expense_id: Option<ExpenseId>,
    pub fiscal_year_id: Option<FiscalYearId>,
    pub note: Option<String>,
    pub idempotency_key: Option<String>,
}

impl Command for ArchivePaper {
    type Output = Paper;
    const NAME: &'static str = "papers.archive";

    fn idempotency_key(&self) -> Option<&str> {
        self.idempotency_key.as_deref()
    }

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        if self.origin == PaperOrigin::Issued
            && let Some(invoice_id) = self.invoice_id
            && let Some(existing) = existing_issued(conn, invoice_id, self.kind)?
        {
            return Ok(existing);
        }
        let paper = Paper {
            id: PaperId::new(),
            kind: self.kind,
            origin: self.origin,
            original_name: self.original_name.clone(),
            filename: self.filename.clone(),
            content_hash: self.content_hash.clone(),
            mime: self.mime.clone(),
            byte_size: self.byte_size,
            period: self.period,
            issued_on: self.issued_on,
            client_id: self.client_id,
            invoice_id: self.invoice_id,
            expense_id: self.expense_id,
            fiscal_year_id: self.fiscal_year_id,
            note: self.note.clone(),
            superseded_by: None,
            captured_at: OffsetDateTime::now_utc(),
        };
        insert_paper(conn, &paper, self.idempotency_key.as_deref())?;
        Ok(paper)
    }
}

/// Remplace une pièce déposée : l'ancienne reçoit `superseded_by`, la nouvelle est insérée.
/// Un original `Issued` est refusé ([`PapersError::IssuedImmutable`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupersedePaper {
    pub id: PaperId,
    pub replacement: ArchivePaper,
}

impl Command for SupersedePaper {
    type Output = Paper;
    const NAME: &'static str = "papers.supersede";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let current = paper_by_id(conn, self.id)?.ok_or(PapersError::NotFound(self.id))?;
        if current.origin == PaperOrigin::Issued {
            return Err(PapersError::IssuedImmutable.into());
        }
        let replacement = self.replacement.apply(conn)?;
        conn.execute(
            "UPDATE papers SET superseded_by = ?1 WHERE id = ?2",
            params![replacement.id.to_string(), self.id.to_string()],
        )?;
        Ok(replacement)
    }
}

/// Purge l'index d'une pièce dont le délai de conservation est échu. Ne supprime pas le blob
/// (orphelin possible — pas de GC en v1). Un agent dépose une action en attente.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PurgePaper {
    pub id: PaperId,
    #[serde(with = "crate::domain::serde_date::date")]
    pub today: Date,
}

impl Command for PurgePaper {
    type Output = ();
    const NAME: &'static str = "papers.purge";

    fn requires_confirmation(&self) -> bool {
        true
    }

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let paper = paper_by_id(conn, self.id)?.ok_or(PapersError::NotFound(self.id))?;
        let year_end = year_end_of(conn, &paper)?;
        let until = retained_until(paper.kind, year_end);
        if until.is_none_or(|d| d >= self.today) {
            return Err(PapersError::RetentionNotElapsed { until }.into());
        }
        conn.execute("DELETE FROM papers WHERE id = ?1", [self.id.to_string()])?;
        Ok(())
    }
}

fn insert_paper(
    conn: &Connection,
    paper: &Paper,
    idempotency_key: Option<&str>,
) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO papers
            (id, kind, origin, original_name, filename, content_hash, mime, byte_size,
             period, issued_on, client_id, invoice_id, expense_id, fiscal_year_id, note,
             superseded_by, captured_at, idempotency_key)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
        params![
            paper.id.to_string(),
            paper.kind.as_str(),
            paper.origin.as_str(),
            paper.original_name,
            paper.filename,
            paper.content_hash,
            paper.mime,
            paper.byte_size,
            paper.period,
            paper.issued_on.map(format_date),
            paper.client_id.map(|id| id.to_string()),
            paper.invoice_id.map(|id| id.to_string()),
            paper.expense_id.map(|id| id.to_string()),
            paper.fiscal_year_id.map(|id| id.to_string()),
            paper.note,
            paper.superseded_by.map(|id| id.to_string()),
            paper.captured_at.format(&Rfc3339)?,
            idempotency_key,
        ],
    )?;
    Ok(())
}

fn existing_issued(
    conn: &Connection,
    invoice_id: InvoiceId,
    kind: PaperKind,
) -> Result<Option<Paper>, AppError> {
    conn.query_row(
        &format!(
            "{PAPER_SELECT}
             WHERE invoice_id = ?1 AND kind = ?2 AND superseded_by IS NULL AND origin = 'issued'"
        ),
        params![invoice_id.to_string(), kind.as_str()],
        row_to_paper,
    )
    .optional()
    .map_err(AppError::from)
}

fn year_end_of(conn: &Connection, paper: &Paper) -> Result<Option<Date>, AppError> {
    if let Some(id) = paper.fiscal_year_id
        && let Some(year) = crate::fiscal_year::fiscal_year_by_id(conn, id)?
    {
        return Ok(Some(year.ends_on));
    }
    let Some(period) = paper.period else {
        return Ok(None);
    };
    if let Some(year) = crate::fiscal_year::fiscal_year_ending_in(conn, period)? {
        return Ok(Some(year.ends_on));
    }
    let fye = crate::company::company_profile(conn)?
        .and_then(|p| p.fiscal_year_end)
        .unwrap_or(FiscalYearEnd::CALENDAR);
    Ok(Some(fye.end_in_year(period)))
}

fn conv_err(e: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
}

fn parse_id<T: std::str::FromStr>(value: Option<String>) -> rusqlite::Result<Option<T>>
where
    T::Err: std::error::Error + Send + Sync + 'static,
{
    value.map(|s| s.parse::<T>()).transpose().map_err(conv_err)
}

fn row_to_paper(row: &Row<'_>) -> rusqlite::Result<Paper> {
    let id: String = row.get("id")?;
    let kind: String = row.get("kind")?;
    let origin: String = row.get("origin")?;
    let issued_on: Option<String> = row.get("issued_on")?;
    let captured_at: String = row.get("captured_at")?;
    Ok(Paper {
        id: id.parse().map_err(conv_err)?,
        kind: kind.parse().map_err(conv_err)?,
        origin: origin.parse().map_err(conv_err)?,
        original_name: row.get("original_name")?,
        filename: row.get("filename")?,
        content_hash: row.get("content_hash")?,
        mime: row.get("mime")?,
        byte_size: row.get("byte_size")?,
        period: row.get("period")?,
        issued_on: issued_on
            .as_deref()
            .map(parse_date)
            .transpose()
            .map_err(conv_err)?,
        client_id: parse_id(row.get("client_id")?)?,
        invoice_id: parse_id(row.get("invoice_id")?)?,
        expense_id: parse_id(row.get("expense_id")?)?,
        fiscal_year_id: parse_id(row.get("fiscal_year_id")?)?,
        note: row.get("note")?,
        superseded_by: parse_id(row.get("superseded_by")?)?,
        captured_at: OffsetDateTime::parse(&captured_at, &Rfc3339).map_err(conv_err)?,
    })
}

/// # Errors
///
/// Erreur de lecture SQLite.
pub fn paper_by_id(conn: &Connection, id: PaperId) -> Result<Option<Paper>, AppError> {
    conn.query_row(
        &format!("{PAPER_SELECT} WHERE id = ?1"),
        [id.to_string()],
        row_to_paper,
    )
    .optional()
    .map_err(AppError::from)
}

/// # Errors
///
/// Erreur de lecture SQLite.
pub fn list_papers(conn: &Connection, filter: PaperFilter) -> Result<Vec<Paper>, AppError> {
    let mut stmt = conn.prepare(&format!(
        "{PAPER_SELECT}
         WHERE (?1 IS NULL OR period = ?1)
           AND (?2 IS NULL OR kind = ?2)
           AND (?3 = 1 OR superseded_by IS NULL)
         ORDER BY captured_at DESC, original_name ASC"
    ))?;
    let rows = stmt.query_map(
        params![
            filter.period,
            filter.kind.map(PaperKind::as_str),
            i64::from(filter.include_superseded),
        ],
        row_to_paper,
    )?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
}

/// Guess MIME from the original file name — adapter convenience, not a legal rule.
#[must_use]
pub fn mime_from_name(original_name: &str) -> String {
    let ext = std::path::Path::new(original_name)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("pdf") => "application/pdf",
        Some("txt") => "text/plain",
        Some("json") => "application/json",
        Some("xml") => "application/xml",
        Some("eml") => "message/rfc822",
        Some("csv") => "text/csv",
        Some("ofx") => "application/x-ofx",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("xlsx") => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        _ => "application/octet-stream",
    }
    .to_string()
}

/// Archive les octets via [`receipts::archive`] puis indexe. En `dry_run`, n'écrit rien.
/// Si `idempotency_key` est déjà vue, l'[`Executor`] renvoie le `Paper` mémorisé.
///
/// # Errors
///
/// IO d'archivage ou commande d'index.
pub fn archive_paper(
    store: &mut Store,
    spec: NewPaper,
    bytes: &[u8],
    ctx: &ExecutionContext,
) -> Result<Outcome<Paper>, AppError> {
    let hash = hash_receipt(bytes);
    let byte_size = i64::try_from(bytes.len()).unwrap_or(i64::MAX);
    let filename = if ctx.dry_run {
        format!("{hash}-dry-run")
    } else {
        receipts::archive(store, &spec.original_name, bytes)
            .map_err(PapersError::from)?
            .filename
    };
    let cmd = ArchivePaper {
        kind: spec.kind,
        origin: spec.origin,
        original_name: spec.original_name,
        filename,
        content_hash: hash,
        mime: spec.mime,
        byte_size,
        period: spec.period,
        issued_on: spec.issued_on,
        client_id: spec.client_id,
        invoice_id: spec.invoice_id,
        expense_id: spec.expense_id,
        fiscal_year_id: spec.fiscal_year_id,
        note: spec.note,
        idempotency_key: spec.idempotency_key,
    };
    Executor::new(store).execute(&cmd, ctx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Actor;
    use crate::receipts::ReceiptError;
    use crate::store::Passphrase;
    use std::fs;
    use time::Month;

    fn test_store(label: &str) -> Store {
        let dir = std::env::temp_dir().join(format!(
            "freeflow-papers-test-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ));
        Store::create(&dir.join("vault.db"), &Passphrase::from("s3cret")).unwrap()
    }

    fn human() -> ExecutionContext {
        ExecutionContext::new(Actor::Human, false)
    }

    fn d(year: i32, month: u8, day: u8) -> Date {
        Date::from_calendar_date(year, Month::try_from(month).unwrap(), day).unwrap()
    }

    fn statutes_spec(key: Option<&str>) -> NewPaper {
        NewPaper {
            kind: PaperKind::Statutes,
            origin: PaperOrigin::Uploaded,
            original_name: "statuts.pdf".into(),
            mime: "application/pdf".into(),
            period: None,
            issued_on: None,
            client_id: None,
            invoice_id: None,
            expense_id: None,
            fiscal_year_id: None,
            note: None,
            idempotency_key: key.map(str::to_string),
        }
    }

    fn applied(outcome: Outcome<Paper>) -> Paper {
        match outcome {
            Outcome::Applied(p) | Outcome::AlreadyApplied(p) => p,
            other => panic!("attendu Applied, obtenu {other:?}"),
        }
    }

    #[test]
    fn archive_paper_encrypts_in_the_receipts_sidecar_and_lists_the_row() {
        let mut store = test_store("roundtrip");
        let paper = applied(
            archive_paper(
                &mut store,
                statutes_spec(None),
                b"%PDF-1.4 secret",
                &human(),
            )
            .unwrap(),
        );
        assert!(
            paper.filename.ends_with("-statuts.pdf"),
            "{}",
            paper.filename
        );
        assert_eq!(paper.kind, PaperKind::Statutes);
        assert_eq!(paper.origin, PaperOrigin::Uploaded);
        assert_eq!(
            receipts::read(&store, &paper.filename).unwrap(),
            b"%PDF-1.4 secret"
        );
        let listed = list_papers(store.connection(), PaperFilter::ACTIVE).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, paper.id);
        assert_eq!(listed[0].content_hash, hash_receipt(b"%PDF-1.4 secret"));
    }

    #[test]
    fn the_same_idempotency_key_does_not_duplicate() {
        let mut store = test_store("idem");
        let key = Some("papers:upload:abc:statutes");
        let first =
            applied(archive_paper(&mut store, statutes_spec(key), b"%PDF A", &human()).unwrap());
        let second = archive_paper(&mut store, statutes_spec(key), b"%PDF A", &human()).unwrap();
        match second {
            Outcome::AlreadyApplied(p) => assert_eq!(p.id, first.id),
            other => panic!("attendu AlreadyApplied, obtenu {other:?}"),
        }
        assert_eq!(
            list_papers(store.connection(), PaperFilter::ACTIVE)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn purge_before_ten_years_is_refused() {
        let mut store = test_store("purge-early");
        let spec = NewPaper {
            kind: PaperKind::TaxNotice,
            origin: PaperOrigin::Uploaded,
            original_name: "2572.pdf".into(),
            mime: "application/pdf".into(),
            period: Some(2026),
            issued_on: None,
            client_id: None,
            invoice_id: None,
            expense_id: None,
            fiscal_year_id: None,
            note: None,
            idempotency_key: None,
        };
        let paper = applied(archive_paper(&mut store, spec, b"%PDF 2572", &human()).unwrap());
        let err = Executor::new(&mut store)
            .execute(
                &PurgePaper {
                    id: paper.id,
                    today: d(2026, 9, 9),
                },
                &human(),
            )
            .unwrap_err();
        assert!(err.to_string().contains("délai de conservation"), "{err}");
        assert_eq!(
            list_papers(store.connection(), PaperFilter::ACTIVE)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn purge_of_statutes_is_refused_because_the_clock_never_elapses() {
        let mut store = test_store("purge-statutes");
        let paper = applied(
            archive_paper(&mut store, statutes_spec(None), b"%PDF statuts", &human()).unwrap(),
        );
        let err = Executor::new(&mut store)
            .execute(
                &PurgePaper {
                    id: paper.id,
                    today: d(2099, 1, 1),
                },
                &human(),
            )
            .unwrap_err();
        assert!(err.to_string().contains("délai de conservation"), "{err}");
    }

    #[test]
    fn purge_after_ten_years_removes_the_index_and_leaves_the_blob() {
        let mut store = test_store("purge-late");
        let spec = NewPaper {
            kind: PaperKind::Other,
            origin: PaperOrigin::Uploaded,
            original_name: "note.pdf".into(),
            mime: "application/pdf".into(),
            period: Some(2010),
            issued_on: None,
            client_id: None,
            invoice_id: None,
            expense_id: None,
            fiscal_year_id: None,
            note: None,
            idempotency_key: None,
        };
        let paper = applied(archive_paper(&mut store, spec, b"%PDF note", &human()).unwrap());
        let filename = paper.filename.clone();
        Executor::new(&mut store)
            .execute(
                &PurgePaper {
                    id: paper.id,
                    today: d(2026, 9, 9),
                },
                &human(),
            )
            .unwrap();
        assert!(paper_by_id(store.connection(), paper.id).unwrap().is_none());
        assert_eq!(receipts::read(&store, &filename).unwrap(), b"%PDF note");
    }

    #[test]
    fn a_blob_from_another_vault_is_corrupt() {
        let mut store = test_store("home");
        let paper = applied(
            archive_paper(&mut store, statutes_spec(None), b"%PDF home", &human()).unwrap(),
        );
        let src = receipts::path_of(&store, &paper.filename);
        let other = test_store("other");
        fs::create_dir_all(other.receipts_dir()).unwrap();
        fs::copy(&src, other.receipts_dir().join(&paper.filename)).unwrap();
        assert!(matches!(
            receipts::read(&other, &paper.filename),
            Err(ReceiptError::Corrupt(_))
        ));
    }

    #[test]
    fn an_agent_cannot_purge_without_confirmation() {
        let mut store = test_store("agent-purge");
        let paper =
            applied(archive_paper(&mut store, statutes_spec(None), b"%PDF", &human()).unwrap());
        let outcome = Executor::new(&mut store)
            .execute(
                &PurgePaper {
                    id: paper.id,
                    today: d(2099, 1, 1),
                },
                &ExecutionContext::new(
                    Actor::Agent {
                        session: "mcp-test".into(),
                    },
                    false,
                ),
            )
            .unwrap();
        assert!(matches!(outcome, Outcome::PendingConfirmation(_)));
        assert_eq!(
            list_papers(store.connection(), PaperFilter::ACTIVE)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn dry_run_does_not_write_the_blob() {
        let mut store = test_store("dry");
        let outcome = archive_paper(
            &mut store,
            statutes_spec(None),
            b"%PDF dry",
            &ExecutionContext::new(Actor::Human, true),
        )
        .unwrap();
        assert_eq!(outcome, Outcome::DryRun);
        assert!(
            !store.receipts_dir().exists()
                || fs::read_dir(store.receipts_dir()).map_or(true, |mut d| d.next().is_none())
        );
        assert!(
            list_papers(store.connection(), PaperFilter::ACTIVE)
                .unwrap()
                .is_empty()
        );
    }
}
