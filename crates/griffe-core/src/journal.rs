//! Écritures conservées : paiements de dépenses, factures de prestation et
//! leurs encaissements, liquidation de TVA d'un mois écoulé, impôt sur les
//! sociétés posé à la clôture.
//!
//! La charge est constatée au jour du paiement : journal `BQ` contre 512, ou
//! journal `AC` contre 455 si l'associé a avancé. Une facture non payée
//! n'entre pas. CGI art. 302 septies A ter A, 1 bis — les dettes peuvent
//! n'être constatées qu'à la clôture. Ce livre n'ouvre pas de compte 401
//! pour une charge non payée.
//!
//! Une facture de prestation est écrite à sa date : 411, 706 et 445881. La
//! TVA passe en 445710 à proportion de l'encaissement (CGI art. 269, 2, c).
//! Une facture déjà écrite n'est pas réécrite : l'avoir est une autre vente.
//!
//! Corriger, c'est une extourne datée du jour de l'écriture d'origine. Les
//! montants déjà écrits ne sont pas modifiés. Une paie saisie pose trois
//! écritures (`PAIE`, `PAIE-NET`, `PAIE-URSSAF`), une fois pour le mois. Le
//! brut du profil n'en pose aucune.

use rusqlite::{Connection, OptionalExtension, params};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use crate::app::AppError;
use crate::billing::compute_totals;
use crate::domain::{
    self, ExpenseId, ExpensePaidBy, FiscalYear, Invoice, InvoiceId, Money, PaymentId, format_date,
    parse_date,
};
use crate::expenses::{self, ExpensesError};
use crate::fixed_assets;
use crate::ledger::{self, ExpensePayment, Journal, LedgerEntry, LedgerLine};

const PAYMENT: &str = "expense_payment";
const REVERSAL: &str = "expense_payment_reversal";
const SALE: &str = "sale";
const SALE_COLLECTION: &str = "sale_collection";
const COLLECTION_REVERSAL: &str = "sale_collection_reversal";
const VAT_LIQUIDATION: &str = "vat_liquidation";
const VAT_LIQUIDATION_REVERSAL: &str = "vat_liquidation_reversal";
const CORPORATE_TAX: &str = "corporate_tax";
const CORPORATE_TAX_REVERSAL: &str = "corporate_tax_reversal";
const PAYROLL: &str = "payroll";
const PAYROLL_REVERSAL: &str = "payroll_reversal";
const PAYROLL_NET: &str = "payroll_net";
const PAYROLL_NET_REVERSAL: &str = "payroll_net_reversal";
const PAYROLL_URSSAF: &str = "payroll_urssaf";
const PAYROLL_URSSAF_REVERSAL: &str = "payroll_urssaf_reversal";

/// Garde d'une écriture dont la date tombe dans un exercice clos.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncGuard {
    /// Un paiement nouveau, ou l'extourne d'un paiement déjà écrit, changerait
    /// le résultat figé : refusé.
    RefuseClosedYear,
    /// Reprise, à l'ouverture du coffre, des faits déjà rapprochés ou avancés
    /// par l'associé. L'écriture est posée même si l'exercice est clos : sans
    /// elle, le livre de cet exercice perdrait la charge.
    Backfill,
}

struct Stored {
    id: String,
    entry: LedgerEntry,
}

