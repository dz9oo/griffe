//! Dépenses professionnelles : enregistrement, modification, suppression, TVA déductible,
//! justificatifs.
//!
//! Le hachage et la copie du fichier justificatif (stockage adressé par contenu, à côté du
//! coffre) sont un souci d'adaptateur, pas de domaine : [`RecordExpense`] ne prend qu'un hash et
//! un nom de fichier déjà calculés — exactement comme aucune autre commande ne touche le système
//! de fichiers depuis `apply`. C'est la CLI (ou la GUI) qui lit le fichier, le hache, et le copie
//! avant de construire la commande.
//!
//! Lot 21 : une dépense est mutable ([`UpdateExpense`], état complet — jamais un patch, doctrine
//! du lot 15) et supprimable réellement ([`DeleteExpense`]) — rien dans le schéma ne référence
//! `expenses` (aucune FK entrante), donc pas de régime d'archivage. La vraie barrière est
//! ailleurs : **une dépense dont la date tombe dans un exercice clôturé (lot 20) ne se crée, ne
//! se modifie et ne se supprime plus** ([`ExpensesError::FiscalYearClosed`]). Le snapshot figé
//! par `CloseFiscalYear` a été calculé sur ces lignes-là ; les faire bouger après coup ferait
//! silencieusement diverger la comptabilité vivante de ce qui a été déclaré. L'échappatoire,
//! tant que l'exercice n'est qu'un projet non approuvé : `freeflow year rm` d'abord, corriger,
//! re-clore. Un exercice approuvé, lui, est définitif — comme la liasse qu'il a produite.

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::app::{AppError, Command};
use crate::domain::{self, Expense, ExpenseCategory, ExpenseId, Money, VatRate};

fn conv_err(e: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
}

/// Libellé porté par `AppError::Conflict` pour une dépense — voir `crate::app::revision`.
const EXPENSE_ENTITY: &str = "dépense";

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ExpensesError {
    #[error("dépense introuvable : {0}")]
    NotFound(ExpenseId),

    #[error("la TVA déductible ne peut pas dépasser le montant total de la dépense")]
    VatExceedsAmount,

    #[error(
        "la dépense du {0} tombe dans un exercice déjà clôturé, dont le résultat a été figé — \
         supprimez d'abord l'exercice s'il n'est qu'un projet (freeflow year rm), ou rattachez \
         la dépense à l'exercice courant"
    )]
    FiscalYearClosed(String),
}

impl From<ExpensesError> for AppError {
    fn from(e: ExpensesError) -> Self {
        Self::Domain(e.to_string())
    }
}

fn require_expense_revision(
    conn: &Connection,
    id: ExpenseId,
    expected: i64,
) -> Result<i64, AppError> {
    let id_str = id.to_string();
    let current = crate::app::revision::current_revision(conn, "expenses", &id_str)?
        .ok_or(ExpensesError::NotFound(id))?;
    crate::app::revision::require_revision(current, expected, EXPENSE_ENTITY, &id_str)
}

/// Refuse toute écriture sur une dépense datée dans la période d'un exercice clôturé (une ligne
/// de `fiscal_years`, projet ou approuvé — voir le commentaire de module).
fn ensure_outside_closed_fiscal_year(conn: &Connection, date: time::Date) -> Result<(), AppError> {
    let formatted = domain::format_date(date);
    let covered: i64 = conn.query_row(
        "SELECT count(*) FROM fiscal_years WHERE starts_on <= ?1 AND ends_on >= ?1",
        [&formatted],
        |row| row.get(0),
    )?;
    if covered > 0 {
        return Err(ExpensesError::FiscalYearClosed(formatted).into());
    }
    Ok(())
}

