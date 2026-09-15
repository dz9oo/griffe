//! Coffre documentaire (lots 57–60) : index SQL des pièces, primitive d'archivage, checklist,
//! manifeste du pack contrôle.
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
use crate::billing::list_invoices;
use crate::closing::StepStatus;
use crate::company::company_profile;
use crate::domain::{
    ClientId, ExpenseId, FiscalYear, FiscalYearEnd, FiscalYearId, InvoiceId, PaperId, PaperKind,
    PaperOrigin, format_date, parse_date, retained_until,
};
use crate::expenses::{expenses_between, hash_receipt};
use crate::fiscal_year::fiscal_year_ending_in;
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
            && let Some(existing) = issued_paper(conn, invoice_id, self.kind)?
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

/// Original `Issued` déjà figé pour cette facture et cette nature — le premier gagne.
///
/// # Errors
///
/// Erreur de lecture SQLite.
pub fn issued_paper(
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
/// Première capture d'un `(kind, invoice_id)` `Issued` : si une ligne existe déjà, la renvoyer
/// sans réécrire le blob (no-op — le premier original gagne).
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
    if spec.origin == PaperOrigin::Issued
        && let Some(invoice_id) = spec.invoice_id
        && let Some(existing) = issued_paper(store.connection(), invoice_id, spec.kind)?
    {
        return Ok(Outcome::Applied(existing));
    }
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

/// Une ligne de [`papers_checklist`] : une nature (ou une facture / dépense) et où elle en est.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PapersCheckItem {
    pub kind: PaperKind,
    pub status: StepStatus,
    pub required: bool,
    pub paper_id: Option<PaperId>,
}

/// Ce qui manque au coffre pour un exercice, vu depuis `today`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PapersChecklist {
    pub period: i32,
    #[serde(with = "crate::domain::serde_date::date")]
    pub today: Date,
    pub items: Vec<PapersCheckItem>,
}

fn exercise_of(conn: &Connection, period: i32) -> Result<FiscalYear, AppError> {
    if let Some(record) = fiscal_year_ending_in(conn, period)? {
        return Ok(record.period());
    }
    let fye = company_profile(conn)?
        .and_then(|p| p.fiscal_year_end)
        .unwrap_or(FiscalYearEnd::CALENDAR);
    Ok(fye.containing(fye.end_in_year(period)))
}

fn first_of(
    conn: &Connection,
    kind: PaperKind,
    period: Option<i32>,
) -> Result<Option<Paper>, AppError> {
    Ok(list_papers(
        conn,
        PaperFilter {
            period,
            kind: Some(kind),
            include_superseded: false,
        },
    )?
    .into_iter()
    .next())
}

fn item(
    kind: PaperKind,
    paper_id: Option<PaperId>,
    present: bool,
    due: bool,
    required: bool,
) -> PapersCheckItem {
    let status = if present {
        StepStatus::Done
    } else if due {
        StepStatus::Todo
    } else {
        StepStatus::Later
    };
    PapersCheckItem {
        kind,
        status,
        required,
        paper_id,
    }
}

fn warning_item(kind: PaperKind, paper: Option<&Paper>) -> PapersCheckItem {
    PapersCheckItem {
        kind,
        status: if paper.is_some() {
            StepStatus::Done
        } else {
            StepStatus::Warning
        },
        required: false,
        paper_id: paper.map(|p| p.id),
    }
}

fn filing_belongs(period_key: &str, due_on: Date, period: i32, exercise: FiscalYear) -> bool {
    period_key == period.to_string()
        || period_key.starts_with(&format!("{period}-"))
        || exercise.contains(due_on)
}

/// Natures d'originaux **générés** que l'étape `Documents` du parcours attend au coffre.
const GENERATED_KINDS: [PaperKind; 6] = [
    PaperKind::Fec,
    PaperKind::Minutes,
    PaperKind::Appropriation,
    PaperKind::Synthesis,
    PaperKind::BalanceSheet,
    PaperKind::Liasse,
];

