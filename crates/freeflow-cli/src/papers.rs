//! `freeflow papers list|show|add|rm` — Les papiers (lot 57) et capture automatique des
//! originaux nés ici (lot 58). Les helpers [`capture_invoice`], [`capture_year`],
//! [`capture_bank_statement`] et [`capture_expense_receipt`] sont publics : la fenêtre et le
//! serveur MCP les appellent après l'effet juridique, jamais depuis un `Command::apply`.

use std::path::PathBuf;

use clap::Subcommand;
use freeflow_core::app::{ExecutionContext, Executor, Outcome};
use freeflow_core::clock::today_local;
use freeflow_core::company::company_profile;
use freeflow_core::domain::{
    ExpenseId, FiscalYearEnd, FiscalYearId, InvoiceId, PaperKind, PaperOrigin, format_date,
};
use freeflow_core::expenses::{expense_by_id, hash_receipt};
use freeflow_core::fec::build_fec;
use freeflow_core::fiscal_year::fiscal_year_ending_in;
use freeflow_core::papers::{
    ArchivePaper, NewPaper, Paper, PaperFilter, PurgePaper, archive_paper, issued_paper,
    list_papers, mime_from_name, paper_by_id,
};
use freeflow_core::store::Store;
use time::Date;

use crate::error::CliError;
use crate::output::{
    HumanRender, format_json, format_outcome, format_outcome_as, format_value, key_values, or_dash,
};
use crate::parsers::parse_date;
use crate::refs;
use crate::table;

impl HumanRender for Paper {
    fn render_human(&self) -> String {
        key_values(&[
            ("Pièce", self.original_name.clone()),
            ("id", self.id.to_string()),
            ("nature", kind_label(self.kind).to_string()),
            ("origine", origin_label(self.origin).to_string()),
            ("période", or_dash(self.period.map(|y| y.to_string()))),
            ("date", or_dash(self.issued_on.map(format_date))),
            ("fichier", self.filename.clone()),
            ("empreinte", self.content_hash.clone()),
            ("taille", format!("{} octets", self.byte_size)),
            ("note", or_dash(self.note.as_deref())),
        ])
    }
}

fn kind_label(kind: PaperKind) -> &'static str {
    match kind {
        PaperKind::IssuedInvoice => "facture émise",
        PaperKind::CreditNote => "avoir",
        PaperKind::Fec => "FEC",
        PaperKind::Minutes => "PV",
        PaperKind::Appropriation => "affectation",
        PaperKind::Synthesis => "synthèse",
        PaperKind::BalanceSheet => "bilan",
        PaperKind::Inventory => "inventaire",
        PaperKind::EfiNotice => "notice EFI",
        PaperKind::Liasse => "liasse",
        PaperKind::BankStatement => "relevé",
        PaperKind::ExpenseReceipt => "justificatif",
        PaperKind::Statutes => "statuts",
        PaperKind::Kbis => "Kbis",
        PaperKind::ShareLedger => "registre des mouvements de titres",
        PaperKind::ClientContract => "contrat",
        PaperKind::Insurance => "assurance",
        PaperKind::TaxNotice => "avis d'imposition",
        PaperKind::FilingAck => "accusé de dépôt",
        PaperKind::Payroll => "bulletin de paie",
        PaperKind::Other => "autre",
    }
}

fn origin_label(origin: PaperOrigin) -> &'static str {
    match origin {
        PaperOrigin::Issued => "original",
        PaperOrigin::Imported => "importé",
        PaperOrigin::Uploaded => "déposé",
    }
}

