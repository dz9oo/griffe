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
//!
//! Lot 33 : **rapprochement bancaire des dépenses.** Un débit du relevé importé (`bank import`)
//! se rapproche d'une dépense ([`ReconcileExpense`], ou [`RecordExpense`] avec
//! `bank_transaction_id` pour créer la dépense *depuis* le débit), au montant exact — comme un
//! crédit se rapproche d'une facture (`billing::ReconcileTransaction`). Le rapprochement n'est
//! pas un attribut de la dépense mais de la transaction (`bank_transactions.matched_expense_id`,
//! migration `0016`) : il ne touche ni au montant ni à la date de la dépense, donc jamais au
//! résultat figé d'un exercice clos — il n'est pas soumis à la garde « exercice clôturé ». Sa
//! seule conséquence comptable est dans le grand livre dérivé (`crate::ledger`) : une dépense
//! rapprochée n'est plus réputée payée à sa date, son décaissement est daté du relevé, via un
//! compte fournisseur. Une dépense rapprochée garde le montant de son débit
//! ([`ExpensesError::ReconciledAmountLocked`]) ; la supprimer libère la transaction (elle
//! redevient « à rapprocher ») ; `billing::UnreconcileTransaction` libère la transaction sans
//! toucher à la dépense.

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::app::{AppError, Command};
use crate::billing;
use crate::domain::{
    self, BankTransaction, BankTransactionId, Expense, ExpenseCategory, ExpenseId, ExpensePaidBy,
    Money, VatRate,
};

fn conv_err(e: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
}

/// Libellé porté par `AppError::Conflict` pour une dépense — voir `crate::app::revision`.
const EXPENSE_ENTITY: &str = "dépense";

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ExpensesError {
    #[error("dépense introuvable : {0}")]
    NotFound(ExpenseId),

    #[error(
        "la TVA déductible ({requested}) dépasse la TVA contenue dans {amount} au taux {rate} : \
         au plus {max} (montant TTC − montant TTC / (1 + taux))"
    )]
    VatExceedsRate {
        requested: Money,
        amount: Money,
        rate: &'static str,
        max: Money,
    },

    #[error(
        "la dépense du {0} tombe dans un exercice déjà clôturé, dont le résultat a été figé — \
         supprimez d'abord l'exercice s'il n'est qu'un projet, ou rattachez \
         la dépense à l'exercice courant"
    )]
    FiscalYearClosed(String),

    #[error(
        "la dépense du {incurred_on} est antérieure au bilan d'ouverture ({opens_on}) : ce bilan \
         reprend déjà tout ce qui s'est passé avant — une pièce datée avant ce jour a été \
         comptabilisée par le cabinet, pas ici (corrigez la date, ou la date du bilan)"
    )]
    BeforeOpeningBalance {
        incurred_on: String,
        opens_on: String,
    },

    #[error("transaction bancaire introuvable : {0}")]
    TransactionNotFound(BankTransactionId),

    #[error("la transaction {0} est un crédit : seul un débit (sortie d'argent) paie une dépense")]
    NotADebit(BankTransactionId),

    #[error(
        "la transaction {0} est déjà rapprochée — défaites d'abord ce rapprochement \
         si elle visait la mauvaise dépense ou la mauvaise facture"
    )]
    TransactionAlreadyReconciled(BankTransactionId),

    #[error(
        "la dépense {0} est déjà rapprochée d'un débit du relevé — défaites d'abord ce \
         rapprochement"
    )]
    ExpenseAlreadyReconciled(ExpenseId),

    #[error(
        "le montant de la dépense ({expense}) diffère du débit du relevé ({transaction}) : un \
         rapprochement se fait au montant exact"
    )]
    AmountMismatch { expense: Money, transaction: Money },

    #[error(
        "la dépense {0} est rapprochée d'un débit du relevé : son montant est celui du relevé — \
         défaites d'abord le rapprochement pour le modifier"
    )]
    ReconciledAmountLocked(ExpenseId),

    #[error(
        "la dépense {0} est immobilisée : son net (TTC − TVA déductible) est la base amortissable \
         de l'immobilisation — supprimez d'abord l'immobilisation pour la modifier ou la supprimer"
    )]
    Immobilized(ExpenseId),

    #[error(
        "une dépense avancée par l'associé n'est pas un paiement du relevé : elle n'apparaît pas \
         sur le compte de la société"
    )]
    PaidByAssociateNotABankPayment,

    #[error(
        "cette dépense a été avancée par l'associé : elle n'est pas sur le relevé de la société"
    )]
    ExpenseAdvancedByAssociate(ExpenseId),

    #[error("cette dépense est rapprochée d'un débit du relevé : elle a été payée par la société")]
    PaidByLockedWhenReconciled(ExpenseId),
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

