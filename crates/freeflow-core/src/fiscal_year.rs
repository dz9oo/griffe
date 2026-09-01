//! Clôture d'exercice (lot 20) : la table `fiscal_years` posée par la migration 0010 (lot 17)
//! trouve ici ses commandes et requêtes. La **décision d'affectation** du résultat (réserve
//! légale, dividendes, report à nouveau) est un acte juridique daté qui se cumule d'un exercice
//! sur l'autre — ce module fige un *snapshot* du résultat calculé par [`crate::accounting`] au
//! moment de la clôture, jamais recalculé après coup.
//!
//! Régime de mutabilité (doctrine du lot 15, troisième cas) : un exercice **approuvé**
//! (`approved_on` non nul) est immuable — triggers SQL de la migration 0011, comme une facture ;
//! un **projet** non approuvé reste éditable ([`UpdateFiscalYearAppropriation`]) et supprimable
//! ([`DeleteFiscalYear`]), sous révision optimiste. Un exercice suivi d'un exercice plus récent
//! n'est plus ni éditable ni supprimable même en projet : son report à nouveau a déjà été copié
//! dans la chaîne du suivant.

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::format_description::well_known::Rfc3339;
use time::{Date, OffsetDateTime};

use crate::accounting::compute_result;
use crate::app::{AppError, Command};
use crate::company::{CompanyProfile, company_profile};
use crate::domain::{FiscalYear, FiscalYearId, Money, format_date, parse_date};

/// Part du capital social que la réserve légale cumulée ne peut pas dépasser (10 %,
/// art. L232-10 du Code de commerce), en dix-millièmes.
const LEGAL_RESERVE_CAP_BPS: u32 = 1_000;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FiscalYearError {
    #[error("exercice introuvable : {0}")]
    NotFound(FiscalYearId),

    #[error(
        "aucun profil d'entreprise défini : `freeflow company set-profile` d'abord (le résultat \
         et la réserve légale en dépendent)"
    )]
    ProfileMissing,

    #[error("période invalide : {starts_on} n'est pas antérieur à {ends_on}")]
    InvalidPeriod { starts_on: String, ends_on: String },

    #[error("la période chevauche un exercice déjà clos ({0})")]
    Overlaps(String),

    #[error("montant d'affectation négatif : {0}")]
    NegativeAmount(&'static str),

    #[error(
        "affectation impossible : {requested} demandés pour {distributable} distribuables \
         (résultat net + report à nouveau antérieur)"
    )]
    ExceedsDistributable {
        requested: String,
        distributable: String,
    },

    #[error(
        "la réserve légale cumulée ({cumulated}) dépasserait 10 % du capital social ({cap}) — \
         plafond de l'art. L232-10 du Code de commerce"
    )]
    ReserveExceedsCap { cumulated: String, cap: String },

    #[error("l'exercice {0} est déjà approuvé : la décision d'AG fait foi, il est immuable")]
    AlreadyApproved(FiscalYearId),

    #[error(
        "l'exercice {0} est suivi d'un exercice plus récent qui a hérité de son report à \
         nouveau — corriger la chaîne se fait du plus récent au plus ancien"
    )]
    HasSuccessor(FiscalYearId),

    #[error("la date d'AG ({approved_on}) ne peut pas précéder la clôture ({ends_on})")]
    ApprovalBeforeEnd {
        approved_on: String,
        ends_on: String,
    },
}

impl From<FiscalYearError> for AppError {
    fn from(e: FiscalYearError) -> Self {
        Self::Domain(e.to_string())
    }
}

/// Un exercice clos tel qu'en base : le snapshot du résultat figé à la clôture, la décision
/// d'affectation, et son état d'approbation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FiscalYearRecord {
    pub id: FiscalYearId,
    pub starts_on: Date,
    pub ends_on: Date,
    pub revenue_ht: Money,
    pub expenses: Money,
    pub director_remuneration: Money,
    pub result_before_tax: Money,
    pub corporate_tax: Money,
    pub net_result: Money,
    pub legal_reserve: Money,
    pub dividends: Money,
    /// Report à nouveau cumulé après cette affectation (peut être négatif).
    pub retained_earnings: Money,
    /// Date de l'AG d'approbation — `None` tant que l'exercice est un projet éditable.
    pub approved_on: Option<Date>,
    pub revision: i64,
    pub created_at: OffsetDateTime,
}

