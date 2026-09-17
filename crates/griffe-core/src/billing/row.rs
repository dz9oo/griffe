//! Correspondance ligne SQL <-> types du domaine pour les factures et les encaissements.

use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::app::AppError;
use crate::domain::{
    self, BankTransaction, BankTransactionId, Invoice, InvoiceId, InvoiceLine, InvoiceOrigin,
    InvoiceStatus, InvoiceWriteOff, Money, Payment, PaymentMethod, VatRate, WriteOffId,
};

use super::import::ParsedTransaction;

fn conv_err(e: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
}

/// Alloue le prochain numéro de facture pour `fiscal_year`, dans la transaction courante — un
/// `UPSERT` atomique : sous concurrence, SQLite sérialise les écrivains, donc deux appels
/// concurrents ne peuvent jamais lire la même valeur de départ.
pub(super) fn allocate_invoice_number(
    conn: &Connection,
    fiscal_year: i32,
) -> Result<String, AppError> {
    conn.execute(
        "INSERT INTO invoice_sequences (fiscal_year, last_number) VALUES (?1, 1)
         ON CONFLICT(fiscal_year) DO UPDATE SET last_number = last_number + 1",
        params![fiscal_year],
    )?;
    let last_number: i64 = conn.query_row(
        "SELECT last_number FROM invoice_sequences WHERE fiscal_year = ?1",
        [fiscal_year],
        |row| row.get(0),
    )?;
    Ok(format!("FA-{fiscal_year:04}-{last_number:04}"))
}

/// Hash de la dernière facture insérée (chaîne globale, tous exercices confondus), lu dans la
/// transaction courante.
pub(super) fn last_invoice_hash(conn: &Connection) -> Result<Option<String>, AppError> {
    conn.query_row(
        "SELECT hash FROM invoices ORDER BY sequence DESC LIMIT 1",
        [],
        |row| row.get(0),
    )
    .optional()
    .map_err(AppError::from)
}

pub(super) fn insert_invoice(conn: &Connection, invoice: &Invoice) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO invoices
            (id, sequence, number, client_id, mission_id, status, origin, issued_on, due_on, previous_hash, hash, credited_invoice_id)
         VALUES (?1, (SELECT COALESCE(MAX(sequence), 0) + 1 FROM invoices), ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            invoice.id.to_string(),
            invoice.number,
            invoice.client_id.to_string(),
            invoice.mission_id.map(|id| id.to_string()),
            "issued", // le statut stocké ne varie jamais après émission ; le statut affiché
                      // (payée / en retard / partiellement payée) se calcule à la lecture.
            invoice.origin.as_str(),
            domain::format_date(invoice.issued_on),
            domain::format_date(invoice.due_on),
            invoice.previous_hash,
            invoice.hash,
            invoice.credited_invoice_id.map(|id| id.to_string()),
        ],
    )?;
    for (position, line) in invoice.lines.iter().enumerate() {
        conn.execute(
            "INSERT INTO invoice_lines (invoice_id, position, description, quantity, unit_price_cents, vat_rate)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![invoice.id.to_string(), i64::try_from(position).unwrap_or(i64::MAX), line.description, line.quantity, line.unit_price.cents(), line.vat_rate.as_str()],
        )?;
    }
    Ok(())
}

fn row_to_invoice_without_lines(row: &Row) -> rusqlite::Result<Invoice> {
    let id: String = row.get("id")?;
    let client_id: String = row.get("client_id")?;
    let mission_id: Option<String> = row.get("mission_id")?;
    let origin: String = row.get("origin")?;
    let issued_on: String = row.get("issued_on")?;
    let due_on: String = row.get("due_on")?;
    let credited_invoice_id: Option<String> = row.get("credited_invoice_id")?;

    Ok(Invoice {
        id: id.parse().map_err(conv_err)?,
        number: row.get("number")?,
        client_id: client_id.parse().map_err(conv_err)?,
        mission_id: mission_id
            .map(|s| s.parse())
            .transpose()
            .map_err(conv_err)?,
        lines: Vec::new(),
        status: InvoiceStatus::Issued,
        origin: origin.parse::<InvoiceOrigin>().map_err(conv_err)?,
        issued_on: domain::parse_date(&issued_on).map_err(conv_err)?,
        due_on: domain::parse_date(&due_on).map_err(conv_err)?,
        previous_hash: row.get("previous_hash")?,
        hash: row.get("hash")?,
        credited_invoice_id: credited_invoice_id
            .map(|s| s.parse())
            .transpose()
            .map_err(conv_err)?,
    })
}

