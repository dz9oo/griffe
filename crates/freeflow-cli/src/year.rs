//! `freeflow year ...` — clôture d'exercice (lot 20) : snapshot du résultat, affectation,
//! approbation, documents de clôture (`freeflow-docs`).
//!
//! Depuis le lot 31, `year balance` et `year render … balance-sheet` exposent le grand livre
//! dérivé (`freeflow_core::ledger`) : balance des comptes et bilan 2033-A, pour un exercice clos
//! ou non — comme le FEC, c'est ce qu'on regarde *avant* de clore.
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
use freeflow_core::domain::{FiscalYearEnd, Money, OpeningBalanceLine, format_date};
use freeflow_core::fiscal_year::{
    ApproveFiscalYear, CloseFiscalYear, DeleteFiscalYear, FiscalYearRecord,
    UpdateFiscalYearAppropriation, fiscal_year_ending_in, list_fiscal_years,
};
use freeflow_core::ledger::{
    BalanceSheet, TrialBalance, balance_json, build_ledger, ledger_ending_in,
};
use freeflow_core::opening_balance::{
    DeleteOpeningBalance, OpeningBalanceRecord, RecordOpeningBalance, UpdateOpeningBalance,
    opening_balance,
};
use freeflow_core::store::Store;
use serde_json::json;
use time::Date;

use crate::error::CliError;
use crate::output::{format_outcome, format_value};
use crate::parsers::{parse_date, parse_money, parse_opening_line};
use crate::table;

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum DocKind {
    /// PV des décisions de l'associé unique (approbation, affectation, quitus) — PDF.
    Minutes,
    /// Décision d'affectation du résultat (origine → affectation) — PDF.
    Appropriation,
    /// Compte de résultat simplifié, avec colonne N−1 si l'exercice précédent est clos — PDF.
    Synthesis,
    /// Cases principales 2065/2033 (dont le bilan 2033-A dérivé du grand livre) en JSON, à
    /// transmettre à l'expert-comptable.
    Liasse,
    /// Bilan simplifié (2033-A : actif brut/amortissements/net, passif) et balance des comptes,
    /// dérivés du grand livre — PDF. L'exercice n'a pas besoin d'être clos.
    BalanceSheet,
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
    /// Balance des comptes et bilan (2033-A) dérivés du grand livre de l'exercice clos dans
    /// PERIOD — clos ou non : à-nouveaux, ventes, achats, banque, opérations de clôture.
    Balance {
        /// Année civile de la clôture (ex. `2026`) — même désignation que `show` et `fec export`.
        period: i32,
    },
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
    /// Bilan d'ouverture : reprise du dernier bilan tenu avant FreeFlow (expert-comptable),
    /// point de départ du report à nouveau, de la réserve légale et des à-nouveaux du FEC.
    #[command(subcommand)]
    Opening(OpeningCommand),
}

/// La reprise se saisit **avant** toute clôture dans l'application : dès qu'un exercice est clos
/// ici, son snapshot a hérité de ce bilan et le figer devient la règle. Une ligne s'écrit
/// `compte:libellé:D|C:montant` (ex. `101000:Capital social:C:1000.00`) ; seuls les comptes de
/// bilan (classes 1 à 5) sont admis, et le total des débits doit égaler celui des crédits.
#[derive(Debug, Subcommand)]
pub enum OpeningCommand {
    /// Affiche le bilan d'ouverture : lignes, totaux, capitaux propres repris.
    Show,
    /// Enregistre le bilan d'ouverture, ou le remplace en entier s'il existe déjà. Nécessite
    /// confirmation humaine quand `--actor agent:...`.
    Set {
        /// Premier jour de l'exercice qui s'ouvre sur ce bilan (lendemain de la clôture reprise).
        #[arg(long, value_parser = parse_date)]
        opens_on: Date,
        /// Provenance, libre (ex. « bilan au 30/09/2025, cabinet X »).
        #[arg(long)]
        source: Option<String>,
        /// Une ligne `compte:libellé:D|C:montant`, répétable.
        #[arg(long = "line", value_name = "SPEC", value_parser = parse_opening_line, required_unless_present = "lines_file")]
        lines: Vec<OpeningBalanceLine>,
        /// Fichier texte : une ligne par ligne de bilan, même syntaxe ; lignes vides et `#`
        /// ignorées. Se cumule avec `--line`.
        #[arg(long, value_name = "FICHIER")]
        lines_file: Option<PathBuf>,
    },
    /// Supprime le bilan d'ouverture (tant qu'aucun exercice n'est clos).
    Rm,
}