impl FiscalYearRecord {
    #[must_use]
    pub const fn period(&self) -> FiscalYear {
        FiscalYear::new(self.starts_on, self.ends_on)
    }

    #[must_use]
    pub const fn is_approved(&self) -> bool {
        self.approved_on.is_some()
    }
}

// ---------------------------------------------------------------------------------------------
// Validation d'affectation, partagée par la clôture et la révision d'un projet
// ---------------------------------------------------------------------------------------------

/// Vérifie une affectation (réserve légale, dividendes) contre le distribuable et le plafond de
/// réserve légale, et renvoie le report à nouveau résultant.
fn validate_appropriation(
    profile: &CompanyProfile,
    net_result: Money,
    prior_retained: Money,
    prior_reserve: Money,
    legal_reserve: Money,
    dividends: Money,
) -> Result<Money, AppError> {
    if legal_reserve.cents() < 0 {
        return Err(FiscalYearError::NegativeAmount("réserve légale").into());
    }
    if dividends.cents() < 0 {
        return Err(FiscalYearError::NegativeAmount("dividendes").into());
    }
    let distributable = net_result + prior_retained;
    let requested = legal_reserve + dividends;
    if requested > distributable.max(Money::ZERO) {
        return Err(FiscalYearError::ExceedsDistributable {
            requested: requested.to_string(),
            distributable: distributable.max(Money::ZERO).to_string(),
        }
        .into());
    }
    if let Some(capital) = profile.share_capital {
        let cap = capital.apply_rate_bps(LEGAL_RESERVE_CAP_BPS);
        let cumulated = prior_reserve + legal_reserve;
        if cumulated > cap {
            return Err(FiscalYearError::ReserveExceedsCap {
                cumulated: cumulated.to_string(),
                cap: cap.to_string(),
            }
            .into());
        }
    }
    Ok(prior_retained + net_result - legal_reserve - dividends)
}

/// Report à nouveau et réserve légale cumulés des exercices clos **avant** `starts_on` — la
/// chaîne dont hérite l'exercice qui commence à cette date.
fn prior_chain(conn: &Connection, starts_on: Date) -> Result<(Money, Money), AppError> {
    let retained: Option<i64> = conn
        .query_row(
            "SELECT retained_earnings_cents FROM fiscal_years WHERE ends_on < ?1
             ORDER BY ends_on DESC LIMIT 1",
            [format_date(starts_on)],
            |row| row.get(0),
        )
        .optional()?;
    let reserve: i64 = conn.query_row(
        "SELECT coalesce(sum(legal_reserve_cents), 0) FROM fiscal_years WHERE ends_on < ?1",
        [format_date(starts_on)],
        |row| row.get(0),
    )?;
    Ok((
        Money::from_cents(retained.unwrap_or(0)),
        Money::from_cents(reserve),
    ))
}

/// Report à nouveau hérité par un exercice commençant à `starts_on` : celui du dernier exercice
/// clos avant cette date, ou zéro s'il n'y en a aucun.
///
/// # Errors
///
/// Erreur de lecture SQLite.
pub fn latest_retained_earnings(conn: &Connection, starts_on: Date) -> Result<Money, AppError> {
    prior_chain(conn, starts_on).map(|(retained, _)| retained)
}

// ---------------------------------------------------------------------------------------------
// Commandes
// ---------------------------------------------------------------------------------------------

/// Clôt un exercice : recalcule le résultat via [`crate::accounting::compute_result`], le fige
/// en snapshot, et enregistre la décision d'affectation en **projet** (non approuvé). Le report
/// à nouveau résultant est `report antérieur + résultat net − réserve légale − dividendes`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloseFiscalYear {
    pub starts_on: Date,
    pub ends_on: Date,
    pub legal_reserve: Money,
    pub dividends: Money,
}

impl Command for CloseFiscalYear {
    type Output = FiscalYearId;
    const NAME: &'static str = "fiscal.close_year";

    /// Acte à portée juridique et financière : un agent MCP le propose, un humain le confirme.
    fn requires_confirmation(&self) -> bool {
        true
    }

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        if self.starts_on >= self.ends_on {
            return Err(FiscalYearError::InvalidPeriod {
                starts_on: format_date(self.starts_on),
                ends_on: format_date(self.ends_on),
            }
            .into());
        }
        let profile = company_profile(conn)?.ok_or(FiscalYearError::ProfileMissing)?;