fn lines_for_invoice(conn: &Connection, id: InvoiceId) -> Result<Vec<InvoiceLine>, AppError> {
    let mut stmt = conn.prepare("SELECT description, quantity, unit_price_cents, vat_rate FROM invoice_lines WHERE invoice_id = ?1 ORDER BY position ASC")?;
    let rows = stmt.query_map([id.to_string()], |row| {
        let vat_rate: String = row.get("vat_rate")?;
        Ok(InvoiceLine {
            description: row.get("description")?,
            quantity: row.get("quantity")?,
            unit_price: Money::from_cents(row.get("unit_price_cents")?),
            vat_rate: vat_rate.parse::<VatRate>().map_err(conv_err)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
}

pub(super) fn invoice_by_id(conn: &Connection, id: InvoiceId) -> Result<Option<Invoice>, AppError> {
    let Some(mut invoice) = conn
        .query_row(
            "SELECT * FROM invoices WHERE id = ?1",
            [id.to_string()],
            row_to_invoice_without_lines,
        )
        .optional()?
    else {
        return Ok(None);
    };
    invoice.lines = lines_for_invoice(conn, id)?;
    Ok(Some(invoice))
}

pub(super) fn invoice_by_number(
    conn: &Connection,
    number: &str,
) -> Result<Option<Invoice>, AppError> {
    let Some(mut invoice) = conn
        .query_row(
            "SELECT * FROM invoices WHERE number = ?1",
            [number],
            row_to_invoice_without_lines,
        )
        .optional()?
    else {
        return Ok(None);
    };
    invoice.lines = lines_for_invoice(conn, invoice.id)?;
    Ok(Some(invoice))
}

/// Toutes les factures, dans l'ordre de la chaîne (celui de leur émission).
pub(super) fn all_invoices(conn: &Connection) -> Result<Vec<Invoice>, AppError> {
    let mut stmt = conn.prepare("SELECT * FROM invoices ORDER BY sequence ASC")?;
    let rows = stmt.query_map([], row_to_invoice_without_lines)?;
    let mut invoices = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(AppError::from)?;
    for invoice in &mut invoices {
        invoice.lines = lines_for_invoice(conn, invoice.id)?;
    }
    Ok(invoices)
}

pub(super) fn has_credit_note(conn: &Connection, invoice_id: InvoiceId) -> Result<bool, AppError> {
    let count: i64 = conn.query_row(
        "SELECT count(*) FROM invoices WHERE credited_invoice_id = ?1",
        [invoice_id.to_string()],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

pub(super) fn insert_payment(conn: &Connection, payment: &Payment) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO payments (id, invoice_id, amount_cents, received_on, method, bank_transaction_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            payment.id.to_string(),
            payment.invoice_id.to_string(),
            payment.amount.cents(),
            domain::format_date(payment.received_on),
            payment.method.as_str(),
            payment.bank_transaction_id.map(|id| id.to_string()),
        ],
    )?;
    Ok(())
}

/// Tous les paiements, annulés compris — le filtre `voided_at IS NULL` appartient aux calculs
/// (`aged_balance`), jamais à la lecture brute : l'historique doit rester affichable.
pub(super) fn all_payments(conn: &Connection) -> Result<Vec<Payment>, AppError> {
    let mut stmt = conn.prepare("SELECT * FROM payments ORDER BY received_on DESC, id DESC")?;
    let rows = stmt.query_map([], row_to_payment)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
}

pub(super) fn payment_by_id(
    conn: &Connection,
    id: crate::domain::PaymentId,
) -> Result<Option<Payment>, AppError> {
    conn.query_row(
        "SELECT * FROM payments WHERE id = ?1",
        [id.to_string()],
        row_to_payment,
    )
    .optional()
    .map_err(AppError::from)
}

pub(super) fn payments_for_invoice(
    conn: &Connection,
    invoice_id: InvoiceId,
) -> Result<Vec<Payment>, AppError> {
    let mut stmt = conn
        .prepare("SELECT * FROM payments WHERE invoice_id = ?1 ORDER BY received_on ASC, id ASC")?;
    let rows = stmt.query_map([invoice_id.to_string()], row_to_payment)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
}

pub(super) fn mark_payment_voided(
    conn: &Connection,
    id: crate::domain::PaymentId,
    voided_at: &str,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE payments SET voided_at = ?1 WHERE id = ?2 AND voided_at IS NULL",
        params![voided_at, id.to_string()],
    )?;
    Ok(())
}

fn row_to_payment(row: &Row) -> rusqlite::Result<Payment> {
    let id: String = row.get("id")?;
    let invoice_id: String = row.get("invoice_id")?;
    let received_on: String = row.get("received_on")?;
    let method: String = row.get("method")?;
    let bank_transaction_id: Option<String> = row.get("bank_transaction_id")?;
    let voided_at: Option<String> = row.get("voided_at")?;
    Ok(Payment {
        id: id.parse().map_err(conv_err)?,
        invoice_id: invoice_id.parse().map_err(conv_err)?,
        amount: Money::from_cents(row.get("amount_cents")?),
        received_on: domain::parse_date(&received_on).map_err(conv_err)?,
        method: method.parse::<PaymentMethod>().map_err(conv_err)?,
        bank_transaction_id: bank_transaction_id
            .map(|s| s.parse())
            .transpose()
            .map_err(conv_err)?,
        voided_at: voided_at
            .map(|s| {
                time::OffsetDateTime::parse(&s, &time::format_description::well_known::Rfc3339)
            })
            .transpose()
            .map_err(conv_err)?,
    })
}

/// Insère la transaction si elle n'est pas déjà présente (même date, montant, libellé) ;
/// renvoie `true` si une ligne a réellement été ajoutée.
pub(super) fn insert_bank_transaction(
    conn: &Connection,
    tx: &ParsedTransaction,
) -> Result<bool, AppError> {
    // Lot 38 : avec un identifiant de banque, c'est lui qui dédoublonne (le libellé d'un même
    // mouvement diffère entre l'export CSV et l'OFX) ; sans, le triplet date/montant/libellé
    // comme avant. `INSERT OR IGNORE` couvre les deux index uniques.
    let changed = conn.execute(
        "INSERT OR IGNORE INTO bank_transactions
            (id, occurred_on, amount_cents, description, matched_invoice_id, fitid)
         VALUES (?1, ?2, ?3, ?4, NULL, ?5)",
        params![
            BankTransactionId::new().to_string(),
            domain::format_date(tx.occurred_on),
            tx.amount_cents,
            tx.description,
            tx.fitid,
        ],
    )?;
    Ok(changed > 0)
}

pub(crate) fn bank_transaction_by_id(
    conn: &Connection,
    id: BankTransactionId,
) -> Result<Option<BankTransaction>, AppError> {
    conn.query_row(
        "SELECT * FROM bank_transactions WHERE id = ?1",
        [id.to_string()],
        row_to_bank_transaction,
    )
    .optional()
    .map_err(AppError::from)
}

fn row_to_bank_transaction(row: &Row) -> rusqlite::Result<BankTransaction> {
    let id: String = row.get("id")?;
    let occurred_on: String = row.get("occurred_on")?;
    let matched_invoice_id: Option<String> = row.get("matched_invoice_id")?;
    let matched_expense_id: Option<String> = row.get("matched_expense_id")?;
    let settlement_account: Option<String> = row.get("settlement_account")?;
    Ok(BankTransaction {
        id: id.parse().map_err(conv_err)?,
        occurred_on: domain::parse_date(&occurred_on).map_err(conv_err)?,
        amount_cents: row.get("amount_cents")?,
        description: row.get("description")?,
        matched_invoice_id: matched_invoice_id
            .map(|s| s.parse())
            .transpose()
            .map_err(conv_err)?,
        matched_expense_id: matched_expense_id
            .map(|s| s.parse())
            .transpose()
            .map_err(conv_err)?,
        settlement_account: settlement_account
            .map(|s| s.parse())
            .transpose()
            .map_err(conv_err)?,
        settlement_label: row.get("settlement_label")?,
        fitid: row.get("fitid")?,
    })
}

/// Une transaction analysée est-elle déjà en base (lot 38) — par son identifiant de banque s'il
/// en a un, sinon par le triplet date/montant/libellé : la même règle que l'insertion.
pub(super) fn is_already_imported(
    conn: &Connection,
    tx: &ParsedTransaction,
) -> Result<bool, AppError> {
    let count: i64 = match &tx.fitid {
        Some(fitid) => conn.query_row(
            "SELECT count(*) FROM bank_transactions
              WHERE fitid = ?1 OR (occurred_on = ?2 AND amount_cents = ?3 AND description = ?4)",
            params![
                fitid,
                domain::format_date(tx.occurred_on),
                tx.amount_cents,
                tx.description
            ],
            |row| row.get(0),
        )?,
        None => conn.query_row(
            "SELECT count(*) FROM bank_transactions
              WHERE occurred_on = ?1 AND amount_cents = ?2 AND description = ?3",
            params![
                domain::format_date(tx.occurred_on),
                tx.amount_cents,
                tx.description
            ],
            |row| row.get(0),
        )?,
    };
    Ok(count > 0)
}

/// Supprime une transaction importée non rapprochée (lot 38).
pub(super) fn delete_bank_transaction(
    conn: &Connection,
    id: BankTransactionId,
) -> Result<(), AppError> {
    conn.execute(
        "DELETE FROM bank_transactions WHERE id = ?1",
        [id.to_string()],
    )?;
    Ok(())
}

/// Marque la transaction réglée sur un compte de bilan (lot 37) — les gardes (non rapprochée,
/// compte valide) sont celles de `SettleBankTransaction`.
pub(super) fn mark_transaction_settled(
    conn: &Connection,
    id: BankTransactionId,
    account: &crate::domain::SettlementAccount,
    label: &str,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE bank_transactions SET settlement_account = ?1, settlement_label = ?2
          WHERE id = ?3",
        params![account.as_str(), label, id.to_string()],
    )?;
    Ok(())
}

pub(super) fn mark_transaction_matched(
    conn: &Connection,
    id: BankTransactionId,
    invoice_id: InvoiceId,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE bank_transactions SET matched_invoice_id = ?1 WHERE id = ?2",
        params![invoice_id.to_string(), id.to_string()],
    )?;
    Ok(())
}

/// Libère la transaction, quel que soit le côté rapproché (facture ou dépense, lot 33 ;
/// règlement, lot 37).
pub(crate) fn clear_transaction_match(
    conn: &Connection,
    id: BankTransactionId,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE bank_transactions SET matched_invoice_id = NULL, matched_expense_id = NULL,
                settlement_account = NULL, settlement_label = NULL
          WHERE id = ?1",
        [id.to_string()],
    )?;
    Ok(())
}