/// Lot 36 : un fait daté **avant** le bilan d'ouverture contredit ce bilan (qui reprend tout ce
/// qui précède) — l'audit a montré qu'une telle dépense disparaissait en silence de tout
/// exercice, sans jamais être signalée. Refusée à la saisie plutôt qu'ignorée au calcul.
fn ensure_not_before_opening_balance(conn: &Connection, date: time::Date) -> Result<(), AppError> {
    if let Some(opening) = crate::opening_balance::opening_balance(conn)?
        && date < opening.balance.opens_on
    {
        return Err(ExpensesError::BeforeOpeningBalance {
            incurred_on: domain::format_date(date),
            opens_on: domain::format_date(opening.balance.opens_on),
        }
        .into());
    }
    Ok(())
}

/// La TVA qu'un montant TTC peut contenir au plus à ce taux : `TTC − TTC / (1 + taux)`, soit
/// `TTC × taux / (1 + taux)`, arrondi au centime **supérieur** pour la borne — une facture
/// arrondie au centime peut porter un centime de TVA de plus que le calcul exact. Taux zéro :
/// aucune TVA. Lot 37 : jusqu'ici seule `vat_deductible ≤ amount` était vérifiée, et 150 € de
/// TVA sur 600 € à 20 % passaient.
#[must_use]
pub fn max_deductible_vat(amount: Money, rate: VatRate) -> Money {
    let bps = i128::from(rate.basis_points());
    if bps == 0 || amount.cents() <= 0 {
        return Money::ZERO;
    }
    let ttc = i128::from(amount.cents());
    let (product, divisor) = (ttc * bps, 10_000 + bps);
    let max = product / divisor + i128::from(product % divisor != 0);
    Money::from_cents(i64::try_from(max).unwrap_or(i64::MAX))
}

fn ensure_vat_within_amount(
    vat_deductible: Money,
    amount: Money,
    rate: VatRate,
) -> Result<(), AppError> {
    let max = max_deductible_vat(amount, rate);
    if vat_deductible > max {
        return Err(ExpensesError::VatExceedsRate {
            requested: vat_deductible,
            amount,
            rate: rate.as_str(),
            max,
        }
        .into());
    }
    Ok(())
}

/// Le montant TTC qu'un débit du relevé paie : l'opposé de sa sortie d'argent.
fn debit_amount(tx: &BankTransaction) -> Money {
    Money::from_cents(-tx.amount_cents)
}

/// Les gardes communes d'un rapprochement de dépense : la transaction existe, est un débit,
/// n'est rapprochée de rien, et son montant est exactement `amount`. Renvoie la transaction
/// relue.
fn rapprochable_debit(
    conn: &Connection,
    transaction_id: BankTransactionId,
    amount: Money,
) -> Result<BankTransaction, AppError> {
    let tx = billing::bank_transaction_by_id(conn, transaction_id)?
        .ok_or(ExpensesError::TransactionNotFound(transaction_id))?;
    if tx.is_matched() {
        return Err(ExpensesError::TransactionAlreadyReconciled(transaction_id).into());
    }
    if !tx.is_debit() {
        return Err(ExpensesError::NotADebit(transaction_id).into());
    }
    if debit_amount(&tx) != amount {
        return Err(ExpensesError::AmountMismatch {
            expense: amount,
            transaction: debit_amount(&tx),
        }
        .into());
    }
    Ok(tx)
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
    #[serde(with = "crate::domain::serde_date::date")]
    pub incurred_on: time::Date,
    pub receipt_hash: Option<String>,
    pub receipt_filename: Option<String>,
    /// Lot 33 : le débit du relevé que cette dépense paie — la dépense est créée et rapprochée
    /// dans le même geste. Mêmes gardes que [`ReconcileExpense`] ; `amount` doit être le montant
    /// exact du débit (les façades le pré-remplissent depuis la transaction). `default` pour que
    /// les entrées d'audit antérieures restent lisibles.
    #[serde(default)]
    pub bank_transaction_id: Option<BankTransactionId>,
    /// Bénéficiaire (honoraires : le cabinet, l'avocat…), pour la DAS2 (lot 41).
    #[serde(default)]
    pub supplier: Option<String>,
    /// Qui a payé (lot 63). Défaut `Company` : audit antérieur et CLI sans `--paid-by`.
    #[serde(default)]
    pub paid_by: ExpensePaidBy,
}

impl Command for RecordExpense {
    type Output = ExpenseId;
    const NAME: &'static str = "expenses.record";

    /// Créer une dépense est un geste ordinaire ; la rapprocher d'un débit du relevé, ou
    /// l'enregistrer comme avance de l'associé (lot 63), est derrière confirmation humaine.
    fn requires_confirmation(&self) -> bool {
        self.bank_transaction_id.is_some() || self.paid_by == ExpensePaidBy::Associate
    }

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        ensure_vat_within_amount(self.vat_deductible, self.amount, self.vat_rate)?;
        ensure_outside_closed_fiscal_year(conn, self.incurred_on)?;
        ensure_not_before_opening_balance(conn, self.incurred_on)?;
        if self.paid_by == ExpensePaidBy::Associate && self.bank_transaction_id.is_some() {
            return Err(ExpensesError::PaidByAssociateNotABankPayment.into());
        }
        if let Some(transaction_id) = self.bank_transaction_id {
            rapprochable_debit(conn, transaction_id, self.amount)?;
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
            supplier: self.supplier.clone(),
            paid_by: self.paid_by,
            created_at: OffsetDateTime::now_utc(),
            revision: 1,
        };
        insert_expense(conn, &expense)?;
        if let Some(transaction_id) = self.bank_transaction_id {
            billing::mark_transaction_matched_expense(conn, transaction_id, expense.id)?;
        }
        Ok(expense.id)
    }
}