#[derive(Debug, Subcommand)]
pub enum PapersCommand {
    /// Liste les pièces du coffre (hors remplacées).
    List {
        /// Année civile de clôture de l'exercice portant la pièce.
        #[arg(long)]
        period: Option<i32>,
        /// Nature (`statutes`, `kbis`, `issued_invoice`, …).
        #[arg(long, value_parser = clap::value_parser!(PaperKind))]
        kind: Option<PaperKind>,
    },
    /// Affiche une pièce (UUID, préfixe, ou nom de fichier d'origine).
    Show {
        #[arg(value_name = "RÉFÉRENCE")]
        reference: String,
    },
    /// Dépose un fichier au coffre (statuts, Kbis, avis, contrat…) — chiffré comme un
    /// justificatif. Un scan est une copie de travail ; un original né ici se fige à l'émission.
    Add {
        #[arg(value_name = "FICHIER")]
        file: PathBuf,
        /// Nature de la pièce.
        #[arg(long, value_parser = clap::value_parser!(PaperKind))]
        kind: PaperKind,
        /// Année civile de clôture de l'exercice portant la pièce.
        #[arg(long)]
        period: Option<i32>,
        /// Client concerné (contrat, facture reçue…).
        #[arg(long)]
        client: Option<String>,
        #[arg(long)]
        note: Option<String>,
    },
    /// Purge une pièce dont le délai de conservation est échu. Un agent propose, un humain
    /// confirme. Les statuts et le Kbis ne se purgent pas tant que la société existe.
    Rm {
        #[arg(value_name = "RÉFÉRENCE")]
        reference: String,
        /// Date du jour (défaut : aujourd'hui, heure locale) — pour les tests.
        #[arg(long, value_parser = parse_date)]
        today: Option<Date>,
    },
}

pub fn run(
    cmd: PapersCommand,
    store: &mut Store,
    ctx: &ExecutionContext,
    json: bool,
) -> Result<String, CliError> {
    match cmd {
        PapersCommand::List { period, kind } => {
            let papers = list_papers(
                store.connection(),
                PaperFilter {
                    period,
                    kind,
                    include_superseded: false,
                },
            )?;
            if json {
                return Ok(format_json(&papers));
            }
            if papers.is_empty() {
                return Ok(
                    "aucune pièce — déposez les statuts, un Kbis, un avis… avec `papers add`"
                        .to_string(),
                );
            }
            let rows: Vec<Vec<String>> = papers
                .iter()
                .map(|p| {
                    vec![
                        p.id.to_string().chars().take(8).collect(),
                        kind_label(p.kind).to_string(),
                        origin_label(p.origin).to_string(),
                        p.original_name.clone(),
                        or_dash(p.period.map(|y| y.to_string())),
                    ]
                })
                .collect();
            Ok(table::render(
                &["id", "nature", "origine", "fichier", "période"],
                &rows,
            ))
        }
        PapersCommand::Show { reference } => {
            let id = refs::resolve_paper(store, &reference)?;
            let paper = paper_by_id(store.connection(), id)?
                .ok_or_else(|| CliError::Domain("pièce introuvable".into()))?;
            Ok(format_value(&paper, json))
        }
        PapersCommand::Add {
            file,
            kind,
            period,
            client,
            note,
        } => {
            let bytes = std::fs::read(&file).map_err(|e| {
                CliError::Domain(format!("lecture de {} impossible : {e}", file.display()))
            })?;
            let original_name = file
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| "document".to_string());
            let hash = hash_receipt(&bytes);
            let client_id = client
                .as_deref()
                .map(|r| refs::resolve_client(store, r))
                .transpose()?;
            let spec = NewPaper {
                kind,
                origin: PaperOrigin::Uploaded,
                mime: mime_from_name(&original_name),
                original_name,
                period,
                issued_on: None,
                client_id,
                invoice_id: None,
                expense_id: None,
                fiscal_year_id: None,
                note,
                idempotency_key: Some(format!("papers:upload:{hash}:{kind}")),
            };
            let outcome = archive_paper(store, spec, &bytes, ctx)?;
            Ok(format_outcome_as(&outcome, json, |p| {
                format!(
                    "pièce archivée : {} ({})",
                    p.original_name,
                    kind_label(p.kind)
                )
            }))
        }
        PapersCommand::Rm { reference, today } => {
            let id = refs::resolve_paper(store, &reference)?;
            let outcome = Executor::new(store).execute(
                &PurgePaper {
                    id,
                    today: today.unwrap_or_else(today_local),
                },
                ctx,
            )?;
            Ok(format_outcome(&outcome, json))
        }
    }
}

/// Rapport de [`capture_year`] : chaque nature est indépendante.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct YearCaptureReport {
    pub captured: Vec<PaperKind>,
    pub already: Vec<PaperKind>,
    pub missing: Vec<(PaperKind, String)>,
}

