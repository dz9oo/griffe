//! `freeflow asset list|show|add|rm` — immobilisations et plan d'amortissement (lot 42).

use clap::{Args, Subcommand};
use griffe_core::app::{ExecutionContext, Executor};
use griffe_core::clock::today_local;
use griffe_core::company::company_profile;
use griffe_core::domain::{
    AccountCode, DEFAULT_DURATION_MONTHS, ExpenseId, FiscalYear, FiscalYearEnd, Money, format_date,
};
use griffe_core::fixed_assets::{
    self, AddFixedAsset, DeleteFixedAsset, asset_json, list_fixed_assets, schedule,
};
use griffe_core::store::Store;
use time::Date;

use crate::error::CliError;
use crate::output::{format_json, format_outcome_as, key_values};
use crate::parsers::{parse_date, parse_money};
use crate::refs;
use crate::table;

#[derive(Debug, Subcommand)]
pub enum AssetCommand {
    /// Liste les immobilisations, avec la dotation de l'exercice en cours (ou `--period`).
    List {
        /// Année civile de clôture dont afficher la dotation (défaut : exercice contenant
        /// aujourd'hui).
        #[arg(long)]
        period: Option<i32>,
    },
    /// Affiche une immobilisation et son plan d'amortissement.
    Show {
        #[arg(value_name = "RÉFÉRENCE")]
        reference: String,
        #[arg(long)]
        period: Option<i32>,
    },
    /// Déclare une immobilisation (reprise de bilan, ou dépense `equipment` à immobiliser).
    Add(Box<AddArgs>),
    /// Supprime une immobilisation — refusé dès qu'un exercice clos a porté une de ses
    /// dotations. La dépense d'origine, s'il y en a une, redevient une charge.
    Rm {
        #[arg(value_name = "RÉFÉRENCE")]
        reference: String,
    },
}

#[derive(Debug, Args)]
pub struct AddArgs {
    #[arg(long)]
    label: String,
    /// Compte d'immobilisation (2xx amortissable : 218300 matériel, 205000 logiciel, 218400
    /// mobilier…).
    #[arg(long, value_parser = clap::value_parser!(AccountCode))]
    account: AccountCode,
    /// Date de mise en service.
    #[arg(long, value_parser = parse_date)]
    acquired_on: Date,
    /// Base amortissable HT (euros). Pour une dépense, le net TTC − TVA déductible, exigé
    /// exactement.
    #[arg(long, value_parser = parse_money)]
    base: Money,
    /// Durée d'usage en mois (défaut : 36, matériel informatique).
    #[arg(long, default_value_t = DEFAULT_DURATION_MONTHS)]
    duration: u32,
    /// Amortissements déjà pratiqués, repris d'un 28x au bilan d'ouverture (défaut : 0).
    #[arg(long, value_parser = parse_money, default_value = "0")]
    prior: Money,
    /// Dépense `equipment` à immobiliser (son net doit égaler `--base`).
    #[arg(long)]
    expense: Option<String>,
}

fn current_period(store: &Store, period: Option<i32>) -> Result<FiscalYear, CliError> {
    let fye = company_profile(store.connection())?
        .and_then(|p| p.fiscal_year_end)
        .unwrap_or(FiscalYearEnd::CALENDAR);
    Ok(match period {
        Some(year) => fye.containing(fye.end_in_year(year)),
        None => fye.containing(today_local()),
    })
}

