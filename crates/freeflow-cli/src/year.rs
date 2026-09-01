//! `freeflow year ...` — clôture d'exercice (lot 20) : snapshot du résultat, affectation,
//! approbation, documents de clôture (`freeflow-docs`).
//!
//! Un exercice se désigne par l'**année civile de sa clôture** (`freeflow year show 2026`) :
//! contrairement aux clients/missions, il n'a pas de nom et sa période dérive du profil — pas
//! besoin du résolveur de référence générique. Comme pour `invoice render`, l'écriture disque
//! et l'appel `typst` vivent ici, dans l'adaptateur : le cœur ne touche que `&Connection`.

use std::path::{Path, PathBuf};

use clap::{Subcommand, ValueEnum};
use freeflow_core::accounting::AccountingResult;
use freeflow_core::app::{ExecutionContext, Executor};
use freeflow_core::company::{CompanyProfile, company_profile};
use freeflow_core::domain::{FiscalYearEnd, Money, format_date};
use freeflow_core::fiscal_year::{
    ApproveFiscalYear, CloseFiscalYear, DeleteFiscalYear, FiscalYearRecord,
    UpdateFiscalYearAppropriation, fiscal_year_ending_in, list_fiscal_years,
};
use freeflow_core::store::Store;
use serde_json::json;
use time::Date;

use crate::error::CliError;
use crate::output::{format_outcome, format_value};
use crate::parsers::{parse_date, parse_money};
use crate::table;

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum DocKind {
    /// PV des décisions de l'associé unique (approbation, affectation, quitus) — PDF.
    Minutes,
    /// Décision d'affectation du résultat (origine → affectation) — PDF.
    Appropriation,
    /// Compte de résultat simplifié, avec colonne N−1 si l'exercice précédent est clos — PDF.
    Synthesis,
    /// Cases principales 2065/2033 en JSON, à transmettre à l'expert-comptable.
    Liasse,
}

#[derive(Debug, Subcommand)]
pub enum YearCommand {
    /// Clôt un exercice : fige le résultat calculé (CA, charges, IS) et enregistre
    /// l'affectation (réserve légale, dividendes) en projet, à approuver ensuite. Nécessite
    /// confirmation humaine quand `--actor agent:...`.
    Close {
        /// Année civile de la clôture — la période dérive de la date de clôture du profil
        /// (année civile à défaut). Sinon, précisez `--starts-on`/`--ends-on`.
        #[arg(long, conflicts_with_all = ["starts_on", "ends_on"])]
        period: Option<i32>,
        #[arg(long, value_parser = parse_date, requires = "ends_on")]
        starts_on: Option<Date>,
        #[arg(long, value_parser = parse_date, requires = "starts_on")]
        ends_on: Option<Date>,
        /// Dotation à la réserve légale (euros, ex. `500` ou `500.00`).
        #[arg(long, value_parser = parse_money, default_value = "0")]
        legal_reserve: Money,
        /// Dividendes distribués (euros).
        #[arg(long, value_parser = parse_money, default_value = "0")]
        dividends: Money,
    },
    /// Liste les exercices clos, du plus ancien au plus récent.
    List,
    /// Détail d'un exercice clos (snapshot, affectation, approbation).
    Show {
        /// Année civile de la clôture (ex. `2026`).
        period: i32,
    },
    /// Révise l'affectation d'un exercice encore en projet (le snapshot reste figé).
    Amend {
        period: i32,
        #[arg(long, value_parser = parse_money)]
        legal_reserve: Option<Money>,
        #[arg(long, value_parser = parse_money)]
        dividends: Option<Money>,
    },
    /// Approuve un exercice (date de l'AG) — il devient immuable. Nécessite confirmation
    /// humaine quand `--actor agent:...`.
    Approve {
        period: i32,
        #[arg(long, value_parser = parse_date)]
        approved_on: Date,
    },
    /// Supprime un exercice encore en projet (clos par erreur).
    Rm { period: i32 },
    /// Rend un document de clôture (PDF, ou JSON pour la liasse) dans `--out`.
    Render {
        period: i32,
        #[arg(value_enum)]
        doc: DocKind,
        #[arg(long)]
        out: PathBuf,
        /// Date du jour, requise pour un PV en projet (non approuvé), ignorée sinon.
        #[arg(long, value_parser = parse_date)]
        today: Option<Date>,
    },
}

