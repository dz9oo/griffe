//! Export du **Fichier des Écritures Comptables** (FEC) d'un exercice — art. A. 47 A-1 du livre
//! des procédures fiscales (lot 28).
//!
//! `FreeFlow` ne tient pas de grand livre : le FEC est *dérivé* des faits du domaine, qui sont déjà
//! immuables ou contre-écrits (factures et avoirs, encaissements et leurs annulations, dépenses),
//! selon un plan de comptes PCG minimal et fixe (voir [`accounts`]). Trois journaux : `VE`
//! (ventes), `AC` (achats), `BQ` (banque). Chaque écriture est équilibrée par construction — une
//! ligne porte un montant *signé* (débit positif, crédit négatif) et l'écriture est la somme
//! nulle de ses lignes ; un avoir, dont les lignes sont négatives, se retourne donc tout seul.
//!
//! Le format suit le BOI-CF-IOR-60-40-20 : 18 colonnes dans l'ordre imposé, séparateur `|`,
//! dates en `AAAAMMJJ`, montants avec la virgule décimale et sans séparateur de milliers,
//! encodage UTF-8, nom de fichier `<SIREN>FEC<AAAAMMJJ>.txt` daté de la clôture. Les colonnes de
//! lettrage et de devise restent vides (euro seul ; le lettrage n'est pas modélisé).
//!
//! Limites assumées, dites dans le fichier lui-même par les libellés : les dépenses sont réputées
//! payées à leur date (le domaine enregistre un « montant TTC réellement payé », il n'y a pas de
//! compte fournisseur), l'équipement est passé en charge (606300) sans seuil d'immobilisation, la
//! rémunération du dirigeant et l'IS ne sont pas des écritures (ils ne sont pas saisis comme
//! faits datés). Le FEC produit est un **export pour l'expert-comptable**, qui reste maître des
//! écritures définitives.

use std::collections::HashMap;

use rusqlite::Connection;
use time::Date;

use crate::app::AppError;
use crate::billing::{compute_totals, list_invoices, list_payments};
use crate::clients::list_clients;
use crate::company::{CompanyProfile, company_profile};
use crate::domain::{
    Client, Expense, ExpenseCategory, FiscalYear, FiscalYearEnd, Invoice, Money, Payment,
    PaymentMethod, Siren,
};
use crate::expenses::list_expenses;
use crate::fiscal_year::fiscal_year_ending_in;

/// Séparateur de champs — l'un des deux admis (`|` ou tabulation).
pub const SEPARATOR: char = '|';

/// Les 18 colonnes, dans l'ordre imposé par l'art. A. 47 A-1 (comptabilité informatisée, régime
/// réel, colonnes `Debit`/`Credit`).
pub const HEADER: [&str; 18] = [
    "JournalCode",
    "JournalLib",
    "EcritureNum",
    "EcritureDate",
    "CompteNum",
    "CompteLib",
    "CompAuxNum",
    "CompAuxLib",
    "PieceRef",
    "PieceDate",
    "EcritureLib",
    "Debit",
    "Credit",
    "EcritureLet",
    "DateLet",
    "ValidDate",
    "Montantdevise",
    "Idevise",
];

/// Journal comptable — l'ordre des variantes est l'ordre de tri à date égale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Journal {
    /// `VE` : factures et avoirs.
    Sales,
    /// `AC` : dépenses.
    Purchases,
    /// `BQ` : encaissements et leurs annulations.
    Bank,
}

impl Journal {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Sales => "VE",
            Self::Purchases => "AC",
            Self::Bank => "BQ",
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Sales => "Ventes",
            Self::Purchases => "Achats",
            Self::Bank => "Banque",
        }
    }
}

/// Un compte du plan comptable général, numéro et libellé.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Account {
    pub number: &'static str,
    pub label: &'static str,
}

/// Le plan de comptes minimal du FEC — fixe, sans paramétrage : un compte par fait du domaine.
pub mod accounts {
    use super::Account;

    pub const CLIENTS: Account = Account {
        number: "411000",
        label: "Clients",
    };
    pub const BANK: Account = Account {
        number: "512000",
        label: "Banque",
    };
    pub const SERVICES: Account = Account {
        number: "706000",
        label: "Prestations de services",
    };
    pub const VAT_COLLECTED: Account = Account {
        number: "445710",
        label: "TVA collectée",
    };
    pub const VAT_DEDUCTIBLE: Account = Account {
        number: "445660",
        label: "TVA déductible sur autres biens et services",
    };
    pub const SOFTWARE: Account = Account {
        number: "651000",
        label: "Redevances pour logiciels et licences",
    };
    pub const SMALL_EQUIPMENT: Account = Account {
        number: "606300",
        label: "Fournitures d'entretien et de petit équipement",
    };
    pub const TRAVEL: Account = Account {
        number: "625100",
        label: "Voyages et déplacements",
    };
    pub const MEALS: Account = Account {
        number: "625600",
        label: "Missions",
    };
    pub const OFFICE: Account = Account {
        number: "606400",
        label: "Fournitures administratives",
    };
    pub const PROFESSIONAL: Account = Account {
        number: "618000",
        label: "Divers : formation, documentation, cotisations",
    };
    pub const OTHER: Account = Account {
        number: "658000",
        label: "Charges diverses de gestion courante",
    };
}