/// Checklist de conservation d'un exercice : originaux que l'application doit avoir figés
/// (`required`), et pièces extérieures signalées en avertissement (statuts, Kbis, relevé,
/// accusé de dépôt). Un justificatif de dépense compte s'il a une ligne `papers` **ou** un
/// `receipt_filename` (pièces antérieures au lot 57).
///
/// # Errors
///
/// Erreur de lecture SQLite.
#[allow(clippy::too_many_lines)]
pub fn papers_checklist(
    conn: &Connection,
    period: i32,
    today: Date,
) -> Result<PapersChecklist, AppError> {
    let record = fiscal_year_ending_in(conn, period)?;
    let exercise = exercise_of(conn, period)?;
    let approved = record
        .as_ref()
        .is_some_and(crate::fiscal_year::FiscalYearRecord::is_approved);
    let closed = record.is_some();
    let mut items = Vec::new();

    let invoices = list_invoices(conn)?
        .into_iter()
        .filter(|inv| exercise.contains(inv.issued_on))
        .collect::<Vec<_>>();
    for invoice in &invoices {
        let kind = if invoice.credited_invoice_id.is_some() {
            PaperKind::CreditNote
        } else {
            PaperKind::IssuedInvoice
        };
        let paper = issued_paper(conn, invoice.id, kind)?;
        items.push(item(
            kind,
            paper.as_ref().map(|p| p.id),
            paper.is_some(),
            closed || approved,
            approved,
        ));
    }

    for kind in GENERATED_KINDS {
        let paper = first_of(conn, kind, Some(period))?;
        let capturable = match kind {
            PaperKind::Minutes | PaperKind::Appropriation => approved,
            _ => closed,
        };
        items.push(item(
            kind,
            paper.as_ref().map(|p| p.id),
            paper.is_some(),
            capturable,
            approved && capturable,
        ));
    }

    let expenses = expenses_between(conn, exercise.start(), exercise.end())?;
    let receipt_papers = list_papers(
        conn,
        PaperFilter {
            period: None,
            kind: Some(PaperKind::ExpenseReceipt),
            include_superseded: false,
        },
    )?;
    for expense in &expenses {
        let paper = receipt_papers
            .iter()
            .find(|p| p.expense_id == Some(expense.id));
        let present = paper.is_some() || expense.receipt_filename.is_some();
        items.push(item(
            PaperKind::ExpenseReceipt,
            paper.map(|p| p.id),
            present,
            closed || approved,
            approved,
        ));
    }

    for kind in [PaperKind::Statutes, PaperKind::Kbis] {
        let paper = first_of(conn, kind, None)?;
        items.push(warning_item(kind, paper.as_ref()));
    }

    let bank_txs = crate::billing::list_bank_transactions(conn)?
        .into_iter()
        .filter(|t| exercise.contains(t.occurred_on))
        .count();
    if bank_txs > 0 {
        let statements = list_papers(
            conn,
            PaperFilter {
                period: None,
                kind: Some(PaperKind::BankStatement),
                include_superseded: false,
            },
        )?;
        let paper = statements
            .iter()
            .find(|p| p.period == Some(period) || p.period.is_none());
        items.push(warning_item(PaperKind::BankStatement, paper));
    }

    let filings = crate::society::list_filings(conn)?;
    let has_filed = filings
        .iter()
        .any(|f| filing_belongs(&f.period_key, f.due_on, period, exercise));
    if has_filed {
        let acks = list_papers(
            conn,
            PaperFilter {
                period: None,
                kind: Some(PaperKind::FilingAck),
                include_superseded: false,
            },
        )?;
        let paper = acks
            .iter()
            .find(|p| p.period == Some(period) || p.period.is_none());
        items.push(warning_item(PaperKind::FilingAck, paper));
    }

    Ok(PapersChecklist {
        period,
        today,
        items,
    })
}

/// Les originaux générés requis manquent-ils ? L'étape `Documents` du parcours s'en sert —
/// pas les dépôts extérieurs (Kbis, statuts, accusés), ni les justificatifs déjà suivis par
/// l'étape Dépenses.
#[must_use]
pub fn missing_generated_originals(checklist: &PapersChecklist) -> Vec<PaperKind> {
    checklist
        .items
        .iter()
        .filter(|i| {
            (matches!(i.kind, PaperKind::IssuedInvoice | PaperKind::CreditNote)
                || GENERATED_KINDS.contains(&i.kind))
                && i.status != StepStatus::Done
                && i.status != StepStatus::Later
        })
        .map(|i| i.kind)
        .collect()
}

