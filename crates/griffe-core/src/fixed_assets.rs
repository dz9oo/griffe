//! Immobilisations (lot 42) : persistance et commandes autour de [`crate::domain::FixedAsset`].
//!
//! Une immobilisation est un fait durable dont les **dotations sont dérivées** à chaque lecture
//! ([`FixedAsset::depreciation_for`]), jamais stockées — comme le grand livre. Le résultat
//! ([`crate::accounting::compute_result_with`]) compte la dotation de l'exercice et **retire**
//! de ses charges la dépense immobilisée ; le grand livre ([`crate::ledger`]) passe cette
//! dépense en 2xx au lieu du compte de charge et écrit la dotation `681 / 28x` au dernier jour.
//!
//! Deux origines : une **dépense immobilisée** (`expense_id`, équipement au-delà de 500 € HT —
//! BOI-BIC-CHG-20-30-10 § 20 ; la base amortissable est alors *exactement* le net de la dépense,
//! [`FixedAssetsError::BaseMismatch`], pour que la charge remplacée et l'entrée à l'actif
//! coïncident au centime) ; ou une **reprise** du bilan d'ouverture (`acquired_on` antérieur à
//! `opens_on` : les dotations sont calculées depuis la date du bilan, le cumul repris est celui
//! du 28x). Régime de mutabilité (doctrine du lot 15) : *pas d'`Update`* — une immobilisation
//! se supprime et se redéclare (sa suppression est un geste à confirmer), et se fige dès qu'un
//! exercice clos porte une de ses dotations (le snapshot l'a comptée,
//! [`FixedAssetsError::DepreciatedInClosedYear`]).

use std::collections::HashMap;

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::format_description::well_known::Rfc3339;
use time::{Date, OffsetDateTime};

use crate::app::{AppError, Command};
use crate::domain::{
    AccountCode, Expense, ExpenseCategory, ExpenseId, FiscalYear, FiscalYearEnd, FixedAsset,
    FixedAssetError, FixedAssetId, Money, SMALL_EQUIPMENT_THRESHOLD, format_date, parse_date,
};
use crate::expenses::{expense_by_id, expenses_between};

const ASSET_ENTITY: &str = "immobilisation";

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FixedAssetsError {
    #[error("immobilisation introuvable : {0}")]
    NotFound(FixedAssetId),

    #[error(transparent)]
    Invalid(#[from] FixedAssetError),

    #[error("dépense introuvable : {0}")]
    ExpenseNotFound(ExpenseId),

    #[error("la dépense {0} est déjà immobilisée — supprimez d'abord cette immobilisation")]
    ExpenseAlreadyImmobilized(ExpenseId),

    #[error(
        "la base amortissable ({base}) diffère du net de la dépense immobilisée ({expense_net} = \
         TTC − TVA déductible) : une dépense s'immobilise pour son coût exact"
    )]
    BaseMismatch { base: Money, expense_net: Money },

    #[error(
        "les dotations de cette immobilisation commenceraient le {from}, dans un exercice déjà \
         clôturé ({period}) dont le résultat a été figé — supprimez d'abord le projet de \
         clôture, ou reprenez-la au bilan d'ouverture avec son cumul"
    )]
    FiscalYearClosed { from: String, period: String },

    #[error(
        "l'immobilisation {id} a été amortie dans un exercice déjà clôturé ({period}) : sa \
         dotation est figée au snapshot — supprimez d'abord le projet de clôture"
    )]
    DepreciatedInClosedYear { id: FixedAssetId, period: String },
}

impl From<FixedAssetsError> for AppError {
    fn from(e: FixedAssetsError) -> Self {
        Self::Domain(e.to_string())
    }
}

// ---------------------------------------------------------------------------------------------
// Gardes
// ---------------------------------------------------------------------------------------------

fn require_asset_revision(
    conn: &Connection,
    id: FixedAssetId,
    expected: i64,
) -> Result<i64, AppError> {
    let id_str = id.to_string();
    let current = crate::app::revision::current_revision(conn, "fixed_assets", &id_str)?
        .ok_or(FixedAssetsError::NotFound(id))?;
    crate::app::revision::require_revision(current, expected, ASSET_ENTITY, &id_str)
}