/// Le compte de charge d'une catégorie de dépense.
#[must_use]
pub const fn charge_account(category: ExpenseCategory) -> Account {
    match category {
        ExpenseCategory::Software => accounts::SOFTWARE,
        ExpenseCategory::Equipment => accounts::SMALL_EQUIPMENT,
        ExpenseCategory::Travel => accounts::TRAVEL,
        ExpenseCategory::Meals => accounts::MEALS,
        ExpenseCategory::Office => accounts::OFFICE,
        ExpenseCategory::Professional => accounts::PROFESSIONAL,
        ExpenseCategory::Other => accounts::OTHER,
    }
}

/// Compte auxiliaire (tiers) d'une ligne — ici toujours un client sous 411.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AuxAccount {
    /// Code stable et court : `C` suivi des huit premiers caractères hexadécimaux de l'UUID du
    /// client (la même idée qu'un hash git court, ou qu'un préfixe de référence CLI).
    pub number: String,
    pub label: String,
}

impl AuxAccount {
    fn for_client(client: &Client) -> Self {
        let hex: String = client
            .id
            .as_uuid()
            .simple()
            .to_string()
            .chars()
            .take(8)
            .collect::<String>()
            .to_ascii_uppercase();
        Self {
            number: format!("C{hex}"),
            label: client.name.clone(),
        }
    }
}

/// Une ligne d'écriture : `amount` positif au débit, négatif au crédit.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FecLine {
    pub account: Account,
    pub aux: Option<AuxAccount>,
    pub amount: Money,
}

/// Une écriture : ses lignes somment à zéro (voir [`FecEntry::is_balanced`]).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FecEntry {
    pub journal: Journal,
    /// Numéro séquentiel continu *par journal*, attribué après tri chronologique.
    pub number: u32,
    pub date: Date,
    pub piece_ref: String,
    pub piece_date: Date,
    pub label: String,
    pub lines: Vec<FecLine>,
}

impl FecEntry {
    #[must_use]
    pub fn is_balanced(&self) -> bool {
        self.lines.iter().map(|l| l.amount).sum::<Money>() == Money::ZERO
    }

    #[must_use]
    pub fn total_debit(&self) -> Money {
        self.lines
            .iter()
            .filter(|l| !l.amount.is_negative())
            .map(|l| l.amount)
            .sum()
    }
}

/// Le FEC d'un exercice : identité, période, écritures triées et numérotées.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Fec {
    pub siren: Siren,
    pub exercise: FiscalYear,
    pub entries: Vec<FecEntry>,
}

/// Résumé sérialisable d'un export — ce que les façades rendent en `--json`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FecSummary {
    pub file_name: String,
    pub siren: Siren,
    pub starts_on: Date,
    pub ends_on: Date,
    pub entries: usize,
    pub lines: usize,
    pub total_debit_cents: i64,
    pub total_credit_cents: i64,
}

fn method_label(method: PaymentMethod) -> &'static str {
    match method {
        PaymentMethod::BankTransfer => "virement",
        PaymentMethod::Check => "chèque",
        PaymentMethod::Card => "carte",
        PaymentMethod::Other => "autre",
    }
}

fn short_id(id: impl std::fmt::Display) -> String {
    id.to_string().chars().take(8).collect::<String>()
}

/// Ligne signée sur un compte général, sans tiers.
fn line(account: Account, amount: Money) -> FecLine {
    FecLine {
        account,
        aux: None,
        amount,
    }
}

/// Les faits déjà indexés, partagés par les constructeurs d'écritures.
struct Facts<'a> {
    exercise: FiscalYear,
    clients_by_id: HashMap<crate::domain::ClientId, &'a Client>,
    invoices_by_id: HashMap<crate::domain::InvoiceId, &'a Invoice>,
}