/// Rapproche la transaction d'une dépense (lot 33) — les gardes (débit, montant, exclusivité)
/// sont celles de `crate::expenses::ReconcileExpense`.
pub(crate) fn mark_transaction_matched_expense(
    conn: &Connection,
    id: BankTransactionId,
    expense_id: crate::domain::ExpenseId,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE bank_transactions SET matched_expense_id = ?1 WHERE id = ?2",
        params![expense_id.to_string(), id.to_string()],
    )?;
    Ok(())
}

/// Le débit rapproché d'une dépense, s'il y en a un (lot 33).
pub(crate) fn bank_transaction_for_expense(
    conn: &Connection,
    expense_id: crate::domain::ExpenseId,
) -> Result<Option<BankTransaction>, AppError> {
    conn.query_row(
        "SELECT * FROM bank_transactions WHERE matched_expense_id = ?1",
        [expense_id.to_string()],
        row_to_bank_transaction,
    )
    .optional()
    .map_err(AppError::from)
}

/// Toutes les transactions importées, les plus récentes d'abord — jusqu'au lot 22, rien ne
/// savait les lister : après un `bank import`, obtenir l'id d'une transaction à rapprocher
/// exigeait une requête SQL manuelle.
pub(super) fn all_bank_transactions(conn: &Connection) -> Result<Vec<BankTransaction>, AppError> {
    let mut stmt =
        conn.prepare("SELECT * FROM bank_transactions ORDER BY occurred_on DESC, id DESC")?;
    let rows = stmt.query_map([], row_to_bank_transaction)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
}