/// Rapproche un débit du relevé d'une dépense existante — le miroir, côté dépenses, de
/// `billing::ReconcileTransaction` : au montant exact, une transaction par dépense, sans rien
/// créer. Pas de révision : le rapprochement est porté par la transaction, pas par la dépense
/// (voir le commentaire de module), et ses gardes (« déjà rapprochée ») sont sémantiques.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReconcileExpense {
    pub transaction_id: BankTransactionId,
    pub expense_id: ExpenseId,
}

impl Command for ReconcileExpense {
    type Output = ();
    const NAME: &'static str = "expenses.reconcile";

    /// Même barrière que `billing::ReconcileTransaction` : un agent propose, un humain confirme.
    fn requires_confirmation(&self) -> bool {
        true
    }

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let expense = expense_by_id(conn, self.expense_id)?
            .ok_or(ExpensesError::NotFound(self.expense_id))?;
        if expense.paid_by == ExpensePaidBy::Associate {
            return Err(ExpensesError::ExpenseAdvancedByAssociate(self.expense_id).into());
        }
        if billing::bank_transaction_for_expense(conn, self.expense_id)?.is_some() {
            return Err(ExpensesError::ExpenseAlreadyReconciled(self.expense_id).into());
        }
        rapprochable_debit(conn, self.transaction_id, expense.amount)?;
        billing::mark_transaction_matched_expense(conn, self.transaction_id, self.expense_id)?;
        Ok(())
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
    #[serde(with = "crate::domain::serde_date::date")]
    pub incurred_on: time::Date,
    pub receipt_hash: Option<String>,
    pub receipt_filename: Option<String>,
    #[serde(default)]
    pub supplier: Option<String>,
    #[serde(default)]
    pub paid_by: ExpensePaidBy,
}

impl Command for UpdateExpense {
    /// La révision résultante.
    type Output = i64;
    const NAME: &'static str = "expenses.update";

    fn requires_confirmation(&self) -> bool {
        self.paid_by == ExpensePaidBy::Associate
    }

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        ensure_vat_within_amount(self.vat_deductible, self.amount, self.vat_rate)?;
        let current = expense_by_id(conn, self.id)?.ok_or(ExpensesError::NotFound(self.id))?;
        // Les deux dates comptent : sortir une dépense d'un exercice clos (ancienne date) le
        // viderait autant que d'en faire entrer une (nouvelle date).
        ensure_outside_closed_fiscal_year(conn, current.incurred_on)?;
        ensure_outside_closed_fiscal_year(conn, self.incurred_on)?;
        // Seule la nouvelle date compte ici : une dépense saisie avant que le bilan
        // d'ouverture existe doit rester corrigeable (vers une date valide).
        ensure_not_before_opening_balance(conn, self.incurred_on)?;
        let reconciled = billing::bank_transaction_for_expense(conn, self.id)?.is_some();
        // Une dépense rapprochée garde le montant de son débit : le relevé fait foi (lot 33).
        if self.amount != current.amount && reconciled {
            return Err(ExpensesError::ReconciledAmountLocked(self.id).into());
        }
        if self.paid_by != ExpensePaidBy::Company && reconciled {
            return Err(ExpensesError::PaidByLockedWhenReconciled(self.id).into());
        }
        // Une dépense immobilisée (lot 42) a donné sa base amortissable : montant et TVA figés.
        if (self.amount != current.amount || self.vat_deductible != current.vat_deductible)
            && crate::fixed_assets::asset_for_expense(conn, self.id)?.is_some()
        {
            return Err(ExpensesError::Immobilized(self.id).into());
        }
        let new_revision = require_expense_revision(conn, self.id, self.revision)?;
        conn.execute(
            "UPDATE expenses
                SET label = ?1, category = ?2, amount_cents = ?3, vat_rate = ?4,
                    vat_deductible_cents = ?5, incurred_on = ?6, receipt_hash = ?7,
                    receipt_filename = ?8, revision = ?9, supplier = ?12, paid_by = ?13
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
                self.supplier,
                self.paid_by.as_str(),
            ],
        )?;
        Ok(new_revision)
    }
}

/// Joint (ou remplace) le justificatif d'une dépense — lot 39 : une commande distincte
/// d'[`UpdateExpense`], **non soumise à la garde « exercice clôturé »** : une pièce ne change ni
/// le montant ni la date, donc jamais le résultat figé — et la facture du cabinet arrive
/// souvent après la clôture. Le fichier lui-même est archivé par l'adaptateur (hash + copie
/// chiffrée), jamais ici ; `receipt_hash`/`receipt_filename` sont ce qu'il en reste à persister.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachReceipt {
    pub id: ExpenseId,
    /// Révision lue avant modification.
    pub revision: i64,
    pub receipt_hash: String,
    pub receipt_filename: String,
}