fn opening_json(r: &OpeningBalanceRecord) -> serde_json::Value {
    let equity = r.equity();
    json!({
        "opens_on": format_date(r.balance.opens_on),
        "source": r.balance.source,
        "lines": r.balance.lines.iter().map(|l| json!({
            "account": l.account.as_str(),
            "label": l.label,
            "side": l.side.as_str(),
            "amount_cents": l.amount.cents(),
        })).collect::<Vec<_>>(),
        "total_debit_cents": r.balance.total_debit().cents(),
        "total_credit_cents": r.balance.total_credit().cents(),
        "equity": {
            "share_capital_cents": equity.share_capital.cents(),
            "legal_reserve_cents": equity.legal_reserve.cents(),
            "retained_earnings_cents": equity.retained_earnings.cents(),
        },
        "revision": r.revision,
    })
}

fn opening_human(r: &OpeningBalanceRecord) -> String {
    let equity = r.equity();
    let rows: Vec<Vec<String>> = r
        .balance
        .lines
        .iter()
        .map(|l| {
            let (debit, credit) = match l.side {
                freeflow_core::domain::Side::Debit => (l.amount.to_string(), String::new()),
                freeflow_core::domain::Side::Credit => (String::new(), l.amount.to_string()),
            };
            vec![l.account.to_string(), l.label.clone(), debit, credit]
        })
        .collect();
    format!(
        "Bilan d'ouverture au {}{}\n{}\nTotal débit : {} — total crédit : {}\n\
         Capitaux propres repris : capital {}, réserve légale {}, report à nouveau {}\n\
         (révision {})",
        format_date(r.balance.opens_on),
        r.balance
            .source
            .as_deref()
            .map_or_else(String::new, |s| format!(" — {s}")),
        table::render(&["Compte", "Libellé", "Débit", "Crédit"], &rows),
        r.balance.total_debit(),
        r.balance.total_credit(),
        equity.share_capital,
        equity.legal_reserve,
        equity.retained_earnings,
        r.revision,
    )
}

/// Les lignes de `--lines-file` : une par ligne, vides et commentaires `#` ignorés.
fn read_lines_file(path: &Path) -> Result<Vec<OpeningBalanceLine>, CliError> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        CliError::Unexpected(format!("lecture de {} impossible : {e}", path.display()))
    })?;
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| parse_opening_line(l).map_err(CliError::Domain))
        .collect()
}

