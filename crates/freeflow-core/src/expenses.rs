//! Dépenses professionnelles : enregistrement, TVA déductible, justificatifs.
//!
//! Le hachage et la copie du fichier justificatif (stockage adressé par contenu, à côté du
//! coffre) sont un souci d'adaptateur, pas de domaine : [`RecordExpense`] ne prend qu'un hash et
//! un nom de fichier déjà calculés — exactement comme aucune autre commande ne touche le système
//! de fichiers depuis `apply`. C'est la CLI (ou la GUI) qui lit le fichier, le hache, et le copie
//! avant de construire la commande.

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::app::{AppError, Command};
use crate::domain::{self, Expense, ExpenseCategory, ExpenseId, Money, VatRate};

fn conv_err(e: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
}

/// Hash SHA-256 hexadécimal d'un justificatif — le même algorithme que le chaînage de factures
/// et le journal d'audit, pour n'avoir qu'une seule primitive d'intégrité dans toute l'appli.
#[must_use]
pub fn hash_receipt(content: &[u8]) -> String {
    hex::encode(Sha256::digest(content))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordExpense {
    pub label: String,
    pub category: ExpenseCategory,
    pub amount: Money,
    pub vat_rate: VatRate,
    pub vat_deductible: Money,
    pub incurred_on: time::Date,
    pub receipt_hash: Option<String>,
    pub receipt_filename: Option<String>,
}

impl Command for RecordExpense {
    type Output = ExpenseId;
    const NAME: &'static str = "expenses.record";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        if self.vat_deductible.cents() > self.amount.cents() {
            return Err(AppError::Domain(
                "la TVA déductible ne peut pas dépasser le montant total de la dépense".to_string(),
            ));
        }
        let expense = Expense {
            id: ExpenseId::new(),
            label: self.label.clone(),
            category: self.category,
            amount: self.amount,
            vat_rate: self.vat_rate,
            vat_deductible: self.vat_deductible,
            incurred_on: self.incurred_on,
            receipt_hash: self.receipt_hash.clone(),
            receipt_filename: self.receipt_filename.clone(),
            created_at: OffsetDateTime::now_utc(),
        };
        insert_expense(conn, &expense)?;
        Ok(expense.id)
    }
}

fn insert_expense(conn: &Connection, expense: &Expense) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO expenses
            (id, label, category, amount_cents, vat_rate, vat_deductible_cents, incurred_on,
             receipt_hash, receipt_filename, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            expense.id.to_string(),
            expense.label,
            expense.category.as_str(),
            expense.amount.cents(),
            expense.vat_rate.as_str(),
            expense.vat_deductible.cents(),
            domain::format_date(expense.incurred_on),
            expense.receipt_hash,
            expense.receipt_filename,
            expense.created_at.format(&Rfc3339)?,
        ],
    )?;
    Ok(())
}

fn row_to_expense(row: &Row) -> rusqlite::Result<Expense> {
    let category: String = row.get("category")?;
    let vat_rate: String = row.get("vat_rate")?;
    let incurred_on: String = row.get("incurred_on")?;
    let created_at: String = row.get("created_at")?;
    Ok(Expense {
        id: row.get::<_, String>("id")?.parse().map_err(conv_err)?,
        label: row.get("label")?,
        category: category.parse().map_err(conv_err)?,
        amount: Money::from_cents(row.get("amount_cents")?),
        vat_rate: vat_rate.parse().map_err(conv_err)?,
        vat_deductible: Money::from_cents(row.get("vat_deductible_cents")?),
        incurred_on: domain::parse_date(&incurred_on).map_err(conv_err)?,
        receipt_hash: row.get("receipt_hash")?,
        receipt_filename: row.get("receipt_filename")?,
        created_at: OffsetDateTime::parse(&created_at, &Rfc3339).map_err(conv_err)?,
    })
}

/// # Errors
pub fn expense_by_id(conn: &Connection, id: ExpenseId) -> Result<Option<Expense>, AppError> {
    conn.query_row(
        "SELECT * FROM expenses WHERE id = ?1",
        [id.to_string()],
        row_to_expense,
    )
    .optional()
    .map_err(AppError::from)
}