/// Pose, extourne ou laisse l'écriture de paiement de `expense_id` selon
/// l'état actuel du fait (rapprochement, avance de l'associé, immobilisation).
///
/// Idempotent : une écriture vivante déjà identique n'est pas réécrite.
///
/// # Errors
///
/// [`ExpensesError::FiscalYearClosed`] si `guard` est
/// [`SyncGuard::RefuseClosedYear`] et que l'écriture à poser ou à extourner
/// est datée dans un exercice clos. Erreur SQLite sinon.
pub fn sync_expense_payment(
    conn: &Connection,
    expense_id: ExpenseId,
    guard: SyncGuard,
) -> Result<(), AppError> {
    let desired = desired_entry(conn, expense_id)?;
    let live = live_payment(conn, &expense_id.to_string())?;
    if let (Some(live), Some(desired)) = (&live, &desired)
        && same_booking(&live.entry, desired)
    {
        return Ok(());
    }
    ensure_open(conn, guard, live.as_ref(), desired.as_ref())?;
    if let Some(live) = &live {
        reverse(conn, live, REVERSAL)?;
    }
    if let Some(desired) = desired {
        if !desired.is_balanced() {
            return Err(AppError::Domain(
                "écriture de paiement déséquilibrée — refusée".to_string(),
            ));
        }
        insert_entry(conn, &desired, PAYMENT, &expense_id.to_string(), None)?;
    }
    Ok(())
}

/// Reprend, une fois, les dépenses déjà payées (relevé ou associé) qui n'ont
/// pas encore d'écriture vivante. Une seconde ouverture ne les réécrit pas.
///
/// # Errors
///
/// Erreur SQLite, ou écriture illisible.
pub fn backfill_expense_payments(conn: &mut Connection) -> Result<(), rusqlite::Error> {
    let tx = conn.transaction()?;
    let mut stmt = tx.prepare(
        "SELECT id FROM expenses
          WHERE paid_by = 'associate'
             OR id IN (
                 SELECT matched_expense_id FROM bank_transactions
                  WHERE matched_expense_id IS NOT NULL
             )",
    )?;
    let ids: Vec<String> = stmt
        .query_map([], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?;
    drop(stmt);
    for id in ids {
        let expense_id = id.parse::<ExpenseId>().map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })?;
        sync_expense_payment(&tx, expense_id, SyncGuard::Backfill).map_err(app_to_sqlite)?;
    }
    tx.commit()?;
    Ok(())
}

/// Pose l'écriture de vente de `invoice_id` si elle n'existe pas encore.
///
/// Une facture déjà écrite reste telle quelle : un changement de libellé ou
/// de nom de client ne la réécrit pas. L'avoir est une autre facture.
///
/// # Errors
///
/// [`crate::billing::BillingError::ExerciseClosed`] si `guard` est
/// [`SyncGuard::RefuseClosedYear`] et que la facture est datée dans un
/// exercice clos. Erreur SQLite sinon.
pub fn sync_sale(
    conn: &Connection,
    invoice_id: InvoiceId,
    guard: SyncGuard,
) -> Result<(), AppError> {
    if live_source(conn, SALE, &invoice_id.to_string())?.is_some() {
        return Ok(());
    }
    let Some(desired) = desired_sale(conn, invoice_id)? else {
        return Ok(());
    };
    refuse_closed_sale(conn, guard, None, Some(&desired))?;
    if !desired.is_balanced() {
        return Err(AppError::Domain(
            "écriture de vente déséquilibrée — refusée".to_string(),
        ));
    }
    insert_entry(conn, &desired, SALE, &invoice_id.to_string(), None)?;
    Ok(())
}

/// Pose, extourne ou laisse l'encaissement de `payment_id`.
///
/// Un encaissement annulé n'a plus d'écriture désirée : l'écriture vivante
/// est extournée à sa date, et la TVA déjà passée en 445710 revient en 445881.
/// Les centimes des autres encaissements ne sont pas recalculés.
///
/// # Errors
///
/// [`crate::billing::BillingError::ExerciseClosed`] si `guard` est
/// [`SyncGuard::RefuseClosedYear`] et que l'écriture à poser ou à extourner
/// est datée dans un exercice clos. Erreur SQLite sinon.
pub fn sync_collection(
    conn: &Connection,
    payment_id: PaymentId,
    guard: SyncGuard,
) -> Result<(), AppError> {
    let desired = desired_collection(conn, payment_id)?;
    let live = live_source(conn, SALE_COLLECTION, &payment_id.to_string())?;
    if let (Some(live), Some(desired)) = (&live, &desired)
        && same_booking(&live.entry, desired)
    {
        return Ok(());
    }
    refuse_closed_sale(conn, guard, live.as_ref(), desired.as_ref())?;
    if let Some(live) = &live {
        reverse(conn, live, COLLECTION_REVERSAL)?;
    }
    if let Some(desired) = desired {
        if !desired.is_balanced() {
            return Err(AppError::Domain(
                "écriture d'encaissement déséquilibrée — refusée".to_string(),
            ));
        }
        insert_entry(
            conn,
            &desired,
            SALE_COLLECTION,
            &payment_id.to_string(),
            None,
        )?;
    }
    Ok(())
}