        let overlapping: Option<String> = conn
            .query_row(
                "SELECT starts_on || ' → ' || ends_on FROM fiscal_years
                 WHERE starts_on <= ?1 AND ends_on >= ?2 LIMIT 1",
                [format_date(self.ends_on), format_date(self.starts_on)],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(period) = overlapping {
            return Err(FiscalYearError::Overlaps(period).into());
        }

        let (prior_retained, prior_reserve) = prior_chain(conn, self.starts_on)?;
        let result = compute_result(
            conn,
            FiscalYear::new(self.starts_on, self.ends_on),
            &profile,
        )?;
        let retained_earnings = validate_appropriation(
            &profile,
            result.net_result,
            prior_retained,
            prior_reserve,
            self.legal_reserve,
            self.dividends,
        )?;

        let id = FiscalYearId::new();
        conn.execute(
            "INSERT INTO fiscal_years
                (id, starts_on, ends_on, revenue_ht_cents, expenses_cents,
                 director_remuneration_cents, result_before_tax_cents, corporate_tax_cents,
                 net_result_cents, legal_reserve_cents, dividends_cents, retained_earnings_cents,
                 approved_on, revision, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, NULL, 1, ?13)",
            params![
                id.to_string(),
                format_date(self.starts_on),
                format_date(self.ends_on),
                result.revenue_ht.cents(),
                result.expenses.cents(),
                result.director_remuneration.cents(),
                result.result_before_tax.cents(),
                result.corporate_tax.cents(),
                result.net_result.cents(),
                self.legal_reserve.cents(),
                self.dividends.cents(),
                retained_earnings.cents(),
                OffsetDateTime::now_utc().format(&Rfc3339)?,
            ],
        )?;
        Ok(id)
    }
}

/// Révise l'affectation d'un exercice encore en **projet** — le snapshot du résultat, lui, reste
/// figé (c'est la photo prise à la clôture, pas une valeur vivante). Refusée si l'exercice est
/// approuvé ou si un exercice plus récent a déjà hérité de son report à nouveau.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateFiscalYearAppropriation {
    pub id: FiscalYearId,
    /// Révision lue avant modification — écriture concurrente ⇒ [`AppError::Conflict`].
    pub revision: i64,
    pub legal_reserve: Money,
    pub dividends: Money,
}

impl Command for UpdateFiscalYearAppropriation {
    /// La révision résultante, comme les autres `Update*` du dépôt.
    type Output = i64;
    const NAME: &'static str = "fiscal.update_year_appropriation";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let record = fiscal_year_by_id(conn, self.id)?.ok_or(FiscalYearError::NotFound(self.id))?;
        require_editable(conn, &record)?;
        let profile = company_profile(conn)?.ok_or(FiscalYearError::ProfileMissing)?;
        let (prior_retained, prior_reserve) = prior_chain(conn, record.starts_on)?;
        let retained_earnings = validate_appropriation(
            &profile,
            record.net_result,
            prior_retained,
            prior_reserve,
            self.legal_reserve,
            self.dividends,
        )?;

        let new_revision = require_fiscal_year_revision(conn, self.id, self.revision)?;
        conn.execute(
            "UPDATE fiscal_years SET legal_reserve_cents = ?1, dividends_cents = ?2,
                retained_earnings_cents = ?3, revision = ?4
             WHERE id = ?5 AND revision = ?6",
            params![
                self.legal_reserve.cents(),
                self.dividends.cents(),
                retained_earnings.cents(),
                new_revision,
                self.id.to_string(),
                self.revision,
            ],
        )?;
        Ok(new_revision)
    }
}

/// Approuve un exercice : y appose la date de l'AG, après quoi la ligne devient immuable
/// (triggers de la migration 0011).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApproveFiscalYear {
    pub id: FiscalYearId,
    pub revision: i64,
    /// Date de l'AG d'approbation — au plus tôt le jour de la clôture.
    pub approved_on: Date,
}

impl Command for ApproveFiscalYear {
    type Output = i64;
    const NAME: &'static str = "fiscal.approve_year";