impl YearCaptureReport {
    /// Phrase pour la sortie humaine d'une approbation / d'un rendu. `None` si tout était déjà là.
    #[must_use]
    pub fn human_note(&self) -> Option<String> {
        let mut parts = Vec::new();
        if !self.captured.is_empty() {
            let names = self
                .captured
                .iter()
                .map(|k| kind_label(*k))
                .collect::<Vec<_>>()
                .join(", ");
            parts.push(format!("originaux figés : {names}"));
        }
        if !self.missing.is_empty() {
            let details = self
                .missing
                .iter()
                .map(|(k, reason)| format!("{} ({reason})", kind_label(*k)))
                .collect::<Vec<_>>()
                .join(", ");
            parts.push(format!("non figés : {details}"));
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" ; "))
        }
    }
}

/// Ajoute une note de capture au texte humain, sans toucher au JSON.
#[must_use]
pub fn append_capture_note(rendered: String, json: bool, note: Option<String>) -> String {
    match note {
        Some(note) if !json => format!("{rendered}\n{note}"),
        _ => rendered,
    }
}

fn paper_period(store: &Store, on: Date) -> Result<i32, CliError> {
    let fye = company_profile(store.connection())?
        .and_then(|p| p.fiscal_year_end)
        .unwrap_or(FiscalYearEnd::CALENDAR);
    Ok(fye.containing(on).end().year())
}

fn existing_kind(store: &Store, kind: PaperKind, period: i32) -> Result<Option<Paper>, CliError> {
    Ok(list_papers(
        store.connection(),
        PaperFilter {
            period: Some(period),
            kind: Some(kind),
            include_superseded: false,
        },
    )?
    .into_iter()
    .next())
}

fn take_paper(outcome: Outcome<Paper>) -> Result<Option<Paper>, CliError> {
    match outcome {
        Outcome::Applied(p) | Outcome::AlreadyApplied(p) => Ok(Some(p)),
        Outcome::DryRun => Ok(None),
        Outcome::PendingConfirmation(_) => Ok(None),
    }
}

/// Rend le Factur-X et l'archive une fois. No-op si `(IssuedInvoice|CreditNote, invoice_id)`
/// existe. Échec de rendu (profil manquant, Typst) : `Ok(None)`, jamais un rollback de l'émission.
///
/// # Errors
///
/// Facture introuvable, ou erreur d'index / d'IO hors rendu.
pub fn capture_invoice(
    store: &mut Store,
    ctx: &ExecutionContext,
    invoice_id: InvoiceId,
) -> Result<Option<Paper>, CliError> {
    if ctx.dry_run {
        return Ok(None);
    }
    let invoice = freeflow_core::billing::invoice_by_id(store.connection(), invoice_id)?
        .ok_or_else(|| CliError::Domain(format!("facture introuvable : {invoice_id}")))?;
    let kind = if invoice.credited_invoice_id.is_some() {
        PaperKind::CreditNote
    } else {
        PaperKind::IssuedInvoice
    };
    if let Some(existing) = issued_paper(store.connection(), invoice_id, kind)? {
        return Ok(Some(existing));
    }
    let Some(profile) = company_profile(store.connection())? else {
        return Ok(None);
    };
    let Some(client) = freeflow_core::clients::client_by_id(store.connection(), invoice.client_id)?
    else {
        return Ok(None);
    };
    let Ok(pdf) = freeflow_invoice::render_pdf(&invoice, &client, &profile, None) else {
        return Ok(None);
    };
    let period = paper_period(store, invoice.issued_on)?;
    let spec = NewPaper {
        kind,
        origin: PaperOrigin::Issued,
        original_name: format!("{}.pdf", invoice.number),
        mime: "application/pdf".into(),
        period: Some(period),
        issued_on: Some(invoice.issued_on),
        client_id: Some(invoice.client_id),
        invoice_id: Some(invoice.id),
        expense_id: None,
        fiscal_year_id: None,
        note: None,
        idempotency_key: Some(format!("papers:{kind}:{invoice_id}")),
    };
    take_paper(archive_paper(store, spec, &pdf, ctx)?)
}

fn year_doc_kind(kind: PaperKind) -> Option<crate::year::DocKind> {
    match kind {
        PaperKind::Minutes => Some(crate::year::DocKind::Minutes),
        PaperKind::Appropriation => Some(crate::year::DocKind::Appropriation),
        PaperKind::Synthesis => Some(crate::year::DocKind::Synthesis),
        PaperKind::BalanceSheet => Some(crate::year::DocKind::BalanceSheet),
        PaperKind::Inventory => Some(crate::year::DocKind::Inventory),
        PaperKind::EfiNotice => Some(crate::year::DocKind::EfiNotice),
        PaperKind::Liasse => Some(crate::year::DocKind::Liasse),
        _ => None,
    }
}