/// Reprend, une fois, les factures de prestation et les encaissements non
/// annulés qui n'ont pas encore d'écriture vivante. Les encaissements déjà
/// annulés avant cette reprise ne sont pas rejoués : leur net est nul.
///
/// # Errors
///
/// Erreur SQLite, ou écriture illisible.
pub fn backfill_service_bookings(conn: &mut Connection) -> Result<(), rusqlite::Error> {
    let tx = conn.transaction()?;
    let mut invoices = crate::billing::list_invoices(&tx).map_err(app_to_sqlite)?;
    invoices.sort_by_key(|invoice| (invoice.issued_on, invoice.id));
    for invoice in invoices {
        sync_sale(&tx, invoice.id, SyncGuard::Backfill).map_err(app_to_sqlite)?;
    }
    let mut payments = crate::billing::list_payments(&tx).map_err(app_to_sqlite)?;
    payments.retain(|payment| !payment.is_voided());
    payments.sort_by_key(|payment| (payment.received_on, payment.id));
    for payment in payments {
        sync_collection(&tx, payment.id, SyncGuard::Backfill).map_err(app_to_sqlite)?;
    }
    tx.commit()?;
    Ok(())
}

/// Toutes les écritures conservées, extournes comprises. Le grand livre les
/// filtre par exercice et les numérote.
///
/// # Errors
///
/// Erreur SQLite, ou date / journal illisible.
pub fn list_entries(conn: &Connection) -> Result<Vec<LedgerEntry>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, journal, entry_date, piece_ref, piece_date, label
           FROM journal_entries
          ORDER BY entry_date, id",
    )?;
    let heads: Vec<(String, String, String, String, String, String)> = stmt
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(stmt);
    let mut entries = Vec::with_capacity(heads.len());
    for (id, journal, entry_date, piece_ref, piece_date, label) in heads {
        entries.push(load_entry(
            conn,
            &id,
            &journal,
            &entry_date,
            &piece_ref,
            &piece_date,
            &label,
        )?);
    }
    Ok(entries)
}

/// Pose, extourne ou laisse l'OD de liquidation `period_key`.
///
/// Renvoie vrai quand une écriture a été posée ou extournée. Faux quand
/// l'écriture vivante est déjà celle demandée, ou quand il n'y a rien à poser.
///
/// # Errors
///
/// Écriture déséquilibrée, ou erreur SQLite.
#[must_use = "le booléen dit si le livre a changé"]
pub fn sync_vat_liquidation(
    conn: &Connection,
    period_key: &str,
    desired: Option<LedgerEntry>,
) -> Result<bool, AppError> {
    let live = live_source(conn, VAT_LIQUIDATION, period_key)?;
    match (&live, &desired) {
        (Some(live), Some(desired)) if same_booking(&live.entry, desired) => return Ok(false),
        (None, None) => return Ok(false),
        _ => {}
    }
    if let Some(live) = &live {
        reverse(conn, live, VAT_LIQUIDATION_REVERSAL)?;
    }
    if let Some(desired) = desired {
        if !desired.is_balanced() {
            return Err(AppError::Domain(
                "écriture de liquidation de TVA déséquilibrée — refusée".into(),
            ));
        }
        insert_entry(conn, &desired, VAT_LIQUIDATION, period_key, None)?;
    }
    Ok(true)
}