fn year_json(r: &FiscalYearRecord) -> serde_json::Value {
    json!({
        "id": r.id.to_string(),
        "starts_on": format_date(r.starts_on),
        "ends_on": format_date(r.ends_on),
        "revenue_ht_cents": r.revenue_ht.cents(),
        "expenses_cents": r.expenses.cents(),
        "director_remuneration_cents": r.director_remuneration.cents(),
        "result_before_tax_cents": r.result_before_tax.cents(),
        "corporate_tax_cents": r.corporate_tax.cents(),
        "net_result_cents": r.net_result.cents(),
        "legal_reserve_cents": r.legal_reserve.cents(),
        "dividends_cents": r.dividends.cents(),
        "retained_earnings_cents": r.retained_earnings.cents(),
        "approved_on": r.approved_on.map(format_date),
        "revision": r.revision,
    })
}

fn require_year(store: &Store, period: i32) -> Result<FiscalYearRecord, CliError> {
    fiscal_year_ending_in(store.connection(), period)?.ok_or_else(|| {
        CliError::Domain(format!(
            "aucun exercice clos ne se termine en {period} — voir `freeflow year list`"
        ))
    })
}

fn require_profile(store: &Store) -> Result<CompanyProfile, CliError> {
    company_profile(store.connection())?.ok_or_else(|| {
        CliError::Domain("aucun profil d'entreprise défini : freeflow company set-profile".into())
    })
}

/// La période à clore : explicite (`--starts-on`/`--ends-on`), ou dérivée du profil pour
/// `--period` (l'exercice dont la clôture récurrente tombe dans cette année civile).
fn resolve_close_period(
    store: &Store,
    period: Option<i32>,
    starts_on: Option<Date>,
    ends_on: Option<Date>,
) -> Result<(Date, Date), CliError> {
    match (period, starts_on, ends_on) {
        (Some(year), None, None) => {
            let fiscal_year_end = require_profile(store)?
                .fiscal_year_end
                .unwrap_or(FiscalYearEnd::CALENDAR);
            let end = fiscal_year_end.end_in_year(year);
            let fy = fiscal_year_end.containing(end);
            Ok((fy.start(), fy.end()))
        }
        (None, Some(start), Some(end)) => Ok((start, end)),
        _ => Err(CliError::Domain(
            "précisez soit --period, soit --starts-on et --ends-on".into(),
        )),
    }
}

fn write_document(out: &Path, bytes: &[u8]) -> Result<String, CliError> {
    std::fs::write(out, bytes).map_err(|e| {
        CliError::Unexpected(format!("écriture de {} impossible : {e}", out.display()))
    })?;
    Ok(format!(
        "✓ {} écrit ({} octets)",
        out.display(),
        bytes.len()
    ))
}

fn render(
    store: &Store,
    period: i32,
    doc: DocKind,
    out: &Path,
    today: Option<Date>,
) -> Result<String, CliError> {
    let record = require_year(store, period)?;
    let profile = require_profile(store)?;
    let bytes = match doc {
        DocKind::Minutes => {
            let today = record.approved_on.or(today).ok_or_else(|| {
                CliError::Domain(
                    "cet exercice n'est pas approuvé : précisez --today pour dater le projet \
                     de PV"
                        .into(),
                )
            })?;
            freeflow_docs::render_approval_minutes(&profile, &record, today)
                .map_err(|e| CliError::Unexpected(e.to_string()))?
        }
        DocKind::Appropriation => freeflow_docs::render_appropriation_decision(&profile, &record)
            .map_err(|e| CliError::Unexpected(e.to_string()))?,
        DocKind::Synthesis => {
            // Le document reflète le snapshot figé à la clôture, pas un recalcul vivant.
            let result = AccountingResult {
                period: record.period(),
                revenue_ht: record.revenue_ht,
                expenses: record.expenses,
                director_remuneration: record.director_remuneration,
                result_before_tax: record.result_before_tax,
                corporate_tax: record.corporate_tax,
                net_result: record.net_result,
            };
            let years = list_fiscal_years(store.connection())?;
            let prior = years
                .iter()
                .filter(|y| y.ends_on < record.starts_on)
                .max_by_key(|y| y.ends_on);
            freeflow_docs::render_synthesis(&profile, &result, prior)
                .map_err(|e| CliError::Unexpected(e.to_string()))?
        }
        DocKind::Liasse => {
            let export = freeflow_docs::liasse_export(&profile, &record);
            let mut bytes = serde_json::to_vec_pretty(&export)
                .map_err(|e| CliError::Unexpected(e.to_string()))?;
            bytes.push(b'\n');
            bytes
        }
    };
    write_document(out, &bytes)
}