fn year_original_name(kind: PaperKind, period: i32, fec_name: Option<&str>) -> String {
    match kind {
        PaperKind::Minutes => format!("pv-{period}.pdf"),
        PaperKind::Appropriation => format!("affectation-{period}.pdf"),
        PaperKind::Synthesis => format!("synthese-{period}.pdf"),
        PaperKind::BalanceSheet => format!("bilan-{period}.pdf"),
        PaperKind::Inventory => format!("inventaire-{period}.pdf"),
        PaperKind::EfiNotice => format!("notice-efi-{period}.pdf"),
        PaperKind::Liasse => format!("liasse-{period}.json"),
        PaperKind::Fec => fec_name
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| format!("fec-{period}.txt")),
        other => format!("{other}-{period}"),
    }
}

fn year_idempotency_key(kind: PaperKind, period: i32, year_id: Option<FiscalYearId>) -> String {
    match kind {
        PaperKind::Fec | PaperKind::BalanceSheet | PaperKind::Inventory => {
            format!("papers:{kind}:{period}")
        }
        _ => match year_id {
            Some(id) => format!("papers:{kind}:{id}"),
            None => format!("papers:{kind}:{period}"),
        },
    }
}

fn try_capture_year_kind(
    store: &mut Store,
    ctx: &ExecutionContext,
    period: i32,
    kind: PaperKind,
    year_id: Option<FiscalYearId>,
    issued_on: Option<Date>,
    report: &mut YearCaptureReport,
) {
    match existing_kind(store, kind, period) {
        Ok(Some(_)) => {
            report.already.push(kind);
            return;
        }
        Ok(None) => {}
        Err(e) => {
            report.missing.push((kind, e.to_string()));
            return;
        }
    }
    let rendered = if kind == PaperKind::Fec {
        match build_fec(store.connection(), period) {
            Ok(fec) => Ok((
                year_original_name(kind, period, Some(&fec.file_name())),
                fec.render().into_bytes(),
            )),
            Err(e) => Err(e.to_string()),
        }
    } else if let Some(doc) = year_doc_kind(kind) {
        match crate::year::render_bytes(store, period, doc, issued_on) {
            Ok(bytes) => Ok((year_original_name(kind, period, None), bytes)),
            Err(e) => Err(e.to_string()),
        }
    } else {
        Err("nature non générée par FreeFlow".into())
    };
    let (original_name, bytes) = match rendered {
        Ok(pair) => pair,
        Err(reason) => {
            report.missing.push((kind, reason));
            return;
        }
    };
    let spec = NewPaper {
        kind,
        origin: PaperOrigin::Issued,
        mime: mime_from_name(&original_name),
        original_name,
        period: Some(period),
        issued_on,
        client_id: None,
        invoice_id: None,
        expense_id: None,
        fiscal_year_id: year_id,
        note: None,
        idempotency_key: Some(year_idempotency_key(kind, period, year_id)),
    };
    match archive_paper(store, spec, &bytes, ctx) {
        Ok(Outcome::Applied(_) | Outcome::AlreadyApplied(_)) => report.captured.push(kind),
        Ok(Outcome::DryRun | Outcome::PendingConfirmation(_)) => {}
        Err(e) => report.missing.push((kind, e.to_string())),
    }
}

/// Rend et archive, pour l'exercice, ce qui peut l'être : Minutes et Appropriation seulement
/// si approuvé ; Synthesis, BalanceSheet, Inventory, EfiNotice, Liasse, Fec dès qu'un snapshot
/// ou un exercice dérivable existe. Chaque nature est indépendante.
///
/// # Errors
///
/// Erreur de lecture du coffre (pas un échec de Typst : ceux-là vont dans [`YearCaptureReport::missing`]).
pub fn capture_year(
    store: &mut Store,
    ctx: &ExecutionContext,
    period: i32,
) -> Result<YearCaptureReport, CliError> {
    let mut report = YearCaptureReport::default();
    if ctx.dry_run {
        return Ok(report);
    }
    let record = fiscal_year_ending_in(store.connection(), period)?;
    let year_id = record.as_ref().map(|r| r.id);
    let issued_on = record.as_ref().map(|r| r.ends_on);
    for kind in [
        PaperKind::BalanceSheet,
        PaperKind::Inventory,
        PaperKind::Fec,
    ] {
        try_capture_year_kind(store, ctx, period, kind, year_id, issued_on, &mut report);
    }
    if let Some(record) = record.as_ref() {
        for kind in [
            PaperKind::Synthesis,
            PaperKind::Liasse,
            PaperKind::EfiNotice,
        ] {
            try_capture_year_kind(store, ctx, period, kind, year_id, issued_on, &mut report);
        }
        if record.approved_on.is_some() {
            for kind in [PaperKind::Minutes, PaperKind::Appropriation] {
                try_capture_year_kind(store, ctx, period, kind, year_id, issued_on, &mut report);
            }
        }
    }
    Ok(report)
}