/// Les trois pièces d'une paie du mois, déjà construites par
/// [`crate::ledger::payroll_form`]. Une pièce absente signifie qu'elle est nulle.
pub(crate) struct PayrollPieces {
    pub accrual: Option<LedgerEntry>,
    pub net: Option<LedgerEntry>,
    pub urssaf: Option<LedgerEntry>,
}

/// Paie vivante : jour de la pièce et crédit du compte 431 de cette pièce.
pub(crate) struct BookedPayroll {
    pub on: time::Date,
    pub social: Money,
}

/// Pose, extourne ou laisse les trois écritures de paie du mois `period_key`.
///
/// Renvoie vrai quand une écriture a été posée ou extournée. Faux quand les
/// pièces vivantes sont déjà celles demandées. Une pièce identique n'est pas
/// réécrite. La lecture ne passe pas ici.
///
/// # Errors
///
/// Écriture déséquilibrée, date dans un exercice clos, ou erreur SQLite.
#[must_use = "le booléen dit si le livre a changé"]
pub(crate) fn sync_payroll(
    conn: &Connection,
    period_key: &str,
    pieces: &PayrollPieces,
) -> Result<bool, AppError> {
    for entry in [&pieces.accrual, &pieces.net, &pieces.urssaf]
        .into_iter()
        .filter_map(Option::as_ref)
    {
        if !entry.is_balanced() {
            return Err(AppError::Domain(
                "écriture de paie déséquilibrée — refusée".into(),
            ));
        }
    }
    let lives = [
        live_source(conn, PAYROLL, period_key)?,
        live_source(conn, PAYROLL_NET, period_key)?,
        live_source(conn, PAYROLL_URSSAF, period_key)?,
    ];
    let desired = [
        pieces.accrual.as_ref(),
        pieces.net.as_ref(),
        pieces.urssaf.as_ref(),
    ];
    let kinds = [
        (PAYROLL, PAYROLL_REVERSAL),
        (PAYROLL_NET, PAYROLL_NET_REVERSAL),
        (PAYROLL_URSSAF, PAYROLL_URSSAF_REVERSAL),
    ];
    let mut rewrite = [false; 3];
    for (index, (live, want)) in lives.iter().zip(desired).enumerate() {
        let unchanged = match (live, want) {
            (Some(stored), Some(entry)) => same_booking(&stored.entry, entry),
            (None, None) => true,
            _ => false,
        };
        if unchanged {
            continue;
        }
        ensure_open(conn, SyncGuard::RefuseClosedYear, live.as_ref(), want)?;
        rewrite[index] = true;
    }
    for (index, ((live, want), (kind, reversal_kind))) in
        lives.iter().zip(desired).zip(kinds).enumerate()
    {
        if !rewrite[index] {
            continue;
        }
        if let Some(live) = live {
            reverse(conn, live, reversal_kind)?;
        }
        if let Some(want) = want {
            insert_entry(conn, want, kind, period_key, None)?;
        }
    }
    Ok(rewrite.into_iter().any(|changed| changed))
}

/// Paies vivantes. Le montant est le crédit du compte 431 de la pièce `PAIE`,
/// pas le solde du compte après le règlement, et pas un débit qui n'est pas
/// une paie.
///
/// # Errors
///
/// Erreur SQLite, date illisible, ou crédit hors plafond.
pub(crate) fn booked_payrolls(conn: &Connection) -> Result<Vec<BookedPayroll>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, entry_date
           FROM journal_entries
          WHERE source_kind = ?1
            AND reversed_at IS NULL
            AND reversal_of IS NULL
          ORDER BY entry_date, id",
    )?;
    let heads = stmt
        .query_map([PAYROLL], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(stmt);
    let mut booked = Vec::with_capacity(heads.len());
    for (id, entry_date) in heads {
        let on = parse_date(&entry_date).map_err(|error| {
            AppError::Domain(format!("date de paie illisible ({entry_date}) : {error}"))
        })?;
        booked.push(BookedPayroll {
            on,
            social: payroll_social(conn, &id)?,
        });
    }
    Ok(booked)
}