impl Command for AttachReceipt {
    /// La révision résultante.
    type Output = i64;
    const NAME: &'static str = "expenses.attach_receipt";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        expense_by_id(conn, self.id)?.ok_or(ExpensesError::NotFound(self.id))?;
        let new_revision = require_expense_revision(conn, self.id, self.revision)?;
        conn.execute(
            "UPDATE expenses SET receipt_hash = ?1, receipt_filename = ?2, revision = ?3
              WHERE id = ?4 AND revision = ?5",
            params![
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
        // Lot 42 : l'immobilisation issue de cette dépense la référence (FK sans cascade) — dit
        // en clair plutôt que laissé à l'erreur de contrainte.
        if crate::fixed_assets::asset_for_expense(conn, self.id)?.is_some() {
            return Err(ExpensesError::Immobilized(self.id).into());
        }
        require_expense_revision(conn, self.id, self.revision)?;
        // Seule référence entrante depuis le lot 33 : le débit du relevé rapproché, qui est
        // libéré (il redevient « à rapprocher ») — le relevé, lui, ne ment pas. Le fichier
        // justificatif archivé reste en place — adressé par contenu, il peut être partagé par
        // une autre dépense, et le supprimer serait de l'IO d'adaptateur de toute façon.
        if let Some(tx) = billing::bank_transaction_for_expense(conn, self.id)? {
            billing::clear_transaction_match(conn, tx.id)?;
        }
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
             receipt_hash, receipt_filename, created_at, revision, supplier, paid_by)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
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
            expense.supplier,
            expense.paid_by.as_str(),
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
        supplier: row.get("supplier")?,
        paid_by: row.get::<_, String>("paid_by")?.parse().map_err(conv_err)?,
        created_at: OffsetDateTime::parse(&created_at, &Rfc3339).map_err(conv_err)?,
        revision: row.get("revision")?,
    })
}

/// Les honoraires (`fees`) d'une **année civile**, cumulés par bénéficiaire (lot 41, DAS2) —
/// `None` regroupe les honoraires sans bénéficiaire renseigné. Montants TTC (c'est ce que la
/// DAS2 déclare), datés de l'engagement faute de date de paiement.
///
/// # Errors
///
/// Erreur de lecture SQLite.
pub fn fees_by_supplier(
    conn: &Connection,
    calendar_year: i32,
) -> Result<Vec<(Option<String>, Money)>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT supplier, sum(amount_cents) FROM expenses
          WHERE category = 'fees' AND incurred_on >= ?1 AND incurred_on <= ?2
          GROUP BY supplier ORDER BY sum(amount_cents) DESC",
    )?;
    let rows = stmt.query_map(
        params![
            format!("{calendar_year:04}-01-01"),
            format!("{calendar_year:04}-12-31")
        ],
        |row| {
            let supplier: Option<String> = row.get(0)?;
            let cents: i64 = row.get(1)?;
            Ok((
                supplier.filter(|s| !s.trim().is_empty()),
                Money::from_cents(cents),
            ))
        },
    )?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
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

/// Le débit du relevé rapproché d'une dépense, s'il y en a un (lot 33).
///
/// # Errors
pub fn bank_transaction_for_expense(
    conn: &Connection,
    id: ExpenseId,
) -> Result<Option<BankTransaction>, AppError> {
    billing::bank_transaction_for_expense(conn, id)
}

/// Une dépense avec son débit rapproché — la vue que `expense show`, `expense.show` et la
/// ressource `griffe://expenses/{référence}` partagent : les champs de la dépense restent au
/// premier niveau du JSON, seule une clé `bank_transaction` s'ajoute (`null` si non rapprochée).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ExpenseDetail {
    #[serde(flatten)]
    pub expense: Expense,
    pub bank_transaction: Option<BankTransaction>,
}

/// # Errors
pub fn expense_detail(conn: &Connection, id: ExpenseId) -> Result<Option<ExpenseDetail>, AppError> {
    let Some(expense) = expense_by_id(conn, id)? else {
        return Ok(None);
    };
    let bank_transaction = billing::bank_transaction_for_expense(conn, id)?;
    Ok(Some(ExpenseDetail {
        expense,
        bank_transaction,
    }))
}