fn ensure_vat_within_amount(vat_deductible: Money, amount: Money) -> Result<(), AppError> {
    if vat_deductible.cents() > amount.cents() {
        return Err(ExpensesError::VatExceedsAmount.into());
    }
    Ok(())
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
        ensure_vat_within_amount(self.vat_deductible, self.amount)?;
        ensure_outside_closed_fiscal_year(conn, self.incurred_on)?;
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
            revision: 1,
        };
        insert_expense(conn, &expense)?;
        Ok(expense.id)
    }
}

/// État complet d'une dépense — pas de patch, même doctrine que `UpdateClient` (lot 15) : la
/// sémantique « champ omis = champ conservé » est offerte par les façades, qui relisent la
/// dépense et renvoient l'état entier, pour que l'audit garde une image complète. Les champs de
/// justificatif voyagent comme le reste de l'état : y mettre le hash/nom relus reconduit le
/// justificatif existant, un nouveau couple le remplace (fichier déjà copié par l'adaptateur),
/// `None` le détache — sans jamais toucher au fichier archivé, qui est adressé par contenu et
/// peut être partagé par une autre dépense.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateExpense {
    pub id: ExpenseId,
    /// Révision lue avant modification.
    pub revision: i64,
    pub label: String,
    pub category: ExpenseCategory,
    pub amount: Money,
    pub vat_rate: VatRate,
    pub vat_deductible: Money,
    pub incurred_on: time::Date,
    pub receipt_hash: Option<String>,
    pub receipt_filename: Option<String>,
}

impl Command for UpdateExpense {
    /// La révision résultante.
    type Output = i64;
    const NAME: &'static str = "expenses.update";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        ensure_vat_within_amount(self.vat_deductible, self.amount)?;
        let current = expense_by_id(conn, self.id)?.ok_or(ExpensesError::NotFound(self.id))?;
        // Les deux dates comptent : sortir une dépense d'un exercice clos (ancienne date) le
        // viderait autant que d'en faire entrer une (nouvelle date).
        ensure_outside_closed_fiscal_year(conn, current.incurred_on)?;
        ensure_outside_closed_fiscal_year(conn, self.incurred_on)?;
        let new_revision = require_expense_revision(conn, self.id, self.revision)?;
        conn.execute(
            "UPDATE expenses
                SET label = ?1, category = ?2, amount_cents = ?3, vat_rate = ?4,
                    vat_deductible_cents = ?5, incurred_on = ?6, receipt_hash = ?7,
                    receipt_filename = ?8, revision = ?9
              WHERE id = ?10 AND revision = ?11",
            params![
                self.label,
                self.category.as_str(),
                self.amount.cents(),
                self.vat_rate.as_str(),
                self.vat_deductible.cents(),
                domain::format_date(self.incurred_on),
                self.receipt_hash,
                self.receipt_filename,
                new_revision,
                self.id.to_string(),
                self.revision,
            ],
        )?;
        Ok(new_revision)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteExpense {
    pub id: ExpenseId,
    pub revision: i64,
}

impl Command for DeleteExpense {
    type Output = ();
    const NAME: &'static str = "expenses.delete";

    /// Destructeur réel : un agent MCP ne doit jamais le déclencher sans validation humaine
    /// explicite.
    fn requires_confirmation(&self) -> bool {
        true
    }

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let current = expense_by_id(conn, self.id)?.ok_or(ExpensesError::NotFound(self.id))?;
        ensure_outside_closed_fiscal_year(conn, current.incurred_on)?;
        require_expense_revision(conn, self.id, self.revision)?;
        // Rien ne référence une dépense dans le schéma (aucune FK entrante) : la suppression est
        // réelle, sans tableau de références à consulter. Le fichier justificatif archivé, lui,
        // reste en place — adressé par contenu, il peut être partagé par une autre dépense, et
        // le supprimer serait de l'IO d'adaptateur de toute façon.
        conn.execute(
            "DELETE FROM expenses WHERE id = ?1 AND revision = ?2",
            params![self.id.to_string(), self.revision],
        )?;
        Ok(())
    }
}