fn payroll_social(conn: &Connection, entry_id: &str) -> Result<Money, AppError> {
    let mut stmt = conn
        .prepare("SELECT amount_cents FROM journal_lines WHERE entry_id = ?1 AND account = ?2")?;
    let amounts = stmt
        .query_map(
            params![entry_id, ledger::accounts::SOCIAL_DUE.number.as_ref()],
            |row| row.get::<_, i64>(0),
        )?
        .collect::<Result<Vec<_>, _>>()?;
    drop(stmt);
    let mut social = Money::ZERO;
    for cents in amounts {
        if cents >= 0 {
            continue;
        }
        let credit = cents
            .checked_neg()
            .ok_or_else(|| AppError::Domain("crédit 431 de la paie illisible".into()))?;
        social = social
            .checked_add(Money::from_cents(credit))
            .ok_or_else(|| AppError::Domain("crédit 431 de la paie hors plafond".into()))?;
    }
    Ok(social)
}

/// Pose l'écriture d'IS de l'exercice : journal OD, pièce OD-IS, dernier jour,
/// débit 695, crédit 444. Un impôt nul ne pose rien. Une écriture vivante déjà
/// là pour cet exercice n'est pas doublée.
///
/// # Errors
///
/// [`ExpensesError::FiscalYearClosed`] si la date tombe dans un exercice déjà
/// enregistré. Erreur SQLite sinon.
pub fn post_corporate_tax(
    conn: &Connection,
    period: FiscalYear,
    tax: Money,
) -> Result<(), AppError> {
    if tax.cents() <= 0 {
        return Ok(());
    }
    let source_id = corporate_tax_source(period);
    if live_source(conn, CORPORATE_TAX, &source_id)?.is_some() {
        return Ok(());
    }
    let Some(entry) = ledger::corporate_tax_booking(period.end(), tax) else {
        return Ok(());
    };
    ensure_open(conn, SyncGuard::RefuseClosedYear, None, Some(&entry))?;
    insert_entry(conn, &entry, CORPORATE_TAX, &source_id, None)?;
    Ok(())
}

/// Extourne l'écriture d'IS vivante de l'exercice. Pièce `EXT-OD-IS`, même date,
/// libellé d'extourne. Sans écriture vivante, ne fait rien.
///
/// L'exercice doit déjà être retiré de `fiscal_years` : tant que la ligne existe,
/// la garde d'exercice clos refuse la date.
///
/// # Errors
///
/// [`ExpensesError::FiscalYearClosed`] si un exercice enregistré couvre encore
/// la date. Erreur SQLite sinon.
pub fn reverse_corporate_tax(conn: &Connection, period: FiscalYear) -> Result<(), AppError> {
    let Some(live) = live_source(conn, CORPORATE_TAX, &corporate_tax_source(period))? else {
        return Ok(());
    };
    ensure_open(conn, SyncGuard::RefuseClosedYear, Some(&live), None)?;
    reverse(conn, &live, CORPORATE_TAX_REVERSAL)?;
    Ok(())
}

fn corporate_tax_source(period: FiscalYear) -> String {
    format!(
        "{}|{}",
        format_date(period.start()),
        format_date(period.end())
    )
}

fn desired_entry(
    conn: &Connection,
    expense_id: ExpenseId,
) -> Result<Option<LedgerEntry>, AppError> {
    let Some(expense) = expenses::expense_by_id(conn, expense_id)? else {
        return Ok(None);
    };
    let payment = match expense.paid_by {
        ExpensePaidBy::Associate => ExpensePayment::Associate,
        ExpensePaidBy::Company => match bank_date(conn, expense_id)? {
            Some(on) => ExpensePayment::Bank { on },
            None => return Ok(None),
        },
    };
    let asset = fixed_assets::asset_for_expense(conn, expense_id)?;
    let immobilized = asset
        .as_ref()
        .map(|asset| (asset.account.as_str(), asset.label.as_str()));
    Ok(ledger::expense_payment_entry(
        &expense,
        payment,
        immobilized,
    ))
}