/// Les dépenses rapprochées, indexées par id de dépense : le débit qui les paie (lot 33) —
/// une seule requête, pour le grand livre.
///
/// # Errors
pub fn reconciled_debits(
    conn: &Connection,
) -> Result<std::collections::HashMap<ExpenseId, BankTransaction>, AppError> {
    Ok(billing::list_bank_transactions(conn)?
        .into_iter()
        .filter_map(|t| t.matched_expense_id.map(|id| (id, t)))
        .collect())
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
    use crate::billing::{
        ImportBankTransactions, ParsedTransaction, UnreconcileTransaction, list_bank_transactions,
    };
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
            supplier: None,
            bank_transaction_id: None,
            paid_by: ExpensePaidBy::Company,
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
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("dépasse la TVA contenue")));
    }

    /// Lot 37 : la TVA déductible est bornée par le taux, pas seulement par le TTC — 150 € sur
    /// 600 € à 20 % est refusé (au plus 100 €), 100 € passe ; à taux zéro, rien.
    #[test]
    fn deductible_vat_is_bounded_by_the_rate() {
        assert_eq!(
            max_deductible_vat(Money::from_cents(60_000), VatRate::Standard),
            Money::from_cents(10_000)
        );
        // 119,88 € TTC à 20 % contiennent 19,98 € de TVA exactement.
        assert_eq!(
            max_deductible_vat(Money::from_cents(11_988), VatRate::Standard),
            Money::from_cents(1_998)
        );
        // 10 € à 5,5 % : 0,5213… → borne arrondie au centime supérieur, 0,53 €.
        assert_eq!(
            max_deductible_vat(Money::from_cents(1_000), VatRate::Reduced),
            Money::from_cents(53)
        );
        assert_eq!(
            max_deductible_vat(Money::from_cents(1_000), VatRate::Zero),
            Money::ZERO
        );

        let mut store = test_store("vat-rate-bound");
        let refused = RecordExpense {
            amount: Money::from_cents(60_000),
            vat_deductible: Money::from_cents(15_000),
            ..sample()
        };
        let err = Executor::new(&mut store)
            .execute(&refused, &human_ctx())
            .unwrap_err();
        assert!(err.to_string().contains("au plus 100,00"), "{err}");
        let accepted = RecordExpense {
            amount: Money::from_cents(60_000),
            vat_deductible: Money::from_cents(10_000),
            ..sample()
        };
        assert!(matches!(
            Executor::new(&mut store)
                .execute(&accepted, &human_ctx())
                .unwrap(),
            Outcome::Applied(_)
        ));
        let zero_rate = RecordExpense {
            vat_rate: VatRate::Zero,
            vat_deductible: Money::from_cents(1),
            ..sample()
        };
        assert!(
            Executor::new(&mut store)
                .execute(&zero_rate, &human_ctx())
                .is_err()
        );
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
            supplier: None,
            paid_by: expense.paid_by,
        }
    }

    /// Ligne d'exercice clôturé minimale, insérée directement — le chemin officiel
    /// (`CloseFiscalYear`) exige un profil d'entreprise complet qui n'apporterait rien à ces
    /// tests ; la garde ne regarde que `starts_on`/`ends_on`.
    /// Importe un débit (montant négatif) ou un crédit du relevé et renvoie son id.
    fn import_transaction(
        store: &mut Store,
        on: time::Date,
        amount_cents: i64,
    ) -> BankTransactionId {
        Executor::new(store)
            .execute(
                &ImportBankTransactions {
                    transactions: vec![ParsedTransaction {
                        occurred_on: on,
                        amount_cents,
                        description: format!("CB {amount_cents}"),
                        fitid: None,
                    }],
                },
                &human_ctx(),
            )
            .unwrap();
        list_bank_transactions(store.connection())
            .unwrap()
            .into_iter()
            .find(|t| t.occurred_on == on && t.amount_cents == amount_cents)
            .unwrap()
            .id
    }

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
        assert!(matches!(err, AppError::Domain(msg) if msg.contains("dépasse la TVA contenue")));
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

    // --- Rapprochement bancaire (lot 33). ---

    #[test]
    fn reconciling_an_expense_with_a_debit_of_the_same_amount_links_both_ways() {
        let mut store = test_store("reconcile-ok");
        let id = record(&mut store, &sample()); // 120,00 €
        let tx = import_transaction(&mut store, date(2026, Month::September, 7), -12_000);

        let outcome = Executor::new(&mut store)
            .execute(
                &ReconcileExpense {
                    transaction_id: tx,
                    expense_id: id,
                },
                &human_ctx(),
            )
            .unwrap();
        assert!(matches!(outcome, Outcome::Applied(())));

        let detail = expense_detail(store.connection(), id).unwrap().unwrap();
        assert_eq!(detail.bank_transaction.as_ref().map(|t| t.id), Some(tx));
        let listed = list_bank_transactions(store.connection()).unwrap();
        assert_eq!(listed[0].matched_expense_id, Some(id));
        assert!(listed[0].is_matched());
        assert!(
            crate::billing::unmatched_debits(store.connection())
                .unwrap()
                .is_empty()
        );

        // Une seconde dépense ne peut pas se rapprocher du même débit, ni la même dépense d'un
        // second débit.
        let other = record(&mut store, &sample());
        let err = Executor::new(&mut store)
            .execute(
                &ReconcileExpense {
                    transaction_id: tx,
                    expense_id: other,
                },
                &human_ctx(),
            )
            .unwrap_err();
        assert!(matches!(&err, AppError::Domain(msg) if msg.contains("déjà rapprochée")));
        let tx2 = import_transaction(&mut store, date(2026, Month::September, 8), -12_000);
        let err = Executor::new(&mut store)
            .execute(
                &ReconcileExpense {
                    transaction_id: tx2,
                    expense_id: id,
                },
                &human_ctx(),
            )
            .unwrap_err();
        assert!(matches!(&err, AppError::Domain(msg) if msg.contains("déjà rapprochée")));
    }

    #[test]
    fn a_credit_or_a_different_amount_cannot_be_reconciled_with_an_expense() {
        let mut store = test_store("reconcile-refused");
        let id = record(&mut store, &sample()); // 120,00 €
        let credit = import_transaction(&mut store, date(2026, Month::September, 7), 12_000);
        let err = Executor::new(&mut store)
            .execute(
                &ReconcileExpense {
                    transaction_id: credit,
                    expense_id: id,
                },
                &human_ctx(),
            )
            .unwrap_err();
        assert!(matches!(&err, AppError::Domain(msg) if msg.contains("crédit")));

        let other_amount = import_transaction(&mut store, date(2026, Month::September, 7), -12_001);
        let err = Executor::new(&mut store)
            .execute(
                &ReconcileExpense {
                    transaction_id: other_amount,
                    expense_id: id,
                },
                &human_ctx(),
            )
            .unwrap_err();
        assert!(matches!(&err, AppError::Domain(msg) if msg.contains("montant exact")));

        let err = Executor::new(&mut store)
            .execute(
                &ReconcileExpense {
                    transaction_id: BankTransactionId::new(),
                    expense_id: id,
                },
                &human_ctx(),
            )
            .unwrap_err();
        assert!(matches!(&err, AppError::Domain(msg) if msg.contains("introuvable")));
    }

    #[test]
    fn recording_an_expense_from_a_debit_creates_it_reconciled_and_needs_a_human() {
        let mut store = test_store("record-from-debit");
        let tx = import_transaction(&mut store, date(2026, Month::September, 7), -12_000);
        let mut cmd = sample();
        cmd.bank_transaction_id = Some(tx);

        // Proposée par un agent : action en attente, rien n'est créé.
        let outcome = Executor::new(&mut store)
            .execute(
                &cmd,
                &ExecutionContext::new(
                    Actor::Agent {
                        session: "sess-test".into(),
                    },
                    false,
                ),
            )
            .unwrap();
        assert!(matches!(outcome, Outcome::PendingConfirmation(_)));
        assert!(list_expenses(store.connection()).unwrap().is_empty());
        assert!(
            !sample().requires_confirmation(),
            "sans débit, créer une dépense reste un geste ordinaire"
        );

        let id = record(&mut store, &cmd);
        let detail = expense_detail(store.connection(), id).unwrap().unwrap();
        assert_eq!(detail.bank_transaction.as_ref().map(|t| t.id), Some(tx));

        // Au mauvais montant, rien n'est créé — ni la dépense, ni le lien.
        let tx2 = import_transaction(&mut store, date(2026, Month::September, 8), -9_900);
        let mut wrong = sample();
        wrong.bank_transaction_id = Some(tx2);
        let err = Executor::new(&mut store)
            .execute(&wrong, &human_ctx())
            .unwrap_err();
        assert!(matches!(&err, AppError::Domain(msg) if msg.contains("montant exact")));
        assert_eq!(list_expenses(store.connection()).unwrap().len(), 1);
    }

    #[test]
    fn a_reconciled_expense_keeps_the_statement_amount_until_unreconciled() {
        let mut store = test_store("reconciled-amount-locked");
        let id = record(&mut store, &sample());
        let tx = import_transaction(&mut store, date(2026, Month::September, 7), -12_000);
        Executor::new(&mut store)
            .execute(
                &ReconcileExpense {
                    transaction_id: tx,
                    expense_id: id,
                },
                &human_ctx(),
            )
            .unwrap();

        // Le libellé, la catégorie, la date restent libres…
        let current = expense_by_id(store.connection(), id).unwrap().unwrap();
        let mut update = update_from(&current);
        update.label = "Hébergement (septembre)".to_string();
        update.category = ExpenseCategory::Fees;
        update.incurred_on = date(2026, Month::September, 1);
        Executor::new(&mut store)
            .execute(&update, &human_ctx())
            .unwrap();

        // … pas le montant : le relevé fait foi.
        let current = expense_by_id(store.connection(), id).unwrap().unwrap();
        let mut update = update_from(&current);
        update.amount = Money::from_cents(13_000);
        let err = Executor::new(&mut store)
            .execute(&update, &human_ctx())
            .unwrap_err();
        assert!(matches!(&err, AppError::Domain(msg) if msg.contains("relevé")));

        // Défaire libère le débit sans toucher à la dépense, qui redevient modifiable.
        let outcome = Executor::new(&mut store)
            .execute(&UnreconcileTransaction { transaction_id: tx }, &human_ctx())
            .unwrap();
        assert!(matches!(outcome, Outcome::Applied(None)));
        let detail = expense_detail(store.connection(), id).unwrap().unwrap();
        assert!(detail.bank_transaction.is_none());
        assert_eq!(detail.expense.label, "Hébergement (septembre)");
        assert_eq!(
            crate::billing::unmatched_debits(store.connection())
                .unwrap()
                .len(),
            1
        );
        let current = expense_by_id(store.connection(), id).unwrap().unwrap();
        let mut update = update_from(&current);
        update.amount = Money::from_cents(13_000);
        Executor::new(&mut store)
            .execute(&update, &human_ctx())
            .unwrap();
    }

    #[test]
    fn deleting_a_reconciled_expense_frees_its_debit() {
        let mut store = test_store("delete-frees-debit");
        let id = record(&mut store, &sample());
        let tx = import_transaction(&mut store, date(2026, Month::September, 7), -12_000);
        Executor::new(&mut store)
            .execute(
                &ReconcileExpense {
                    transaction_id: tx,
                    expense_id: id,
                },
                &human_ctx(),
            )
            .unwrap();

        Executor::new(&mut store)
            .execute(&DeleteExpense { id, revision: 1 }, &human_ctx())
            .unwrap();
        assert!(expense_by_id(store.connection(), id).unwrap().is_none());
        let listed = list_bank_transactions(store.connection()).unwrap();
        assert_eq!(listed.len(), 1);
        assert!(
            !listed[0].is_matched(),
            "le relevé ne ment pas : le débit reste, libéré"
        );
    }

    #[test]
    fn reconciling_is_not_blocked_by_a_closed_fiscal_year() {
        // Le rapprochement ne touche ni au montant ni à la date : le résultat figé ne bouge
        // pas, seul le grand livre dérivé date autrement le décaissement.
        let mut store = test_store("reconcile-closed-year");
        let id = record(&mut store, &sample()); // 2026-09-05
        let tx = import_transaction(&mut store, date(2026, Month::October, 2), -12_000);
        seed_closed_year(store.connection(), 2026);
        Executor::new(&mut store)
            .execute(
                &ReconcileExpense {
                    transaction_id: tx,
                    expense_id: id,
                },
                &human_ctx(),
            )
            .unwrap();
        Executor::new(&mut store)
            .execute(&UnreconcileTransaction { transaction_id: tx }, &human_ctx())
            .unwrap();
    }

    #[test]
    fn every_category_round_trips_through_its_text_form() {
        for category in ExpenseCategory::ALL {
            assert_eq!(category.as_str().parse::<ExpenseCategory>(), Ok(category));
        }
        assert_eq!("fees".parse::<ExpenseCategory>(), Ok(ExpenseCategory::Fees));
        assert_eq!(
            "bank_charges".parse::<ExpenseCategory>(),
            Ok(ExpenseCategory::BankCharges)
        );
    }

    /// Lot 36 : une dépense datée avant le bilan d'ouverture contredit ce bilan — refusée à
    /// la saisie (création et changement de date), au lieu de disparaître en silence.
    #[test]
    fn an_expense_dated_before_the_opening_balance_is_refused() {
        use crate::domain::OpeningBalanceLine;
        use crate::opening_balance::RecordOpeningBalance;
        let mut store = test_store("before-opening");
        Executor::new(&mut store)
            .execute(
                &RecordOpeningBalance {
                    opens_on: date(2025, Month::October, 1),
                    source: None,
                    lines: vec![
                        "101000:Capital:C:1000.00"
                            .parse::<OpeningBalanceLine>()
                            .unwrap(),
                        "512000:Banque:D:1000.00"
                            .parse::<OpeningBalanceLine>()
                            .unwrap(),
                    ],
                    tax_losses: Money::ZERO,
                    prior_corporate_tax: None,
                    prior_vat_due: None,
                },
                &human_ctx(),
            )
            .unwrap();
        let before = RecordExpense {
            incurred_on: date(2025, Month::September, 30),
            ..sample()
        };
        let err = Executor::new(&mut store)
            .execute(&before, &human_ctx())
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("antérieure au bilan d'ouverture (2025-10-01)"),
            "{err}"
        );
        assert!(list_expenses(store.connection()).unwrap().is_empty());

        // Le jour même passe ; le déplacer avant le bilan est refusé de la même façon.
        let id = record(
            &mut store,
            &RecordExpense {
                incurred_on: date(2025, Month::October, 1),
                ..sample()
            },
        );
        let current = expense_by_id(store.connection(), id).unwrap().unwrap();
        let moved = UpdateExpense {
            incurred_on: date(2025, Month::September, 1),
            ..update_from(&current)
        };
        let err = Executor::new(&mut store)
            .execute(&moved, &human_ctx())
            .unwrap_err();
        assert!(
            err.to_string().contains("antérieure au bilan d'ouverture"),
            "{err}"
        );
    }

    fn cfe_advance() -> RecordExpense {
        RecordExpense {
            label: "CFE 2025".to_string(),
            category: ExpenseCategory::Taxes,
            amount: Money::from_cents(20_400),
            vat_rate: VatRate::Zero,
            vat_deductible: Money::ZERO,
            incurred_on: date(2026, Month::January, 15),
            receipt_hash: None,
            receipt_filename: None,
            supplier: None,
            bank_transaction_id: None,
            paid_by: ExpensePaidBy::Associate,
        }
    }

    #[test]
    fn an_associate_paid_expense_is_stored_and_not_a_bank_payment() {
        let mut store = test_store("associate-paid");
        let id = record(&mut store, &cfe_advance());
        let got = expense_by_id(store.connection(), id).unwrap().unwrap();
        assert_eq!(got.paid_by, ExpensePaidBy::Associate);
        assert_eq!(got.amount, Money::from_cents(20_400));
        assert_eq!(got.category, ExpenseCategory::Taxes);
    }

    #[test]
    fn associate_and_a_bank_debit_together_are_refused() {
        let mut store = test_store("associate-and-debit");
        let tx = import_transaction(&mut store, date(2026, Month::January, 15), -20_400);
        let mut cmd = cfe_advance();
        cmd.bank_transaction_id = Some(tx);
        let err = Executor::new(&mut store)
            .execute(&cmd, &human_ctx())
            .unwrap_err();
        assert!(
            err.to_string().contains("pas sur le relevé")
                || err.to_string().contains("avancée par l'associé"),
            "{err}"
        );
        assert!(list_expenses(store.connection()).unwrap().is_empty());
    }

    #[test]
    fn reconciling_an_associate_paid_expense_is_refused() {
        let mut store = test_store("reconcile-associate");
        let id = record(&mut store, &cfe_advance());
        let tx = import_transaction(&mut store, date(2026, Month::January, 15), -20_400);
        let err = Executor::new(&mut store)
            .execute(
                &ReconcileExpense {
                    transaction_id: tx,
                    expense_id: id,
                },
                &human_ctx(),
            )
            .unwrap_err();
        assert!(
            err.to_string().contains("avancée par l'associé")
                || err.to_string().contains("pas sur le relevé"),
            "{err}"
        );
    }

    #[test]
    fn an_agent_recording_an_associate_advance_only_deposits_a_pending_action() {
        let mut store = test_store("agent-associate");
        let outcome = Executor::new(&mut store)
            .execute(
                &cfe_advance(),
                &ExecutionContext::new(
                    Actor::Agent {
                        session: "sess-test".into(),
                    },
                    false,
                ),
            )
            .unwrap();
        assert!(matches!(outcome, Outcome::PendingConfirmation(_)));
        assert!(list_expenses(store.connection()).unwrap().is_empty());
        assert!(cfe_advance().requires_confirmation());
        assert!(
            !sample().requires_confirmation(),
            "sans avance ni débit, créer une dépense reste un geste ordinaire"
        );
    }

    #[test]
    fn omitted_paid_by_deserializes_as_company() {
        let mut value = serde_json::to_value(sample()).unwrap();
        value.as_object_mut().expect("object").remove("paid_by");
        let cmd: RecordExpense = serde_json::from_value(value).unwrap();
        assert_eq!(cmd.paid_by, ExpensePaidBy::Company);
    }

    #[test]
    fn flipping_company_to_associate_is_allowed_until_reconciled() {
        let mut store = test_store("flip-paid-by");
        let id = record(&mut store, &sample());
        let current = expense_by_id(store.connection(), id).unwrap().unwrap();
        let mut update = update_from(&current);
        update.paid_by = ExpensePaidBy::Associate;
        Executor::new(&mut store)
            .execute(&update, &human_ctx())
            .unwrap();
        let reread = expense_by_id(store.connection(), id).unwrap().unwrap();
        assert_eq!(reread.paid_by, ExpensePaidBy::Associate);

        let tx = import_transaction(&mut store, date(2026, Month::September, 7), -12_000);
        let company = record(&mut store, &sample());
        Executor::new(&mut store)
            .execute(
                &ReconcileExpense {
                    transaction_id: tx,
                    expense_id: company,
                },
                &human_ctx(),
            )
            .unwrap();
        let reconciled = expense_by_id(store.connection(), company).unwrap().unwrap();
        let mut locked = update_from(&reconciled);
        locked.paid_by = ExpensePaidBy::Associate;
        let err = Executor::new(&mut store)
            .execute(&locked, &human_ctx())
            .unwrap_err();
        assert!(
            err.to_string().contains("rapprochée") || err.to_string().contains("relevé"),
            "{err}"
        );
    }

    #[test]
    fn associate_advances_are_not_cash_burn() {
        let mut store = test_store("associate-burn");
        record(&mut store, &cfe_advance());
        let inputs = crate::forecast::build_forecast_inputs(
            store.connection(),
            date(2026, Month::January, 31),
            Money::ZERO,
        )
        .unwrap();
        assert_eq!(
            inputs.monthly_known_expenses,
            Money::ZERO,
            "une CFE payée de la poche n'est pas un décaissement"
        );
    }
}