impl Facts<'_> {
    fn aux_of(&self, client_id: crate::domain::ClientId) -> AuxAccount {
        self.clients_by_id.get(&client_id).map_or_else(
            || AuxAccount {
                number: format!("C{}", short_id(client_id).to_ascii_uppercase()),
                label: "Client inconnu".to_string(),
            },
            |c| AuxAccount::for_client(c),
        )
    }

    fn client_name(&self, client_id: crate::domain::ClientId) -> &str {
        self.clients_by_id
            .get(&client_id)
            .map_or("client inconnu", |c| c.name.as_str())
    }

    fn invoice_number(&self, id: crate::domain::InvoiceId) -> String {
        self.invoices_by_id
            .get(&id)
            .map_or_else(|| short_id(id), |i| i.number.clone())
    }

    /// Ventes : une écriture par facture ou avoir émis dans l'exercice — 411 au TTC, 706 au HT,
    /// 445710 par taux ; les lignes négatives d'un avoir inversent les côtés d'elles-mêmes.
    fn sales_entries(&self, invoices: &[Invoice]) -> Vec<FecEntry> {
        let mut entries = Vec::new();
        for invoice in invoices
            .iter()
            .filter(|i| self.exercise.contains(i.issued_on))
        {
            let totals = compute_totals(&invoice.lines);
            let ttc = totals.subtotal_ht + totals.total_vat;
            let mut lines = vec![FecLine {
                account: accounts::CLIENTS,
                aux: Some(self.aux_of(invoice.client_id)),
                amount: ttc,
            }];
            lines.push(line(accounts::SERVICES, -totals.subtotal_ht));
            for vat in &totals.vat_breakdown {
                lines.push(line(accounts::VAT_COLLECTED, -vat.vat_amount));
            }
            lines.retain(|l| !l.amount.is_zero());
            if lines.is_empty() {
                continue;
            }
            let label = match invoice.credited_invoice_id {
                Some(original) => format!(
                    "Avoir {} sur {} — {}",
                    invoice.number,
                    self.invoice_number(original),
                    self.client_name(invoice.client_id)
                ),
                None => format!(
                    "Facture {} — {}",
                    invoice.number,
                    self.client_name(invoice.client_id)
                ),
            };
            entries.push(FecEntry {
                journal: Journal::Sales,
                number: 0,
                date: invoice.issued_on,
                piece_ref: invoice.number.clone(),
                piece_date: invoice.issued_on,
                label,
                lines,
            });
        }
        entries
    }

    /// Banque : un encaissement (512 / 411) daté de sa réception, et l'extourne d'un
    /// encaissement annulé (411 / 512) datée de son annulation — deux écritures distinctes, qui
    /// peuvent tomber dans deux exercices.
    fn bank_entries(&self, payments: &[Payment]) -> Vec<FecEntry> {
        let mut entries = Vec::new();
        for payment in payments.iter().filter(|p| !p.amount.is_zero()) {
            let number = self.invoice_number(payment.invoice_id);
            let client_id = self
                .invoices_by_id
                .get(&payment.invoice_id)
                .map(|i| i.client_id);
            let client_line = |amount| FecLine {
                account: accounts::CLIENTS,
                aux: client_id.map(|id| self.aux_of(id)),
                amount,
            };
            if self.exercise.contains(payment.received_on) {
                entries.push(FecEntry {
                    journal: Journal::Bank,
                    number: 0,
                    date: payment.received_on,
                    piece_ref: number.clone(),
                    piece_date: payment.received_on,
                    label: format!("Règlement {number} ({})", method_label(payment.method)),
                    lines: vec![
                        line(accounts::BANK, payment.amount),
                        client_line(-payment.amount),
                    ],
                });
            }
            let Some(voided_on) = payment.voided_at.map(time::OffsetDateTime::date) else {
                continue;
            };
            if self.exercise.contains(voided_on) {
                entries.push(FecEntry {
                    journal: Journal::Bank,
                    number: 0,
                    date: voided_on,
                    piece_ref: number.clone(),
                    piece_date: voided_on,
                    label: format!(
                        "Annulation du règlement {number} du {}",
                        crate::domain::format_date(payment.received_on)
                    ),
                    lines: vec![
                        client_line(payment.amount),
                        line(accounts::BANK, -payment.amount),
                    ],
                });
            }
        }
        entries
    }

    /// Achats : une dépense est réputée payée à sa date (le domaine enregistre un montant TTC
    /// réellement payé) — charge HT (TTC − TVA déductible) et 445660 contre 512.
    fn purchase_entries(&self, expenses: &[Expense]) -> Vec<FecEntry> {
        let mut entries = Vec::new();
        for expense in expenses
            .iter()
            .filter(|e| self.exercise.contains(e.incurred_on))
        {
            let charge = expense.amount - expense.vat_deductible;
            let mut lines = vec![
                line(charge_account(expense.category), charge),
                line(accounts::VAT_DEDUCTIBLE, expense.vat_deductible),
                line(accounts::BANK, -expense.amount),
            ];
            lines.retain(|l| !l.amount.is_zero());
            if lines.is_empty() {
                continue;
            }
            entries.push(FecEntry {
                journal: Journal::Purchases,
                number: 0,
                date: expense.incurred_on,
                piece_ref: expense.receipt_filename.clone().unwrap_or_else(|| {
                    format!("DEP-{}", short_id(expense.id).to_ascii_uppercase())
                }),
                piece_date: expense.incurred_on,
                label: expense.label.clone(),
                lines,
            });
        }
        entries
    }
}

impl Fec {
    /// Construit le FEC d'un exercice à partir des faits du domaine — fonction pure, testable
    /// sans base. Seuls les faits *datés dans l'exercice* sont retenus : une facture par sa date
    /// d'émission, un encaissement par sa date de réception, son annulation par sa date
    /// d'annulation (les deux peuvent tomber dans des exercices différents — chacune est une
    /// écriture à part entière), une dépense par sa date d'engagement.
    #[must_use]
    pub fn build(
        profile: &CompanyProfile,
        exercise: FiscalYear,
        invoices: &[Invoice],
        clients: &[Client],
        payments: &[Payment],
        expenses: &[Expense],
    ) -> Self {
        let facts = Facts {
            exercise,
            clients_by_id: clients.iter().map(|c| (c.id, c)).collect(),
            invoices_by_id: invoices.iter().map(|i| (i.id, i)).collect(),
        };
        let mut entries = facts.sales_entries(invoices);
        entries.extend(facts.bank_entries(payments));
        entries.extend(facts.purchase_entries(expenses));

        // Ordre chronologique (exigé), puis journal et pièce pour un ordre total reproductible ;
        // numérotation continue par journal une fois l'ordre fixé.
        entries.sort_by(|a, b| {
            (a.date, a.journal, &a.piece_ref, &a.label).cmp(&(
                b.date,
                b.journal,
                &b.piece_ref,
                &b.label,
            ))
        });
        let mut counters: HashMap<Journal, u32> = HashMap::new();
        for entry in &mut entries {
            let n = counters.entry(entry.journal).or_insert(0);
            *n += 1;
            entry.number = *n;
        }

        Self {
            siren: profile.siren,
            exercise,
            entries,
        }
    }