fn bank_date(
    conn: &Connection,
    expense_id: ExpenseId,
) -> Result<Option<time::Date>, rusqlite::Error> {
    let Some(text): Option<String> = conn
        .query_row(
            "SELECT occurred_on FROM bank_transactions WHERE matched_expense_id = ?1",
            [expense_id.to_string()],
            |row| row.get(0),
        )
        .optional()?
    else {
        return Ok(None);
    };
    parse_date(&text).map(Some).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })
}

fn desired_sale(conn: &Connection, invoice_id: InvoiceId) -> Result<Option<LedgerEntry>, AppError> {
    let Some(invoice) = crate::billing::invoice_by_id(conn, invoice_id)? else {
        return Ok(None);
    };
    let client = crate::clients::client_by_id(conn, invoice.client_id)?;
    let original = match invoice.credited_invoice_id {
        Some(id) => crate::billing::invoice_by_id(conn, id)?.map(|original| original.number),
        None => None,
    };
    Ok(ledger::service_sale_entry(
        &invoice,
        client.as_ref(),
        original.as_deref(),
    ))
}

fn desired_collection(
    conn: &Connection,
    payment_id: PaymentId,
) -> Result<Option<LedgerEntry>, AppError> {
    let Some(payment) = crate::billing::payment_by_id(conn, payment_id)? else {
        return Ok(None);
    };
    if payment.is_voided() {
        return Ok(None);
    }
    let Some(invoice) = crate::billing::invoice_by_id(conn, payment.invoice_id)? else {
        return Ok(None);
    };
    let client = crate::clients::client_by_id(conn, invoice.client_id)?;
    let (pending, due) = collection_basis(conn, &invoice, &payment)?;
    let vat_moved = ledger::vat_transferred_on_receipt(pending, payment.amount, due);
    Ok(ledger::receipt_entry(
        &payment,
        &invoice.number,
        invoice.client_id,
        client.as_ref(),
        vat_moved,
    ))
}

/// TVA encore sur 445881 et TTC encore dû, avant cet encaissement.
///
/// Les avoirs datés après l'encaissement ne comptent pas : la TVA de cet
/// encaissement était déjà exigible. Les autres encaissements vivants sont
/// lus tels qu'écrits, pas recalculés.
fn collection_basis(
    conn: &Connection,
    invoice: &Invoice,
    payment: &domain::Payment,
) -> Result<(Money, Money), AppError> {
    let totals = compute_totals(&invoice.lines);
    let mut credit_vat = Money::ZERO;
    let mut credit_ttc = Money::ZERO;
    for credit in crate::billing::list_invoices(conn)? {
        if credit.credited_invoice_id != Some(invoice.id) || credit.issued_on > payment.received_on
        {
            continue;
        }
        let credit_totals = compute_totals(&credit.lines);
        credit_vat += credit_totals.total_vat;
        credit_ttc += credit_totals.total_ttc;
    }
    let mut paid = Money::ZERO;
    let mut moved = Money::ZERO;
    for other in crate::billing::payments_for_invoice(conn, invoice.id)? {
        if other.id == payment.id || other.is_voided() {
            continue;
        }
        paid += other.amount;
        moved += vat_on_live_collection(conn, other.id)?;
    }
    let pending = totals.total_vat + credit_vat - moved;
    let pending = if pending.is_negative() {
        Money::ZERO
    } else {
        pending
    };
    Ok((pending, totals.total_ttc + credit_ttc - paid))
}

fn vat_on_live_collection(conn: &Connection, payment_id: PaymentId) -> Result<Money, AppError> {
    let Some(stored) = live_source(conn, SALE_COLLECTION, &payment_id.to_string())? else {
        return Ok(Money::ZERO);
    };
    Ok(stored
        .entry
        .lines
        .iter()
        .filter(|line| {
            line.account.number == ledger::accounts::VAT_PENDING.number && line.amount.cents() > 0
        })
        .map(|line| line.amount)
        .sum())
}