/// Un Kbis ou des statuts manquent (avertissement, jamais bloquant pour `Documents`).
#[must_use]
pub fn missing_identity_papers(checklist: &PapersChecklist) -> Vec<PaperKind> {
    checklist
        .items
        .iter()
        .filter(|i| {
            matches!(i.kind, PaperKind::Statutes | PaperKind::Kbis)
                && i.status == StepStatus::Warning
        })
        .map(|i| i.kind)
        .collect()
}

/// Une ligne du pack contrôle : chemin relatif en clair dans le dossier exporté.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlPackItem {
    pub relative_path: String,
    pub paper_id: PaperId,
    pub content_hash: String,
    pub kind: PaperKind,
}

fn safe_file_name(original_name: &str) -> String {
    std::path::Path::new(original_name)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty() && n != "." && n != "..")
        .unwrap_or_else(|| "document".to_string())
}

fn extension_of(name: &str) -> String {
    std::path::Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .filter(|e| !e.is_empty())
        .unwrap_or_else(|| "bin".to_string())
}

fn hash12(hash: &str) -> String {
    hash.chars().take(12).collect()
}

fn compact_date(date: Date) -> String {
    format_date(date).replace('-', "")
}

fn uniquify(path: String, hash: &str, used: &mut std::collections::HashSet<String>) -> String {
    if used.insert(path.clone()) {
        return path;
    }
    let insert = format!("-{}", hash12(hash));
    let uniqued = match path.rsplit_once('.') {
        Some((stem, ext)) => format!("{stem}{insert}.{ext}"),
        None => format!("{path}{insert}"),
    };
    if used.insert(uniqued.clone()) {
        return uniqued;
    }
    let fallback = format!("{uniqued}-{}", used.len());
    used.insert(fallback.clone());
    fallback
}

fn fec_pack_name(conn: &Connection, period: i32) -> Result<String, AppError> {
    let Some(profile) = company_profile(conn)? else {
        return Ok(format!("fec-{period}.txt"));
    };
    let end = exercise_of(conn, period)?.end();
    Ok(format!("{}FEC{}.txt", profile.siren, compact_date(end)))
}

fn invoice_pack_name(conn: &Connection, paper: &Paper) -> Result<String, AppError> {
    if let Some(id) = paper.invoice_id
        && let Some(invoice) = crate::billing::invoice_by_id(conn, id)?
    {
        return Ok(format!("{}.pdf", invoice.number));
    }
    let name = safe_file_name(&paper.original_name);
    if name.rsplit_once('.').is_some() {
        Ok(name)
    } else {
        Ok(format!("{name}.pdf"))
    }
}

fn kind_relative_path(conn: &Connection, paper: &Paper, period: i32) -> Result<String, AppError> {
    Ok(match paper.kind {
        PaperKind::IssuedInvoice | PaperKind::CreditNote => {
            format!("factures/{}", invoice_pack_name(conn, paper)?)
        }
        PaperKind::Fec => format!("fec/{}", fec_pack_name(conn, period)?),
        PaperKind::Minutes => "cloture/minutes.pdf".into(),
        PaperKind::Appropriation => "cloture/appropriation.pdf".into(),
        PaperKind::Synthesis => "cloture/synthesis.pdf".into(),
        PaperKind::BalanceSheet => "cloture/bilan.pdf".into(),
        PaperKind::Inventory => "cloture/inventaire.pdf".into(),
        PaperKind::EfiNotice => "cloture/efi.pdf".into(),
        PaperKind::Liasse => "cloture/liasse.json".into(),
        PaperKind::BankStatement => {
            format!("releves/{}", safe_file_name(&paper.original_name))
        }
        PaperKind::ExpenseReceipt => {
            format!("justificatifs/{}", safe_file_name(&paper.original_name))
        }
        other => {
            let ext = extension_of(&paper.original_name);
            format!(
                "societe/{}-{}.{ext}",
                other.as_str(),
                hash12(&paper.content_hash)
            )
        }
    })
}

fn pack_belongs(paper: &Paper, period: i32) -> bool {
    paper.period == Some(period)
        || (paper.period.is_none() && matches!(paper.kind, PaperKind::Statutes | PaperKind::Kbis))
}