fn run_opening(
    cmd: OpeningCommand,
    store: &mut Store,
    ctx: &ExecutionContext,
    json: bool,
) -> Result<String, CliError> {
    let output = match cmd {
        OpeningCommand::Show => match opening_balance(store.connection())? {
            Some(record) if json => format_value(&opening_json(&record), true),
            Some(record) => opening_human(&record),
            None if json => format_value(&serde_json::Value::Null, true),
            None => "aucun bilan d'ouverture — `freeflow year opening set`".to_string(),
        },
        OpeningCommand::Set {
            opens_on,
            source,
            mut lines,
            lines_file,
        } => {
            if let Some(path) = lines_file {
                lines.extend(read_lines_file(&path)?);
            }
            // Enregistrer ou remplacer : la CLI offre la sémantique « set », la commande envoyée
            // au cœur reste l'une des deux commandes en état complet.
            let outcome = match opening_balance(store.connection())? {
                Some(existing) => Executor::new(store).execute(
                    &UpdateOpeningBalance {
                        revision: existing.revision,
                        opens_on,
                        source,
                        lines,
                    },
                    ctx,
                )?,
                None => Executor::new(store).execute(
                    &RecordOpeningBalance {
                        opens_on,
                        source,
                        lines,
                    },
                    ctx,
                )?,
            };
            format_outcome(&outcome, json)
        }
        OpeningCommand::Rm => {
            let existing = opening_balance(store.connection())?.ok_or_else(|| {
                CliError::Domain("aucun bilan d'ouverture enregistré".to_string())
            })?;
            let outcome = Executor::new(store).execute(
                &DeleteOpeningBalance {
                    revision: existing.revision,
                },
                ctx,
            )?;
            format_outcome(&outcome, json)
        }
    };
    Ok(output)
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

/// Balance des comptes et bilan en texte : les deux tableaux du 2033-A, puis la balance.
fn balance_human(balance: &TrialBalance, sheet: &BalanceSheet) -> String {
    let money = |m: Money| {
        if m.is_zero() {
            String::new()
        } else {
            m.to_string()
        }
    };
    let asset_rows: Vec<Vec<String>> = sheet
        .assets
        .iter()
        .map(|a| {
            vec![
                a.case_gross.to_string(),
                a.label.to_string(),
                money(a.gross),
                money(a.depreciation),
                money(a.net),
            ]
        })
        .chain([vec![
            "110/112".to_string(),
            "Total général".to_string(),
            sheet.total_assets_gross.to_string(),
            money(sheet.total_depreciation),
            sheet.total_assets_net.to_string(),
        ]])
        .collect();
    let liability_rows: Vec<Vec<String>> = sheet
        .liabilities
        .iter()
        .map(|l| vec![l.case.to_string(), l.label.to_string(), money(l.amount)])
        .chain([
            vec![
                "142".to_string(),
                "Total I — capitaux propres".to_string(),
                sheet.total_equity.to_string(),
            ],
            vec![
                "180".to_string(),
                "Total général".to_string(),
                sheet.total_liabilities.to_string(),
            ],
        ])
        .collect();
    let balance_rows: Vec<Vec<String>> = balance
        .rows
        .iter()
        .map(|r| {
            let (debit, credit) = if r.balance.is_negative() {
                (String::new(), (-r.balance).to_string())
            } else {
                (r.balance.to_string(), String::new())
            };
            vec![
                r.account.number.to_string(),
                r.account.label.to_string(),
                money(r.debit),
                money(r.credit),
                debit,
                credit,
            ]
        })
        .collect();
    format!(
        "Bilan au {} (exercice du {} au {}) — présentation 2033-A\n\nActif\n{}\n\nPassif\n{}\n\n\
         Balance des comptes\n{}\nTotal débit : {} — total crédit : {}{}",
        format_date(sheet.exercise.end()),
        format_date(sheet.exercise.start()),
        format_date(sheet.exercise.end()),
        table::render(&["Case", "Rubrique", "Brut", "Amort.", "Net"], &asset_rows),
        table::render(&["Case", "Rubrique", "Montant"], &liability_rows),
        table::render(
            &["Compte", "Libellé", "Débit", "Crédit", "Solde D", "Solde C"],
            &balance_rows,
        ),
        balance.total_debit,
        balance.total_credit,
        if sheet.is_balanced() {
            String::new()
        } else {
            "\n⚠ bilan déséquilibré".to_string()
        },
    )
}

fn render(
    store: &Store,
    period: i32,
    doc: DocKind,
    out: &Path,
    today: Option<Date>,
) -> Result<String, CliError> {
    if matches!(doc, DocKind::BalanceSheet) {
        // Dérivé du grand livre : pas besoin d'un exercice clos, comme le FEC.
        let (profile, ledger) = ledger_ending_in(store.connection(), period)?;
        let bytes = freeflow_docs::render_balance_sheet(
            &profile,
            &ledger.balance_sheet(),
            &ledger.trial_balance(),
        )
        .map_err(|e| CliError::Unexpected(e.to_string()))?;
        return write_document(out, &bytes);
    }
    let record = require_year(store, period)?;
    let profile = require_profile(store)?;
    let bytes = match doc {
        DocKind::BalanceSheet => unreachable!("traité ci-dessus"),
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
            let sheet = build_ledger(store.connection(), record.period())?.balance_sheet();
            let export = freeflow_docs::liasse_export(&profile, &record, Some(&sheet));
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
        YearCommand::Opening(cmd) => return run_opening(cmd, store, ctx, json),
        YearCommand::Balance { period } => {
            let (_, ledger) = ledger_ending_in(store.connection(), period)?;
            let (balance, sheet) = (ledger.trial_balance(), ledger.balance_sheet());
            if json {
                format_value(&balance_json(&balance, &sheet), true)
            } else {
                balance_human(&balance, &sheet)
            }
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