    /// `<SIREN>FEC<AAAAMMJJ>.txt`, daté de la clôture de l'exercice — le nom imposé.
    #[must_use]
    pub fn file_name(&self) -> String {
        format!("{}FEC{}.txt", self.siren, compact_date(self.exercise.end()))
    }

    #[must_use]
    pub fn total_debit(&self) -> Money {
        self.entries.iter().map(FecEntry::total_debit).sum()
    }

    #[must_use]
    pub fn total_credit(&self) -> Money {
        self.entries
            .iter()
            .flat_map(|e| e.lines.iter())
            .filter(|l| l.amount.is_negative())
            .map(|l| -l.amount)
            .sum()
    }

    #[must_use]
    pub fn summary(&self) -> FecSummary {
        FecSummary {
            file_name: self.file_name(),
            siren: self.siren,
            starts_on: self.exercise.start(),
            ends_on: self.exercise.end(),
            entries: self.entries.len(),
            lines: self.entries.iter().map(|e| e.lines.len()).sum(),
            total_debit_cents: self.total_debit().cents(),
            total_credit_cents: self.total_credit().cents(),
        }
    }

    /// Le contenu du fichier : en-tête puis une ligne par ligne d'écriture, `\n` en fin de chaque
    /// enregistrement, UTF-8.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        push_record(&mut out, &HEADER);
        for entry in &self.entries {
            let date = compact_date(entry.date);
            let piece_date = compact_date(entry.piece_date);
            for l in &entry.lines {
                let (debit, credit) = if l.amount.is_negative() {
                    (Money::ZERO, -l.amount)
                } else {
                    (l.amount, Money::ZERO)
                };
                let (aux_num, aux_lib) = l
                    .aux
                    .as_ref()
                    .map_or(("", ""), |a| (a.number.as_str(), a.label.as_str()));
                let fields: [&str; 18] = [
                    entry.journal.code(),
                    entry.journal.label(),
                    &entry.number.to_string(),
                    &date,
                    l.account.number,
                    l.account.label,
                    aux_num,
                    aux_lib,
                    &entry.piece_ref,
                    &piece_date,
                    &entry.label,
                    &decimal_comma(debit),
                    &decimal_comma(credit),
                    "",
                    "",
                    &date,
                    "",
                    "",
                ];
                push_record(&mut out, &fields);
            }
        }
        out
    }
}

/// `AAAAMMJJ`.
fn compact_date(date: Date) -> String {
    format!(
        "{:04}{:02}{:02}",
        date.year(),
        u8::from(date.month()),
        date.day()
    )
}

/// Montant positif avec la virgule décimale (`1234,56`), sans séparateur de milliers.
fn decimal_comma(amount: Money) -> String {
    amount.to_decimal_string().replace('.', ",")
}

/// Un champ ne doit contenir ni le séparateur ni une fin de ligne : ils deviennent des espaces.
fn sanitize(field: &str) -> String {
    field
        .chars()
        .map(|c| match c {
            SEPARATOR | '\t' | '\r' | '\n' => ' ',
            other => other,
        })
        .collect::<String>()
        .trim()
        .to_string()
}

fn push_record(out: &mut String, fields: &[&str; 18]) {
    for (i, field) in fields.iter().enumerate() {
        if i > 0 {
            out.push(SEPARATOR);
        }
        out.push_str(&sanitize(field));
    }
    out.push('\n');
}

/// L'exercice dont la clôture tombe dans l'année civile `period` — les dates de l'exercice clos
/// enregistré s'il existe (la date de clôture du profil a pu changer depuis), sinon dérivées de
/// la date de clôture du profil (année civile à défaut). Même désignation que `year show`.
///
/// # Errors
///
/// Erreur de lecture SQLite.
pub fn exercise_ending_in(
    conn: &Connection,
    profile: &CompanyProfile,
    period: i32,
) -> Result<FiscalYear, AppError> {
    if let Some(record) = fiscal_year_ending_in(conn, period)? {
        return Ok(record.period());
    }
    let fye = profile.fiscal_year_end.unwrap_or(FiscalYearEnd::CALENDAR);
    Ok(fye.containing(fye.end_in_year(period)))
}

