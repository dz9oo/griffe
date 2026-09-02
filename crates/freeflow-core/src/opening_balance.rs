//! Bilan d'ouverture (lot 30) : persistance et commandes autour de
//! [`crate::domain::OpeningBalance`] — la reprise du bilan tenu avant l'usage de l'application.
//!
//! Il n'y a **qu'un seul** bilan d'ouverture par coffre (ligne `id = 1`, migration 0014) : c'est
//! le point de départ de l'historique comptable, dont [`crate::fiscal_year`] enchaîne ensuite les
//! exercices clos (report à nouveau, réserve légale) et dont [`crate::fec`] tire les à-nouveaux
//! du premier exercice. Les exercices suivants n'en ont pas besoin : leur ouverture *est* le
//! snapshot de clôture du précédent.
//!
//! Régime de mutabilité (doctrine du lot 15) : *mutable* sous révision optimiste
//! ([`UpdateOpeningBalance`] en état complet, lignes réécrites en bloc comme les jalons d'une
//! mission), **tant qu'aucun exercice n'est clos** dans l'application — dès qu'une ligne
//! `fiscal_years` existe, son snapshot a copié la chaîne issue de ce bilan, et le corriger
//! après coup réécrirait silencieusement un report à nouveau déjà figé
//! ([`OpeningBalanceError::FiscalYearsExist`]). Corriger se fait alors du plus récent au plus
//! ancien : supprimer les projets de clôture, puis le bilan ; un exercice approuvé fige la
//! chaîne pour de bon. Les trois commandes exigent une confirmation humaine : la reprise d'un
//! bilan est un fait comptable qu'un agent propose, jamais qu'il pose seul.

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::format_description::well_known::Rfc3339;
use time::{Date, OffsetDateTime};

use crate::app::{AppError, Command};
use crate::domain::{
    AccountCode, Money, OpeningBalance, OpeningBalanceLine, OpeningEquity, Side, format_date,
    parse_date,
};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum OpeningBalanceError {
    #[error("aucun bilan d'ouverture enregistré")]
    NotFound,

    #[error(
        "un bilan d'ouverture existe déjà (ouvert le {0}) : modifiez-le plutôt que d'en \
         enregistrer un second"
    )]
    AlreadyRecorded(String),

    #[error(
        "un exercice est déjà clos dans l'application ({0}) : son snapshot a hérité de ce bilan \
         d'ouverture — supprimez d'abord les projets de clôture (un exercice approuvé fige la \
         chaîne définitivement)"
    )]
    FiscalYearsExist(String),

    #[error(transparent)]
    Invalid(#[from] crate::domain::OpeningBalanceError),
}

impl From<OpeningBalanceError> for AppError {
    fn from(e: OpeningBalanceError) -> Self {
        Self::Domain(e.to_string())
    }
}

/// Le bilan d'ouverture tel qu'en base, avec sa révision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OpeningBalanceRecord {
    #[serde(flatten)]
    pub balance: OpeningBalance,
    pub revision: i64,
    pub created_at: OffsetDateTime,
}

impl OpeningBalanceRecord {
    #[must_use]
    pub fn equity(&self) -> OpeningEquity {
        self.balance.equity()
    }
}

// ---------------------------------------------------------------------------------------------
// Gardes partagées
// ---------------------------------------------------------------------------------------------

/// Refuse toute écriture du bilan d'ouverture dès qu'un exercice (projet ou approuvé) est clos
/// dans l'application — voir le commentaire de module.
fn require_no_fiscal_year(conn: &Connection) -> Result<(), AppError> {
    let existing: Option<String> = conn
        .query_row(
            "SELECT starts_on || ' → ' || ends_on FROM fiscal_years ORDER BY starts_on LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(period) = existing {
        return Err(OpeningBalanceError::FiscalYearsExist(period).into());
    }
    Ok(())
}

fn require_opening_revision(conn: &Connection, expected: i64) -> Result<i64, AppError> {
    let current = crate::app::revision::current_revision(conn, "opening_balance", "1")?
        .ok_or(OpeningBalanceError::NotFound)?;
    crate::app::revision::require_revision(current, expected, "bilan d'ouverture", "1")
}