pub(super) fn insert_write_off(conn: &Connection, w: &InvoiceWriteOff) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO invoice_write_offs
            (id, invoice_id, written_off_on, ht_cents, vat_cents, ttc_cents, recovers_vat, retracted_on)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            w.id.to_string(),
            w.invoice_id.to_string(),
            domain::format_date(w.written_off_on),
            w.ht.cents(),
            w.vat.cents(),
            w.ttc.cents(),
            i64::from(w.recovers_vat),
            w.retracted_on.map(domain::format_date),
        ],
    )?;
    Ok(())
}

fn row_to_write_off(row: &Row) -> rusqlite::Result<InvoiceWriteOff> {
    let id: String = row.get("id")?;
    let invoice_id: String = row.get("invoice_id")?;
    let written_off_on: String = row.get("written_off_on")?;
    let recovers_vat: i64 = row.get("recovers_vat")?;
    let retracted_on: Option<String> = row.get("retracted_on")?;
    Ok(InvoiceWriteOff {
        id: id.parse().map_err(conv_err)?,
        invoice_id: invoice_id.parse().map_err(conv_err)?,
        written_off_on: domain::parse_date(&written_off_on).map_err(conv_err)?,
        ht: Money::from_cents(row.get("ht_cents")?),
        vat: Money::from_cents(row.get("vat_cents")?),
        ttc: Money::from_cents(row.get("ttc_cents")?),
        recovers_vat: recovers_vat != 0,
        retracted_on: retracted_on
            .map(|s| domain::parse_date(&s))
            .transpose()
            .map_err(conv_err)?,
    })
}