    /// Le geste qui scelle un acte juridique : jamais appliqué par un agent sans humain.
    fn requires_confirmation(&self) -> bool {
        true
    }

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let record = fiscal_year_by_id(conn, self.id)?.ok_or(FiscalYearError::NotFound(self.id))?;
        if record.is_approved() {
            return Err(FiscalYearError::AlreadyApproved(self.id).into());
        }
        if self.approved_on < record.ends_on {
            return Err(FiscalYearError::ApprovalBeforeEnd {
                approved_on: format_date(self.approved_on),
                ends_on: format_date(record.ends_on),
            }
            .into());
        }
        let new_revision = require_fiscal_year_revision(conn, self.id, self.revision)?;
        conn.execute(
            "UPDATE fiscal_years SET approved_on = ?1, revision = ?2
             WHERE id = ?3 AND revision = ?4",
            params![
                format_date(self.approved_on),
                new_revision,
                self.id.to_string(),
                self.revision,
            ],
        )?;
        Ok(new_revision)
    }
}

/// Supprime un exercice encore en **projet** (clos par erreur, par exemple). Un exercice
/// approuvé est immuable ; un projet suivi d'un exercice plus récent ne se supprime pas non
/// plus, sa chaîne de report ayant déjà été copiée.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteFiscalYear {
    pub id: FiscalYearId,
    pub revision: i64,
}

impl Command for DeleteFiscalYear {
    type Output = ();
    const NAME: &'static str = "fiscal.delete_year";

    /// Destructeur réel, cohérent avec les autres `Delete*` : un agent propose, un humain
    /// confirme.
    fn requires_confirmation(&self) -> bool {
        true
    }

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let record = fiscal_year_by_id(conn, self.id)?.ok_or(FiscalYearError::NotFound(self.id))?;
        require_editable(conn, &record)?;
        require_fiscal_year_revision(conn, self.id, self.revision)?;
        conn.execute(
            "DELETE FROM fiscal_years WHERE id = ?1 AND revision = ?2",
            params![self.id.to_string(), self.revision],
        )?;
        Ok(())
    }
}

/// Un exercice n'est éditable/supprimable que non approuvé **et** sans successeur — voir le
/// commentaire de module.
fn require_editable(conn: &Connection, record: &FiscalYearRecord) -> Result<(), AppError> {
    if record.is_approved() {
        return Err(FiscalYearError::AlreadyApproved(record.id).into());
    }
    let successors: i64 = conn.query_row(
        "SELECT count(*) FROM fiscal_years WHERE ends_on > ?1",
        [format_date(record.ends_on)],
        |row| row.get(0),
    )?;
    if successors > 0 {
        return Err(FiscalYearError::HasSuccessor(record.id).into());
    }
    Ok(())
}

fn require_fiscal_year_revision(
    conn: &Connection,
    id: FiscalYearId,
    expected: i64,
) -> Result<i64, AppError> {
    let id_str = id.to_string();
    let current = crate::app::revision::current_revision(conn, "fiscal_years", &id_str)?
        .ok_or(FiscalYearError::NotFound(id))?;
    crate::app::revision::require_revision(current, expected, "exercice", &id_str)
}

// ---------------------------------------------------------------------------------------------
// Requêtes
// ---------------------------------------------------------------------------------------------

fn conv_err(e: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
}

fn row_to_record(row: &Row) -> rusqlite::Result<FiscalYearRecord> {
    let id: String = row.get("id")?;
    let starts_on: String = row.get("starts_on")?;
    let ends_on: String = row.get("ends_on")?;
    let approved_on: Option<String> = row.get("approved_on")?;
    let created_at: String = row.get("created_at")?;
    Ok(FiscalYearRecord {
        id: id.parse().map_err(conv_err)?,
        starts_on: parse_date(&starts_on).map_err(conv_err)?,
        ends_on: parse_date(&ends_on).map_err(conv_err)?,
        revenue_ht: Money::from_cents(row.get("revenue_ht_cents")?),
        expenses: Money::from_cents(row.get("expenses_cents")?),
        director_remuneration: Money::from_cents(row.get("director_remuneration_cents")?),
        result_before_tax: Money::from_cents(row.get("result_before_tax_cents")?),
        corporate_tax: Money::from_cents(row.get("corporate_tax_cents")?),
        net_result: Money::from_cents(row.get("net_result_cents")?),
        legal_reserve: Money::from_cents(row.get("legal_reserve_cents")?),
        dividends: Money::from_cents(row.get("dividends_cents")?),
        retained_earnings: Money::from_cents(row.get("retained_earnings_cents")?),
        approved_on: approved_on
            .map(|s| parse_date(&s))
            .transpose()
            .map_err(conv_err)?,
        revision: row.get("revision")?,
        created_at: OffsetDateTime::parse(&created_at, &Rfc3339).map_err(conv_err)?,
    })
}