/// L'exercice clos (projet ou approuvé) qui contient `date`, s'il y en a un.
fn closed_year_containing(conn: &Connection, date: Date) -> Result<Option<String>, AppError> {
    conn.query_row(
        "SELECT starts_on || ' → ' || ends_on FROM fiscal_years
         WHERE starts_on <= ?1 AND ends_on >= ?1 LIMIT 1",
        [format_date(date)],
        |row| row.get(0),
    )
    .optional()
    .map_err(AppError::from)
}

/// Le net d'une dépense — ce qu'elle coûte hors TVA récupérée, la base amortissable si on
/// l'immobilise.
#[must_use]
pub fn expense_net(expense: &Expense) -> Money {
    expense.amount - expense.vat_deductible
}

// ---------------------------------------------------------------------------------------------
// Commandes
// ---------------------------------------------------------------------------------------------

/// Déclare une immobilisation. Avec `expense_id`, la dépense est immobilisée : sa charge est
/// remplacée par l'entrée à l'actif (`base` doit être son net exact). Une mise en service
/// antérieure au bilan d'ouverture est une reprise : les dotations commencent à la date du
/// bilan, `prior_depreciation` est le 28x repris.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddFixedAsset {
    pub label: String,
    pub account: AccountCode,
    #[serde(with = "crate::domain::serde_date::date")]
    pub acquired_on: Date,
    pub base: Money,
    pub duration_months: u32,
    #[serde(default)]
    pub prior_depreciation: Money,
    #[serde(default)]
    pub expense_id: Option<ExpenseId>,
}

impl AddFixedAsset {
    /// Reprise d'un couple 2xx/28x lu au bilan d'ouverture : la mise en service est estimée
    /// au prorata du cumul repris ([`crate::domain::inferred_acquired_on`]).
    #[must_use]
    pub fn from_candidate(
        candidate: &crate::domain::AssetCandidate,
        duration_months: u32,
        opens_on: Date,
    ) -> Self {
        Self {
            label: candidate.label.clone(),
            account: candidate.account.clone(),
            acquired_on: crate::domain::inferred_acquired_on(
                opens_on,
                candidate.gross,
                candidate.depreciation,
                duration_months,
            ),
            base: candidate.gross,
            duration_months,
            prior_depreciation: candidate.depreciation,
            expense_id: None,
        }
    }
}

impl Command for AddFixedAsset {
    type Output = FixedAssetId;
    const NAME: &'static str = "fixed_assets.add";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let opening = crate::opening_balance::opening_balance(conn)?;
        let depreciated_from = match &opening {
            Some(o) if self.acquired_on < o.balance.opens_on => o.balance.opens_on,
            _ => self.acquired_on,
        };
        let asset = FixedAsset {
            id: FixedAssetId::new(),
            label: self.label.trim().to_string(),
            account: self.account.clone(),
            acquired_on: self.acquired_on,
            base: self.base,
            duration_months: self.duration_months,
            depreciated_from,
            prior_depreciation: self.prior_depreciation,
            expense_id: self.expense_id,
            revision: 1,
            created_at: OffsetDateTime::now_utc(),
        };
        asset.validate().map_err(FixedAssetsError::Invalid)?;
        if let Some(period) = closed_year_containing(conn, depreciated_from)? {
            return Err(FixedAssetsError::FiscalYearClosed {
                from: format_date(depreciated_from),
                period,
            }
            .into());
        }
        if let Some(expense_id) = self.expense_id {
            let expense = expense_by_id(conn, expense_id)?
                .ok_or(FixedAssetsError::ExpenseNotFound(expense_id))?;
            if asset_for_expense(conn, expense_id)?.is_some() {
                return Err(FixedAssetsError::ExpenseAlreadyImmobilized(expense_id).into());
            }
            let net = expense_net(&expense);
            if net != self.base {
                return Err(FixedAssetsError::BaseMismatch {
                    base: self.base,
                    expense_net: net,
                }
                .into());
            }
            // La charge remplacée est datée de la dépense : elle doit être libre aussi.
            if let Some(period) = closed_year_containing(conn, expense.incurred_on)? {
                return Err(FixedAssetsError::FiscalYearClosed {
                    from: format_date(expense.incurred_on),
                    period,
                }
                .into());
            }
        }
        conn.execute(
            "INSERT INTO fixed_assets
                (id, label, account, acquired_on, base_cents, duration_months, depreciated_from,
                 prior_depreciation_cents, expense_id, revision, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 1, ?10)",
            params![
                asset.id.to_string(),
                asset.label,
                asset.account.as_str(),
                format_date(asset.acquired_on),
                asset.base.cents(),
                i64::from(asset.duration_months),
                format_date(asset.depreciated_from),
                asset.prior_depreciation.cents(),
                asset.expense_id.map(|id| id.to_string()),
                asset.created_at.format(&Rfc3339)?,
            ],
        )?;
        Ok(asset.id)
    }
}