/// Manifeste du pack contrôle d'un exercice : chemins relatifs en clair, hors pièces remplacées.
/// L'écriture du dossier est de l'IO d'adaptateur.
///
/// # Errors
///
/// Erreur de lecture SQLite.
pub fn control_pack_manifest(
    conn: &Connection,
    period: i32,
) -> Result<Vec<ControlPackItem>, AppError> {
    let papers = list_papers(conn, PaperFilter::ACTIVE)?
        .into_iter()
        .filter(|p| pack_belongs(p, period))
        .collect::<Vec<_>>();
    let mut used = std::collections::HashSet::new();
    let mut items = Vec::with_capacity(papers.len());
    for paper in papers {
        let relative_path = uniquify(
            kind_relative_path(conn, &paper, period)?,
            &paper.content_hash,
            &mut used,
        );
        items.push(ControlPackItem {
            relative_path,
            paper_id: paper.id,
            content_hash: paper.content_hash,
            kind: paper.kind,
        });
    }
    items.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok(items)
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

    fn invoice_spec(invoice_id: InvoiceId, original_name: &str, key: Option<&str>) -> NewPaper {
        NewPaper {
            kind: PaperKind::IssuedInvoice,
            origin: PaperOrigin::Issued,
            original_name: original_name.into(),
            mime: "application/pdf".into(),
            period: Some(2026),
            issued_on: Some(d(2026, 3, 10)),
            client_id: None,
            invoice_id: Some(invoice_id),
            expense_id: None,
            fiscal_year_id: None,
            note: None,
            idempotency_key: key.map(str::to_string),
        }
    }

    fn emit_invoice(store: &mut Store) -> InvoiceId {
        let client_id = match Executor::new(store)
            .execute(
                &crate::clients::CreateClient {
                    name: "Kappa Software".into(),
                    siren: None,
                    vat_number: None,
                    address: None,
                },
                &human(),
            )
            .unwrap()
        {
            Outcome::Applied(id) => id,
            other => panic!("attendu Applied, obtenu {other:?}"),
        };
        match Executor::new(store)
            .execute(
                &crate::billing::EmitInvoice {
                    client_id,
                    mission_id: None,
                    lines: vec![crate::domain::InvoiceLine {
                        description: "Prestation".into(),
                        quantity: 1.0,
                        unit_price: crate::domain::Money::from_cents(100_000),
                        vat_rate: crate::domain::VatRate::Standard,
                    }],
                    issued_on: d(2026, 3, 10),
                    payment_terms_days: 30,
                },
                &human(),
            )
            .unwrap()
        {
            Outcome::Applied(emitted) => emitted.id,
            other => panic!("attendu Applied, obtenu {other:?}"),
        }
    }

    fn receipts_count(store: &Store) -> usize {
        let dir = store.receipts_dir();
        if !dir.is_dir() {
            return 0;
        }
        fs::read_dir(dir).map_or(0, |entries| entries.filter_map(Result::ok).count())
    }

    #[test]
    fn a_second_issued_invoice_archive_returns_the_first_and_does_not_replace_the_blob() {
        let mut store = test_store("first-wins");
        let invoice_id = emit_invoice(&mut store);
        let first = applied(
            archive_paper(
                &mut store,
                invoice_spec(
                    invoice_id,
                    "FA-2026-0001.pdf",
                    Some("papers:issued_invoice:a"),
                ),
                b"%PDF A",
                &human(),
            )
            .unwrap(),
        );
        let hash_a = first.content_hash.clone();
        let filename_a = first.filename.clone();
        assert_eq!(receipts_count(&store), 1);

        let second = applied(
            archive_paper(
                &mut store,
                invoice_spec(
                    invoice_id,
                    "FA-2026-0001.pdf",
                    Some("papers:issued_invoice:b"),
                ),
                b"%PDF B another render",
                &human(),
            )
            .unwrap(),
        );
        assert_eq!(second.id, first.id);
        assert_eq!(second.content_hash, hash_a);
        assert_eq!(receipts::read(&store, &filename_a).unwrap(), b"%PDF A");
        assert_eq!(receipts_count(&store), 1);
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

    #[test]
    fn checklist_warns_on_missing_kbis_and_counts_a_legacy_expense_receipt() {
        use crate::company::SetCompanyProfile;
        use crate::domain::{Address, ExpenseCategory, Money, Siren, VatRate, VatRegime};
        use crate::expenses::RecordExpense;
        use crate::fiscal_year::CloseFiscalYear;

        let mut store = test_store("checklist");
        Executor::new(&mut store)
            .execute(
                &SetCompanyProfile {
                    name: "Argon Digital".into(),
                    legal_form: "SASU".into(),
                    siren: Siren::parse("552100554").unwrap(),
                    vat_number: None,
                    address: Address {
                        street: "12 rue de la Paix".into(),
                        postal_code: "75002".into(),
                        city: "Paris".into(),
                        country: "FR".into(),
                    },
                    share_capital: Some(Money::from_cents(1_000_000)),
                    rcs_city: Some("Paris".into()),
                    iban: None,
                    fiscal_year_end: Some(FiscalYearEnd::CALENDAR),
                    vat_regime: Some(VatRegime::RealNormalMonthly),
                    director_monthly_gross: None,
                    director_charge_ratio_bps: None,
                    president_name: None,
                    sole_shareholder_name: None,
                    sole_shareholder_address: None,
                    share_count: None,
                },
                &human(),
            )
            .unwrap();
        Executor::new(&mut store)
            .execute(
                &RecordExpense {
                    label: "Hébergement".into(),
                    category: ExpenseCategory::Software,
                    amount: Money::from_cents(12_000),
                    vat_rate: VatRate::Standard,
                    vat_deductible: Money::from_cents(2_000),
                    incurred_on: d(2026, 6, 1),
                    receipt_hash: Some("abc".into()),
                    receipt_filename: Some("hebergement.pdf".into()),
                    supplier: None,
                    bank_transaction_id: None,
                    paid_by: crate::domain::ExpensePaidBy::Company,
                },
                &human(),
            )
            .unwrap();
        Executor::new(&mut store)
            .execute(
                &CloseFiscalYear {
                    starts_on: d(2026, 1, 1),
                    ends_on: d(2026, 12, 31),
                    legal_reserve: Money::ZERO,
                    dividends: Money::ZERO,
                    carry_back: false,
                    today: None,
                    non_deductible_expenses: Money::ZERO,
                },
                &human(),
            )
            .unwrap();

        let empty_identity = papers_checklist(store.connection(), 2026, d(2027, 4, 20)).unwrap();
        assert!(
            empty_identity
                .items
                .iter()
                .any(|i| i.kind == PaperKind::Kbis
                    && i.status == StepStatus::Warning
                    && !i.required),
            "{empty_identity:?}"
        );
        assert!(
            empty_identity.items.iter().any(|i| {
                i.kind == PaperKind::ExpenseReceipt
                    && i.status == StepStatus::Done
                    && i.paper_id.is_none()
            }),
            "un justificatif déjà sur la dépense compte sans ligne papers : {empty_identity:?}"
        );
    }

    fn spec(
        kind: PaperKind,
        origin: PaperOrigin,
        original_name: &str,
        period: Option<i32>,
        invoice_id: Option<InvoiceId>,
    ) -> NewPaper {
        NewPaper {
            kind,
            origin,
            original_name: original_name.into(),
            mime: mime_from_name(original_name),
            period,
            issued_on: None,
            client_id: None,
            invoice_id,
            expense_id: None,
            fiscal_year_id: None,
            note: None,
            idempotency_key: None,
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn control_pack_manifest_lists_fec_invoice_and_minutes_with_unique_paths_and_skips_superseded()
    {
        use crate::company::SetCompanyProfile;
        use crate::domain::{Address, Money, Siren, VatRegime};

        let mut store = test_store("pack");
        Executor::new(&mut store)
            .execute(
                &SetCompanyProfile {
                    name: "Argon Digital".into(),
                    legal_form: "SASU".into(),
                    siren: Siren::parse("552100554").unwrap(),
                    vat_number: None,
                    address: Address {
                        street: "12 rue de la Paix".into(),
                        postal_code: "75002".into(),
                        city: "Paris".into(),
                        country: "FR".into(),
                    },
                    share_capital: Some(Money::from_cents(1_000_000)),
                    rcs_city: Some("Paris".into()),
                    iban: None,
                    fiscal_year_end: Some(FiscalYearEnd::CALENDAR),
                    vat_regime: Some(VatRegime::RealNormalMonthly),
                    director_monthly_gross: None,
                    director_charge_ratio_bps: None,
                    president_name: None,
                    sole_shareholder_name: None,
                    sole_shareholder_address: None,
                    share_count: None,
                },
                &human(),
            )
            .unwrap();
        let invoice_id = emit_invoice(&mut store);
        let invoice = crate::billing::invoice_by_id(store.connection(), invoice_id)
            .unwrap()
            .unwrap();

        let invoice_paper = applied(
            archive_paper(
                &mut store,
                spec(
                    PaperKind::IssuedInvoice,
                    PaperOrigin::Issued,
                    &format!("{}.pdf", invoice.number),
                    Some(2026),
                    Some(invoice_id),
                ),
                b"%PDF FA",
                &human(),
            )
            .unwrap(),
        );
        applied(
            archive_paper(
                &mut store,
                spec(
                    PaperKind::Fec,
                    PaperOrigin::Issued,
                    "552100554FEC20261231.txt",
                    Some(2026),
                    None,
                ),
                b"JournalCode|JournalLib|",
                &human(),
            )
            .unwrap(),
        );
        applied(
            archive_paper(
                &mut store,
                spec(
                    PaperKind::Minutes,
                    PaperOrigin::Issued,
                    "pv-2026.pdf",
                    Some(2026),
                    None,
                ),
                b"%PDF PV",
                &human(),
            )
            .unwrap(),
        );
        applied(
            archive_paper(
                &mut store,
                spec(
                    PaperKind::ExpenseReceipt,
                    PaperOrigin::Imported,
                    "facture.pdf",
                    Some(2026),
                    None,
                ),
                b"%PDF recu A",
                &human(),
            )
            .unwrap(),
        );
        applied(
            archive_paper(
                &mut store,
                spec(
                    PaperKind::ExpenseReceipt,
                    PaperOrigin::Imported,
                    "facture.pdf",
                    Some(2026),
                    None,
                ),
                b"%PDF recu B different",
                &human(),
            )
            .unwrap(),
        );

        let old_statutes = applied(
            archive_paper(
                &mut store,
                spec(
                    PaperKind::Statutes,
                    PaperOrigin::Uploaded,
                    "statuts-v1.pdf",
                    None,
                    None,
                ),
                b"%PDF v1",
                &human(),
            )
            .unwrap(),
        );
        let replacement = applied(
            archive_paper(
                &mut store,
                spec(
                    PaperKind::Statutes,
                    PaperOrigin::Uploaded,
                    "statuts-v2.pdf",
                    None,
                    None,
                ),
                b"%PDF v2",
                &human(),
            )
            .unwrap(),
        );
        store
            .connection()
            .execute(
                "UPDATE papers SET superseded_by = ?1 WHERE id = ?2",
                rusqlite::params![replacement.id.to_string(), old_statutes.id.to_string()],
            )
            .unwrap();

        let pack = control_pack_manifest(store.connection(), 2026).unwrap();
        let paths: Vec<&str> = pack.iter().map(|i| i.relative_path.as_str()).collect();
        let unique: std::collections::HashSet<&str> = paths.iter().copied().collect();
        assert_eq!(unique.len(), paths.len(), "chemins uniques : {paths:?}");

        assert!(
            pack.iter().any(|i| {
                i.kind == PaperKind::IssuedInvoice
                    && i.paper_id == invoice_paper.id
                    && i.relative_path == format!("factures/{}.pdf", invoice.number)
            }),
            "{pack:?}"
        );
        assert!(
            pack.iter().any(|i| {
                i.kind == PaperKind::Fec && i.relative_path == "fec/552100554FEC20261231.txt"
            }),
            "{pack:?}"
        );
        assert!(
            pack.iter()
                .any(|i| i.kind == PaperKind::Minutes && i.relative_path == "cloture/minutes.pdf"),
            "{pack:?}"
        );
        let receipts: Vec<_> = pack
            .iter()
            .filter(|i| i.kind == PaperKind::ExpenseReceipt)
            .collect();
        assert_eq!(receipts.len(), 2, "{pack:?}");
        assert!(
            pack.iter()
                .any(|i| i.kind == PaperKind::Statutes && i.paper_id == replacement.id),
            "le remplacement est dans le pack : {pack:?}"
        );
        assert!(
            pack.iter().all(|i| i.paper_id != old_statutes.id),
            "une pièce remplacée n'entre pas dans le pack : {pack:?}"
        );
    }
}