/// Archive le fichier source d'un relevé (kind `bank_statement`). Même contenu deux fois : no-op
/// (`idempotency_key` = empreinte).
///
/// # Errors
///
/// IO d'archivage ou commande d'index.
pub fn capture_bank_statement(
    store: &mut Store,
    ctx: &ExecutionContext,
    original_name: &str,
    bytes: &[u8],
    period: Option<i32>,
) -> Result<Paper, CliError> {
    let hash = hash_receipt(bytes);
    let spec = NewPaper {
        kind: PaperKind::BankStatement,
        origin: PaperOrigin::Imported,
        original_name: original_name.to_string(),
        mime: mime_from_name(original_name),
        period,
        issued_on: None,
        client_id: None,
        invoice_id: None,
        expense_id: None,
        fiscal_year_id: None,
        note: None,
        idempotency_key: Some(format!("papers:bank:{hash}")),
    };
    take_paper(archive_paper(store, spec, bytes, ctx)?)?
        .ok_or_else(|| CliError::Domain("relevé non archivé (dry-run)".into()))
}

/// Indexe un justificatif **déjà** dans `.receipts/` (`AttachReceipt`) — n'écrit pas le blob.
///
/// # Errors
///
/// Dépense introuvable, pièce illisible, ou commande d'index.
pub fn capture_expense_receipt(
    store: &mut Store,
    ctx: &ExecutionContext,
    expense_id: ExpenseId,
    filename: &str,
    hash: &str,
) -> Result<Paper, CliError> {
    let already = list_papers(
        store.connection(),
        PaperFilter {
            period: None,
            kind: Some(PaperKind::ExpenseReceipt),
            include_superseded: false,
        },
    )?
    .into_iter()
    .find(|p| p.expense_id == Some(expense_id));
    if let Some(paper) = already {
        return Ok(paper);
    }
    let expense = expense_by_id(store.connection(), expense_id)?
        .ok_or_else(|| CliError::Domain(format!("dépense introuvable : {expense_id}")))?;
    let bytes = freeflow_core::receipts::read(store, filename)
        .map_err(|e| CliError::Unexpected(e.to_string()))?;
    let original_name = filename
        .split_once('-')
        .map(|(_, n)| n.to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| filename.to_string());
    let period = paper_period(store, expense.incurred_on)?;
    let byte_size = i64::try_from(bytes.len()).unwrap_or(i64::MAX);
    let cmd = ArchivePaper {
        kind: PaperKind::ExpenseReceipt,
        origin: PaperOrigin::Imported,
        original_name,
        filename: filename.to_string(),
        content_hash: hash.to_string(),
        mime: mime_from_name(filename),
        byte_size,
        period: Some(period),
        issued_on: Some(expense.incurred_on),
        client_id: None,
        invoice_id: None,
        expense_id: Some(expense_id),
        fiscal_year_id: None,
        note: None,
        idempotency_key: Some(format!("papers:expense_receipt:{expense_id}")),
    };
    take_paper(Executor::new(store).execute(&cmd, ctx)?)?
        .ok_or_else(|| CliError::Domain("justificatif non indexé (dry-run)".into()))
}

/// Note humaine d'une capture de facture — n'échoue jamais l'émission.
#[must_use]
pub fn invoice_capture_note(result: Result<Option<Paper>, CliError>) -> Option<String> {
    match result {
        Ok(Some(_)) => Some("original figé au coffre".to_string()),
        Ok(None) => Some("original non figé : profil incomplet ou rendu impossible".to_string()),
        Err(e) => Some(format!("original non figé : {e}")),
    }
}