fn ensure_open(
    conn: &Connection,
    guard: SyncGuard,
    live: Option<&Stored>,
    desired: Option<&LedgerEntry>,
) -> Result<(), AppError> {
    if guard == SyncGuard::Backfill {
        return Ok(());
    }
    let dates = live
        .map(|stored| stored.entry.date)
        .into_iter()
        .chain(desired.map(|entry| entry.date));
    for date in dates {
        if date_in_recorded_exercise(conn, date)? {
            return Err(ExpensesError::FiscalYearClosed(format_date(date)).into());
        }
    }
    Ok(())
}

fn refuse_closed_sale(
    conn: &Connection,
    guard: SyncGuard,
    live: Option<&Stored>,
    desired: Option<&LedgerEntry>,
) -> Result<(), AppError> {
    if guard == SyncGuard::Backfill {
        return Ok(());
    }
    let dates = live
        .map(|stored| stored.entry.date)
        .into_iter()
        .chain(desired.map(|entry| entry.date));
    for date in dates {
        if date_in_recorded_exercise(conn, date)? {
            return Err(crate::billing::BillingError::ExerciseClosed.into());
        }
    }
    Ok(())
}

/// Vrai si une clôture enregistrée couvre `date`.
///
/// # Errors
///
/// Erreur SQLite.
pub(crate) fn date_in_recorded_exercise(
    conn: &Connection,
    date: time::Date,
) -> Result<bool, AppError> {
    let covered: i64 = conn.query_row(
        "SELECT count(*) FROM fiscal_years WHERE starts_on <= ?1 AND ends_on >= ?1",
        [format_date(date)],
        |row| row.get(0),
    )?;
    Ok(covered > 0)
}

fn live_payment(conn: &Connection, source_id: &str) -> Result<Option<Stored>, AppError> {
    live_source(conn, PAYMENT, source_id)
}

/// Une OD de liquidation vivante pour `period_key`
/// (`source_kind = vat_liquidation`, ni extournée ni extourne).
///
/// # Errors
///
/// Erreur SQLite.
pub fn vat_liquidation_posted(conn: &Connection, period_key: &str) -> Result<bool, AppError> {
    Ok(live_source(conn, VAT_LIQUIDATION, period_key)?.is_some())
}