fn write_lines(conn: &Connection, lines: &[OpeningBalanceLine]) -> Result<(), AppError> {
    conn.execute(
        "DELETE FROM opening_balance_lines WHERE opening_balance_id = 1",
        [],
    )?;
    let mut stmt = conn.prepare(
        "INSERT INTO opening_balance_lines
            (opening_balance_id, position, account, label, side, amount_cents)
         VALUES (1, ?1, ?2, ?3, ?4, ?5)",
    )?;
    for (position, line) in lines.iter().enumerate() {
        stmt.execute(params![
            i64::try_from(position).unwrap_or(i64::MAX),
            line.account.as_str(),
            line.label,
            line.side.as_str(),
            line.amount.cents(),
        ])?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Commandes
// ---------------------------------------------------------------------------------------------

/// Enregistre le bilan d'ouverture — une seule fois par coffre, avant toute clôture.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordOpeningBalance {
    pub opens_on: Date,
    pub source: Option<String>,
    pub lines: Vec<OpeningBalanceLine>,
    /// Déficits fiscaux antérieurs encore reportables (case 870 du dernier 2033-D), hors
    /// bilan — zéro par défaut (lot 32).
    #[serde(default)]
    pub tax_losses: Money,
}

impl RecordOpeningBalance {
    fn balance(&self) -> OpeningBalance {
        OpeningBalance {
            opens_on: self.opens_on,
            source: self.source.clone(),
            lines: self.lines.clone(),
            tax_losses: self.tax_losses,
        }
    }
}

impl Command for RecordOpeningBalance {
    /// La révision initiale (toujours 1), comme les autres `Update*` du dépôt renvoient la leur.
    type Output = i64;
    const NAME: &'static str = "fiscal.record_opening_balance";

    /// Fait comptable fondateur : un agent le propose, un humain le confirme.
    fn requires_confirmation(&self) -> bool {
        true
    }

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let balance = self.balance();
        balance.validate().map_err(OpeningBalanceError::from)?;
        if let Some(existing) = opening_balance(conn)? {
            return Err(OpeningBalanceError::AlreadyRecorded(format_date(
                existing.balance.opens_on,
            ))
            .into());
        }
        require_no_fiscal_year(conn)?;
        conn.execute(
            "INSERT INTO opening_balance (id, opens_on, source, revision, created_at, \
             tax_losses_cents)
             VALUES (1, ?1, ?2, 1, ?3, ?4)",
            params![
                format_date(self.opens_on),
                self.source,
                OffsetDateTime::now_utc().format(&Rfc3339)?,
                self.tax_losses.cents(),
            ],
        )?;
        write_lines(conn, &self.lines)?;
        Ok(1)
    }
}

/// Remplace le bilan d'ouverture en **état complet** (date, provenance, toutes les lignes) —
/// jamais un patch, pour que l'audit garde une image entière.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateOpeningBalance {
    /// Révision lue avant modification — écriture concurrente ⇒ [`AppError::Conflict`].
    pub revision: i64,
    pub opens_on: Date,
    pub source: Option<String>,
    pub lines: Vec<OpeningBalanceLine>,
    #[serde(default)]
    pub tax_losses: Money,
}

impl Command for UpdateOpeningBalance {
    type Output = i64;
    const NAME: &'static str = "fiscal.update_opening_balance";

    fn requires_confirmation(&self) -> bool {
        true
    }

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let balance = OpeningBalance {
            opens_on: self.opens_on,
            source: self.source.clone(),
            lines: self.lines.clone(),
            tax_losses: self.tax_losses,
        };
        balance.validate().map_err(OpeningBalanceError::from)?;
        require_no_fiscal_year(conn)?;
        let new_revision = require_opening_revision(conn, self.revision)?;
        let changed = conn.execute(
            "UPDATE opening_balance SET opens_on = ?1, source = ?2, revision = ?3, \
             tax_losses_cents = ?5
             WHERE id = 1 AND revision = ?4",
            params![
                format_date(self.opens_on),
                self.source,
                new_revision,
                self.revision,
                self.tax_losses.cents(),
            ],
        )?;
        if changed == 0 {
            return Err(AppError::Conflict {
                entity: "bilan d'ouverture",
                id: "1".to_string(),
            });
        }
        write_lines(conn, &self.lines)?;
        Ok(new_revision)
    }
}

/// Supprime le bilan d'ouverture (lignes comprises) — tant qu'aucun exercice n'est clos.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteOpeningBalance {
    pub revision: i64,
}

impl Command for DeleteOpeningBalance {
    type Output = ();
    const NAME: &'static str = "fiscal.delete_opening_balance";