/// Supprime une immobilisation — refusé dès qu'un exercice clos a porté une de ses dotations.
/// La dépense d'origine, s'il y en a une, redevient une charge ordinaire.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteFixedAsset {
    pub id: FixedAssetId,
    pub revision: i64,
}

impl Command for DeleteFixedAsset {
    type Output = ();
    const NAME: &'static str = "fixed_assets.delete";

    /// Retirer une immobilisation change le résultat de tous les exercices ouverts qu'elle
    /// touche : un agent le propose, un humain le confirme (même rail que `expenses.delete`).
    fn requires_confirmation(&self) -> bool {
        true
    }

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let asset = fixed_asset_by_id(conn, self.id)?.ok_or(FixedAssetsError::NotFound(self.id))?;
        let frozen: Option<String> = conn
            .query_row(
                "SELECT starts_on || ' → ' || ends_on FROM fiscal_years
                 WHERE ends_on >= ?1 ORDER BY ends_on LIMIT 1",
                [format_date(asset.depreciated_from)],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(period) = frozen {
            return Err(FixedAssetsError::DepreciatedInClosedYear {
                id: self.id,
                period,
            }
            .into());
        }
        require_asset_revision(conn, self.id, self.revision)?;
        conn.execute(
            "DELETE FROM fixed_assets WHERE id = ?1 AND revision = ?2",
            params![self.id.to_string(), self.revision],
        )?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// Requêtes
// ---------------------------------------------------------------------------------------------

fn conv_err(e: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
}

fn row_to_asset(row: &Row) -> rusqlite::Result<FixedAsset> {
    let id: String = row.get("id")?;
    let account: String = row.get("account")?;
    let acquired_on: String = row.get("acquired_on")?;
    let depreciated_from: String = row.get("depreciated_from")?;
    let expense_id: Option<String> = row.get("expense_id")?;
    let created_at: String = row.get("created_at")?;
    let duration: i64 = row.get("duration_months")?;
    Ok(FixedAsset {
        id: id.parse().map_err(conv_err)?,
        label: row.get("label")?,
        account: AccountCode::parse(&account).map_err(conv_err)?,
        acquired_on: parse_date(&acquired_on).map_err(conv_err)?,
        base: Money::from_cents(row.get("base_cents")?),
        duration_months: u32::try_from(duration).unwrap_or(u32::MAX),
        depreciated_from: parse_date(&depreciated_from).map_err(conv_err)?,
        prior_depreciation: Money::from_cents(row.get("prior_depreciation_cents")?),
        expense_id: expense_id
            .map(|s| s.parse::<ExpenseId>())
            .transpose()
            .map_err(conv_err)?,
        revision: row.get("revision")?,
        created_at: OffsetDateTime::parse(&created_at, &Rfc3339).map_err(conv_err)?,
    })
}

/// Toutes les immobilisations, par date de mise en service.
///
/// # Errors
///
/// Erreur de lecture SQLite.
pub fn list_fixed_assets(conn: &Connection) -> Result<Vec<FixedAsset>, AppError> {
    let mut stmt =
        conn.prepare("SELECT * FROM fixed_assets ORDER BY acquired_on ASC, created_at ASC")?;
    let rows = stmt.query_map([], row_to_asset)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
}

/// # Errors
///
/// Erreur de lecture SQLite.
pub fn fixed_asset_by_id(
    conn: &Connection,
    id: FixedAssetId,
) -> Result<Option<FixedAsset>, AppError> {
    conn.query_row(
        "SELECT * FROM fixed_assets WHERE id = ?1",
        [id.to_string()],
        row_to_asset,
    )
    .optional()
    .map_err(AppError::from)
}

/// L'immobilisation issue d'une dépense, s'il y en a une.
///
/// # Errors
///
/// Erreur de lecture SQLite.
pub fn asset_for_expense(
    conn: &Connection,
    expense_id: ExpenseId,
) -> Result<Option<FixedAsset>, AppError> {
    conn.query_row(
        "SELECT * FROM fixed_assets WHERE expense_id = ?1",
        [expense_id.to_string()],
        row_to_asset,
    )
    .optional()
    .map_err(AppError::from)
}

/// Les immobilisations indexées par la dépense dont elles sont issues — ce que le résultat et
/// le grand livre consultent pour remplacer une charge par une entrée à l'actif.
#[must_use]
pub fn assets_by_expense(assets: &[FixedAsset]) -> HashMap<ExpenseId, &FixedAsset> {
    assets
        .iter()
        .filter_map(|a| a.expense_id.map(|id| (id, a)))
        .collect()
}

/// Dotation totale de `period` sur une liste d'immobilisations.
#[must_use]
pub fn depreciation_of(assets: &[FixedAsset], period: FiscalYear) -> Money {
    assets.iter().map(|a| a.depreciation_for(period)).sum()
}

/// Une dépense `equipment` d'un net (TTC − TVA déductible) au-delà de 500 € HT qui n'est pas
/// immobilisée : ce que la tolérance BOI-BIC-CHG-20-30-10 ne couvre plus.
#[must_use]
pub fn should_be_immobilized(expense: &Expense, assets: &[FixedAsset]) -> bool {
    expense.category == ExpenseCategory::Equipment
        && expense_net(expense) > SMALL_EQUIPMENT_THRESHOLD
        && !assets.iter().any(|a| a.expense_id == Some(expense.id))
}

/// Les dépenses d'équipement de `[start, end]` qui devraient être immobilisées
/// ([`should_be_immobilized`]).
///
/// # Errors
///
/// Erreur de lecture SQLite.
pub fn immobilization_candidates(
    conn: &Connection,
    start: Date,
    end: Date,
) -> Result<Vec<Expense>, AppError> {
    let assets = list_fixed_assets(conn)?;
    Ok(expenses_between(conn, start, end)?
        .into_iter()
        .filter(|e| should_be_immobilized(e, &assets))
        .collect())
}

/// Une ligne du plan d'amortissement : l'exercice, sa dotation, le cumul et la valeur nette en
/// fin d'exercice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScheduleRow {
    pub period: FiscalYear,
    pub depreciation: Money,
    pub accumulated: Money,
    pub net_book_value: Money,
}

/// Le plan d'amortissement d'une immobilisation, exercice par exercice (selon la date de
/// clôture `fye`), du premier exercice qui porte une dotation au dernier.
#[must_use]
pub fn schedule(asset: &FixedAsset, fye: FiscalYearEnd) -> Vec<ScheduleRow> {
    let mut rows = Vec::new();
    let mut period = fye.containing(asset.depreciated_from);
    let end = asset.depreciation_end();
    // Au plus la durée en années, plus l'exercice de départ et celui du terme.
    let max_rows = asset.duration_months / 12 + 3;
    for _ in 0..max_rows {
        let depreciation = asset.depreciation_for(period);
        rows.push(ScheduleRow {
            period,
            depreciation,
            accumulated: asset.accumulated_at_end(period),
            net_book_value: asset.net_book_value_at_end(period),
        });
        if period.end() >= end || asset.net_book_value_at_end(period).is_zero() {
            break;
        }
        let Some(next) = period.end().next_day() else {
            break;
        };
        period = fye.containing(next);
    }
    rows
}

/// La vue JSON partagée d'une immobilisation (CLI `--json`, MCP, ressource) : les champs, le
/// compte d'amortissement, et pour `period` la dotation, le cumul et la valeur nette.
#[must_use]
pub fn asset_json(asset: &FixedAsset, period: FiscalYear) -> serde_json::Value {
    serde_json::json!({
        "id": asset.id,
        "label": asset.label,
        "account": asset.account,
        "account_label": crate::domain::asset_account_label(asset.account.as_str()),
        "depreciation_account": asset.depreciation_account(),
        "acquired_on": format_date(asset.acquired_on),
        "base_cents": asset.base.cents(),
        "duration_months": asset.duration_months,
        "depreciated_from": format_date(asset.depreciated_from),
        "prior_depreciation_cents": asset.prior_depreciation.cents(),
        "depreciation_end": format_date(asset.depreciation_end()),
        "expense_id": asset.expense_id,
        "revision": asset.revision,
        "period": {
            "starts_on": format_date(period.start()),
            "ends_on": format_date(period.end()),
            "depreciation_cents": asset.depreciation_for(period).cents(),
            "accumulated_cents": asset.accumulated_at_end(period).cents(),
            "net_book_value_cents": asset.net_book_value_at_end(period).cents(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Actor, ExecutionContext, Executor, Outcome};
    use crate::company::SetCompanyProfile;
    use crate::domain::{Address, FiscalYearEnd, Siren, VatRate, VatRegime};
    use crate::expenses::{DeleteExpense, RecordExpense, UpdateExpense};
    use crate::fiscal_year::CloseFiscalYear;
    use crate::opening_balance::RecordOpeningBalance;
    use crate::store::{Passphrase, Store};
    use time::Month;

    fn test_store(label: &str) -> Store {
        let dir = std::env::temp_dir().join(format!(
            "freeflow-assets-test-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ));
        Store::create(&dir.join("vault.db"), &Passphrase::from("s3cret")).unwrap()
    }

    fn human() -> ExecutionContext {
        ExecutionContext::new(Actor::Human, false)
    }

    fn date(y: i32, m: Month, d: u8) -> Date {
        Date::from_calendar_date(y, m, d).unwrap()
    }

    fn account(n: &str) -> AccountCode {
        AccountCode::parse(n).unwrap()
    }

    fn set_profile(store: &mut Store) {
        let cmd = SetCompanyProfile {
            name: "Nova Dev".to_string(),
            legal_form: "SASU".to_string(),
            siren: Siren::parse("552100554").unwrap(),
            vat_number: None,
            address: Address {
                street: "1 rue Test".to_string(),
                postal_code: "69003".to_string(),
                city: "Lyon".to_string(),
                country: "FR".to_string(),
            },
            share_capital: Some(Money::from_cents(100_000)),
            rcs_city: Some("Lyon".to_string()),
            iban: None,
            fiscal_year_end: Some(FiscalYearEnd::new(9, 30).unwrap()),
            vat_regime: Some(VatRegime::RealNormalMonthly),
            director_monthly_gross: None,
            director_charge_ratio_bps: None,
            president_name: None,
            sole_shareholder_name: None,
            sole_shareholder_address: None,
            share_count: None,
        };
        Executor::new(store).execute(&cmd, &human()).unwrap();
    }

    /// Un ordinateur à 1 440 € TTC (TVA 240 €, net 1 200 €) le 15 mars 2026.
    fn record_laptop(store: &mut Store) -> ExpenseId {
        let cmd = RecordExpense {
            label: "Ordinateur portable".to_string(),
            category: ExpenseCategory::Equipment,
            amount: Money::from_cents(144_000),
            vat_rate: VatRate::Standard,
            vat_deductible: Money::from_cents(24_000),
            incurred_on: date(2026, Month::March, 15),
            receipt_hash: None,
            receipt_filename: None,
            bank_transaction_id: None,
            supplier: None,
            paid_by: crate::domain::ExpensePaidBy::Company,
        };
        match Executor::new(store).execute(&cmd, &human()).unwrap() {
            Outcome::Applied(id) => id,
            other => panic!("attendu Applied, obtenu {other:?}"),
        }
    }

    fn add_from_expense(
        store: &mut Store,
        expense_id: ExpenseId,
        base: i64,
    ) -> Result<FixedAssetId, AppError> {
        let cmd = AddFixedAsset {
            label: "Ordinateur portable".to_string(),
            account: account("218300"),
            acquired_on: date(2026, Month::March, 15),
            base: Money::from_cents(base),
            duration_months: 36,
            prior_depreciation: Money::ZERO,
            expense_id: Some(expense_id),
        };
        match Executor::new(store).execute(&cmd, &human())? {
            Outcome::Applied(id) => Ok(id),
            other => panic!("attendu Applied, obtenu {other:?}"),
        }
    }

    #[test]
    fn an_expense_is_immobilized_for_its_exact_net_and_only_once() {
        let mut store = test_store("immobilize");
        set_profile(&mut store);
        let expense_id = record_laptop(&mut store);
        let candidates = immobilization_candidates(
            store.connection(),
            date(2025, Month::October, 1),
            date(2026, Month::September, 30),
        )
        .unwrap();
        assert_eq!(
            candidates.len(),
            1,
            "1 200 € HT de matériel dépasse la tolérance"
        );

        let err = add_from_expense(&mut store, expense_id, 144_000).unwrap_err();
        assert!(err.to_string().contains("diffère du net"), "{err}");

        let id = add_from_expense(&mut store, expense_id, 120_000).unwrap();
        let asset = fixed_asset_by_id(store.connection(), id).unwrap().unwrap();
        assert_eq!(asset.depreciated_from, date(2026, Month::March, 15));
        assert_eq!(asset.expense_id, Some(expense_id));
        assert_eq!(
            asset_for_expense(store.connection(), expense_id)
                .unwrap()
                .unwrap()
                .id,
            id
        );
        assert!(
            immobilization_candidates(
                store.connection(),
                date(2025, Month::October, 1),
                date(2026, Month::September, 30),
            )
            .unwrap()
            .is_empty()
        );

        let err = add_from_expense(&mut store, expense_id, 120_000).unwrap_err();
        assert!(err.to_string().contains("déjà immobilisée"), "{err}");

        // La dépense immobilisée ne change plus de montant et ne se supprime plus.
        let current = expense_by_id(store.connection(), expense_id)
            .unwrap()
            .unwrap();
        let update = UpdateExpense {
            id: expense_id,
            label: current.label.clone(),
            category: current.category,
            amount: Money::from_cents(100_000),
            vat_rate: current.vat_rate,
            vat_deductible: Money::from_cents(10_000),
            incurred_on: current.incurred_on,
            receipt_hash: None,
            receipt_filename: None,
            supplier: None,
            paid_by: crate::domain::ExpensePaidBy::Company,
            revision: current.revision,
        };
        let err = Executor::new(&mut store)
            .execute(&update, &human())
            .unwrap_err();
        assert!(err.to_string().contains("immobilisée"), "{err}");
        let delete = DeleteExpense {
            id: expense_id,
            revision: current.revision,
        };
        let err = Executor::new(&mut store)
            .execute(&delete, &human())
            .unwrap_err();
        assert!(err.to_string().contains("immobilisée"), "{err}");

        // Supprimée, l'immobilisation libère la dépense.
        let delete_asset = DeleteFixedAsset {
            id,
            revision: asset.revision,
        };
        Executor::new(&mut store)
            .execute(&delete_asset, &human())
            .unwrap();
        assert!(fixed_asset_by_id(store.connection(), id).unwrap().is_none());
        Executor::new(&mut store)
            .execute(&delete, &human())
            .unwrap();
    }

    #[test]
    fn an_asset_acquired_before_the_opening_balance_is_a_reprise() {
        let mut store = test_store("reprise");
        set_profile(&mut store);
        let opening = RecordOpeningBalance {
            opens_on: date(2025, Month::October, 1),
            source: None,
            lines: [
                "218300:Ordinateur:D:1500.00",
                "281830:Amortissements ordinateur:C:900.00",
                "512000:Banque:D:400.00",
                "101000:Capital:C:1000.00",
            ]
            .iter()
            .map(|l| l.parse().unwrap())
            .collect(),
            tax_losses: Money::ZERO,
            prior_corporate_tax: None,
            prior_vat_due: None,
        };
        Executor::new(&mut store)
            .execute(&opening, &human())
            .unwrap();
        let cmd = AddFixedAsset {
            label: "Ordinateur".to_string(),
            account: account("218300"),
            acquired_on: date(2024, Month::April, 1),
            base: Money::from_cents(150_000),
            duration_months: 36,
            prior_depreciation: Money::from_cents(90_000),
            expense_id: None,
        };
        let Outcome::Applied(id) = Executor::new(&mut store).execute(&cmd, &human()).unwrap()
        else {
            panic!("attendu Applied")
        };
        let asset = fixed_asset_by_id(store.connection(), id).unwrap().unwrap();
        assert_eq!(asset.depreciated_from, date(2025, Month::October, 1));
        let fy = FiscalYear::new(
            date(2025, Month::October, 1),
            date(2026, Month::September, 30),
        );
        assert_eq!(asset.depreciation_for(fy), Money::from_cents(40_000));
        let rows = schedule(&asset, FiscalYearEnd::new(9, 30).unwrap());
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].depreciation, Money::from_cents(40_000));
        assert_eq!(rows[1].depreciation, Money::from_cents(20_000));
        assert_eq!(rows[1].net_book_value, Money::ZERO);
        let json = asset_json(&asset, fy);
        assert_eq!(json["period"]["depreciation_cents"], 40_000);
        assert_eq!(json["depreciation_account"], "281830");

        // Un cumul repris sans reprise (mise en service après le bilan) est refusé.
        let wrong = AddFixedAsset {
            acquired_on: date(2026, Month::January, 10),
            ..cmd
        };
        let err = Executor::new(&mut store)
            .execute(&wrong, &human())
            .unwrap_err();
        assert!(err.to_string().contains("postérieure"), "{err}");
    }

    #[test]
    fn a_closed_year_freezes_the_assets_it_depreciated() {
        let mut store = test_store("frozen");
        set_profile(&mut store);
        let expense_id = record_laptop(&mut store);
        let id = add_from_expense(&mut store, expense_id, 120_000).unwrap();
        let close = CloseFiscalYear {
            starts_on: date(2025, Month::October, 1),
            ends_on: date(2026, Month::September, 30),
            legal_reserve: Money::ZERO,
            dividends: Money::ZERO,
            carry_back: false,
            today: None,
            non_deductible_expenses: Money::ZERO,
        };
        Executor::new(&mut store).execute(&close, &human()).unwrap();
        let record = crate::fiscal_year::fiscal_year_ending_in(store.connection(), 2026)
            .unwrap()
            .unwrap();
        // 1 200 € sur 36 mois depuis le 15 mars : 217,78 € de dotation, et la dépense n'est
        // plus une charge.
        assert_eq!(record.depreciation, Money::from_cents(21_778));
        assert_eq!(record.expenses, Money::ZERO);
        assert_eq!(record.result_before_tax, Money::from_cents(-21_778));

        let asset = fixed_asset_by_id(store.connection(), id).unwrap().unwrap();
        let err = Executor::new(&mut store)
            .execute(
                &DeleteFixedAsset {
                    id,
                    revision: asset.revision,
                },
                &human(),
            )
            .unwrap_err();
        assert!(err.to_string().contains("figée"), "{err}");

        let late = AddFixedAsset {
            label: "Écran".to_string(),
            account: account("218300"),
            acquired_on: date(2026, Month::June, 1),
            base: Money::from_cents(60_000),
            duration_months: 36,
            prior_depreciation: Money::ZERO,
            expense_id: None,
        };
        let err = Executor::new(&mut store)
            .execute(&late, &human())
            .unwrap_err();
        assert!(err.to_string().contains("déjà clôturé"), "{err}");
    }
}