pub fn run(
    cmd: YearCommand,
    store: &mut Store,
    ctx: &ExecutionContext,
    json: bool,
) -> Result<String, CliError> {
    let output = match cmd {
        YearCommand::Close {
            period,
            starts_on,
            ends_on,
            legal_reserve,
            dividends,
        } => {
            let (starts_on, ends_on) = resolve_close_period(store, period, starts_on, ends_on)?;
            let command = CloseFiscalYear {
                starts_on,
                ends_on,
                legal_reserve,
                dividends,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        YearCommand::List => {
            let years = list_fiscal_years(store.connection())?;
            if json {
                format_value(&years.iter().map(year_json).collect::<Vec<_>>(), true)
            } else {
                let rows: Vec<Vec<String>> = years
                    .iter()
                    .map(|y| {
                        vec![
                            format_date(y.ends_on),
                            y.net_result.to_string(),
                            y.legal_reserve.to_string(),
                            y.dividends.to_string(),
                            y.retained_earnings.to_string(),
                            y.approved_on.map_or_else(
                                || "projet".to_string(),
                                |d| format!("approuvé le {}", format_date(d)),
                            ),
                        ]
                    })
                    .collect();
                table::render(
                    &[
                        "Clôture",
                        "Résultat net",
                        "Réserve",
                        "Dividendes",
                        "Report",
                        "Statut",
                    ],
                    &rows,
                )
            }
        }
        YearCommand::Show { period } => {
            let record = require_year(store, period)?;
            if json {
                format_value(&year_json(&record), true)
            } else {
                let status = record.approved_on.map_or_else(
                    || "projet (non approuvé)".to_string(),
                    |d| format!("approuvé le {}", format_date(d)),
                );
                format!(
                    "Exercice du {} au {} — {status}\n\
                     CA HT : {}\nCharges externes : {}\nRémunération dirigeant : {}\n\
                     Résultat avant IS : {}\nIS : {}\nRésultat net : {}\n\
                     Réserve légale : {}\nDividendes : {}\nReport à nouveau : {}\n\
                     (id {}, révision {})",
                    format_date(record.starts_on),
                    format_date(record.ends_on),
                    record.revenue_ht,
                    record.expenses,
                    record.director_remuneration,
                    record.result_before_tax,
                    record.corporate_tax,
                    record.net_result,
                    record.legal_reserve,
                    record.dividends,
                    record.retained_earnings,
                    record.id,
                    record.revision,
                )
            }
        }
        YearCommand::Amend {
            period,
            legal_reserve,
            dividends,
        } => {
            // Lire-modifier-écrire dans la même invocation, comme `client edit` (lot 15) : la
            // commande envoyée au cœur porte toujours l'affectation complète.
            let record = require_year(store, period)?;
            let command = UpdateFiscalYearAppropriation {
                id: record.id,
                revision: record.revision,
                legal_reserve: legal_reserve.unwrap_or(record.legal_reserve),
                dividends: dividends.unwrap_or(record.dividends),
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        YearCommand::Approve {
            period,
            approved_on,
        } => {
            let record = require_year(store, period)?;
            let command = ApproveFiscalYear {
                id: record.id,
                revision: record.revision,
                approved_on,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        YearCommand::Rm { period } => {
            let record = require_year(store, period)?;
            let command = DeleteFiscalYear {
                id: record.id,
                revision: record.revision,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        YearCommand::Render {
            period,
            doc,
            out,
            today,
        } => render(store, period, doc, &out, today)?,
    };
    Ok(output)
}