#[allow(dead_code)]
pub(super) fn write_off_by_id(
    conn: &Connection,
    id: WriteOffId,
) -> Result<Option<InvoiceWriteOff>, AppError> {
    conn.query_row(
        "SELECT * FROM invoice_write_offs WHERE id = ?1",
        [id.to_string()],
        row_to_write_off,
    )
    .optional()
    .map_err(AppError::from)
}

pub(super) fn active_write_off_for(
    conn: &Connection,
    invoice_id: InvoiceId,
) -> Result<Option<InvoiceWriteOff>, AppError> {
    conn.query_row(
        "SELECT * FROM invoice_write_offs WHERE invoice_id = ?1 AND retracted_on IS NULL",
        [invoice_id.to_string()],
        row_to_write_off,
    )
    .optional()
    .map_err(AppError::from)
}

/// Toutes les pertes, y compris rétractées — le grand livre a besoin de l'historique.
#[allow(dead_code)]
pub(super) fn list_write_offs(conn: &Connection) -> Result<Vec<InvoiceWriteOff>, AppError> {
    let mut stmt =
        conn.prepare("SELECT * FROM invoice_write_offs ORDER BY written_off_on ASC, id ASC")?;
    let rows = stmt.query_map([], row_to_write_off)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
}

#[cfg(test)]
pub(super) fn seed_client(conn: &Connection) -> crate::domain::ClientId {
    let id = crate::domain::ClientId::new();
    conn.execute("INSERT INTO clients (id, name, created_at) VALUES (?1, 'Argon Digital', '2026-01-01T00:00:00Z')", [id.to_string()]).unwrap();
    id
}