/// # Errors
pub fn fiscal_year_by_id(
    conn: &Connection,
    id: FiscalYearId,
) -> Result<Option<FiscalYearRecord>, AppError> {
    conn.query_row(
        "SELECT * FROM fiscal_years WHERE id = ?1",
        [id.to_string()],
        row_to_record,
    )
    .optional()
    .map_err(AppError::from)
}

/// # Errors
pub fn fiscal_year_by_period(
    conn: &Connection,
    period: FiscalYear,
) -> Result<Option<FiscalYearRecord>, AppError> {
    conn.query_row(
        "SELECT * FROM fiscal_years WHERE starts_on = ?1 AND ends_on = ?2",
        [format_date(period.start()), format_date(period.end())],
        row_to_record,
    )
    .optional()
    .map_err(AppError::from)
}

/// L'exercice clos dont la clôture tombe dans l'année civile `year` — la façon dont CLI, MCP et
/// GUI désignent un exercice (`--period 2025`), unique par construction (deux exercices ne se
/// chevauchent pas et durent ≥ 1 jour... un seul peut se terminer dans une année donnée pour
/// une clôture annuelle récurrente ; en cas d'exercices multiples la requête prend le dernier).
///
/// # Errors
pub fn fiscal_year_ending_in(
    conn: &Connection,
    year: i32,
) -> Result<Option<FiscalYearRecord>, AppError> {
    conn.query_row(
        "SELECT * FROM fiscal_years WHERE ends_on >= ?1 AND ends_on <= ?2
         ORDER BY ends_on DESC LIMIT 1",
        [format!("{year:04}-01-01"), format!("{year:04}-12-31")],
        row_to_record,
    )
    .optional()
    .map_err(AppError::from)
}

