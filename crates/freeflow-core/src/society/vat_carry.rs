//! Crédit de TVA à reporter (case 27 de la dernière CA3 déjà déposée).
//!
//! Un fait administratif, pas un solde de bilan : il ancre la case 25 de la période
//! suivante, puis la chaîne se dérive des factures et dépenses du coffre. Saisi une
//! seule fois ; ensuite figé — recoller le stock, c'est réécrire une CA3 déjà déposée.
//! Un agent propose, un humain confirme.

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::app::{AppError, Command};
use crate::domain::{Money, Month};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum VatCarryInError {
    #[error("aucun crédit de TVA repris")]
    NotFound,

    #[error(
        "un crédit de TVA repris existe déjà (après {0}) : il est figé. La suite se déclare \
         période par période, à partir des factures et des dépenses"
    )]
    AlreadyRecorded(String),

    #[error("période invalide {0:?} : attendu AAAA-MM (mois de la dernière CA3 déjà déposée)")]
    InvalidPeriod(String),

    #[error("le crédit à reporter ne peut pas être négatif")]
    NegativeCredit,

    #[error(
        "le crédit de TVA repris est figé : on ne le recollera pas. La suite se déclare sur \
         chaque CA3, à partir des factures et des dépenses"
    )]
    Immutable,
}

impl From<VatCarryInError> for AppError {
    fn from(e: VatCarryInError) -> Self {
        Self::Domain(e.to_string())
    }
}

/// Le crédit repris, tel qu'en base.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VatCarryInRecord {
    /// Période `AAAA-MM` de la dernière CA3 déjà déposée auprès de l'administration.
    pub after_period: String,
    /// Case 27 de cette CA3.
    pub credit: Money,
    pub source: Option<String>,
    pub revision: i64,
    #[serde(with = "crate::domain::serde_date::datetime")]
    pub created_at: OffsetDateTime,
}

/// Analyse une période CA3 (`AAAA-MM`).
///
/// # Errors
///
/// [`VatCarryInError::InvalidPeriod`] si le texte n'est pas un mois calendaire.
pub fn parse_after_period(raw: &str) -> Result<Month, VatCarryInError> {
    let trimmed = raw.trim();
    let (year, month) = trimmed
        .split_once('-')
        .ok_or_else(|| VatCarryInError::InvalidPeriod(raw.to_string()))?;
    if month.len() != 2 {
        return Err(VatCarryInError::InvalidPeriod(raw.to_string()));
    }
    let year: i32 = year
        .parse()
        .map_err(|_| VatCarryInError::InvalidPeriod(raw.to_string()))?;
    let month: u8 = month
        .parse()
        .map_err(|_| VatCarryInError::InvalidPeriod(raw.to_string()))?;
    Month::new(year, month).map_err(|_| VatCarryInError::InvalidPeriod(raw.to_string()))
}

fn period_key(month: Month) -> String {
    crate::fiscal::ca3_period_key(month)
}

fn require_credit(credit: Money) -> Result<(), AppError> {
    if credit.is_negative() {
        return Err(VatCarryInError::NegativeCredit.into());
    }
    Ok(())
}

/// Enregistre le crédit à reporter — une seule fois. Ensuite il est figé
/// ([`VatCarryInError::Immutable`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordVatCarryIn {
    pub after_period: String,
    pub credit: Money,
    #[serde(default)]
    pub source: Option<String>,
}

impl Command for RecordVatCarryIn {
    type Output = i64;
    const NAME: &'static str = "society.record_vat_carry_in";

    fn requires_confirmation(&self) -> bool {
        true
    }

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let month = parse_after_period(&self.after_period)?;
        require_credit(self.credit)?;
        if let Some(existing) = vat_carry_in(conn)? {
            return Err(VatCarryInError::AlreadyRecorded(existing.after_period).into());
        }
        conn.execute(
            "INSERT INTO vat_carry_in (id, after_period, credit_cents, source, revision, created_at)
             VALUES (1, ?1, ?2, ?3, 1, ?4)",
            params![
                period_key(month),
                self.credit.cents(),
                self.source,
                OffsetDateTime::now_utc()
                    .format(&Rfc3339)
                    .map_err(|e| AppError::Domain(e.to_string()))?,
            ],
        )?;
        Ok(1)
    }
}

/// Conservé pour les actions en attente antérieures : le crédit repris est figé, [`apply`]
/// refuse toujours.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateVatCarryIn {
    pub revision: i64,
    pub after_period: String,
    pub credit: Money,
    #[serde(default)]
    pub source: Option<String>,
}

impl Command for UpdateVatCarryIn {
    type Output = i64;
    const NAME: &'static str = "society.update_vat_carry_in";

    fn requires_confirmation(&self) -> bool {
        true
    }

    fn apply(&self, _conn: &Connection) -> Result<Self::Output, AppError> {
        Err(VatCarryInError::Immutable.into())
    }
}

/// Conservé pour les actions en attente antérieures : le crédit repris est figé, [`apply`]
/// refuse toujours.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteVatCarryIn {
    pub revision: i64,
}

impl Command for DeleteVatCarryIn {
    type Output = ();
    const NAME: &'static str = "society.delete_vat_carry_in";

    fn requires_confirmation(&self) -> bool {
        true
    }

    fn apply(&self, _conn: &Connection) -> Result<Self::Output, AppError> {
        Err(VatCarryInError::Immutable.into())
    }
}