/// Construit le FEC de l'exercice clos en `period` depuis la base — la requête que les trois
/// façades partagent.
///
/// # Errors
///
/// `AppError::Domain` sans profil d'entreprise (le SIREN nomme le fichier) ; erreur de lecture
/// SQLite sinon.
pub fn build_fec(conn: &Connection, period: i32) -> Result<Fec, AppError> {
    let profile = company_profile(conn)?.ok_or_else(|| {
        AppError::Domain(
            "aucun profil d'entreprise défini : le SIREN est nécessaire pour nommer le FEC \
             (company set-profile)"
                .to_string(),
        )
    })?;
    let exercise = exercise_ending_in(conn, &profile, period)?;
    Ok(Fec::build(
        &profile,
        exercise,
        &list_invoices(conn)?,
        &list_clients(conn)?,
        &list_payments(conn)?,
        &list_expenses(conn)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Actor, ExecutionContext, Executor, Outcome};
    use crate::billing::{EmitInvoice, IssueCreditNote, RecordPayment, VoidPayment};
    use crate::company::SetCompanyProfile;
    use crate::domain::{
        Address, ClientId, InvoiceId, InvoiceLine, InvoiceStatus, PaymentId, VatRate,
    };
    use crate::expenses::RecordExpense;
    use crate::store::{Passphrase, Store};
    use proptest::prelude::*;
    use time::Month as TimeMonth;
    use time::OffsetDateTime;

    fn applied<T>(outcome: Outcome<T>) -> T {
        match outcome {
            Outcome::Applied(value) => value,
            other => panic!(
                "attendu Applied, obtenu {}",
                match other {
                    Outcome::DryRun => "DryRun",
                    Outcome::AlreadyApplied(_) => "AlreadyApplied",
                    Outcome::PendingConfirmation(_) => "PendingConfirmation",
                    Outcome::Applied(_) => unreachable!(),
                }
            ),
        }
    }

    fn date(year: i32, month: TimeMonth, day: u8) -> Date {
        Date::from_calendar_date(year, month, day).unwrap()
    }

    fn profile() -> CompanyProfile {
        CompanyProfile {
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
            share_capital: None,
            rcs_city: None,
            iban: None,
            fiscal_year_end: Some(FiscalYearEnd::CALENDAR),
            vat_regime: None,
            director_monthly_gross: None,
            director_charge_ratio_bps: None,
        }
    }

    fn client(id: ClientId, name: &str) -> Client {
        Client {
            id,
            name: name.to_string(),
            siren: None,
            vat_number: None,
            address: None,
            created_at: OffsetDateTime::UNIX_EPOCH,
            revision: 1,
            archived_at: None,
        }
    }

    fn invoice(
        id: InvoiceId,
        number: &str,
        client_id: ClientId,
        lines: Vec<InvoiceLine>,
        issued_on: Date,
        credited: Option<InvoiceId>,
    ) -> Invoice {
        Invoice {
            id,
            number: number.to_string(),
            client_id,
            mission_id: None,
            lines,
            status: InvoiceStatus::Issued,
            issued_on,
            due_on: issued_on,
            previous_hash: None,
            hash: String::new(),
            credited_invoice_id: credited,
        }
    }

    fn standard_line(description: &str, quantity: f64, unit_cents: i64) -> InvoiceLine {
        InvoiceLine {
            description: description.to_string(),
            quantity,
            unit_price: Money::from_cents(unit_cents),
            vat_rate: VatRate::Standard,
        }
    }

    fn expense(
        label: &str,
        category: ExpenseCategory,
        ttc: i64,
        deductible: i64,
        on: Date,
    ) -> Expense {
        Expense {
            id: crate::domain::ExpenseId::new(),
            label: label.to_string(),
            category,
            amount: Money::from_cents(ttc),
            vat_rate: VatRate::Standard,
            vat_deductible: Money::from_cents(deductible),
            incurred_on: on,
            receipt_hash: None,
            receipt_filename: None,
            created_at: OffsetDateTime::UNIX_EPOCH,
            revision: 1,
        }
    }

    fn payment(invoice_id: InvoiceId, cents: i64, on: Date, voided_on: Option<Date>) -> Payment {
        Payment {
            id: PaymentId::new(),
            invoice_id,
            amount: Money::from_cents(cents),
            received_on: on,
            method: PaymentMethod::BankTransfer,
            bank_transaction_id: None,
            voided_at: voided_on.map(|d| d.midnight().assume_utc()),
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn the_rendered_file_is_the_dgfip_layout_with_balanced_entries() {
        let client_id: ClientId = "01900000-0000-7000-8000-00000000abcd".parse().unwrap();
        let inv_id = InvoiceId::new();
        let cn_id = InvoiceId::new();
        let invoices = vec![
            invoice(
                inv_id,
                "FA-2026-0001",
                client_id,
                vec![standard_line("Conseil", 2.0, 100_000)],
                date(2026, TimeMonth::March, 10),
                None,
            ),
            invoice(
                cn_id,
                "FA-2026-0002",
                client_id,
                vec![standard_line("Conseil", -2.0, 100_000)],
                date(2026, TimeMonth::April, 2),
                Some(inv_id),
            ),
        ];
        let clients = vec![client(client_id, "Acme | Cie")];
        let payments = vec![payment(
            inv_id,
            240_000,
            date(2026, TimeMonth::March, 31),
            None,
        )];
        let expenses = vec![expense(
            "Licence IDE",
            ExpenseCategory::Software,
            12_000,
            2_000,
            date(2026, TimeMonth::March, 12),
        )];

        let fec = Fec::build(
            &profile(),
            FiscalYear::calendar(2026),
            &invoices,
            &clients,
            &payments,
            &expenses,
        );
        assert_eq!(fec.file_name(), "552100554FEC20261231.txt");
        assert!(fec.entries.iter().all(FecEntry::is_balanced));
        assert_eq!(fec.total_debit(), fec.total_credit());

        let rendered = fec.render();
        let mut lines = rendered.lines();
        assert_eq!(lines.next().unwrap(), HEADER.join("|"));
        let body: Vec<&str> = lines.collect();
        // Facture : 3 lignes ; dépense : 3 ; règlement : 2 ; avoir : 3.
        assert_eq!(body.len(), 11, "{rendered}");
        assert!(
            body.iter().all(|l| l.split('|').count() == 18),
            "{rendered}"
        );
        assert_eq!(
            body[0],
            "VE|Ventes|1|20260310|411000|Clients|C01900000|Acme   Cie|FA-2026-0001|20260310|Facture FA-2026-0001 — Acme   Cie|2400,00|0,00|||20260310||",
            "{rendered}"
        );
        assert_eq!(
            body[1],
            "VE|Ventes|1|20260310|706000|Prestations de services|||FA-2026-0001|20260310|Facture FA-2026-0001 — Acme   Cie|0,00|2000,00|||20260310||"
        );
        assert_eq!(
            body[2],
            "VE|Ventes|1|20260310|445710|TVA collectée|||FA-2026-0001|20260310|Facture FA-2026-0001 — Acme   Cie|0,00|400,00|||20260310||"
        );
        // Dépense du 12 mars, journal AC : charge HT 100 €, TVA 20 €, banque 120 €.
        assert!(
            body[3].starts_with("AC|Achats|1|20260312|651000|"),
            "{}",
            body[3]
        );
        assert!(
            body[3].ends_with("|Licence IDE|100,00|0,00|||20260312||"),
            "{}",
            body[3]
        );
        assert!(body[4].contains("|445660|"), "{}", body[4]);
        assert!(body[4].ends_with("|20,00|0,00|||20260312||"), "{}", body[4]);
        assert!(body[5].contains("|512000|Banque|||DEP-"), "{}", body[5]);
        assert!(
            body[5].ends_with("|0,00|120,00|||20260312||"),
            "{}",
            body[5]
        );
        // Règlement du 31 mars, journal BQ.
        assert_eq!(
            body[6],
            "BQ|Banque|1|20260331|512000|Banque|||FA-2026-0001|20260331|Règlement FA-2026-0001 (virement)|2400,00|0,00|||20260331||"
        );
        assert_eq!(
            body[7],
            "BQ|Banque|1|20260331|411000|Clients|C01900000|Acme   Cie|FA-2026-0001|20260331|Règlement FA-2026-0001 (virement)|0,00|2400,00|||20260331||"
        );
        // Avoir : les côtés s'inversent, le numéro VE continue.
        assert_eq!(
            body[8],
            "VE|Ventes|2|20260402|411000|Clients|C01900000|Acme   Cie|FA-2026-0002|20260402|Avoir FA-2026-0002 sur FA-2026-0001 — Acme   Cie|0,00|2400,00|||20260402||"
        );
        assert!(
            body[9].ends_with("|2000,00|0,00|||20260402||"),
            "{}",
            body[9]
        );
        assert!(
            body[10].ends_with("|400,00|0,00|||20260402||"),
            "{}",
            body[10]
        );
    }

    #[test]
    fn only_facts_dated_in_the_exercise_are_exported_and_a_void_is_its_own_entry() {
        let client_id = ClientId::new();
        let inv_id = InvoiceId::new();
        let invoices = vec![invoice(
            inv_id,
            "FA-2025-0001",
            client_id,
            vec![standard_line("Conseil", 1.0, 100_000)],
            date(2025, TimeMonth::December, 20),
            None,
        )];
        // Encaissé fin 2025, annulé en 2026 : l'extourne seule figure au FEC 2026.
        let payments = vec![payment(
            inv_id,
            120_000,
            date(2025, TimeMonth::December, 30),
            Some(date(2026, TimeMonth::January, 15)),
        )];
        let expenses = vec![expense(
            "Hors exercice",
            ExpenseCategory::Other,
            5_000,
            0,
            date(2027, TimeMonth::January, 1),
        )];
        let fec = Fec::build(
            &profile(),
            FiscalYear::calendar(2026),
            &invoices,
            &[client(client_id, "Acme")],
            &payments,
            &expenses,
        );
        assert_eq!(fec.entries.len(), 1, "{:?}", fec.entries);
        let void = &fec.entries[0];
        assert_eq!(void.journal, Journal::Bank);
        assert_eq!(void.date, date(2026, TimeMonth::January, 15));
        assert!(
            void.label
                .starts_with("Annulation du règlement FA-2025-0001 du 2025-12-30")
        );
        assert_eq!(void.lines[0].account, accounts::CLIENTS);
        assert_eq!(void.lines[0].amount, Money::from_cents(120_000));
        assert_eq!(void.lines[1].account, accounts::BANK);
        assert_eq!(void.lines[1].amount, Money::from_cents(-120_000));

        let fec_2025 = Fec::build(
            &profile(),
            FiscalYear::calendar(2025),
            &invoices,
            &[client(client_id, "Acme")],
            &payments,
            &expenses,
        );
        let kinds: Vec<_> = fec_2025.entries.iter().map(|e| e.journal).collect();
        assert_eq!(kinds, vec![Journal::Sales, Journal::Bank]);
        assert!(
            fec_2025.entries[1].label.starts_with("Règlement"),
            "l'encaissement lui-même reste au FEC 2025"
        );
    }

    #[test]
    fn every_expense_category_maps_to_a_class_6_account() {
        for category in [
            ExpenseCategory::Software,
            ExpenseCategory::Equipment,
            ExpenseCategory::Travel,
            ExpenseCategory::Meals,
            ExpenseCategory::Office,
            ExpenseCategory::Professional,
            ExpenseCategory::Other,
        ] {
            assert!(charge_account(category).number.starts_with('6'));
        }
    }

    #[test]
    fn a_non_deductible_vat_share_stays_in_the_charge() {
        // Restaurant 120 € TTC, TVA déductible plafonnée à 8 € : charge 112 €, TVA 8 €.
        let fec = Fec::build(
            &profile(),
            FiscalYear::calendar(2026),
            &[],
            &[],
            &[],
            &[expense(
                "Déjeuner client",
                ExpenseCategory::Meals,
                12_000,
                800,
                date(2026, TimeMonth::May, 5),
            )],
        );
        let entry = &fec.entries[0];
        assert_eq!(entry.lines[0].account, accounts::MEALS);
        assert_eq!(entry.lines[0].amount, Money::from_cents(11_200));
        assert_eq!(entry.lines[1].amount, Money::from_cents(800));
        assert!(entry.is_balanced());
    }

    fn line_strategy() -> impl Strategy<Value = InvoiceLine> {
        (
            -50.0f64..50.0,
            -5_000_000i64..5_000_000,
            prop::sample::select(VatRate::ALL.to_vec()),
        )
            .prop_map(|(quantity, unit, vat_rate)| InvoiceLine {
                description: "x".to_string(),
                quantity,
                unit_price: Money::from_cents(unit),
                vat_rate,
            })
    }

    fn expense_strategy() -> impl Strategy<Value = Expense> {
        (0i64..5_000_000, 0u8..=100, 1u8..=28).prop_map(|(ttc, share, day)| {
            let deductible = ttc * i64::from(share) / 100;
            expense(
                "dép.",
                ExpenseCategory::Other,
                ttc,
                deductible,
                date(2026, TimeMonth::June, day),
            )
        })
    }

    proptest! {
        #[test]
        fn every_entry_balances_and_the_file_balances_whatever_the_facts(
            lines in prop::collection::vec(line_strategy(), 1..6),
            expenses in prop::collection::vec(expense_strategy(), 0..4),
            paid in 0i64..1_000_000,
        ) {
            let client_id = ClientId::new();
            let inv_id = InvoiceId::new();
            let invoices = vec![invoice(
                inv_id, "FA-2026-0001", client_id, lines, date(2026, TimeMonth::June, 1), None,
            )];
            let payments = vec![payment(inv_id, paid, date(2026, TimeMonth::June, 15), None)];
            let fec = Fec::build(
                &profile(),
                FiscalYear::calendar(2026),
                &invoices,
                &[client(client_id, "Acme")],
                &payments,
                &expenses,
            );
            prop_assert!(fec.entries.iter().all(FecEntry::is_balanced));
            prop_assert_eq!(fec.total_debit(), fec.total_credit());
            let rendered = fec.render();
            for record in rendered.lines() {
                prop_assert_eq!(record.split('|').count(), 18);
            }
            // Numérotation continue par journal, dans l'ordre du fichier.
            for journal in [Journal::Sales, Journal::Purchases, Journal::Bank] {
                let numbers: Vec<u32> = fec
                    .entries
                    .iter()
                    .filter(|e| e.journal == journal)
                    .map(|e| e.number)
                    .collect();
                let expected: Vec<u32> = (1..=u32::try_from(numbers.len()).unwrap()).collect();
                prop_assert_eq!(numbers, expected);
            }
            for pair in fec.entries.windows(2) {
                prop_assert!(pair[0].date <= pair[1].date);
            }
        }
    }

    // --- Sur base : la requête partagée par les façades. ---

    fn fresh_store(tag: &str) -> (Store, ClientId) {
        let dir = std::env::temp_dir().join(format!(
            "freeflow-fec-{tag}-{}-{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ));
        let store = Store::create(&dir.join("vault.db"), &Passphrase::from("s3cret")).unwrap();
        let client_id = ClientId::new();
        store
            .connection()
            .execute(
                "INSERT INTO clients (id, name, created_at) \
                 VALUES (?1, 'Acme', '2025-01-01T00:00:00Z')",
                [client_id.to_string()],
            )
            .unwrap();
        (store, client_id)
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn build_fec_reads_invoices_credit_notes_payments_voids_and_expenses() {
        let (mut store, client_id) = fresh_store("db");
        let human = ExecutionContext::new(Actor::Human, false);
        let p = profile();
        Executor::new(&mut store)
            .execute(
                &SetCompanyProfile {
                    name: p.name.clone(),
                    legal_form: p.legal_form.clone(),
                    siren: p.siren,
                    vat_number: None,
                    address: p.address.clone(),
                    share_capital: None,
                    rcs_city: None,
                    iban: None,
                    fiscal_year_end: Some(FiscalYearEnd::new(6, 30).unwrap()),
                    vat_regime: None,
                    director_monthly_gross: None,
                    director_charge_ratio_bps: None,
                },
                &human,
            )
            .unwrap();
        let emitted = applied(
            Executor::new(&mut store)
                .execute(
                    &EmitInvoice {
                        client_id,
                        mission_id: None,
                        lines: vec![standard_line("Conseil", 3.0, 50_000)],
                        issued_on: date(2025, TimeMonth::September, 1),
                        payment_terms_days: 30,
                    },
                    &human,
                )
                .unwrap(),
        );
        let paid = applied(
            Executor::new(&mut store)
                .execute(
                    &RecordPayment {
                        invoice_id: emitted.id,
                        amount: Money::from_cents(180_000),
                        received_on: date(2025, TimeMonth::October, 1),
                        method: PaymentMethod::Card,
                    },
                    &human,
                )
                .unwrap(),
        );
        Executor::new(&mut store)
            .execute(
                &VoidPayment {
                    payment_id: paid,
                    reason: Some("doublon".to_string()),
                },
                &human,
            )
            .unwrap();
        Executor::new(&mut store)
            .execute(
                &IssueCreditNote {
                    invoice_id: emitted.id,
                    issued_on: date(2025, TimeMonth::November, 3),
                },
                &human,
            )
            .unwrap();
        Executor::new(&mut store)
            .execute(
                &RecordExpense {
                    label: "Train".to_string(),
                    category: ExpenseCategory::Travel,
                    amount: Money::from_cents(11_000),
                    vat_rate: VatRate::Intermediate,
                    vat_deductible: Money::from_cents(1_000),
                    incurred_on: date(2026, TimeMonth::February, 2),
                    receipt_hash: None,
                    receipt_filename: Some("billet.pdf".to_string()),
                },
                &human,
            )
            .unwrap();
        // Hors exercice (2026-27).
        Executor::new(&mut store)
            .execute(
                &RecordExpense {
                    label: "Après clôture".to_string(),
                    category: ExpenseCategory::Other,
                    amount: Money::from_cents(1_000),
                    vat_rate: VatRate::Zero,
                    vat_deductible: Money::ZERO,
                    incurred_on: date(2026, TimeMonth::July, 1),
                    receipt_hash: None,
                    receipt_filename: None,
                },
                &human,
            )
            .unwrap();

        // Exercice décalé clos le 30 juin 2026 : 2025-07-01 → 2026-06-30, dérivé du profil
        // (aucun exercice clos enregistré).
        let fec = build_fec(store.connection(), 2026).unwrap();
        assert_eq!(fec.exercise.start(), date(2025, TimeMonth::July, 1));
        assert_eq!(fec.exercise.end(), date(2026, TimeMonth::June, 30));
        assert_eq!(fec.file_name(), "552100554FEC20260630.txt");
        let labels: Vec<&str> = fec.entries.iter().map(|e| e.label.as_str()).collect();
        assert_eq!(labels.len(), 4, "{labels:?}");
        assert!(
            labels[0].starts_with("Facture FA-2025-0001 — Acme"),
            "{labels:?}"
        );
        assert!(
            labels[1].starts_with("Règlement FA-2025-0001 (carte)"),
            "{labels:?}"
        );
        assert!(
            labels[2].starts_with("Avoir FA-2025-0002 sur FA-2025-0001"),
            "{labels:?}"
        );
        assert_eq!(labels[3], "Train");
        assert_eq!(fec.entries[3].piece_ref, "billet.pdf");
        assert_eq!(fec.entries[3].lines[0].account, accounts::TRAVEL);
        assert_eq!(fec.entries[3].lines[0].amount, Money::from_cents(10_000));
        assert!(fec.entries.iter().all(FecEntry::is_balanced));
        assert_eq!(fec.total_debit(), fec.total_credit());
        // Numérotation par journal : VE 1-2, BQ 1, AC 1.
        let numbers: Vec<(Journal, u32)> =
            fec.entries.iter().map(|e| (e.journal, e.number)).collect();
        assert_eq!(
            numbers,
            vec![
                (Journal::Sales, 1),
                (Journal::Bank, 1),
                (Journal::Sales, 2),
                (Journal::Purchases, 1),
            ]
        );

        // L'annulation est horodatée au moment de la commande (aujourd'hui) : elle relève de
        // l'exercice suivant, avec la dépense d'après clôture.
        let next = build_fec(store.connection(), 2027).unwrap();
        assert_eq!(next.exercise.start(), date(2026, TimeMonth::July, 1));
        let labels: Vec<&str> = next.entries.iter().map(|e| e.label.as_str()).collect();
        assert_eq!(labels.len(), 2, "{labels:?}");
        assert_eq!(labels[0], "Après clôture");
        assert!(
            labels[1].starts_with("Annulation du règlement FA-2025-0001 du 2025-10-01"),
            "{labels:?}"
        );
        assert_eq!(next.entries[1].lines[0].amount, Money::from_cents(180_000));
        assert!(next.entries[1].is_balanced());
    }

    #[test]
    fn build_fec_requires_a_company_profile() {
        let (store, _) = fresh_store("no-profile");
        let err = build_fec(store.connection(), 2026).unwrap_err();
        assert!(matches!(err, AppError::Domain(_)), "{err}");
    }
}
