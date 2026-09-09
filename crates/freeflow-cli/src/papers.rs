//! `freeflow papers list|show|add|rm` — Les papiers (lot 57). Dépôt manuel seulement ;
//! la capture automatique des originaux nés ici est le lot 58.

use std::path::PathBuf;

use clap::Subcommand;
use freeflow_core::app::{ExecutionContext, Executor};
use freeflow_core::clock::today_local;
use freeflow_core::domain::{PaperKind, PaperOrigin, format_date};
use freeflow_core::expenses::hash_receipt;
use freeflow_core::papers::{
    NewPaper, Paper, PaperFilter, PurgePaper, archive_paper, list_papers, mime_from_name,
    paper_by_id,
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