/// Les plus récemment engagées d'abord.
///
/// # Errors
pub fn list_expenses(conn: &Connection) -> Result<Vec<Expense>, AppError> {
    let mut stmt =
        conn.prepare("SELECT * FROM expenses ORDER BY incurred_on DESC, created_at DESC")?;
    let rows = stmt.query_map([], row_to_expense)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
}

/// Dépenses engagées dans `[start, end]` (bornes inclusives) — alimente le prévisionnel (lot 11)
/// et la déclaration de TVA.
///
/// # Errors
pub fn expenses_between(
    conn: &Connection,
    start: time::Date,
    end: time::Date,
) -> Result<Vec<Expense>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT * FROM expenses WHERE incurred_on >= ?1 AND incurred_on <= ?2 ORDER BY incurred_on ASC",
    )?;
    let rows = stmt.query_map(
        params![domain::format_date(start), domain::format_date(end)],
        row_to_expense,
    )?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Actor, ExecutionContext, Executor, Outcome};
    use crate::store::Store;
    use time::Month;

    fn test_store(label: &str) -> Store {
        let dir = std::env::temp_dir().join(format!(
            "freeflow-expenses-test-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ));
        Store::open_with_passphrase(&dir.join("vault.db"), "s3cret").unwrap()
    }

    fn human_ctx() -> ExecutionContext {
        ExecutionContext::new(Actor::Human, false)
    }

    fn date(year: i32, month: Month, day: u8) -> time::Date {
        time::Date::from_calendar_date(year, month, day).unwrap()
    }

    fn sample() -> RecordExpense {
        RecordExpense {
            label: "Abonnement hébergement".to_string(),
            category: ExpenseCategory::Software,
            amount: Money::from_cents(12_000),
            vat_rate: VatRate::Standard,
            vat_deductible: Money::from_cents(2_000),
            incurred_on: date(2026, Month::September, 5),
            receipt_hash: Some("deadbeef".to_string()),
            receipt_filename: Some("facture.pdf".to_string()),
        }
    }

    #[test]
    fn recording_an_expense_makes_it_immediately_fetchable_and_listed() {
        let mut store = test_store("record-fetch");
        let Outcome::Applied(id) = Executor::new(&mut store)
            .execute(&sample(), &human_ctx())
            .unwrap()
        else {
            panic!("expected Applied")
        };

        let fetched = expense_by_id(store.connection(), id).unwrap().unwrap();
        assert_eq!(fetched.label, "Abonnement hébergement");
        assert_eq!(fetched.vat_deductible, Money::from_cents(2_000));
        assert_eq!(fetched.receipt_hash.as_deref(), Some("deadbeef"));

        let listed = list_expenses(store.connection()).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, id);
    }

    #[test]
    fn deductible_vat_greater_than_the_total_amount_is_rejected() {
        let mut store = test_store("over-deductible");
        let mut cmd = sample();
        cmd.vat_deductible = Money::from_cents(999_999);
        let err = Executor::new(&mut store)
            .execute(&cmd, &human_ctx())
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("ne peut pas dépasser")));
    }

    #[test]
    fn expenses_between_excludes_dates_outside_the_range() {
        let mut store = test_store("between");
        let mut inside = sample();
        inside.incurred_on = date(2026, Month::September, 15);
        let mut before = sample();
        before.incurred_on = date(2026, Month::August, 31);
        let mut after = sample();
        after.incurred_on = date(2026, Month::October, 1);

        for cmd in [&inside, &before, &after] {
            Executor::new(&mut store)
                .execute(cmd, &human_ctx())
                .unwrap();
        }

        let in_range = expenses_between(
            store.connection(),
            date(2026, Month::September, 1),
            date(2026, Month::September, 30),
        )
        .unwrap();
        assert_eq!(in_range.len(), 1);
        assert_eq!(in_range[0].incurred_on, date(2026, Month::September, 15));
    }

    #[test]
    fn hash_receipt_matches_the_known_sha256_test_vector_for_an_empty_input() {
        assert_eq!(
            hash_receipt(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