fn live_source(
    conn: &Connection,
    source_kind: &str,
    source_id: &str,
) -> Result<Option<Stored>, AppError> {
    let found: Option<(String, String, String, String, String, String)> = conn
        .query_row(
            "SELECT id, journal, entry_date, piece_ref, piece_date, label
               FROM journal_entries
              WHERE source_kind = ?1 AND source_id = ?2
                AND reversed_at IS NULL AND reversal_of IS NULL",
            params![source_kind, source_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .optional()?;
    let Some((id, journal, entry_date, piece_ref, piece_date, label)) = found else {
        return Ok(None);
    };
    let entry = load_entry(
        conn,
        &id,
        &journal,
        &entry_date,
        &piece_ref,
        &piece_date,
        &label,
    )?;
    Ok(Some(Stored { id, entry }))
}

fn load_entry(
    conn: &Connection,
    id: &str,
    journal: &str,
    entry_date: &str,
    piece_ref: &str,
    piece_date: &str,
    label: &str,
) -> Result<LedgerEntry, AppError> {
    let journal = Journal::parse_code(journal).ok_or_else(|| {
        AppError::Domain(format!(
            "journal inconnu dans une écriture conservée : {journal}"
        ))
    })?;
    let date = parse_date(entry_date)
        .map_err(|e| AppError::Domain(format!("date d'écriture illisible ({entry_date}) : {e}")))?;
    let piece_date = parse_date(piece_date)
        .map_err(|e| AppError::Domain(format!("date de pièce illisible ({piece_date}) : {e}")))?;
    let mut stmt = conn.prepare(
        "SELECT account, account_label, amount_cents, aux_number, aux_label, ecriture_lib
           FROM journal_lines WHERE entry_id = ?1 ORDER BY position",
    )?;
    let lines = stmt
        .query_map([id], |row| {
            let number: String = row.get(0)?;
            let account_label: String = row.get(1)?;
            let cents: i64 = row.get(2)?;
            let aux_number: Option<String> = row.get(3)?;
            let aux_label: Option<String> = row.get(4)?;
            let ecriture_lib: Option<String> = row.get(5)?;
            Ok((
                number,
                account_label,
                cents,
                aux_number,
                aux_label,
                ecriture_lib,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(stmt);
    let lines = lines
        .into_iter()
        .map(
            |(number, account_label, cents, aux_number, aux_label, ecriture_lib)| LedgerLine {
                account: ledger::numbered_account(&number, &account_label),
                aux: match (aux_number, aux_label) {
                    (Some(number), Some(label)) => Some(ledger::AuxAccount { number, label }),
                    _ => None,
                },
                amount: domain::Money::from_cents(cents),
                line_label: ecriture_lib,
            },
        )
        .collect();
    Ok(LedgerEntry {
        journal,
        number: 0,
        date,
        piece_ref: piece_ref.to_string(),
        piece_date,
        label: label.to_string(),
        lines,
    })
}

fn same_booking(live: &LedgerEntry, desired: &LedgerEntry) -> bool {
    live.journal == desired.journal
        && live.date == desired.date
        && live.piece_ref == desired.piece_ref
        && live.piece_date == desired.piece_date
        && live.label == desired.label
        && live.lines.len() == desired.lines.len()
        && live.lines.iter().zip(&desired.lines).all(|(left, right)| {
            left.account.number == right.account.number
                && left.amount == right.amount
                && left.line_label == right.line_label
                && left.aux.as_ref().map(|aux| aux.number.as_str())
                    == right.aux.as_ref().map(|aux| aux.number.as_str())
        })
}

fn reverse(conn: &Connection, live: &Stored, reversal_kind: &str) -> Result<(), rusqlite::Error> {
    let now = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    conn.execute(
        "UPDATE journal_entries SET reversed_at = ?1 WHERE id = ?2",
        params![now, live.id],
    )?;
    // La pièce nomme le document extourné (`EXT-…`), pas la ligne d'origine.
    let reversal = ledger::extourne_entry(&live.entry);
    insert_entry(conn, &reversal, reversal_kind, &live.id, Some(&live.id))?;
    Ok(())
}

fn insert_entry(
    conn: &Connection,
    entry: &LedgerEntry,
    source_kind: &str,
    source_id: &str,
    reversal_of: Option<&str>,
) -> Result<(), rusqlite::Error> {
    let id = Uuid::now_v7().to_string();
    let now = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    conn.execute(
        "INSERT INTO journal_entries
            (id, journal, entry_date, piece_ref, piece_date, label,
             source_kind, source_id, reversal_of, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            id,
            entry.journal.code(),
            format_date(entry.date),
            entry.piece_ref,
            format_date(entry.piece_date),
            entry.label,
            source_kind,
            source_id,
            reversal_of,
            now,
        ],
    )?;
    for (position, line) in entry.lines.iter().enumerate() {
        let position = i64::try_from(position)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        let (aux_number, aux_label) = line.aux.as_ref().map_or((None, None), |aux| {
            (Some(aux.number.as_str()), Some(aux.label.as_str()))
        });
        conn.execute(
            "INSERT INTO journal_lines
                (entry_id, position, account, account_label, amount_cents, aux_number, aux_label,
                 ecriture_lib)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                id,
                position,
                line.account.number.as_ref(),
                line.account.label.as_ref(),
                line.amount.cents(),
                aux_number,
                aux_label,
                line.line_label,
            ],
        )?;
    }
    Ok(())
}

fn app_to_sqlite(error: AppError) -> rusqlite::Error {
    match error {
        AppError::Sqlite(error) => error,
        other => rusqlite::Error::ToSqlConversionFailure(Box::new(other)),
    }
}