/// Le crédit repris, s'il a été enregistré.
///
/// # Errors
///
/// Erreur de lecture SQLite, ou `created_at` illisible.
pub fn vat_carry_in(conn: &Connection) -> Result<Option<VatCarryInRecord>, AppError> {
    let row: Option<(String, i64, Option<String>, i64, String)> = conn
        .query_row(
            "SELECT after_period, credit_cents, source, revision, created_at
             FROM vat_carry_in WHERE id = 1",
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
    let Some((after_period, credit_cents, source, revision, created_at)) = row else {
        return Ok(None);
    };
    Ok(Some(VatCarryInRecord {
        after_period,
        credit: Money::from_cents(credit_cents),
        source,
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

    fn test_store(label: &str) -> Store {
        let dir = std::env::temp_dir().join(format!(
            "freeflow-vat-carry-test-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ));
        Store::create(&dir.join("vault.db"), &Passphrase::from("s3cret")).unwrap()
    }

    fn human() -> ExecutionContext {
        ExecutionContext::new(Actor::Human, false)
    }

    fn agent() -> ExecutionContext {
        ExecutionContext::new(
            Actor::Agent {
                session: "mcp-test".into(),
            },
            false,
        )
    }

    fn applied<T: std::fmt::Debug>(outcome: Outcome<T>) -> T {
        match outcome {
            Outcome::Applied(v) => v,
            other => panic!("expected Applied, got {other:?}"),
        }
    }

    #[test]
    fn recording_then_reading_back_preserves_the_period_and_credit() {
        let mut store = test_store("record");
        assert!(vat_carry_in(store.connection()).unwrap().is_none());
        assert_eq!(
            applied(
                Executor::new(&mut store)
                    .execute(
                        &RecordVatCarryIn {
                            after_period: "2026-08".into(),
                            credit: Money::from_cents(32_400),
                            source: Some("CA3 août, espace impôts".into()),
                        },
                        &human(),
                    )
                    .unwrap()
            ),
            1
        );
        let rec = vat_carry_in(store.connection()).unwrap().unwrap();
        assert_eq!(rec.after_period, "2026-08");
        assert_eq!(rec.credit, Money::from_cents(32_400));
        assert_eq!(rec.source.as_deref(), Some("CA3 août, espace impôts"));
        assert_eq!(rec.revision, 1);
    }

    #[test]
    fn a_second_record_is_refused() {
        let mut store = test_store("dup");
        applied(
            Executor::new(&mut store)
                .execute(
                    &RecordVatCarryIn {
                        after_period: "2026-08".into(),
                        credit: Money::from_cents(32_400),
                        source: None,
                    },
                    &human(),
                )
                .unwrap(),
        );
        let err = Executor::new(&mut store)
            .execute(
                &RecordVatCarryIn {
                    after_period: "2026-07".into(),
                    credit: Money::ZERO,
                    source: None,
                },
                &human(),
            )
            .unwrap_err();
        assert!(err.to_string().contains("existe déjà"), "{err}");
    }

    #[test]
    fn an_invalid_period_or_a_negative_credit_is_refused() {
        let mut store = test_store("invalid");
        let bad_period = Executor::new(&mut store)
            .execute(
                &RecordVatCarryIn {
                    after_period: "août".into(),
                    credit: Money::from_cents(32_400),
                    source: None,
                },
                &human(),
            )
            .unwrap_err();
        assert!(bad_period.to_string().contains("AAAA-MM"), "{bad_period}");
        let negative = Executor::new(&mut store)
            .execute(
                &RecordVatCarryIn {
                    after_period: "2026-08".into(),
                    credit: Money::from_cents(-1),
                    source: None,
                },
                &human(),
            )
            .unwrap_err();
        assert!(negative.to_string().contains("négatif"), "{negative}");
        assert!(vat_carry_in(store.connection()).unwrap().is_none());
    }

    #[test]
    fn a_recorded_credit_is_frozen() {
        let mut store = test_store("frozen");
        applied(
            Executor::new(&mut store)
                .execute(
                    &RecordVatCarryIn {
                        after_period: "2026-08".into(),
                        credit: Money::from_cents(32_400),
                        source: None,
                    },
                    &human(),
                )
                .unwrap(),
        );
        let update = Executor::new(&mut store)
            .execute(
                &UpdateVatCarryIn {
                    revision: 1,
                    after_period: "2026-07".into(),
                    credit: Money::from_cents(10_000),
                    source: Some("corrigé".into()),
                },
                &human(),
            )
            .unwrap_err();
        assert!(update.to_string().contains("figé"), "{update}");
        let delete = Executor::new(&mut store)
            .execute(&DeleteVatCarryIn { revision: 1 }, &human())
            .unwrap_err();
        assert!(delete.to_string().contains("figé"), "{delete}");
        let rec = vat_carry_in(store.connection()).unwrap().unwrap();
        assert_eq!(rec.after_period, "2026-08");
        assert_eq!(rec.credit, Money::from_cents(32_400));
        assert_eq!(rec.revision, 1);
    }

    #[test]
    fn an_agent_only_deposits_a_pending_action() {
        let mut store = test_store("agent");
        let outcome = Executor::new(&mut store)
            .execute(
                &RecordVatCarryIn {
                    after_period: "2026-08".into(),
                    credit: Money::from_cents(32_400),
                    source: None,
                },
                &agent(),
            )
            .unwrap();
        assert!(
            matches!(outcome, Outcome::PendingConfirmation(_)),
            "agent → pending : {outcome:?}"
        );
        assert!(vat_carry_in(store.connection()).unwrap().is_none());
    }
}