    fn requires_confirmation(&self) -> bool {
        true
    }

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        require_no_fiscal_year(conn)?;
        require_opening_revision(conn, self.revision)?;
        conn.execute(
            "DELETE FROM opening_balance_lines WHERE opening_balance_id = 1",
            [],
        )?;
        let deleted = conn.execute(
            "DELETE FROM opening_balance WHERE id = 1 AND revision = ?1",
            [self.revision],
        )?;
        if deleted == 0 {
            return Err(AppError::Conflict {
                entity: "bilan d'ouverture",
                id: "1".to_string(),
            });
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// Requêtes
// ---------------------------------------------------------------------------------------------

fn conv_err(e: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
}

/// Le bilan d'ouverture, s'il a été enregistré.
///
/// # Errors
///
/// Erreur de lecture SQLite.
pub fn opening_balance(conn: &Connection) -> Result<Option<OpeningBalanceRecord>, AppError> {
    let head: Option<(String, Option<String>, i64, String, i64)> = conn
        .query_row(
            "SELECT opens_on, source, revision, created_at, tax_losses_cents
             FROM opening_balance WHERE id = 1",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()?;
    let Some((opens_on, source, revision, created_at, tax_losses)) = head else {
        return Ok(None);
    };
    let mut stmt = conn.prepare(
        "SELECT account, label, side, amount_cents FROM opening_balance_lines
         WHERE opening_balance_id = 1 ORDER BY position ASC",
    )?;
    let lines = stmt
        .query_map([], |row| {
            let account: String = row.get(0)?;
            let side: String = row.get(2)?;
            Ok(OpeningBalanceLine {
                account: AccountCode::parse(&account).map_err(conv_err)?,
                label: row.get(1)?,
                side: side.parse::<Side>().map_err(conv_err)?,
                amount: Money::from_cents(row.get(3)?),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Some(OpeningBalanceRecord {
        balance: OpeningBalance {
            opens_on: parse_date(&opens_on).map_err(|e| AppError::Domain(e.to_string()))?,
            source,
            lines,
            tax_losses: Money::from_cents(tax_losses),
        },
        revision,
        created_at: OffsetDateTime::parse(&created_at, &Rfc3339)
            .map_err(|e| AppError::Domain(e.to_string()))?,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Actor, ExecutionContext, Executor, Outcome};
    use crate::store::{Passphrase, Store};
    use time::Month;

    fn test_store(label: &str) -> Store {
        let dir = std::env::temp_dir().join(format!(
            "freeflow-opening-test-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ));
        Store::create(&dir.join("vault.db"), &Passphrase::from("s3cret")).unwrap()
    }

    fn human() -> ExecutionContext {
        ExecutionContext::new(Actor::Human, false)
    }

    fn lines(specs: &[&str]) -> Vec<OpeningBalanceLine> {
        specs.iter().map(|s| s.parse().unwrap()).collect()
    }

    fn record(store: &mut Store) -> i64 {
        let cmd = RecordOpeningBalance {
            opens_on: Date::from_calendar_date(2025, Month::October, 1).unwrap(),
            source: Some("bilan au 30/09/2025, cabinet X".to_string()),
            lines: lines(&[
                "101000:Capital social:C:1000.00",
                "106100:Réserve légale:C:100.00",
                "110000:Report à nouveau:C:2500.00",
                "512000:Banque:D:3600.00",
            ]),
            tax_losses: Money::ZERO,
        };
        match Executor::new(store).execute(&cmd, &human()).unwrap() {
            Outcome::Applied(rev) => rev,
            other => panic!("attendu Applied, obtenu {other:?}"),
        }
    }

    #[test]
    fn recording_then_reading_back_preserves_lines_and_order() {
        let mut store = test_store("record");
        assert!(opening_balance(store.connection()).unwrap().is_none());
        assert_eq!(record(&mut store), 1);

        let rec = opening_balance(store.connection()).unwrap().unwrap();
        assert_eq!(
            rec.balance.opens_on,
            Date::from_calendar_date(2025, Month::October, 1).unwrap()
        );
        assert_eq!(
            rec.balance.source.as_deref(),
            Some("bilan au 30/09/2025, cabinet X")
        );
        assert_eq!(rec.revision, 1);
        let codes: Vec<&str> = rec
            .balance
            .lines
            .iter()
            .map(|l| l.account.as_str())
            .collect();
        assert_eq!(codes, ["101000", "106100", "110000", "512000"]);
        assert_eq!(rec.equity().legal_reserve, Money::from_cents(10_000));
        assert_eq!(rec.equity().retained_earnings, Money::from_cents(250_000));

        // Un second enregistrement est refusé : on modifie, on ne double pas.
        let again = RecordOpeningBalance {
            opens_on: Date::from_calendar_date(2025, Month::October, 1).unwrap(),
            source: None,
            lines: lines(&["101000:Capital:C:10.00", "512000:Banque:D:10.00"]),
            tax_losses: Money::ZERO,
        };
        let err = Executor::new(&mut store)
            .execute(&again, &human())
            .unwrap_err();
        assert!(err.to_string().contains("existe déjà"), "{err}");
    }

    #[test]
    fn an_unbalanced_or_income_statement_balance_is_refused_before_any_write() {
        let mut store = test_store("invalid");
        let unbalanced = RecordOpeningBalance {
            opens_on: Date::from_calendar_date(2025, Month::October, 1).unwrap(),
            source: None,
            lines: lines(&["101000:Capital:C:10.00", "512000:Banque:D:9.00"]),
            tax_losses: Money::ZERO,
        };
        let err = Executor::new(&mut store)
            .execute(&unbalanced, &human())
            .unwrap_err();
        assert!(err.to_string().contains("déséquilibré"), "{err}");
        assert!(opening_balance(store.connection()).unwrap().is_none());

        let with_revenue = RecordOpeningBalance {
            opens_on: Date::from_calendar_date(2025, Month::October, 1).unwrap(),
            source: None,
            lines: lines(&["706000:Ventes:C:10.00", "512000:Banque:D:10.00"]),
            tax_losses: Money::ZERO,
        };
        let err = Executor::new(&mut store)
            .execute(&with_revenue, &human())
            .unwrap_err();
        assert!(err.to_string().contains("compte de bilan"), "{err}");
    }

    #[test]
    fn updating_replaces_lines_in_full_under_optimistic_locking() {
        let mut store = test_store("update");
        record(&mut store);

        let update = UpdateOpeningBalance {
            revision: 1,
            opens_on: Date::from_calendar_date(2025, Month::October, 1).unwrap(),
            source: None,
            lines: lines(&["101000:Capital social:C:1000.00", "512000:Banque:D:1000.00"]),
            tax_losses: Money::ZERO,
        };
        let Outcome::Applied(rev) = Executor::new(&mut store)
            .execute(&update, &human())
            .unwrap()
        else {
            panic!("attendu Applied")
        };
        assert_eq!(rev, 2);
        let rec = opening_balance(store.connection()).unwrap().unwrap();
        assert_eq!(rec.balance.lines.len(), 2);
        assert_eq!(rec.balance.source, None);
        assert_eq!(rec.equity().retained_earnings, Money::ZERO);

        // Rejouer avec la révision périmée est un conflit, pas un écrasement.
        let stale = Executor::new(&mut store)
            .execute(&update, &human())
            .unwrap_err();
        assert!(matches!(stale, AppError::Conflict { .. }), "{stale}");
    }

    #[test]
    fn deleting_removes_the_balance_and_its_lines() {
        let mut store = test_store("delete");
        record(&mut store);
        Executor::new(&mut store)
            .execute(&DeleteOpeningBalance { revision: 1 }, &human())
            .unwrap();
        assert!(opening_balance(store.connection()).unwrap().is_none());
        let orphans: i64 = store
            .connection()
            .query_row("SELECT count(*) FROM opening_balance_lines", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(orphans, 0);

        let missing = Executor::new(&mut store)
            .execute(&DeleteOpeningBalance { revision: 1 }, &human())
            .unwrap_err();
        assert!(missing.to_string().contains("aucun bilan"), "{missing}");
    }

    #[test]
    fn an_agent_only_deposits_a_pending_action() {
        let mut store = test_store("agent");
        let cmd = RecordOpeningBalance {
            opens_on: Date::from_calendar_date(2025, Month::October, 1).unwrap(),
            source: None,
            lines: lines(&["101000:Capital:C:10.00", "512000:Banque:D:10.00"]),
            tax_losses: Money::ZERO,
        };
        let ctx = ExecutionContext::new(
            Actor::Agent {
                session: "s1".to_string(),
            },
            false,
        );
        let outcome = Executor::new(&mut store).execute(&cmd, &ctx).unwrap();
        assert!(matches!(outcome, Outcome::PendingConfirmation(_)));
        assert!(opening_balance(store.connection()).unwrap().is_none());
    }
}