fn insert_expense(conn: &Connection, expense: &Expense) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO expenses
            (id, label, category, amount_cents, vat_rate, vat_deductible_cents, incurred_on,
             receipt_hash, receipt_filename, created_at, revision)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
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
            expense.revision,
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
        revision: row.get("revision")?,
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
    use crate::store::{Passphrase, Store};
    use time::Month;

    fn test_store(label: &str) -> Store {
        let dir = std::env::temp_dir().join(format!(
            "freeflow-expenses-test-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ));
        Store::create(&dir.join("vault.db"), &Passphrase::from("s3cret")).unwrap()
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

    fn record(store: &mut Store, cmd: &RecordExpense) -> ExpenseId {
        let Outcome::Applied(id) = Executor::new(store).execute(cmd, &human_ctx()).unwrap() else {
            panic!("expected Applied")
        };
        id
    }

    fn update_from(expense: &Expense) -> UpdateExpense {
        UpdateExpense {
            id: expense.id,
            revision: expense.revision,
            label: expense.label.clone(),
            category: expense.category,
            amount: expense.amount,
            vat_rate: expense.vat_rate,
            vat_deductible: expense.vat_deductible,
            incurred_on: expense.incurred_on,
            receipt_hash: expense.receipt_hash.clone(),
            receipt_filename: expense.receipt_filename.clone(),
        }
    }

    /// Ligne d'exercice clôturé minimale, insérée directement — le chemin officiel
    /// (`CloseFiscalYear`) exige un profil d'entreprise complet qui n'apporterait rien à ces
    /// tests ; la garde ne regarde que `starts_on`/`ends_on`.
    fn seed_closed_year(conn: &Connection, year: i32) {
        conn.execute(
            "INSERT INTO fiscal_years
                (id, starts_on, ends_on, revenue_ht_cents, expenses_cents,
                 director_remuneration_cents, result_before_tax_cents, corporate_tax_cents,
                 net_result_cents, retained_earnings_cents, created_at)
             VALUES (?1, ?2, ?3, 0, 0, 0, 0, 0, 0, 0, '2026-01-01T00:00:00Z')",
            params![
                crate::domain::FiscalYearId::new().to_string(),
                format!("{year}-01-01"),
                format!("{year}-12-31"),
            ],
        )
        .unwrap();
    }

    #[test]
    fn updating_an_expense_rewrites_its_full_state_and_bumps_the_revision() {
        let mut store = test_store("update");
        let id = record(&mut store, &sample());
        let current = expense_by_id(store.connection(), id).unwrap().unwrap();
        assert_eq!(current.revision, 1);

        let mut update = update_from(&current);
        update.label = "Hébergement (annuel)".to_string();
        update.amount = Money::from_cents(24_000);
        update.vat_deductible = Money::from_cents(4_000);
        update.receipt_hash = None;
        update.receipt_filename = None;
        let Outcome::Applied(new_revision) = Executor::new(&mut store)
            .execute(&update, &human_ctx())
            .unwrap()
        else {
            panic!("expected Applied")
        };
        assert_eq!(new_revision, 2);

        let reread = expense_by_id(store.connection(), id).unwrap().unwrap();
        assert_eq!(reread.label, "Hébergement (annuel)");
        assert_eq!(reread.amount, Money::from_cents(24_000));
        assert_eq!(reread.receipt_hash, None, "le justificatif a été détaché");
        assert_eq!(reread.revision, 2);
    }

    #[test]
    fn updating_with_a_stale_revision_is_a_conflict() {
        let mut store = test_store("update-conflict");
        let id = record(&mut store, &sample());
        let current = expense_by_id(store.connection(), id).unwrap().unwrap();

        let mut first = update_from(&current);
        first.label = "Premier gagnant".to_string();
        Executor::new(&mut store)
            .execute(&first, &human_ctx())
            .unwrap();

        let mut second = update_from(&current); // révision périmée (toujours 1)
        second.label = "Écrasement silencieux".to_string();
        let err = Executor::new(&mut store)
            .execute(&second, &human_ctx())
            .unwrap_err();
        assert!(matches!(
            err,
            AppError::Conflict {
                entity: "dépense",
                ..
            }
        ));
        let reread = expense_by_id(store.connection(), id).unwrap().unwrap();
        assert_eq!(reread.label, "Premier gagnant");
    }

    #[test]
    fn updating_cannot_make_deductible_vat_exceed_the_amount() {
        let mut store = test_store("update-vat");
        let id = record(&mut store, &sample());
        let current = expense_by_id(store.connection(), id).unwrap().unwrap();
        let mut update = update_from(&current);
        update.vat_deductible = Money::from_cents(999_999);
        let err = Executor::new(&mut store)
            .execute(&update, &human_ctx())
            .unwrap_err();
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("ne peut pas dépasser")));
    }

    #[test]
    fn deleting_an_expense_removes_it_for_good() {
        let mut store = test_store("delete");
        let id = record(&mut store, &sample());
        Executor::new(&mut store)
            .execute(&DeleteExpense { id, revision: 1 }, &human_ctx())
            .unwrap();
        assert!(expense_by_id(store.connection(), id).unwrap().is_none());
        assert!(list_expenses(store.connection()).unwrap().is_empty());
    }

    #[test]
    fn an_agent_deleting_an_expense_only_files_a_pending_action() {
        let mut store = test_store("delete-agent");
        let id = record(&mut store, &sample());
        let outcome = Executor::new(&mut store)
            .execute(
                &DeleteExpense { id, revision: 1 },
                &ExecutionContext::new(
                    Actor::Agent {
                        session: "sess-test".into(),
                    },
                    false,
                ),
            )
            .unwrap();
        assert!(matches!(outcome, Outcome::PendingConfirmation(_)));
        assert!(
            expense_by_id(store.connection(), id).unwrap().is_some(),
            "la dépense ne doit pas bouger tant qu'un humain n'a pas confirmé"
        );
    }

    #[test]
    fn an_expense_inside_a_closed_fiscal_year_can_no_longer_change() {
        let mut store = test_store("closed-year");
        let id = record(&mut store, &sample()); // 2026-09-05
        seed_closed_year(store.connection(), 2026);

        // Ni la modifier…
        let current = expense_by_id(store.connection(), id).unwrap().unwrap();
        let mut update = update_from(&current);
        update.label = "Libellé corrigé".to_string();
        let err = Executor::new(&mut store)
            .execute(&update, &human_ctx())
            .unwrap_err();
        assert!(matches!(&err, AppError::Domain(msg) if msg.contains("exercice")));

        // … ni la supprimer…
        let err = Executor::new(&mut store)
            .execute(&DeleteExpense { id, revision: 1 }, &human_ctx())
            .unwrap_err();
        assert!(matches!(&err, AppError::Domain(msg) if msg.contains("exercice")));

        // … ni en dater une nouvelle dedans…
        let err = Executor::new(&mut store)
            .execute(&sample(), &human_ctx())
            .unwrap_err();
        assert!(matches!(&err, AppError::Domain(msg) if msg.contains("exercice")));

        // … ni y faire entrer une dépense d'un exercice encore ouvert.
        let mut open = sample();
        open.incurred_on = date(2027, Month::February, 1);
        let open_id = record(&mut store, &open);
        let open_expense = expense_by_id(store.connection(), open_id).unwrap().unwrap();
        let mut move_in = update_from(&open_expense);
        move_in.incurred_on = date(2026, Month::June, 1);
        let err = Executor::new(&mut store)
            .execute(&move_in, &human_ctx())
            .unwrap_err();
        assert!(matches!(&err, AppError::Domain(msg) if msg.contains("exercice")));
    }
}