pub fn run(
    cmd: AssetCommand,
    store: &mut Store,
    ctx: &ExecutionContext,
    json: bool,
) -> Result<String, CliError> {
    match cmd {
        AssetCommand::List { period } => {
            let fy = current_period(store, period)?;
            let assets = list_fixed_assets(store.connection())?;
            if json {
                return Ok(format_json(
                    &assets.iter().map(|a| asset_json(a, fy)).collect::<Vec<_>>(),
                ));
            }
            if assets.is_empty() {
                return Ok(
                    "aucune immobilisation — `freeflow asset add` (ou depuis une dépense \
                     matériel au-delà de 500 € HT)"
                        .to_string(),
                );
            }
            let rows: Vec<Vec<String>> = assets
                .iter()
                .map(|a| {
                    vec![
                        a.id.to_string().chars().take(8).collect(),
                        a.label.clone(),
                        a.account.to_string(),
                        format_date(a.acquired_on),
                        a.base.to_string(),
                        a.depreciation_for(fy).to_string(),
                        a.net_book_value_at_end(fy).to_string(),
                    ]
                })
                .collect();
            Ok(table::render(
                &[
                    "id",
                    "libellé",
                    "compte",
                    "mise en service",
                    "base",
                    "dotation",
                    "net",
                ],
                &rows,
            ))
        }
        AssetCommand::Show { reference, period } => {
            let id = refs::resolve_fixed_asset(store, &reference)?;
            let asset = list_fixed_assets(store.connection())?
                .into_iter()
                .find(|a| a.id == id)
                .ok_or_else(|| CliError::Domain("immobilisation introuvable".into()))?;
            let fy = current_period(store, period)?;
            if json {
                let mut value = asset_json(&asset, fy);
                value["schedule"] = serde_json::json!(
                    schedule(
                        &asset,
                        company_profile(store.connection())?
                            .and_then(|p| p.fiscal_year_end)
                            .unwrap_or(FiscalYearEnd::CALENDAR)
                    )
                    .iter()
                    .map(|r| serde_json::json!({
                        "starts_on": format_date(r.period.start()),
                        "ends_on": format_date(r.period.end()),
                        "depreciation_cents": r.depreciation.cents(),
                        "accumulated_cents": r.accumulated.cents(),
                        "net_book_value_cents": r.net_book_value.cents(),
                    }))
                    .collect::<Vec<_>>()
                );
                return Ok(format_json(&value));
            }
            let fye = company_profile(store.connection())?
                .and_then(|p| p.fiscal_year_end)
                .unwrap_or(FiscalYearEnd::CALENDAR);
            let rows: Vec<Vec<String>> = schedule(&asset, fye)
                .iter()
                .map(|r| {
                    vec![
                        format!(
                            "{} → {}",
                            format_date(r.period.start()),
                            format_date(r.period.end())
                        ),
                        r.depreciation.to_string(),
                        r.accumulated.to_string(),
                        r.net_book_value.to_string(),
                    ]
                })
                .collect();
            Ok(format!(
                "{}\n\nPlan d'amortissement :\n{}",
                key_values(&[
                    ("Immobilisation", asset.label.clone()),
                    ("id", asset.id.to_string()),
                    (
                        "compte",
                        format!(
                            "{} ({})",
                            asset.account,
                            griffe_core::domain::asset_account_label(asset.account.as_str())
                        )
                    ),
                    ("amortissement", asset.depreciation_account().to_string()),
                    ("mise en service", format_date(asset.acquired_on)),
                    ("base", asset.base.to_string()),
                    ("durée", format!("{} mois", asset.duration_months)),
                    ("cumul repris", asset.prior_depreciation.to_string()),
                    (
                        "dotation de l'exercice",
                        asset.depreciation_for(fy).to_string(),
                    ),
                    ("valeur nette", asset.net_book_value_at_end(fy).to_string(),),
                    (
                        "dépense",
                        asset
                            .expense_id
                            .map_or_else(|| "reprise de bilan".to_string(), |id| id.to_string()),
                    ),
                    ("révision", asset.revision.to_string()),
                ]),
                table::render(&["exercice", "dotation", "cumul", "net"], &rows,),
            ))
        }
        AssetCommand::Add(args) => {
            let expense_id = match args.expense {
                Some(reference) => Some(refs::resolve_expense(store, &reference)?),
                None => None::<ExpenseId>,
            };
            let cmd = AddFixedAsset {
                label: args.label,
                account: args.account,
                acquired_on: args.acquired_on,
                base: args.base,
                duration_months: args.duration,
                prior_depreciation: args.prior,
                expense_id,
            };
            let outcome = Executor::new(store).execute(&cmd, ctx)?;
            Ok(format_outcome_as(&outcome, json, |id| {
                format!("immobilisation enregistrée ({id})")
            }))
        }
        AssetCommand::Rm { reference } => {
            let id = refs::resolve_fixed_asset(store, &reference)?;
            let asset = fixed_assets::fixed_asset_by_id(store.connection(), id)?
                .ok_or_else(|| CliError::Domain("immobilisation introuvable".into()))?;
            let outcome = Executor::new(store).execute(
                &DeleteFixedAsset {
                    id,
                    revision: asset.revision,
                },
                ctx,
            )?;
            Ok(format_outcome_as(&outcome, json, |()| {
                "immobilisation supprimée".to_string()
            }))
        }
    }
}