/// Tous les exercices clos, du plus ancien au plus récent.
///
/// # Errors
pub fn list_fiscal_years(conn: &Connection) -> Result<Vec<FiscalYearRecord>, AppError> {
    let mut stmt = conn.prepare("SELECT * FROM fiscal_years ORDER BY starts_on ASC")?;
    let rows = stmt.query_map([], row_to_record)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Actor, ExecutionContext, Executor, Outcome};
    use crate::billing::EmitInvoice;
    use crate::company::SetCompanyProfile;
    use crate::domain::{
        Address, ClientId, ExpenseCategory, FiscalYearEnd, InvoiceLine, Siren, VatRate, VatRegime,
    };
    use crate::expenses::RecordExpense;
    use crate::store::{Passphrase, Store};
    use time::Month as TimeMonth;

    fn test_store(label: &str) -> Store {
        let dir = std::env::temp_dir().join(format!(
            "freeflow-fiscal-year-test-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ));
        Store::create(&dir.join("vault.db"), &Passphrase::from("s3cret")).unwrap()
    }

    fn human() -> ExecutionContext {
        ExecutionContext::new(Actor::Human, false)
    }

    fn date(y: i32, m: TimeMonth, d: u8) -> Date {
        Date::from_calendar_date(y, m, d).unwrap()
    }

    fn set_profile(store: &mut Store, capital_cents: Option<i64>) {
        let cmd = SetCompanyProfile {
            name: "Argon Digital".to_string(),
            legal_form: "SASU".to_string(),
            siren: Siren::parse("552100554").unwrap(),
            vat_number: None,
            address: Address {
                street: "12 rue de la Paix".to_string(),
                postal_code: "75002".to_string(),
                city: "Paris".to_string(),
                country: "FR".to_string(),
            },
            share_capital: capital_cents.map(Money::from_cents),
            rcs_city: Some("Paris".to_string()),
            iban: None,
            fiscal_year_end: Some(FiscalYearEnd::CALENDAR),
            vat_regime: Some(VatRegime::RealNormalMonthly),
            director_monthly_gross: None,
            director_charge_ratio_bps: None,
        };
        Executor::new(store).execute(&cmd, &human()).unwrap();
    }

    /// Une facture de 6 175 € HT en septembre `year` et une dépense nette de 800 € en octobre :
    /// résultat avant IS 5 375 €, IS 806,25 €, net 4 568,75 € (mêmes chiffres que le test
    /// d'intégration de `accounting.rs`).
    fn seed_activity(store: &mut Store, year: i32) {
        let client_id = ClientId::new();
        store
            .connection()
            .execute(
                "INSERT INTO clients (id, name, created_at) \
                 VALUES (?1, 'Argon Digital', '2026-01-01T00:00:00Z')",
                [client_id.to_string()],
            )
            .unwrap();
        Executor::new(store)
            .execute(
                &EmitInvoice {
                    client_id,
                    mission_id: None,
                    lines: vec![InvoiceLine {
                        description: "Prestation".to_string(),
                        quantity: 9.5,
                        unit_price: Money::from_cents(65_000),
                        vat_rate: VatRate::Standard,
                    }],
                    issued_on: date(year, TimeMonth::September, 30),
                    payment_terms_days: 30,
                },
                &human(),
            )
            .unwrap();
        Executor::new(store)
            .execute(
                &RecordExpense {
                    label: "Matériel".to_string(),
                    category: ExpenseCategory::Equipment,
                    amount: Money::from_cents(100_000),
                    vat_rate: VatRate::Standard,
                    vat_deductible: Money::from_cents(20_000),
                    incurred_on: date(year, TimeMonth::October, 5),
                    receipt_hash: None,
                    receipt_filename: None,
                },
                &human(),
            )
            .unwrap();
    }

    fn close_2026(store: &mut Store, legal_reserve: i64, dividends: i64) -> FiscalYearId {
        let cmd = CloseFiscalYear {
            starts_on: date(2026, TimeMonth::January, 1),
            ends_on: date(2026, TimeMonth::December, 31),
            legal_reserve: Money::from_cents(legal_reserve),
            dividends: Money::from_cents(dividends),
        };
        let Outcome::Applied(id) = Executor::new(store).execute(&cmd, &human()).unwrap() else {
            panic!("expected Applied")
        };
        id
    }

    #[test]
    fn closing_a_year_freezes_the_computed_result_and_chains_retained_earnings() {
        let mut store = test_store("close");
        set_profile(&mut store, Some(100_000));
        seed_activity(&mut store, 2026);

        // Réserve légale 50 €, dividendes 1 000 €.
        let id = close_2026(&mut store, 5_000, 100_000);

        let record = fiscal_year_by_id(store.connection(), id).unwrap().unwrap();
        assert_eq!(record.revenue_ht, Money::from_cents(617_500));
        assert_eq!(record.expenses, Money::from_cents(80_000));
        assert_eq!(record.result_before_tax, Money::from_cents(537_500));
        assert_eq!(record.corporate_tax, Money::from_cents(80_625));
        assert_eq!(record.net_result, Money::from_cents(456_875));
        // Report à nouveau : 4 568,75 − 50 − 1 000 = 3 518,75 €.
        assert_eq!(record.retained_earnings, Money::from_cents(351_875));
        assert_eq!(record.approved_on, None);
        assert_eq!(record.revision, 1);

        // L'exercice suivant hérite du report.
        assert_eq!(
            latest_retained_earnings(store.connection(), date(2027, TimeMonth::January, 1))
                .unwrap(),
            Money::from_cents(351_875)
        );
        // Et la liste le contient.
        assert_eq!(list_fiscal_years(store.connection()).unwrap().len(), 1);
        assert_eq!(
            fiscal_year_ending_in(store.connection(), 2026)
                .unwrap()
                .unwrap()
                .id,
            id
        );
    }

    #[test]
    fn closing_requires_a_company_profile() {
        let mut store = test_store("no-profile");
        let cmd = CloseFiscalYear {
            starts_on: date(2026, TimeMonth::January, 1),
            ends_on: date(2026, TimeMonth::December, 31),
            legal_reserve: Money::ZERO,
            dividends: Money::ZERO,
        };
        let err = Executor::new(&mut store)
            .execute(&cmd, &human())
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("profil d'entreprise")));
    }

    #[test]
    fn closing_an_overlapping_period_is_refused() {
        let mut store = test_store("overlap");
        set_profile(&mut store, None);
        seed_activity(&mut store, 2026);
        close_2026(&mut store, 0, 0);

        let overlapping = CloseFiscalYear {
            starts_on: date(2026, TimeMonth::July, 1),
            ends_on: date(2027, TimeMonth::June, 30),
            legal_reserve: Money::ZERO,
            dividends: Money::ZERO,
        };
        let err = Executor::new(&mut store)
            .execute(&overlapping, &human())
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("chevauche")));
    }

    #[test]
    fn dividends_beyond_the_distributable_are_refused() {
        let mut store = test_store("distributable");
        set_profile(&mut store, None);
        seed_activity(&mut store, 2026);

        // Net 4 568,75 € : demander 5 000 € de dividendes dépasse le distribuable.
        let cmd = CloseFiscalYear {
            starts_on: date(2026, TimeMonth::January, 1),
            ends_on: date(2026, TimeMonth::December, 31),
            legal_reserve: Money::ZERO,
            dividends: Money::from_cents(500_000),
        };
        let err = Executor::new(&mut store)
            .execute(&cmd, &human())
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("distribuables")));
    }

    #[test]
    fn legal_reserve_beyond_ten_percent_of_capital_is_refused() {
        let mut store = test_store("reserve-cap");
        // Capital 1 000 € → plafond de réserve légale 100 €.
        set_profile(&mut store, Some(100_000));
        seed_activity(&mut store, 2026);

        let cmd = CloseFiscalYear {
            starts_on: date(2026, TimeMonth::January, 1),
            ends_on: date(2026, TimeMonth::December, 31),
            legal_reserve: Money::from_cents(20_000),
            dividends: Money::ZERO,
        };
        let err = Executor::new(&mut store)
            .execute(&cmd, &human())
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("réserve légale cumulée")));
    }

    #[test]
    fn a_draft_can_be_amended_and_the_retained_earnings_follow() {
        let mut store = test_store("amend");
        set_profile(&mut store, Some(100_000));
        seed_activity(&mut store, 2026);
        let id = close_2026(&mut store, 0, 0);

        let Outcome::Applied(new_revision) = Executor::new(&mut store)
            .execute(
                &UpdateFiscalYearAppropriation {
                    id,
                    revision: 1,
                    legal_reserve: Money::from_cents(10_000),
                    dividends: Money::from_cents(200_000),
                },
                &human(),
            )
            .unwrap()
        else {
            panic!("expected Applied")
        };
        assert_eq!(new_revision, 2);

        let record = fiscal_year_by_id(store.connection(), id).unwrap().unwrap();
        assert_eq!(record.legal_reserve, Money::from_cents(10_000));
        assert_eq!(record.dividends, Money::from_cents(200_000));
        // 4 568,75 − 100 − 2 000 = 2 468,75 €.
        assert_eq!(record.retained_earnings, Money::from_cents(246_875));
        // Le snapshot, lui, n'a pas bougé.
        assert_eq!(record.net_result, Money::from_cents(456_875));
    }

    #[test]
    fn amending_with_a_stale_revision_is_a_conflict() {
        let mut store = test_store("amend-stale");
        set_profile(&mut store, None);
        seed_activity(&mut store, 2026);
        let id = close_2026(&mut store, 0, 0);

        let err = Executor::new(&mut store)
            .execute(
                &UpdateFiscalYearAppropriation {
                    id,
                    revision: 99,
                    legal_reserve: Money::ZERO,
                    dividends: Money::ZERO,
                },
                &human(),
            )
            .unwrap_err();
        assert!(matches!(
            err,
            AppError::Conflict {
                entity: "exercice",
                ..
            }
        ));
    }

    #[test]
    fn an_approved_year_is_immutable_even_by_direct_sql() {
        let mut store = test_store("approved-immutable");
        set_profile(&mut store, None);
        seed_activity(&mut store, 2026);
        let id = close_2026(&mut store, 0, 0);

        Executor::new(&mut store)
            .execute(
                &ApproveFiscalYear {
                    id,
                    revision: 1,
                    approved_on: date(2027, TimeMonth::May, 15),
                },
                &human(),
            )
            .unwrap();
        let record = fiscal_year_by_id(store.connection(), id).unwrap().unwrap();
        assert_eq!(record.approved_on, Some(date(2027, TimeMonth::May, 15)));
        assert_eq!(record.revision, 2);

        // La commande d'édition refuse...
        let err = Executor::new(&mut store)
            .execute(
                &UpdateFiscalYearAppropriation {
                    id,
                    revision: 2,
                    legal_reserve: Money::ZERO,
                    dividends: Money::ZERO,
                },
                &human(),
            )
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("déjà approuvé")));

        // ... la suppression aussi...
        let err = Executor::new(&mut store)
            .execute(&DeleteFiscalYear { id, revision: 2 }, &human())
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("déjà approuvé")));

        // ... et une altération SQL directe échoue sur le trigger — vrai régime de
        // contre-écriture, pas une convention de commande.
        let err = store
            .connection()
            .execute(
                "UPDATE fiscal_years SET dividends_cents = 0 WHERE id = ?1",
                [id.to_string()],
            )
            .unwrap_err();
        assert!(err.to_string().contains("immuable"));
        let err = store
            .connection()
            .execute("DELETE FROM fiscal_years WHERE id = ?1", [id.to_string()])
            .unwrap_err();
        assert!(err.to_string().contains("immuable"));
    }

    #[test]
    fn approving_before_the_period_ends_is_refused() {
        let mut store = test_store("approve-early");
        set_profile(&mut store, None);
        seed_activity(&mut store, 2026);
        let id = close_2026(&mut store, 0, 0);

        let err = Executor::new(&mut store)
            .execute(
                &ApproveFiscalYear {
                    id,
                    revision: 1,
                    approved_on: date(2026, TimeMonth::June, 1),
                },
                &human(),
            )
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("précéder la clôture")));
    }

    #[test]
    fn a_year_with_a_successor_is_neither_editable_nor_deletable() {
        let mut store = test_store("successor");
        set_profile(&mut store, None);
        seed_activity(&mut store, 2026);
        let id_2026 = close_2026(&mut store, 0, 0);

        let cmd = CloseFiscalYear {
            starts_on: date(2027, TimeMonth::January, 1),
            ends_on: date(2027, TimeMonth::December, 31),
            legal_reserve: Money::ZERO,
            dividends: Money::ZERO,
        };
        Executor::new(&mut store).execute(&cmd, &human()).unwrap();

        let err = Executor::new(&mut store)
            .execute(
                &UpdateFiscalYearAppropriation {
                    id: id_2026,
                    revision: 1,
                    legal_reserve: Money::ZERO,
                    dividends: Money::ZERO,
                },
                &human(),
            )
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("hérité")));

        let err = Executor::new(&mut store)
            .execute(
                &DeleteFiscalYear {
                    id: id_2026,
                    revision: 1,
                },
                &human(),
            )
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("hérité")));
    }

    #[test]
    fn deleting_a_draft_removes_it() {
        let mut store = test_store("delete-draft");
        set_profile(&mut store, None);
        seed_activity(&mut store, 2026);
        let id = close_2026(&mut store, 0, 0);

        Executor::new(&mut store)
            .execute(&DeleteFiscalYear { id, revision: 1 }, &human())
            .unwrap();
        assert_eq!(fiscal_year_by_id(store.connection(), id).unwrap(), None);
    }

    #[test]
    fn closing_as_an_agent_requires_human_confirmation() {
        let mut store = test_store("agent-confirm");
        set_profile(&mut store, None);
        seed_activity(&mut store, 2026);

        let cmd = CloseFiscalYear {
            starts_on: date(2026, TimeMonth::January, 1),
            ends_on: date(2026, TimeMonth::December, 31),
            legal_reserve: Money::ZERO,
            dividends: Money::ZERO,
        };
        let agent = ExecutionContext::new(
            Actor::Agent {
                session: "test".to_string(),
            },
            false,
        );
        let outcome = Executor::new(&mut store).execute(&cmd, &agent).unwrap();
        assert!(matches!(outcome, Outcome::PendingConfirmation(_)));
        assert!(
            list_fiscal_years(store.connection()).unwrap().is_empty(),
            "rien n'est écrit tant qu'un humain n'a pas confirmé"
        );
    }
}
