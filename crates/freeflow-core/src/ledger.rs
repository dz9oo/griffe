//! Grand livre **dérivé** d'un exercice (lot 31) — la généralisation du FEC du lot 28.
//!
//! `FreeFlow` ne tient pas de comptabilité : les écritures sont *dérivées* des faits du domaine,
//! qui sont déjà immuables ou contre-écrits (factures et avoirs, encaissements et annulations,
//! dépenses), du bilan d'ouverture (lot 30) et des snapshots de clôture (lot 20). Ce module
//! porte les types d'écriture ([`Journal`], [`Account`], [`LedgerEntry`]), la construction pure
//! ([`Ledger::build`]), la **balance des comptes** ([`TrialBalance`]) et le **bilan** de clôture
//! ([`BalanceSheet`], présenté selon le tableau 2033-A-SD : actif brut / amortissements / net,
//! passif). Le FEC (`crate::fec`) n'est plus que le *format* de rendu de ce grand livre.
//!
//! Cinq journaux : `AN` (à-nouveaux), `VE` (ventes), `AC` (achats), `BQ` (banque) et `OD`
//! (opérations diverses de clôture). Chaque écriture est équilibrée par construction — une ligne
//! porte un montant *signé* (débit positif, crédit négatif) et l'écriture est la somme nulle de
//! ses lignes.
//!
//! **Chaînage des exercices.** Les à-nouveaux du premier exercice suivi viennent du bilan
//! d'ouverture (s'il ouvre ce jour-là) ; ceux d'un exercice suivant viennent du **bilan de
//! clôture dérivé de l'exercice précédent** — à condition que celui-ci soit clos dans
//! l'application (`fiscal_years`), dont le résultat est posé en 120/129 puis affecté par une
//! écriture `OD` datée de l'approbation (ou du premier jour de l'exercice suivant tant que la
//! décision n'est qu'un projet — la même convention que la chaîne du report à nouveau de
//! `fiscal_year::prior_chain`, qui n'attend pas l'approbation non plus). Sans exercice clos
//! entre deux, la chaîne s'arrête : pas d'à-nouveaux, ni d'écriture d'affectation.
//!
//! **Opérations de clôture** (`OD`, datées du dernier jour) : la rémunération du dirigeant
//! (641/645 contre 421/431 — réputée *due, non décaissée* : le domaine n'enregistre aucun fait
//! de paie, l'expert-comptable substitue les écritures réelles) et l'IS (695 contre 444), pris
//! dans le snapshot de clôture s'il existe, recalculés sinon — en imputant d'abord les déficits
//! antérieurs de la chaîne sur le bénéfice (lot 32) — et, depuis un snapshot ayant opté pour le
//! report en arrière, la créance d'IS qui en naît (444 contre le produit 699). Le résultat de
//! l'exercice est la somme des comptes de gestion (classes 6 et 7) ; il se retrouve donc au
//! passif du bilan *après* IS.
//!
//! **Dépenses et relevé** (lot 33) : une dépense *rapprochée* d'un débit du relevé bancaire
//! (`expenses::ReconcileExpense`) est comptabilisée en deux temps — la charge à sa date
//! d'engagement (`AC`, 6xx et 445660 contre 401), le décaissement à la date du relevé (`BQ`,
//! 401 contre 512) — qui peuvent tomber dans deux exercices : un 401 créditeur au bilan est une
//! facture fournisseur reçue avant la clôture et payée après. Une dépense non rapprochée reste
//! réputée payée à sa date (charge contre 512 directement) : sans relevé, le domaine n'a pas de
//! meilleure date. Le 512 dérivé suit donc le relevé exactement là où il a été rapproché.
//!
//! Limites assumées, dites dans les libellés : les dépenses non rapprochées sont réputées payées
//! à leur date, l'équipement est passé en charge sans seuil d'immobilisation, la TVA n'est
//! jamais liquidée (445660/445710 restent bruts, aucune CA3 n'étant un fait daté), pas
//! d'amortissement, de provision ni de régularisation. Un export pour l'expert-comptable, qui
//! reste maître des écritures définitives.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};

use rusqlite::Connection;
use serde::Serialize;
use time::Date;

use crate::accounting::{corporate_income_tax, director_gross, impute_prior_losses};
use crate::app::AppError;

/// Ce que la construction du grand livre peut refuser (lot 36).
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum LedgerError {
    /// La somme des montants en valeur absolue dépasse `i64::MAX` centimes : aucune somme
    /// partielle (balance, bilan, totaux) ne pourrait être calculée sans déborder. Hors
    /// d'atteinte avec des montants bornés par `Money::MAX_INPUT`, sauf données de base
    /// altérées — refusé explicitement plutôt qu'une panique.
    #[error(
        "les montants du grand livre sont trop grands pour être totalisés sans déborder — \
         vérifiez les montants saisis (une ligne de bilan ou une facture aberrante)"
    )]
    Overflow,
}

impl From<LedgerError> for AppError {
    fn from(e: LedgerError) -> Self {
        Self::Domain(e.to_string())
    }
}
use crate::billing::{compute_totals, list_bank_transactions, list_invoices, list_payments};
use crate::clients::list_clients;
use crate::company::{CompanyProfile, company_profile};
use crate::domain::{
    BankTransaction, Client, ClientId, Expense, ExpenseCategory, ExpenseId, FiscalYear,
    FiscalYearEnd, Invoice, InvoiceId, Money, OpeningBalance, Payment, PaymentMethod, format_date,
};
use crate::expenses::list_expenses;
use crate::fiscal_year::{FiscalYearRecord, fiscal_year_ending_in, list_fiscal_years};
use crate::opening_balance::opening_balance;

/// Journal comptable — l'ordre des variantes est l'ordre de tri à date égale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Journal {
    /// `AN` : à-nouveaux — le bilan d'ouverture repris (lot 30) ou le bilan de clôture dérivé
    /// de l'exercice précédent, au premier jour de l'exercice.
    Opening,
    /// `VE` : factures et avoirs.
    Sales,
    /// `AC` : dépenses.
    Purchases,
    /// `BQ` : encaissements et leurs annulations, décaissements des dépenses rapprochées.
    Bank,
    /// `OD` : opérations diverses — rémunération du dirigeant, IS, affectation du résultat.
    Misc,
}

impl Journal {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Opening => "AN",
            Self::Sales => "VE",
            Self::Purchases => "AC",
            Self::Bank => "BQ",
            Self::Misc => "OD",
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Opening => "À-nouveaux",
            Self::Sales => "Ventes",
            Self::Purchases => "Achats",
            Self::Bank => "Banque",
            Self::Misc => "Opérations diverses",
        }
    }
}

/// Un compte du plan comptable général, numéro et libellé. `Cow` : le plan fixe du module
/// [`accounts`] est statique, les comptes d'un bilan d'ouverture (lot 30) sont saisis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Account {
    pub number: Cow<'static, str>,
    pub label: Cow<'static, str>,
}

impl Account {
    /// Le compte du plan fixe portant ce numéro, s'il y figure ; sinon un compte dynamique
    /// avec `label` — `None` si aucun libellé n'est disponible. C'est la règle « un libellé par
    /// compte » du lot 37 : le plan fixe prime toujours sur un libellé saisi.
    #[must_use]
    pub fn for_number(number: &str, label: Option<&str>) -> Option<Self> {
        accounts::FIXED_PLAN
            .iter()
            .find(|a| a.number == number)
            .cloned()
            .or_else(|| {
                label.map(|label| Self {
                    number: Cow::Owned(number.to_string()),
                    label: Cow::Owned(label.to_string()),
                })
            })
    }

    #[must_use]
    pub const fn fixed(number: &'static str, label: &'static str) -> Self {
        Self {
            number: Cow::Borrowed(number),
            label: Cow::Borrowed(label),
        }
    }

    /// La classe du compte : son premier chiffre (`0` pour un numéro vide, inatteignable par
    /// construction des comptes du domaine).
    #[must_use]
    pub fn class(&self) -> u8 {
        self.number
            .as_bytes()
            .first()
            .map_or(0, |b| b.wrapping_sub(b'0'))
    }

    /// Compte de gestion (classes 6 et 7) — ce qui forme le résultat de l'exercice.
    #[must_use]
    pub fn is_income_statement(&self) -> bool {
        matches!(self.class(), 6 | 7)
    }
}

/// Le plan de comptes minimal du grand livre dérivé — fixe, sans paramétrage : un compte par
/// fait du domaine, plus les comptes des opérations de clôture.
pub mod accounts {
    use super::Account;

    pub const CLIENTS: Account = Account::fixed("411000", "Clients");
    /// Fournisseurs (lot 33) : la charge d'une dépense rapprochée y attend son décaissement,
    /// daté du relevé.
    pub const SUPPLIERS: Account = Account::fixed("401000", "Fournisseurs");
    pub const BANK: Account = Account::fixed("512000", "Banque");
    pub const SERVICES: Account = Account::fixed("706000", "Prestations de services");
    pub const VAT_COLLECTED: Account = Account::fixed("445710", "TVA collectée");
    pub const VAT_DEDUCTIBLE: Account =
        Account::fixed("445660", "TVA déductible sur autres biens et services");
    pub const SOFTWARE: Account = Account::fixed("651000", "Redevances pour logiciels et licences");
    pub const SMALL_EQUIPMENT: Account =
        Account::fixed("606300", "Fournitures d'entretien et de petit équipement");
    pub const TRAVEL: Account = Account::fixed("625100", "Voyages et déplacements");
    pub const MEALS: Account = Account::fixed("625600", "Missions");
    pub const OFFICE: Account = Account::fixed("606400", "Fournitures administratives");
    pub const PROFESSIONAL: Account =
        Account::fixed("618000", "Divers : formation, documentation, cotisations");
    pub const FEES: Account = Account::fixed("622600", "Honoraires");
    pub const BANK_CHARGES: Account = Account::fixed("627000", "Services bancaires et assimilés");
    /// Impôts et taxes (lot 37) : CFE, CVAE… — case 244 du 2033-B.
    pub const TAXES: Account = Account::fixed("635000", "Impôts, taxes et versements assimilés");
    pub const OTHER: Account = Account::fixed("658000", "Charges diverses de gestion courante");

    // Comptes de bilan que le relevé règle (lot 37) — repris d'un bilan de cabinet, ou mouvements
    // qui ne sont ni une charge ni un produit.
    pub const VAT_DUE: Account = Account::fixed("445510", "État — TVA à décaisser");
    pub const VAT_CREDIT: Account = Account::fixed("445670", "Crédit de TVA à reporter");
    pub const SHAREHOLDER_ACCOUNT: Account =
        Account::fixed("455000", "Associés — comptes courants");
    pub const INTERNAL_TRANSFER: Account = Account::fixed("580000", "Virements internes");
    pub const LOANS: Account =
        Account::fixed("164000", "Emprunts auprès des établissements de crédit");
    pub const SHARE_CAPITAL: Account = Account::fixed("101000", "Capital social");

    // Opérations de clôture (lot 31).
    pub const DIRECTOR_PAY: Account = Account::fixed("641100", "Rémunération du dirigeant");
    pub const SOCIAL_CHARGES: Account =
        Account::fixed("645000", "Charges de sécurité sociale et de prévoyance");
    pub const PAY_DUE: Account = Account::fixed("421000", "Personnel — rémunérations dues");
    pub const SOCIAL_DUE: Account = Account::fixed("431000", "Sécurité sociale");
    pub const CORPORATE_TAX: Account = Account::fixed("695000", "Impôts sur les bénéfices");
    pub const CORPORATE_TAX_DUE: Account =
        Account::fixed("444000", "État — impôts sur les bénéfices");
    /// Produit du report en arrière d'un déficit (art. 220 quinquies CGI) : la créance d'IS
    /// est débitée en 444 par ce crédit (lot 32).
    pub const CARRY_BACK_INCOME: Account =
        Account::fixed("699000", "Produits — report en arrière des déficits");
    pub const PROFIT: Account = Account::fixed("120000", "Résultat de l'exercice (bénéfice)");
    pub const LOSS: Account = Account::fixed("129000", "Résultat de l'exercice (perte)");
    pub const LEGAL_RESERVE: Account = Account::fixed("106100", "Réserve légale");
    pub const RETAINED_CREDIT: Account =
        Account::fixed("110000", "Report à nouveau (solde créditeur)");
    pub const RETAINED_DEBIT: Account =
        Account::fixed("119000", "Report à nouveau (solde débiteur)");
    pub const DIVIDENDS_DUE: Account = Account::fixed("457000", "Associés — dividendes à payer");

    /// Tout le plan fixe — la source unique du libellé d'un compte (lot 37 : un `CompteNum` du
    /// FEC n'a qu'un seul `CompteLib`, celui-ci ; un libellé saisi au bilan d'ouverture ne
    /// sert qu'à un compte hors de cette liste).
    pub const FIXED_PLAN: [Account; 34] = [
        SHARE_CAPITAL,
        LEGAL_RESERVE,
        RETAINED_CREDIT,
        RETAINED_DEBIT,
        PROFIT,
        LOSS,
        LOANS,
        SUPPLIERS,
        CLIENTS,
        PAY_DUE,
        SOCIAL_DUE,
        CORPORATE_TAX_DUE,
        VAT_DUE,
        VAT_DEDUCTIBLE,
        VAT_COLLECTED,
        VAT_CREDIT,
        SHAREHOLDER_ACCOUNT,
        DIVIDENDS_DUE,
        BANK,
        INTERNAL_TRANSFER,
        SMALL_EQUIPMENT,
        OFFICE,
        PROFESSIONAL,
        FEES,
        TRAVEL,
        MEALS,
        BANK_CHARGES,
        TAXES,
        DIRECTOR_PAY,
        SOCIAL_CHARGES,
        SOFTWARE,
        OTHER,
        CORPORATE_TAX,
        CARRY_BACK_INCOME,
    ];
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
        ExpenseCategory::Fees => accounts::FEES,
        ExpenseCategory::BankCharges => accounts::BANK_CHARGES,
        ExpenseCategory::Taxes => accounts::TAXES,
        ExpenseCategory::Other => accounts::OTHER,
    }
}

/// Compte auxiliaire (tiers) d'une ligne — ici toujours un client sous 411.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LedgerLine {
    pub account: Account,
    pub aux: Option<AuxAccount>,
    pub amount: Money,
}

/// Une écriture : ses lignes somment à zéro (voir [`LedgerEntry::is_balanced`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LedgerEntry {
    pub journal: Journal,
    /// Numéro séquentiel continu *par journal*, attribué après tri chronologique.
    pub number: u32,
    #[serde(with = "crate::domain::serde_date::date")]
    pub date: Date,
    pub piece_ref: String,
    #[serde(with = "crate::domain::serde_date::date")]
    pub piece_date: Date,
    pub label: String,
    pub lines: Vec<LedgerLine>,
}

impl LedgerEntry {
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

/// Les à-nouveaux d'un exercice : d'où ils viennent (libellé de l'écriture `AN`) et leurs
/// lignes, déjà signées. Construits depuis un bilan d'ouverture saisi ou depuis le bilan de
/// clôture dérivé de l'exercice précédent ([`Ledger::closing_opening_lines`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpeningLines {
    pub label: String,
    pub lines: Vec<LedgerLine>,
}

impl OpeningLines {
    /// Les lignes du bilan d'ouverture saisi (lot 30), telles quelles — au libellé près : un
    /// compte du plan fixe reprend le libellé du plan (lot 37), le libellé saisi ne sert qu'aux
    /// comptes hors plan.
    ///
    /// # Panics
    ///
    /// Jamais : `Account::for_number` ne renvoie `None` que sans libellé, et chaque ligne du
    /// bilan en porte un.
    #[must_use]
    pub fn from_opening_balance(opening: &OpeningBalance) -> Self {
        Self {
            label: opening.source.clone().map_or_else(
                || "À-nouveaux".to_string(),
                |source| format!("À-nouveaux — {source}"),
            ),
            lines: opening
                .lines
                .iter()
                .map(|l| LedgerLine {
                    account: Account::for_number(l.account.as_str(), Some(&l.label))
                        .expect("un libellé est toujours fourni"),
                    aux: None,
                    amount: l.signed(),
                })
                .collect(),
        }
    }
}

/// Les faits dont le grand livre d'un exercice est dérivé — l'entrée de [`Ledger::build`].
pub struct LedgerFacts<'a> {
    pub profile: &'a CompanyProfile,
    pub exercise: FiscalYear,
    pub invoices: &'a [Invoice],
    pub clients: &'a [Client],
    pub payments: &'a [Payment],
    pub expenses: &'a [Expense],
    /// Les transactions du relevé importé (lot 33) : seules celles rapprochées d'une dépense
    /// (`matched_expense_id`) comptent ici, elles datent le décaissement de cette dépense.
    pub bank_transactions: &'a [BankTransaction],
    /// À-nouveaux au premier jour, s'il y en a (voir le commentaire de module).
    pub opening: Option<OpeningLines>,
    /// Le snapshot de clôture de *cet* exercice, s'il est clos dans l'application : il fixe
    /// l'IS et la rémunération du dirigeant (figés à la clôture) au lieu de les recalculer.
    pub snapshot: Option<&'a FiscalYearRecord>,
    /// Les exercices clos antérieurs dont l'écriture d'affectation du résultat peut tomber
    /// dans cet exercice — seulement ceux de la chaîne dont les à-nouveaux dérivent.
    pub appropriations: &'a [FiscalYearRecord],
    /// Déficits fiscaux antérieurs reportables à l'ouverture (lot 32) : sans snapshot, l'IS
    /// recalculé les impute d'abord sur le bénéfice, comme la clôture le fera.
    pub prior_losses: Money,
}

/// Le grand livre dérivé d'un exercice : ses écritures triées et numérotées.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ledger {
    pub exercise: FiscalYear,
    pub entries: Vec<LedgerEntry>,
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
fn line(account: Account, amount: Money) -> LedgerLine {
    LedgerLine {
        account,
        aux: None,
        amount,
    }
}

fn entry(
    journal: Journal,
    date: Date,
    piece_ref: impl Into<String>,
    label: impl Into<String>,
    mut lines: Vec<LedgerLine>,
) -> Option<LedgerEntry> {
    lines.retain(|l| !l.amount.is_zero());
    if lines.is_empty() {
        return None;
    }
    Some(LedgerEntry {
        journal,
        number: 0,
        date,
        piece_ref: piece_ref.into(),
        piece_date: date,
        label: label.into(),
        lines,
    })
}

/// Le compte qui porte le résultat de l'exercice avant affectation : 120 pour un bénéfice, 129
/// pour une perte.
#[must_use]
pub fn result_account(net_result: Money) -> Account {
    if net_result.is_negative() {
        accounts::LOSS
    } else {
        accounts::PROFIT
    }
}

/// Les faits déjà indexés, partagés par les constructeurs d'écritures.
struct Facts<'a> {
    exercise: FiscalYear,
    clients_by_id: HashMap<ClientId, &'a Client>,
    invoices_by_id: HashMap<InvoiceId, &'a Invoice>,
}

impl Facts<'_> {
    fn aux_of(&self, client_id: ClientId) -> AuxAccount {
        self.clients_by_id.get(&client_id).map_or_else(
            || AuxAccount {
                number: format!("C{}", short_id(client_id).to_ascii_uppercase()),
                label: "Client inconnu".to_string(),
            },
            |c| AuxAccount::for_client(c),
        )
    }

    fn client_name(&self, client_id: ClientId) -> &str {
        self.clients_by_id
            .get(&client_id)
            .map_or("client inconnu", |c| c.name.as_str())
    }

    fn invoice_number(&self, id: InvoiceId) -> String {
        self.invoices_by_id
            .get(&id)
            .map_or_else(|| short_id(id), |i| i.number.clone())
    }

    /// À-nouveaux : une seule écriture au premier jour de l'exercice. Équilibrée par
    /// construction : un bilan d'ouverture l'est (`OpeningBalance::validate`), un bilan de
    /// clôture dérivé aussi ([`Ledger::closing_opening_lines`]).
    fn opening_entries(&self, opening: Option<OpeningLines>) -> Vec<LedgerEntry> {
        let Some(opening) = opening else {
            return Vec::new();
        };
        entry(
            Journal::Opening,
            self.exercise.start(),
            "AN",
            opening.label,
            opening.lines,
        )
        .into_iter()
        .collect()
    }

    /// Ventes : une écriture par facture ou avoir émis dans l'exercice — 411 au TTC, 706 au HT,
    /// 445710 par taux ; les lignes négatives d'un avoir inversent les côtés d'elles-mêmes.
    fn sales_entries(&self, invoices: &[Invoice]) -> Vec<LedgerEntry> {
        let mut entries = Vec::new();
        for invoice in invoices
            .iter()
            .filter(|i| self.exercise.contains(i.issued_on))
        {
            let totals = compute_totals(&invoice.lines);
            let ttc = totals.subtotal_ht + totals.total_vat;
            let mut lines = vec![LedgerLine {
                account: accounts::CLIENTS,
                aux: Some(self.aux_of(invoice.client_id)),
                amount: ttc,
            }];
            lines.push(line(accounts::SERVICES, -totals.subtotal_ht));
            for vat in &totals.vat_breakdown {
                lines.push(line(accounts::VAT_COLLECTED, -vat.vat_amount));
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
            entries.extend(entry(
                Journal::Sales,
                invoice.issued_on,
                invoice.number.clone(),
                label,
                lines,
            ));
        }
        entries
    }

    /// Banque : un encaissement (512 / 411) daté de sa réception, et l'extourne d'un
    /// encaissement annulé (411 / 512) datée de son annulation — deux écritures distinctes, qui
    /// peuvent tomber dans deux exercices.
    fn bank_entries(&self, payments: &[Payment]) -> Vec<LedgerEntry> {
        let mut entries = Vec::new();
        for payment in payments.iter().filter(|p| !p.amount.is_zero()) {
            let number = self.invoice_number(payment.invoice_id);
            let client_id = self
                .invoices_by_id
                .get(&payment.invoice_id)
                .map(|i| i.client_id);
            let client_line = |amount| LedgerLine {
                account: accounts::CLIENTS,
                aux: client_id.map(|id| self.aux_of(id)),
                amount,
            };
            if self.exercise.contains(payment.received_on) {
                entries.extend(entry(
                    Journal::Bank,
                    payment.received_on,
                    number.clone(),
                    format!("Règlement {number} ({})", method_label(payment.method)),
                    vec![
                        line(accounts::BANK, payment.amount),
                        client_line(-payment.amount),
                    ],
                ));
            }
            let Some(voided_on) = payment.voided_at.map(time::OffsetDateTime::date) else {
                continue;
            };
            if self.exercise.contains(voided_on) {
                entries.extend(entry(
                    Journal::Bank,
                    voided_on,
                    number.clone(),
                    format!(
                        "Annulation du règlement {number} du {}",
                        format_date(payment.received_on)
                    ),
                    vec![
                        client_line(payment.amount),
                        line(accounts::BANK, -payment.amount),
                    ],
                ));
            }
        }
        entries
    }

    /// Achats : une dépense non rapprochée est réputée payée à sa date (le domaine enregistre
    /// un montant TTC réellement payé) — charge HT (TTC − TVA déductible) et 445660 contre 512.
    /// Une dépense rapprochée d'un débit du relevé (lot 33) passe par 401 : la charge à sa date
    /// (`AC`), le décaissement 401/512 à la date du relevé (`BQ`) — deux écritures distinctes,
    /// chacune retenue si *sa* date tombe dans l'exercice.
    fn purchase_entries(
        &self,
        expenses: &[Expense],
        bank_transactions: &[BankTransaction],
    ) -> Vec<LedgerEntry> {
        let debits: HashMap<ExpenseId, &BankTransaction> = bank_transactions
            .iter()
            .filter_map(|t| t.matched_expense_id.map(|id| (id, t)))
            .collect();
        let mut entries = Vec::new();
        for expense in expenses {
            // Lot 37 : une pièce **unique** par dépense (l'UUID complet — huit caractères d'un
            // UUIDv7 sont un horodatage à la seconde, partagé par tout un import), la même pour
            // la charge et son décaissement ; le justificatif, lui, est nommé dans le libellé.
            let piece = format!("DEP-{}", expense.id);
            let label = expense.receipt_filename.as_ref().map_or_else(
                || expense.label.clone(),
                |file| format!("{} — {file}", expense.label),
            );
            let charge = expense.amount - expense.vat_deductible;
            let debit = debits.get(&expense.id).copied();
            let paid_through = debit.map_or(accounts::BANK, |_| accounts::SUPPLIERS);
            if self.exercise.contains(expense.incurred_on) {
                entries.extend(entry(
                    Journal::Purchases,
                    expense.incurred_on,
                    piece.clone(),
                    label,
                    vec![
                        line(charge_account(expense.category), charge),
                        line(accounts::VAT_DEDUCTIBLE, expense.vat_deductible),
                        line(paid_through, -expense.amount),
                    ],
                ));
            }
            let Some(debit) = debit else {
                continue;
            };
            if self.exercise.contains(debit.occurred_on) {
                entries.extend(entry(
                    Journal::Bank,
                    debit.occurred_on,
                    piece,
                    format!(
                        "Paiement {} — relevé du {} ({})",
                        expense.label,
                        format_date(debit.occurred_on),
                        debit.description
                    ),
                    vec![
                        line(accounts::SUPPLIERS, expense.amount),
                        line(accounts::BANK, -expense.amount),
                    ],
                ));
            }
        }
        entries
    }

    /// Règlements de comptes de bilan depuis le relevé (lot 37) : `compte / 512` pour un débit,
    /// `512 / compte` pour un crédit, à la date du relevé, pièce `BQ-<uuid de la transaction>`.
    /// Ni charge ni produit : le solde d'un 401 ou d'un 444 repris tombe, la banque suit le
    /// relevé.
    fn settlement_entries(&self, bank_transactions: &[BankTransaction]) -> Vec<LedgerEntry> {
        let mut entries = Vec::new();
        for tx in bank_transactions
            .iter()
            .filter(|t| self.exercise.contains(t.occurred_on))
        {
            let Some(account) = &tx.settlement_account else {
                continue;
            };
            let account = Account::for_number(account.as_str(), tx.settlement_label.as_deref())
                .unwrap_or_else(|| Account {
                    number: Cow::Owned(account.to_string()),
                    label: Cow::Owned(account.to_string()),
                });
            let amount = Money::from_cents(tx.amount_cents);
            entries.extend(entry(
                Journal::Bank,
                tx.occurred_on,
                format!("BQ-{}", tx.id),
                format!(
                    "Règlement {} — relevé du {} ({})",
                    account.label,
                    format_date(tx.occurred_on),
                    tx.description
                ),
                vec![line(account.clone(), -amount), line(accounts::BANK, amount)],
            ));
        }
        entries
    }

    /// Affectation du résultat d'un exercice clos antérieur : le résultat sort de 120/129 vers
    /// la réserve légale (1061), les dividendes à payer (457) et le report à nouveau (110/119).
    /// Datée de l'approbation, ou du premier jour de l'exercice suivant tant que la décision
    /// n'est qu'un projet — dit dans le libellé.
    fn appropriation_entries(&self, records: &[FiscalYearRecord]) -> Vec<LedgerEntry> {
        let mut entries = Vec::new();
        for record in records.iter().filter(|r| r.ends_on < self.exercise.start()) {
            let (date, status) = match record.approved_on {
                Some(approved_on) => (
                    approved_on,
                    format!("approuvée le {}", format_date(approved_on)),
                ),
                None => (
                    record.ends_on.next_day().unwrap_or(record.ends_on),
                    "projet non approuvé".to_string(),
                ),
            };
            if !self.exercise.contains(date) {
                continue;
            }
            let carried = record.net_result - record.legal_reserve - record.dividends;
            let carry_account = if carried.is_negative() {
                accounts::RETAINED_DEBIT
            } else {
                accounts::RETAINED_CREDIT
            };
            entries.extend(entry(
                Journal::Misc,
                date,
                format!("OD-AFF-{}", record.ends_on.year()),
                format!(
                    "Affectation du résultat de l'exercice clos le {} ({status})",
                    format_date(record.ends_on)
                ),
                vec![
                    line(result_account(record.net_result), record.net_result),
                    line(accounts::LEGAL_RESERVE, -record.legal_reserve),
                    line(accounts::DIVIDENDS_DUE, -record.dividends),
                    line(carry_account, -carried),
                ],
            ));
        }
        entries
    }

    /// Rémunération du dirigeant sur l'exercice, réputée due et non décaissée : brut en 641,
    /// cotisations patronales en 645, contre 421 et 431. Le brut vient du profil ; si le total
    /// figé au snapshot ne s'y prête plus (profil modifié depuis), tout va en 641/421.
    fn director_entry(&self, profile: &CompanyProfile, total: Money) -> Option<LedgerEntry> {
        if total.is_zero() {
            return None;
        }
        let (gross, charges) = match director_gross(profile, self.exercise) {
            Some(gross) if !gross.is_negative() && gross <= total => (gross, total - gross),
            _ => (total, Money::ZERO),
        };
        entry(
            Journal::Misc,
            self.exercise.end(),
            "OD-REM",
            "Rémunération du dirigeant — coût employeur estimé, réputée due (aucun fait de paie \
             enregistré)",
            vec![
                line(accounts::DIRECTOR_PAY, gross),
                line(accounts::SOCIAL_CHARGES, charges),
                line(accounts::PAY_DUE, -gross),
                line(accounts::SOCIAL_DUE, -charges),
            ],
        )
    }

    /// Créance née du report en arrière du déficit (art. 220 quinquies) : 444 débité par le
    /// produit 699 — seulement depuis un snapshot, l'option étant une décision de clôture.
    fn carry_back_entry(&self, credit: Money) -> Option<LedgerEntry> {
        if credit.cents() <= 0 {
            return None;
        }
        entry(
            Journal::Misc,
            self.exercise.end(),
            "OD-RAD",
            "Créance d'IS née du report en arrière du déficit (art. 220 quinquies CGI, 2039-SD)",
            vec![
                line(accounts::CORPORATE_TAX_DUE, credit),
                line(accounts::CARRY_BACK_INCOME, -credit),
            ],
        )
    }

    /// Impôt sur les sociétés de l'exercice : 695 contre 444 (dette d'IS, les acomptes n'étant
    /// pas des faits datés).
    fn corporate_tax_entry(&self, tax: Money) -> Option<LedgerEntry> {
        if tax.cents() <= 0 {
            return None;
        }
        entry(
            Journal::Misc,
            self.exercise.end(),
            "OD-IS",
            "Impôt sur les sociétés de l'exercice",
            vec![
                line(accounts::CORPORATE_TAX, tax),
                line(accounts::CORPORATE_TAX_DUE, -tax),
            ],
        )
    }
}

/// Résultat porté par des écritures : produits − charges, soit l'opposé de la somme signée des
/// comptes de gestion (un produit est un crédit, donc négatif).
fn income_of(entries: &[LedgerEntry]) -> Money {
    -entries
        .iter()
        .flat_map(|e| e.lines.iter())
        .filter(|l| l.account.is_income_statement())
        .map(|l| l.amount)
        .sum::<Money>()
}

impl Ledger {
    /// Construit le grand livre d'un exercice à partir des faits du domaine — fonction pure,
    /// testable sans base. Seuls les faits *datés dans l'exercice* sont retenus : une facture
    /// par sa date d'émission, un encaissement par sa date de réception, son annulation par sa
    /// date d'annulation, une dépense par sa date d'engagement et son décaissement rapproché
    /// par la date du relevé ; les opérations de clôture sont datées du dernier jour.
    ///
    /// # Errors
    ///
    /// [`LedgerError::Overflow`] si la somme des montants en valeur absolue dépasse
    /// `i64::MAX` centimes — la seule condition sous laquelle une somme partielle pourrait
    /// déborder ensuite (balance, bilan, totaux). Vérifiée ici une fois pour toutes, en
    /// arithmétique large, pour que tout le reste du module puisse sommer sans se poser la
    /// question.
    pub fn build(facts: LedgerFacts<'_>) -> Result<Self, LedgerError> {
        let index = Facts {
            exercise: facts.exercise,
            clients_by_id: facts.clients.iter().map(|c| (c.id, c)).collect(),
            invoices_by_id: facts.invoices.iter().map(|i| (i.id, i)).collect(),
        };
        let mut entries = index.opening_entries(facts.opening);
        entries.extend(index.sales_entries(facts.invoices));
        entries.extend(index.bank_entries(facts.payments));
        entries.extend(index.purchase_entries(facts.expenses, facts.bank_transactions));
        entries.extend(index.settlement_entries(facts.bank_transactions));
        entries.extend(index.appropriation_entries(facts.appropriations));

        let director_total = facts.snapshot.map_or_else(
            || crate::accounting::director_cost(facts.profile, facts.exercise),
            |r| r.director_remuneration,
        );
        entries.extend(index.director_entry(facts.profile, director_total));
        let tax = facts.snapshot.map_or_else(
            || {
                corporate_income_tax(
                    impute_prior_losses(income_of(&entries), facts.prior_losses).taxable_result,
                )
            },
            |r| r.corporate_tax,
        );
        entries.extend(index.corporate_tax_entry(tax));
        let credit = facts.snapshot.map_or(Money::ZERO, |r| r.carry_back_credit);
        entries.extend(index.carry_back_entry(credit));

        // Ordre chronologique (exigé par le FEC), puis journal et pièce pour un ordre total
        // reproductible ; numérotation continue par journal une fois l'ordre fixé.
        entries.sort_by(|a, b| {
            (a.date, a.journal, &a.piece_ref, &a.label).cmp(&(
                b.date,
                b.journal,
                &b.piece_ref,
                &b.label,
            ))
        });
        let mut counters: HashMap<Journal, u32> = HashMap::new();
        for e in &mut entries {
            let n = counters.entry(e.journal).or_insert(0);
            *n += 1;
            e.number = *n;
        }

        let magnitude: u128 = entries
            .iter()
            .flat_map(|e| e.lines.iter())
            .map(|l| u128::from(l.amount.cents().unsigned_abs()))
            .sum();
        if magnitude > u128::from(i64::MAX.unsigned_abs()) {
            return Err(LedgerError::Overflow);
        }

        Ok(Self {
            exercise: facts.exercise,
            entries,
        })
    }

    #[must_use]
    pub fn total_debit(&self) -> Money {
        self.entries.iter().map(LedgerEntry::total_debit).sum()
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

    /// Résultat net de l'exercice : produits − charges, IS compris.
    #[must_use]
    pub fn net_result(&self) -> Money {
        income_of(&self.entries)
    }

    /// La balance des comptes : par compte général (les tiers de 411 sont regroupés), total
    /// débit, total crédit, solde — triée par numéro.
    #[must_use]
    pub fn trial_balance(&self) -> TrialBalance {
        let mut rows: BTreeMap<String, TrialBalanceRow> = BTreeMap::new();
        for l in self.entries.iter().flat_map(|e| e.lines.iter()) {
            let row = rows
                .entry(l.account.number.to_string())
                .or_insert_with(|| TrialBalanceRow {
                    account: l.account.clone(),
                    debit: Money::ZERO,
                    credit: Money::ZERO,
                    balance: Money::ZERO,
                });
            if l.amount.is_negative() {
                row.credit += -l.amount;
            } else {
                row.debit += l.amount;
            }
            row.balance += l.amount;
        }
        let rows: Vec<TrialBalanceRow> = rows.into_values().collect();
        let total_debit = rows.iter().map(|r| r.debit).sum();
        let total_credit = rows.iter().map(|r| r.credit).sum();
        TrialBalance {
            exercise: self.exercise,
            rows,
            total_debit,
            total_credit,
        }
    }

    /// Le bilan au dernier jour de l'exercice, présenté selon le tableau 2033-A.
    #[must_use]
    pub fn balance_sheet(&self) -> BalanceSheet {
        BalanceSheet::from_trial_balance(&self.trial_balance(), self.net_result())
    }

    /// Les à-nouveaux de l'exercice **suivant** : le solde de chaque compte de bilan, et le
    /// résultat net posé en 120 (bénéfice) ou 129 (perte) — équilibrés par construction, puisque
    /// la somme de tous les soldes est nulle et que le résultat est l'opposé des comptes de
    /// gestion.
    #[must_use]
    pub fn closing_opening_lines(&self) -> OpeningLines {
        let net = self.net_result();
        // Lot 37 : le report à nouveau se présente **net**, sur 110 (créditeur) ou 119
        // (débiteur), jamais les deux à la fois ; de même pour un résultat encore en 120/129 —
        // un bilan qui porte 110 C 6 350 et 119 D 1 226,90 est arithmétiquement juste mais ne
        // se lit pas.
        let mut retained = Money::ZERO;
        let mut prior_result = Money::ZERO;
        let mut lines: Vec<LedgerLine> = Vec::new();
        for r in self.trial_balance().rows {
            if r.account.is_income_statement() || r.balance.is_zero() {
                continue;
            }
            if r.account.number.starts_with("110") || r.account.number.starts_with("119") {
                retained += r.balance;
            } else if r.account.number.starts_with("120") || r.account.number.starts_with("129") {
                prior_result += r.balance;
            } else {
                lines.push(line(r.account, r.balance));
            }
        }
        if !retained.is_zero() {
            let account = if retained.is_negative() {
                accounts::RETAINED_CREDIT
            } else {
                accounts::RETAINED_DEBIT
            };
            lines.push(line(account, retained));
        }
        if !prior_result.is_zero() {
            // Un solde débiteur en 120/129 est une perte (129), un solde créditeur un bénéfice.
            lines.push(line(result_account(-prior_result), prior_result));
        }
        if !net.is_zero() {
            lines.push(line(result_account(net), -net));
        }
        OpeningLines {
            label: format!(
                "À-nouveaux — bilan de clôture dérivé au {}",
                format_date(self.exercise.end())
            ),
            lines,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Balance des comptes
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TrialBalanceRow {
    pub account: Account,
    pub debit: Money,
    pub credit: Money,
    /// `debit − credit` : positif = solde débiteur.
    pub balance: Money,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TrialBalance {
    pub exercise: FiscalYear,
    pub rows: Vec<TrialBalanceRow>,
    pub total_debit: Money,
    pub total_credit: Money,
}

// ---------------------------------------------------------------------------------------------
// Bilan (tableau 2033-A-SD)
// ---------------------------------------------------------------------------------------------

/// Rubriques de l'actif du bilan simplifié, dans l'ordre du formulaire. Chaque rubrique porte
/// un brut et des amortissements/dépréciations (colonnes 1 et 2 du 2033-A).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetRubric {
    Goodwill,
    Intangible,
    Tangible,
    Financial,
    RawMaterials,
    Goods,
    AdvancesPaid,
    Clients,
    OtherReceivables,
    Securities,
    Cash,
    Prepaid,
}

impl AssetRubric {
    pub const ALL: [Self; 12] = [
        Self::Goodwill,
        Self::Intangible,
        Self::Tangible,
        Self::Financial,
        Self::RawMaterials,
        Self::Goods,
        Self::AdvancesPaid,
        Self::Clients,
        Self::OtherReceivables,
        Self::Securities,
        Self::Cash,
        Self::Prepaid,
    ];

    /// Actif immobilisé (total I) ou actif circulant (total II).
    #[must_use]
    pub const fn is_fixed_asset(self) -> bool {
        matches!(
            self,
            Self::Goodwill | Self::Intangible | Self::Tangible | Self::Financial
        )
    }

    /// Cases (brut, amortissements) du 2033-A-SD.
    #[must_use]
    pub const fn cases(self) -> (&'static str, &'static str) {
        match self {
            Self::Goodwill => ("010", "012"),
            Self::Intangible => ("014", "016"),
            Self::Tangible => ("028", "030"),
            Self::Financial => ("040", "042"),
            Self::RawMaterials => ("050", "052"),
            Self::Goods => ("060", "062"),
            Self::AdvancesPaid => ("064", "066"),
            Self::Clients => ("068", "070"),
            Self::OtherReceivables => ("072", "074"),
            Self::Securities => ("080", "082"),
            Self::Cash => ("084", "086"),
            Self::Prepaid => ("092", "094"),
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Goodwill => "Fonds commercial",
            Self::Intangible => "Autres immobilisations incorporelles",
            Self::Tangible => "Immobilisations corporelles",
            Self::Financial => "Immobilisations financières",
            Self::RawMaterials => "Matières premières, approvisionnements, en cours",
            Self::Goods => "Marchandises",
            Self::AdvancesPaid => "Avances et acomptes versés sur commandes",
            Self::Clients => "Clients et comptes rattachés",
            Self::OtherReceivables => "Autres créances",
            Self::Securities => "Valeurs mobilières de placement",
            Self::Cash => "Disponibilités",
            Self::Prepaid => "Charges constatées d'avance",
        }
    }
}

/// Rubriques du passif du bilan simplifié, dans l'ordre du formulaire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LiabilityRubric {
    Capital,
    Revaluation,
    LegalReserve,
    RegulatedReserves,
    OtherReserves,
    RetainedEarnings,
    /// Résultat de l'exercice (case 136) — calculé, jamais lu dans un compte.
    Result,
    RegulatedProvisions,
    Provisions,
    Borrowings,
    AdvancesReceived,
    Suppliers,
    OtherDebts,
    Deferred,
}

impl LiabilityRubric {
    pub const ALL: [Self; 14] = [
        Self::Capital,
        Self::Revaluation,
        Self::LegalReserve,
        Self::RegulatedReserves,
        Self::OtherReserves,
        Self::RetainedEarnings,
        Self::Result,
        Self::RegulatedProvisions,
        Self::Provisions,
        Self::Borrowings,
        Self::AdvancesReceived,
        Self::Suppliers,
        Self::OtherDebts,
        Self::Deferred,
    ];

    /// Capitaux propres (total I), provisions (total II) ou dettes (total III).
    #[must_use]
    pub const fn section(self) -> LiabilitySection {
        match self {
            Self::Capital
            | Self::Revaluation
            | Self::LegalReserve
            | Self::RegulatedReserves
            | Self::OtherReserves
            | Self::RetainedEarnings
            | Self::Result
            | Self::RegulatedProvisions => LiabilitySection::Equity,
            Self::Provisions => LiabilitySection::Provisions,
            Self::Borrowings
            | Self::AdvancesReceived
            | Self::Suppliers
            | Self::OtherDebts
            | Self::Deferred => LiabilitySection::Debts,
        }
    }

    /// Case du 2033-A-SD.
    #[must_use]
    pub const fn case(self) -> &'static str {
        match self {
            Self::Capital => "120",
            Self::Revaluation => "124",
            Self::LegalReserve => "126",
            Self::RegulatedReserves => "130",
            Self::OtherReserves => "132",
            Self::RetainedEarnings => "134",
            Self::Result => "136",
            Self::RegulatedProvisions => "140",
            Self::Provisions => "154",
            Self::Borrowings => "156",
            Self::AdvancesReceived => "164",
            Self::Suppliers => "166",
            Self::OtherDebts => "172",
            Self::Deferred => "174",
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Capital => "Capital social ou individuel",
            Self::Revaluation => "Écarts de réévaluation",
            Self::LegalReserve => "Réserve légale",
            Self::RegulatedReserves => "Réserves réglementées",
            Self::OtherReserves => "Autres réserves",
            Self::RetainedEarnings => "Report à nouveau",
            Self::Result => "Résultat de l'exercice",
            Self::RegulatedProvisions => "Provisions réglementées",
            Self::Provisions => "Provisions pour risques et charges",
            Self::Borrowings => "Emprunts et dettes assimilées",
            Self::AdvancesReceived => "Avances et acomptes reçus sur commandes en cours",
            Self::Suppliers => "Fournisseurs et comptes rattachés",
            Self::OtherDebts => "Autres dettes",
            Self::Deferred => "Produits constatés d'avance",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LiabilitySection {
    Equity,
    Provisions,
    Debts,
}

/// Une ligne de l'actif : brut, amortissements et dépréciations, net.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AssetLine {
    pub rubric: AssetRubric,
    pub case_gross: &'static str,
    pub case_depreciation: &'static str,
    pub label: &'static str,
    pub gross: Money,
    pub depreciation: Money,
    pub net: Money,
}

/// Une ligne du passif.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct LiabilityLine {
    pub rubric: LiabilityRubric,
    pub case: &'static str,
    pub label: &'static str,
    pub amount: Money,
}

/// Le bilan au dernier jour de l'exercice, dans la présentation du tableau 2033-A-SD. Équilibré
/// par construction : `total_assets_net == total_liabilities` dès que chaque écriture l'est.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BalanceSheet {
    pub exercise: FiscalYear,
    pub assets: Vec<AssetLine>,
    pub liabilities: Vec<LiabilityLine>,
    /// Résultat net de l'exercice (case 136), aussi présent dans `liabilities`.
    pub result: Money,
    /// Total I de l'actif (immobilisé), net.
    pub fixed_assets_net: Money,
    /// Total II de l'actif (circulant), net.
    pub current_assets_net: Money,
    /// Total général brut (case 110).
    pub total_assets_gross: Money,
    /// Total des amortissements et dépréciations (case 112, colonne 2).
    pub total_depreciation: Money,
    /// Total général net (case 112, colonne 3).
    pub total_assets_net: Money,
    /// Total I du passif (case 142).
    pub total_equity: Money,
    /// Total II du passif (case 154).
    pub total_provisions: Money,
    /// Total III du passif (dettes).
    pub total_debts: Money,
    /// Total général du passif (case 180).
    pub total_liabilities: Money,
    /// Dont comptes courants d'associés (case 169), inclus dans « Autres dettes ».
    pub shareholder_current_accounts: Money,
}

enum Slot {
    Gross(AssetRubric),
    Depreciation(AssetRubric),
    Liability(LiabilityRubric),
    Income,
}

/// Classe 1 : capitaux propres, provisions réglementées, emprunts.
fn classify_equity(number: &str) -> LiabilityRubric {
    let p = |prefix: &str| number.starts_with(prefix);
    if p("1061") {
        LiabilityRubric::LegalReserve
    } else if p("1064") {
        LiabilityRubric::RegulatedReserves
    } else if p("104") || p("106") {
        LiabilityRubric::OtherReserves
    } else if p("105") {
        LiabilityRubric::Revaluation
    } else if p("10") {
        LiabilityRubric::Capital
    } else if p("11") || p("12") {
        LiabilityRubric::RetainedEarnings
    } else if p("13") || p("14") {
        LiabilityRubric::RegulatedProvisions
    } else if p("15") {
        LiabilityRubric::Provisions
    } else {
        LiabilityRubric::Borrowings
    }
}

/// Classes 2 et 3 : immobilisations et stocks, bruts ou amortis/dépréciés (28, 29, 39).
fn classify_fixed_assets_and_stocks(number: &str) -> Slot {
    let p = |prefix: &str| number.starts_with(prefix);
    if p("28") || p("29") {
        // 2807/2907 → fonds commercial, 280x/290x → incorporelles, 296/297 → financières, le
        // reste → corporelles.
        let rest = &number[2..];
        Slot::Depreciation(if rest.starts_with("07") {
            AssetRubric::Goodwill
        } else if rest.starts_with('0') {
            AssetRubric::Intangible
        } else if rest.starts_with('6') || rest.starts_with('7') {
            AssetRubric::Financial
        } else {
            AssetRubric::Tangible
        })
    } else if p("207") {
        Slot::Gross(AssetRubric::Goodwill)
    } else if p("20") {
        Slot::Gross(AssetRubric::Intangible)
    } else if p("26") || p("27") {
        Slot::Gross(AssetRubric::Financial)
    } else if p("2") {
        Slot::Gross(AssetRubric::Tangible)
    } else if p("397") {
        Slot::Depreciation(AssetRubric::Goods)
    } else if p("39") {
        Slot::Depreciation(AssetRubric::RawMaterials)
    } else if p("37") {
        Slot::Gross(AssetRubric::Goods)
    } else {
        Slot::Gross(AssetRubric::RawMaterials)
    }
}

/// Classe 4 : tiers — la rubrique dépend du sens du solde (un client créditeur est une avance
/// reçue, un fournisseur débiteur une créance).
fn classify_third_parties(number: &str, debit: bool) -> Slot {
    let p = |prefix: &str| number.starts_with(prefix);
    if p("491") {
        Slot::Depreciation(AssetRubric::Clients)
    } else if p("49") {
        Slot::Depreciation(AssetRubric::OtherReceivables)
    } else if p("409") && debit {
        Slot::Gross(AssetRubric::AdvancesPaid)
    } else if p("40") {
        if debit {
            Slot::Gross(AssetRubric::OtherReceivables)
        } else {
            Slot::Liability(LiabilityRubric::Suppliers)
        }
    } else if p("41") {
        if debit {
            Slot::Gross(AssetRubric::Clients)
        } else {
            Slot::Liability(LiabilityRubric::AdvancesReceived)
        }
    } else if p("486") && debit {
        Slot::Gross(AssetRubric::Prepaid)
    } else if p("487") && !debit {
        Slot::Liability(LiabilityRubric::Deferred)
    } else if debit {
        Slot::Gross(AssetRubric::OtherReceivables)
    } else {
        Slot::Liability(LiabilityRubric::OtherDebts)
    }
}

/// La rubrique d'un compte selon son numéro et, pour les classes 4 et 5, le sens de son solde
/// (une banque créditrice est un concours bancaire).
fn classify(number: &str, balance: Money) -> Slot {
    let debit = !balance.is_negative();
    match number.as_bytes().first().copied() {
        Some(b'6' | b'7') => Slot::Income,
        Some(b'1') => Slot::Liability(classify_equity(number)),
        Some(b'2' | b'3') => classify_fixed_assets_and_stocks(number),
        Some(b'4') => classify_third_parties(number, debit),
        Some(b'5') => {
            if number.starts_with("59") {
                Slot::Depreciation(AssetRubric::Securities)
            } else if !debit {
                Slot::Liability(LiabilityRubric::Borrowings)
            } else if number.starts_with("50") {
                Slot::Gross(AssetRubric::Securities)
            } else {
                Slot::Gross(AssetRubric::Cash)
            }
        }
        // Classes 8/9 (comptes spéciaux) ou numéro vide : par le sens du solde.
        _ => {
            if debit {
                Slot::Gross(AssetRubric::OtherReceivables)
            } else {
                Slot::Liability(LiabilityRubric::OtherDebts)
            }
        }
    }
}

impl BalanceSheet {
    /// Ventile les soldes d'une balance dans les rubriques du 2033-A ; `result` est le résultat
    /// net de l'exercice (l'opposé des comptes de gestion de la même balance).
    #[must_use]
    pub fn from_trial_balance(balance: &TrialBalance, result: Money) -> Self {
        let mut gross: BTreeMap<AssetRubric, Money> = BTreeMap::new();
        let mut depreciation: BTreeMap<AssetRubric, Money> = BTreeMap::new();
        let mut liabilities: BTreeMap<LiabilityRubric, Money> = BTreeMap::new();
        let mut shareholder_current_accounts = Money::ZERO;
        for row in balance.rows.iter().filter(|r| !r.balance.is_zero()) {
            match classify(&row.account.number, row.balance) {
                Slot::Income => {}
                Slot::Gross(r) => *gross.entry(r).or_insert(Money::ZERO) += row.balance,
                // Un compte d'amortissement est créditeur : sa dépréciation est l'opposé du solde.
                Slot::Depreciation(r) => {
                    *depreciation.entry(r).or_insert(Money::ZERO) += -row.balance;
                }
                Slot::Liability(r) => {
                    *liabilities.entry(r).or_insert(Money::ZERO) += -row.balance;
                    if row.account.number.starts_with("455") {
                        shareholder_current_accounts += -row.balance;
                    }
                }
            }
        }
        *liabilities
            .entry(LiabilityRubric::Result)
            .or_insert(Money::ZERO) += result;

        let assets: Vec<AssetLine> = AssetRubric::ALL
            .iter()
            .map(|&rubric| {
                let g = gross.get(&rubric).copied().unwrap_or(Money::ZERO);
                let d = depreciation.get(&rubric).copied().unwrap_or(Money::ZERO);
                let (case_gross, case_depreciation) = rubric.cases();
                AssetLine {
                    rubric,
                    case_gross,
                    case_depreciation,
                    label: rubric.label(),
                    gross: g,
                    depreciation: d,
                    net: g - d,
                }
            })
            .collect();
        let liabilities: Vec<LiabilityLine> = LiabilityRubric::ALL
            .iter()
            .map(|&rubric| LiabilityLine {
                rubric,
                case: rubric.case(),
                label: rubric.label(),
                amount: liabilities.get(&rubric).copied().unwrap_or(Money::ZERO),
            })
            .collect();

        let sum_assets = |fixed: bool| -> Money {
            assets
                .iter()
                .filter(|a| a.rubric.is_fixed_asset() == fixed)
                .map(|a| a.net)
                .sum()
        };
        let sum_section = |section: LiabilitySection| -> Money {
            liabilities
                .iter()
                .filter(|l| l.rubric.section() == section)
                .map(|l| l.amount)
                .sum()
        };
        let fixed_assets_net = sum_assets(true);
        let current_assets_net = sum_assets(false);
        let total_assets_gross = assets.iter().map(|a| a.gross).sum();
        let total_depreciation = assets.iter().map(|a| a.depreciation).sum();
        let total_equity = sum_section(LiabilitySection::Equity);
        let total_provisions = sum_section(LiabilitySection::Provisions);
        let total_debts = sum_section(LiabilitySection::Debts);
        Self {
            exercise: balance.exercise,
            assets,
            liabilities,
            result,
            fixed_assets_net,
            current_assets_net,
            total_assets_gross,
            total_depreciation,
            total_assets_net: fixed_assets_net + current_assets_net,
            total_equity,
            total_provisions,
            total_debts,
            total_liabilities: total_equity + total_provisions + total_debts,
            shareholder_current_accounts,
        }
    }

    /// `total actif net == total passif`.
    #[must_use]
    pub fn is_balanced(&self) -> bool {
        self.total_assets_net == self.total_liabilities
    }

    #[must_use]
    pub fn liability(&self, rubric: LiabilityRubric) -> Money {
        self.liabilities
            .iter()
            .find(|l| l.rubric == rubric)
            .map_or(Money::ZERO, |l| l.amount)
    }

    #[must_use]
    pub fn asset_net(&self, rubric: AssetRubric) -> Money {
        self.assets
            .iter()
            .find(|a| a.rubric == rubric)
            .map_or(Money::ZERO, |a| a.net)
    }
}

/// La vue JSON partagée par les trois façades (`year balance --json`, `fiscal.balance_sheet`,
/// ressource `freeflow://balance-sheet/{période}`) : balance des comptes et bilan 2033-A, montants
/// en centimes, suffixe `_cents` comme partout ailleurs dans les contrats JSON du dépôt.
#[must_use]
pub fn balance_json(balance: &TrialBalance, sheet: &BalanceSheet) -> serde_json::Value {
    serde_json::json!({
        "starts_on": format_date(sheet.exercise.start()),
        "ends_on": format_date(sheet.exercise.end()),
        "trial_balance": {
            "rows": balance.rows.iter().map(|r| serde_json::json!({
                "account": r.account.number,
                "label": r.account.label,
                "debit_cents": r.debit.cents(),
                "credit_cents": r.credit.cents(),
                "balance_cents": r.balance.cents(),
            })).collect::<Vec<_>>(),
            "total_debit_cents": balance.total_debit.cents(),
            "total_credit_cents": balance.total_credit.cents(),
        },
        "balance_sheet": {
            "assets": sheet.assets.iter().map(|a| serde_json::json!({
                "rubric": a.rubric,
                "case_gross": a.case_gross,
                "case_depreciation": a.case_depreciation,
                "label": a.label,
                "gross_cents": a.gross.cents(),
                "depreciation_cents": a.depreciation.cents(),
                "net_cents": a.net.cents(),
            })).collect::<Vec<_>>(),
            "fixed_assets_net_cents": sheet.fixed_assets_net.cents(),
            "current_assets_net_cents": sheet.current_assets_net.cents(),
            "total_assets_gross_cents": sheet.total_assets_gross.cents(),
            "total_depreciation_cents": sheet.total_depreciation.cents(),
            "total_assets_net_cents": sheet.total_assets_net.cents(),
            "liabilities": sheet.liabilities.iter().map(|l| serde_json::json!({
                "rubric": l.rubric,
                "case": l.case,
                "label": l.label,
                "amount_cents": l.amount.cents(),
            })).collect::<Vec<_>>(),
            "result_cents": sheet.result.cents(),
            "total_equity_cents": sheet.total_equity.cents(),
            "total_provisions_cents": sheet.total_provisions.cents(),
            "total_debts_cents": sheet.total_debts.cents(),
            "total_liabilities_cents": sheet.total_liabilities.cents(),
            "shareholder_current_accounts_cents": sheet.shareholder_current_accounts.cents(),
            "balanced": sheet.is_balanced(),
        },
    })
}

// ---------------------------------------------------------------------------------------------
// Requêtes
// ---------------------------------------------------------------------------------------------

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

/// Tout ce que la dérivation lit en base — chargé une fois, puis partagé par la chaîne des
/// exercices.
struct Loaded {
    profile: CompanyProfile,
    invoices: Vec<Invoice>,
    clients: Vec<Client>,
    payments: Vec<Payment>,
    expenses: Vec<Expense>,
    bank_transactions: Vec<BankTransaction>,
    opening: Option<OpeningBalance>,
    fiscal_years: Vec<FiscalYearRecord>,
}

impl Loaded {
    fn read(conn: &Connection) -> Result<Self, AppError> {
        let profile = company_profile(conn)?.ok_or_else(|| {
            AppError::Domain(
                "aucun profil d'entreprise défini : le grand livre en dépend (company set-profile)"
                    .to_string(),
            )
        })?;
        Ok(Self {
            profile,
            invoices: list_invoices(conn)?,
            clients: list_clients(conn)?,
            payments: list_payments(conn)?,
            expenses: list_expenses(conn)?,
            bank_transactions: list_bank_transactions(conn)?,
            opening: opening_balance(conn)?.map(|r| r.balance),
            fiscal_years: list_fiscal_years(conn)?,
        })
    }

    /// Le grand livre d'un exercice, ses à-nouveaux dérivés récursivement de l'exercice clos
    /// précédent quand il y en a un (voir le commentaire de module).
    fn ledger(&self, exercise: FiscalYear) -> Result<Ledger, AppError> {
        let previous = self
            .fiscal_years
            .iter()
            .find(|r| r.ends_on.next_day() == Some(exercise.start()));
        let (opening, appropriations): (Option<OpeningLines>, &[FiscalYearRecord]) = match (
            self.opening
                .as_ref()
                .filter(|o| o.opens_on == exercise.start()),
            previous,
        ) {
            (Some(opening), _) => (Some(OpeningLines::from_opening_balance(opening)), &[]),
            (None, Some(previous)) => {
                let chain_end = self
                    .fiscal_years
                    .iter()
                    .position(|r| r.id == previous.id)
                    .map_or(0, |i| i + 1);
                (
                    Some(self.ledger(previous.period())?.closing_opening_lines()),
                    &self.fiscal_years[..chain_end],
                )
            }
            (None, None) => (None, &[]),
        };
        let snapshot = self
            .fiscal_years
            .iter()
            .find(|r| r.starts_on == exercise.start() && r.ends_on == exercise.end());
        // Le stock de déficits à l'ouverture : celui d'après le dernier exercice clos avant
        // (déjà chaîné à la lecture), sinon les déficits repris au bilan d'ouverture s'il ouvre
        // ce jour-là — la même lecture tolérante que `fiscal_year::tax_losses_available`.
        let prior_losses = self
            .fiscal_years
            .iter()
            .filter(|r| r.ends_on < exercise.start())
            .max_by_key(|r| r.ends_on)
            .map_or_else(
                || {
                    self.opening
                        .as_ref()
                        .filter(|o| o.opens_on == exercise.start())
                        .map_or(Money::ZERO, |o| o.tax_losses)
                },
                |r| r.losses_carried_forward,
            );
        Ok(Ledger::build(LedgerFacts {
            profile: &self.profile,
            exercise,
            invoices: &self.invoices,
            clients: &self.clients,
            payments: &self.payments,
            expenses: &self.expenses,
            bank_transactions: &self.bank_transactions,
            opening,
            snapshot,
            appropriations,
            prior_losses,
        })?)
    }
}

/// Le grand livre dérivé d'un exercice, depuis la base — la requête que les façades partagent
/// (FEC, balance, bilan).
///
/// # Errors
///
/// `AppError::Domain` sans profil d'entreprise ou si les montants débordent
/// ([`LedgerError::Overflow`]) ; erreur de lecture SQLite sinon.
pub fn build_ledger(conn: &Connection, exercise: FiscalYear) -> Result<Ledger, AppError> {
    Loaded::read(conn)?.ledger(exercise)
}

/// Le grand livre de l'exercice clos dans l'année civile `period` (voir
/// [`exercise_ending_in`]), avec le profil qui l'identifie.
///
/// # Errors
///
/// `AppError::Domain` sans profil d'entreprise ; erreur de lecture SQLite sinon.
pub fn ledger_ending_in(
    conn: &Connection,
    period: i32,
) -> Result<(CompanyProfile, Ledger), AppError> {
    let loaded = Loaded::read(conn)?;
    let exercise = exercise_ending_in(conn, &loaded.profile, period)?;
    let ledger = loaded.ledger(exercise)?;
    Ok((loaded.profile, ledger))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        Address, BankTransactionId, FiscalYearId, InvoiceLine, InvoiceStatus, Siren, VatRate,
    };
    use proptest::prelude::*;
    use time::Month as TimeMonth;
    use time::OffsetDateTime;

    /// Un débit du relevé rapproché de `expense`, daté de `on` (lot 33).
    fn debit_for(expense: &Expense, on: Date) -> BankTransaction {
        BankTransaction {
            settlement_account: None,
            settlement_label: None,
            fitid: None,
            id: BankTransactionId::new(),
            occurred_on: on,
            amount_cents: -expense.amount.cents(),
            description: "CB FOURNISSEUR".to_string(),
            matched_invoice_id: None,
            matched_expense_id: Some(expense.id),
        }
    }

    fn date(year: i32, month: TimeMonth, day: u8) -> Date {
        Date::from_calendar_date(year, month, day).unwrap()
    }

    fn profile(director_gross: Option<i64>, ratio_bps: Option<u32>) -> CompanyProfile {
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
            share_capital: Some(Money::from_cents(100_000)),
            rcs_city: None,
            iban: None,
            fiscal_year_end: Some(FiscalYearEnd::CALENDAR),
            vat_regime: None,
            director_monthly_gross: director_gross.map(Money::from_cents),
            director_charge_ratio_bps: ratio_bps,
            president_name: None,
            sole_shareholder_name: None,
            sole_shareholder_address: None,
            share_count: None,
        }
    }

    fn invoice(client_id: ClientId, ht_cents: i64, issued_on: Date) -> Invoice {
        Invoice {
            id: InvoiceId::new(),
            number: format!("FA-{}-0001", issued_on.year()),
            client_id,
            mission_id: None,
            lines: vec![InvoiceLine {
                description: "Conseil".to_string(),
                quantity: 1.0,
                unit_price: Money::from_cents(ht_cents),
                vat_rate: VatRate::Standard,
            }],
            status: InvoiceStatus::Issued,
            issued_on,
            due_on: issued_on,
            previous_hash: None,
            hash: String::new(),
            credited_invoice_id: None,
        }
    }

    fn expense(ttc: i64, deductible: i64, on: Date) -> Expense {
        Expense {
            id: crate::domain::ExpenseId::new(),
            label: "Honoraires".to_string(),
            category: ExpenseCategory::Professional,
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

    fn opening_2026() -> OpeningBalance {
        OpeningBalance {
            opens_on: date(2026, TimeMonth::January, 1),
            source: Some("cabinet X".to_string()),
            lines: vec![
                "101000:Capital social:C:1000.00".parse().unwrap(),
                "106100:Réserve légale:C:60.00".parse().unwrap(),
                "110000:Report à nouveau:C:250.00".parse().unwrap(),
                "215400:Matériel:D:600.00".parse().unwrap(),
                "281540:Amortissement matériel:C:100.00".parse().unwrap(),
                "512000:Banque:D:810.00".parse().unwrap(),
            ],
            tax_losses: Money::ZERO,
        }
    }

    fn facts<'a>(
        profile: &'a CompanyProfile,
        exercise: FiscalYear,
        invoices: &'a [Invoice],
        expenses: &'a [Expense],
        opening: Option<OpeningLines>,
    ) -> LedgerFacts<'a> {
        LedgerFacts {
            profile,
            exercise,
            invoices,
            clients: &[],
            payments: &[],
            expenses,
            bank_transactions: &[],
            opening,
            snapshot: None,
            prior_losses: Money::ZERO,
            appropriations: &[],
        }
    }

    fn record(
        period: FiscalYear,
        net: i64,
        reserve: i64,
        dividends: i64,
        approved: Option<Date>,
    ) -> FiscalYearRecord {
        FiscalYearRecord {
            id: FiscalYearId::new(),
            starts_on: period.start(),
            ends_on: period.end(),
            revenue_ht: Money::ZERO,
            expenses: Money::ZERO,
            director_remuneration: Money::ZERO,
            result_before_tax: Money::from_cents(net),
            corporate_tax: Money::ZERO,
            net_result: Money::from_cents(net),
            legal_reserve: Money::from_cents(reserve),
            dividends: Money::from_cents(dividends),
            retained_earnings: Money::from_cents(net - reserve - dividends),
            approved_on: approved,
            revision: 1,
            created_at: OffsetDateTime::UNIX_EPOCH,
            losses_imputed: Money::ZERO,
            carried_back: Money::ZERO,
            carry_back_credit: Money::ZERO,
            losses_carried_forward: Money::ZERO,
        }
    }

    #[test]
    fn a_loss_making_exercise_with_an_opening_balance_yields_a_balanced_2033a_sheet() {
        // Le cas de l'analyse : société préexistante, aucune facture, une dépense.
        let p = profile(None, None);
        let exercise = FiscalYear::calendar(2026);
        let expenses = vec![expense(120_000, 20_000, date(2026, TimeMonth::March, 3))];
        let ledger = Ledger::build(facts(
            &p,
            exercise,
            &[],
            &expenses,
            Some(OpeningLines::from_opening_balance(&opening_2026())),
        ))
        .unwrap();

        // AN + AC, aucune OD : pas d'IS sur une perte, pas de rémunération.
        let journals: Vec<Journal> = ledger.entries.iter().map(|e| e.journal).collect();
        assert_eq!(journals, vec![Journal::Opening, Journal::Purchases]);
        assert!(ledger.entries.iter().all(LedgerEntry::is_balanced));
        assert_eq!(ledger.net_result(), Money::from_cents(-100_000));

        let balance = ledger.trial_balance();
        assert_eq!(balance.total_debit, balance.total_credit);
        let bank = balance
            .rows
            .iter()
            .find(|r| r.account.number == "512000")
            .unwrap();
        assert_eq!(bank.balance, Money::from_cents(81_000 - 120_000));
        assert_eq!(bank.debit, Money::from_cents(81_000));
        assert_eq!(bank.credit, Money::from_cents(120_000));
        assert_eq!(
            bank.account.label, "Banque",
            "le libellé saisi au bilan prime"
        );

        let sheet = ledger.balance_sheet();
        assert!(sheet.is_balanced(), "{sheet:#?}");
        // Actif : matériel 600 brut − 100 d'amortissement = 500 net ; créance de TVA 200 ;
        // banque créditrice (découvert) → passif.
        let tangible = sheet
            .assets
            .iter()
            .find(|a| a.rubric == AssetRubric::Tangible)
            .unwrap();
        assert_eq!(tangible.gross, Money::from_cents(60_000));
        assert_eq!(tangible.depreciation, Money::from_cents(10_000));
        assert_eq!(tangible.net, Money::from_cents(50_000));
        assert_eq!(tangible.case_gross, "028");
        assert_eq!(
            sheet.asset_net(AssetRubric::OtherReceivables),
            Money::from_cents(20_000)
        );
        assert_eq!(sheet.asset_net(AssetRubric::Cash), Money::ZERO);
        assert_eq!(sheet.total_assets_net, Money::from_cents(70_000));
        assert_eq!(
            sheet.liability(LiabilityRubric::Borrowings),
            Money::from_cents(39_000),
            "le découvert est un concours bancaire"
        );
        assert_eq!(
            sheet.liability(LiabilityRubric::Capital),
            Money::from_cents(100_000)
        );
        assert_eq!(
            sheet.liability(LiabilityRubric::LegalReserve),
            Money::from_cents(6_000)
        );
        assert_eq!(
            sheet.liability(LiabilityRubric::RetainedEarnings),
            Money::from_cents(25_000)
        );
        assert_eq!(
            sheet.liability(LiabilityRubric::Result),
            Money::from_cents(-100_000)
        );
        assert_eq!(sheet.total_equity, Money::from_cents(31_000));
        assert_eq!(sheet.total_liabilities, Money::from_cents(70_000));
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn closing_entries_book_director_pay_and_corporate_tax_then_chain_into_the_next_exercise() {
        // Brut 1 000 €/mois × 12, cotisations 50 % → 18 000 € ; CA 30 000 € HT.
        let p = profile(Some(100_000), Some(5_000));
        let exercise = FiscalYear::calendar(2026);
        let client_id = ClientId::new();
        let invoices = vec![invoice(
            client_id,
            3_000_000,
            date(2026, TimeMonth::June, 1),
        )];
        let ledger = Ledger::build(facts(
            &p,
            exercise,
            &invoices,
            &[],
            Some(OpeningLines::from_opening_balance(&opening_2026())),
        ))
        .unwrap();
        let journals: Vec<Journal> = ledger.entries.iter().map(|e| e.journal).collect();
        assert_eq!(
            journals,
            vec![
                Journal::Opening,
                Journal::Sales,
                Journal::Misc,
                Journal::Misc
            ]
        );
        let pay = ledger
            .entries
            .iter()
            .find(|e| e.piece_ref == "OD-REM")
            .unwrap();
        assert_eq!(pay.date, date(2026, TimeMonth::December, 31));
        assert_eq!(pay.lines[0].account, accounts::DIRECTOR_PAY);
        assert_eq!(pay.lines[0].amount, Money::from_cents(1_200_000));
        assert_eq!(pay.lines[1].account, accounts::SOCIAL_CHARGES);
        assert_eq!(pay.lines[1].amount, Money::from_cents(600_000));
        assert!(pay.is_balanced());
        // Résultat avant IS : 30 000 − 18 000 = 12 000 € → IS 15 % = 1 800 €.
        let tax = ledger
            .entries
            .iter()
            .find(|e| e.piece_ref == "OD-IS")
            .unwrap();
        assert_eq!(tax.lines[0].account, accounts::CORPORATE_TAX);
        assert_eq!(tax.lines[0].amount, Money::from_cents(180_000));
        assert_eq!(ledger.net_result(), Money::from_cents(1_020_000));

        let sheet = ledger.balance_sheet();
        assert!(sheet.is_balanced());
        assert_eq!(
            sheet.liability(LiabilityRubric::Result),
            Money::from_cents(1_020_000)
        );
        // Dettes : IS 1 800 + rémunérations dues 12 000 + URSSAF 6 000 + TVA collectée 6 000.
        assert_eq!(
            sheet.liability(LiabilityRubric::OtherDebts),
            Money::from_cents(2_580_000)
        );
        assert_eq!(
            sheet.asset_net(AssetRubric::Clients),
            Money::from_cents(3_600_000)
        );

        // L'exercice suivant s'ouvre sur ce bilan : le résultat est en 120, puis affecté.
        let next_exercise = FiscalYear::calendar(2027);
        let closed = record(
            exercise,
            1_020_000,
            5_000,
            100_000,
            Some(date(2027, TimeMonth::May, 15)),
        );
        let chain = [closed];
        let next = Ledger::build(LedgerFacts {
            profile: &p,
            exercise: next_exercise,
            invoices: &[],
            clients: &[],
            payments: &[],
            expenses: &[],
            bank_transactions: &[],
            opening: Some(ledger.closing_opening_lines()),
            snapshot: None,
            prior_losses: Money::ZERO,
            appropriations: &chain,
        })
        .unwrap();
        let an = &next.entries[0];
        assert_eq!(an.journal, Journal::Opening);
        assert!(an.is_balanced());
        assert!(an.label.contains("bilan de clôture dérivé au 2026-12-31"));
        let profit = an
            .lines
            .iter()
            .find(|l| l.account.number == "120000")
            .unwrap();
        assert_eq!(profit.amount, Money::from_cents(-1_020_000));
        assert!(
            an.lines.iter().all(|l| !l.account.is_income_statement()),
            "les comptes de gestion sont soldés dans le résultat"
        );
        // Affectation datée de l'AG : 120 débité, 1061 + 457 + 110 crédités.
        let appropriation = next
            .entries
            .iter()
            .find(|e| e.piece_ref == "OD-AFF-2026")
            .unwrap();
        assert_eq!(appropriation.date, date(2027, TimeMonth::May, 15));
        assert!(appropriation.label.contains("approuvée le 2027-05-15"));
        assert!(appropriation.is_balanced());
        let amounts: Vec<(&str, i64)> = appropriation
            .lines
            .iter()
            .map(|l| (l.account.number.as_ref(), l.amount.cents()))
            .collect();
        assert_eq!(
            amounts,
            vec![
                ("120000", 1_020_000),
                ("106100", -5_000),
                ("457000", -100_000),
                ("110000", -915_000),
            ]
        );
        // Rémunération à nouveau due en 2027 (profil inchangé), pas d'IS (perte).
        let next_sheet = next.balance_sheet();
        assert!(next_sheet.is_balanced());
        assert_eq!(
            next_sheet.liability(LiabilityRubric::RetainedEarnings),
            Money::from_cents(25_000 + 915_000)
        );
        assert_eq!(
            next_sheet.liability(LiabilityRubric::LegalReserve),
            Money::from_cents(6_000 + 5_000)
        );
        assert_eq!(
            next_sheet.liability(LiabilityRubric::Result),
            Money::from_cents(-1_800_000)
        );
    }

    #[test]
    fn a_loss_is_appropriated_to_a_debit_carry_forward_on_the_first_day_when_not_approved() {
        let p = profile(None, None);
        let exercise = FiscalYear::calendar(2027);
        let closed = record(FiscalYear::calendar(2026), -100_000, 0, 0, None);
        let chain = [closed];
        let next = Ledger::build(LedgerFacts {
            profile: &p,
            exercise,
            invoices: &[],
            clients: &[],
            payments: &[],
            expenses: &[],
            bank_transactions: &[],
            opening: Some(OpeningLines {
                label: "AN".to_string(),
                lines: vec![
                    line(accounts::LOSS, Money::from_cents(100_000)),
                    line(accounts::BANK, Money::from_cents(-100_000)),
                ],
            }),
            snapshot: None,
            prior_losses: Money::ZERO,
            appropriations: &chain,
        })
        .unwrap();
        let appropriation = &next.entries[1];
        assert_eq!(appropriation.date, date(2027, TimeMonth::January, 1));
        assert!(appropriation.label.contains("projet non approuvé"));
        let amounts: Vec<(&str, i64)> = appropriation
            .lines
            .iter()
            .map(|l| (l.account.number.as_ref(), l.amount.cents()))
            .collect();
        assert_eq!(amounts, vec![("129000", -100_000), ("119000", 100_000)]);
        let sheet = next.balance_sheet();
        assert!(sheet.is_balanced());
        assert_eq!(
            sheet.liability(LiabilityRubric::RetainedEarnings),
            Money::from_cents(-100_000)
        );
    }

    #[test]
    fn a_snapshot_fixes_the_tax_instead_of_recomputing_it() {
        let p = profile(None, None);
        let exercise = FiscalYear::calendar(2026);
        let client_id = ClientId::new();
        let invoices = vec![invoice(
            client_id,
            1_000_000,
            date(2026, TimeMonth::June, 1),
        )];
        let snapshot = FiscalYearRecord {
            corporate_tax: Money::from_cents(123_400),
            ..record(exercise, 0, 0, 0, None)
        };
        let ledger = Ledger::build(LedgerFacts {
            snapshot: Some(&snapshot),
            prior_losses: Money::ZERO,
            ..facts(&p, exercise, &invoices, &[], None)
        })
        .unwrap();
        let tax = ledger
            .entries
            .iter()
            .find(|e| e.piece_ref == "OD-IS")
            .unwrap();
        assert_eq!(tax.lines[0].amount, Money::from_cents(123_400));
    }

    #[test]
    fn prior_losses_lower_the_recomputed_tax_of_an_unclosed_exercise() {
        // Bénéfice 10 000 € ; 4 000 € de déficits antérieurs → IS 15 % sur 6 000 € = 900 €
        // (au lieu de 1 500 €), le même calcul que la clôture fera (lot 32).
        let p = profile(None, None);
        let exercise = FiscalYear::calendar(2026);
        let invoices = vec![invoice(
            ClientId::new(),
            1_000_000,
            date(2026, TimeMonth::June, 1),
        )];
        let ledger = Ledger::build(LedgerFacts {
            prior_losses: Money::from_cents(400_000),
            ..facts(&p, exercise, &invoices, &[], None)
        })
        .unwrap();
        let tax = ledger
            .entries
            .iter()
            .find(|e| e.piece_ref == "OD-IS")
            .unwrap();
        assert_eq!(tax.lines[0].amount, Money::from_cents(90_000));
        assert_eq!(ledger.net_result(), Money::from_cents(910_000));
        assert!(ledger.entries.iter().all(LedgerEntry::is_balanced));
    }

    #[test]
    fn a_carry_back_credit_from_the_snapshot_is_a_444_receivable_against_699() {
        // Exercice déficitaire (dépense de 800 € nets) clos avec report en arrière : la créance
        // de 120 € est un produit, le résultat net passe de −800 à −680 €, et 444 débiteur est
        // une créance à l'actif — bilan équilibré.
        let p = profile(None, None);
        let exercise = FiscalYear::calendar(2027);
        let expenses = vec![expense(96_000, 16_000, date(2027, TimeMonth::March, 5))];
        let snapshot = FiscalYearRecord {
            result_before_tax: Money::from_cents(-80_000),
            carried_back: Money::from_cents(80_000),
            carry_back_credit: Money::from_cents(12_000),
            net_result: Money::from_cents(-68_000),
            ..record(exercise, -68_000, 0, 0, None)
        };
        let ledger = Ledger::build(LedgerFacts {
            snapshot: Some(&snapshot),
            ..facts(&p, exercise, &[], &expenses, None)
        })
        .unwrap();
        let credit = ledger
            .entries
            .iter()
            .find(|e| e.piece_ref == "OD-RAD")
            .expect("écriture de report en arrière");
        assert_eq!(credit.journal, Journal::Misc);
        assert_eq!(credit.date, exercise.end());
        assert_eq!(credit.lines[0].account, accounts::CORPORATE_TAX_DUE);
        assert_eq!(credit.lines[0].amount, Money::from_cents(12_000));
        assert_eq!(credit.lines[1].account, accounts::CARRY_BACK_INCOME);
        assert_eq!(credit.lines[1].amount, Money::from_cents(-12_000));
        assert!(credit.is_balanced());
        assert!(
            ledger.entries.iter().all(|e| e.piece_ref != "OD-IS"),
            "aucun IS sur un déficit"
        );
        assert_eq!(ledger.net_result(), Money::from_cents(-68_000));

        let sheet = ledger.balance_sheet();
        assert!(sheet.is_balanced(), "{sheet:#?}");
        let receivables = sheet
            .assets
            .iter()
            .find(|a| a.rubric == AssetRubric::OtherReceivables)
            .unwrap();
        // TVA déductible 160 € + créance de report en arrière 120 €.
        assert_eq!(receivables.gross, Money::from_cents(28_000));
        assert_eq!(
            sheet.liability(LiabilityRubric::Result),
            Money::from_cents(-68_000)
        );
    }

    #[test]
    fn every_2033a_rubric_is_reachable_and_classification_follows_the_balance_sign() {
        let cases = [
            ("207000", 1, "gross:goodwill"),
            ("205000", 1, "gross:intangible"),
            ("218300", 1, "gross:tangible"),
            ("261000", 1, "gross:financial"),
            ("280500", -1, "dep:intangible"),
            ("281830", -1, "dep:tangible"),
            ("296100", -1, "dep:financial"),
            ("310000", 1, "gross:raw_materials"),
            ("370000", 1, "gross:goods"),
            ("409100", 1, "gross:advances_paid"),
            ("401000", -1, "liab:suppliers"),
            ("411000", 1, "gross:clients"),
            ("419000", -1, "liab:advances_received"),
            ("445660", 1, "gross:other_receivables"),
            ("445710", -1, "liab:other_debts"),
            ("455000", -1, "liab:other_debts"),
            ("486000", 1, "gross:prepaid"),
            ("487000", -1, "liab:deferred"),
            ("491000", -1, "dep:clients"),
            ("503000", 1, "gross:securities"),
            ("512000", 1, "gross:cash"),
            ("512000", -1, "liab:borrowings"),
            ("164000", -1, "liab:borrowings"),
            ("101000", -1, "liab:capital"),
            ("106100", -1, "liab:legal_reserve"),
            ("106400", -1, "liab:regulated_reserves"),
            ("106800", -1, "liab:other_reserves"),
            ("105000", -1, "liab:revaluation"),
            ("110000", -1, "liab:retained_earnings"),
            ("129000", 1, "liab:retained_earnings"),
            ("145000", -1, "liab:regulated_provisions"),
            ("151000", -1, "liab:provisions"),
            ("606300", 1, "income"),
            ("706000", -1, "income"),
        ];
        for (number, sign, expected) in cases {
            let got = match classify(number, Money::from_cents(sign)) {
                Slot::Income => "income".to_string(),
                Slot::Gross(r) => format!(
                    "gross:{}",
                    serde_json::to_value(r).unwrap().as_str().unwrap()
                ),
                Slot::Depreciation(r) => {
                    format!("dep:{}", serde_json::to_value(r).unwrap().as_str().unwrap())
                }
                Slot::Liability(r) => format!(
                    "liab:{}",
                    serde_json::to_value(r).unwrap().as_str().unwrap()
                ),
            };
            assert_eq!(got, expected, "compte {number} (signe {sign})");
        }
    }

    fn account_strategy() -> impl Strategy<Value = String> {
        // Comptes de bilan plausibles, toutes classes 1 à 5, sur 6 chiffres.
        prop::sample::select(vec![
            "101000", "106100", "110000", "119000", "120000", "129000", "164000", "215400",
            "281540", "401000", "409100", "411000", "419000", "445660", "445710", "455000",
            "486000", "487000", "491000", "503000", "512000", "530000",
        ])
        .prop_map(str::to_string)
    }

    proptest! {
        /// Quels que soient les soldes de bilan et les faits, le bilan 2033-A est équilibré et
        /// les à-nouveaux dérivés le sont aussi.
        #[test]
        fn the_derived_balance_sheet_always_balances(
            balances in prop::collection::vec((account_strategy(), -5_000_000i64..5_000_000), 0..12),
            invoices_ht in prop::collection::vec(-1_000_000i64..5_000_000, 0..4),
            expenses_ttc in prop::collection::vec((1i64..1_000_000, 0i64..100_000), 0..4),
            director in prop::option::of(0i64..500_000),
        ) {
            let p = profile(director, Some(4_500));
            let exercise = FiscalYear::calendar(2026);
            // Un bilan d'ouverture équilibré : la banque absorbe le solde.
            let mut lines: Vec<LedgerLine> = balances
                .iter()
                .map(|(n, cents)| line(Account { number: Cow::Owned(n.clone()), label: Cow::Borrowed("x") }, Money::from_cents(*cents)))
                .collect();
            let sum: Money = lines.iter().map(|l| l.amount).sum();
            lines.push(line(accounts::BANK, -sum));
            let client_id = ClientId::new();
            let invoices: Vec<Invoice> = invoices_ht
                .iter()
                .map(|ht| invoice(client_id, *ht, date(2026, TimeMonth::March, 1)))
                .collect();
            let expenses: Vec<Expense> = expenses_ttc
                .iter()
                .map(|(ttc, ded)| expense(*ttc, (*ded).min(*ttc), date(2026, TimeMonth::April, 1)))
                .collect();
            let ledger = Ledger::build(facts(
                &p,
                exercise,
                &invoices,
                &expenses,
                Some(OpeningLines { label: "AN".to_string(), lines }),
            )).unwrap();
            prop_assert!(ledger.entries.iter().all(LedgerEntry::is_balanced));
            let balance = ledger.trial_balance();
            prop_assert_eq!(balance.total_debit, balance.total_credit);
            let sheet = ledger.balance_sheet();
            prop_assert!(sheet.is_balanced(), "{:#?}", sheet);
            prop_assert_eq!(sheet.total_assets_net, sheet.total_assets_gross - sheet.total_depreciation);
            prop_assert_eq!(sheet.liability(LiabilityRubric::Result), ledger.net_result());
            let next = ledger.closing_opening_lines();
            let carried: Money = next.lines.iter().map(|l| l.amount).sum();
            prop_assert_eq!(carried, Money::ZERO);
        }
    }

    // --- Sur base : la chaîne d'un exercice clos sur le suivant. ---

    #[test]
    #[allow(clippy::too_many_lines)]
    fn on_a_real_vault_the_next_exercise_opens_on_the_derived_closing_of_a_closed_one() {
        use crate::app::{Actor, ExecutionContext, Executor};
        use crate::billing::EmitInvoice;
        use crate::company::SetCompanyProfile;
        use crate::fiscal_year::CloseFiscalYear;
        use crate::opening_balance::RecordOpeningBalance;
        use crate::store::{Passphrase, Store};

        let dir = std::env::temp_dir().join(format!(
            "freeflow-ledger-chain-{}-{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ));
        let mut store = Store::create(&dir.join("vault.db"), &Passphrase::from("s3cret")).unwrap();
        let human = ExecutionContext::new(Actor::Human, false);
        let p = profile(None, None);
        Executor::new(&mut store)
            .execute(
                &SetCompanyProfile {
                    name: p.name.clone(),
                    legal_form: p.legal_form.clone(),
                    siren: p.siren,
                    vat_number: None,
                    address: p.address.clone(),
                    share_capital: p.share_capital,
                    rcs_city: None,
                    iban: None,
                    fiscal_year_end: Some(FiscalYearEnd::CALENDAR),
                    vat_regime: None,
                    director_monthly_gross: None,
                    director_charge_ratio_bps: None,
                    president_name: None,
                    sole_shareholder_name: None,
                    sole_shareholder_address: None,
                    share_count: None,
                },
                &human,
            )
            .unwrap();
        Executor::new(&mut store)
            .execute(
                &RecordOpeningBalance {
                    opens_on: date(2026, TimeMonth::January, 1),
                    source: None,
                    lines: vec![
                        "101000:Capital social:C:1000.00".parse().unwrap(),
                        "512000:Banque:D:1000.00".parse().unwrap(),
                    ],
                    tax_losses: Money::ZERO,
                },
                &human,
            )
            .unwrap();
        let client_id = ClientId::new();
        store
            .connection()
            .execute(
                "INSERT INTO clients (id, name, created_at) VALUES (?1, 'Acme', '2026-01-01T00:00:00Z')",
                [client_id.to_string()],
            )
            .unwrap();
        Executor::new(&mut store)
            .execute(
                &EmitInvoice {
                    client_id,
                    mission_id: None,
                    lines: vec![InvoiceLine {
                        description: "Conseil".to_string(),
                        quantity: 1.0,
                        unit_price: Money::from_cents(100_000),
                        vat_rate: VatRate::Standard,
                    }],
                    issued_on: date(2026, TimeMonth::June, 1),
                    payment_terms_days: 30,
                },
                &human,
            )
            .unwrap();

        // Avant clôture : l'IS est recalculé (150 €), le bilan 2026 est équilibré.
        let (_, before) = ledger_ending_in(store.connection(), 2026).unwrap();
        assert_eq!(before.net_result(), Money::from_cents(85_000));
        assert!(before.balance_sheet().is_balanced());
        // 2027 sans exercice clos entre les deux : aucun à-nouveau, la chaîne s'arrête.
        let (_, orphan) = ledger_ending_in(store.connection(), 2027).unwrap();
        assert!(orphan.entries.is_empty(), "{:?}", orphan.entries);

        Executor::new(&mut store)
            .execute(
                &CloseFiscalYear {
                    starts_on: date(2026, TimeMonth::January, 1),
                    ends_on: date(2026, TimeMonth::December, 31),
                    legal_reserve: Money::from_cents(5_000),
                    dividends: Money::ZERO,
                    carry_back: false,
                    today: None,
                },
                &human,
            )
            .unwrap();

        // Après clôture : 2027 s'ouvre sur le bilan dérivé de 2026, résultat en 120 puis
        // affecté au premier jour (projet non approuvé).
        let (_, next) = ledger_ending_in(store.connection(), 2027).unwrap();
        let journals: Vec<Journal> = next.entries.iter().map(|e| e.journal).collect();
        assert_eq!(journals, vec![Journal::Opening, Journal::Misc]);
        let an = &next.entries[0];
        assert!(an.is_balanced());
        assert_eq!(
            an.lines
                .iter()
                .find(|l| l.account.number == "120000")
                .unwrap()
                .amount,
            Money::from_cents(-85_000)
        );
        assert_eq!(next.entries[1].piece_ref, "OD-AFF-2026");
        let sheet = next.balance_sheet();
        assert!(sheet.is_balanced());
        assert_eq!(
            sheet.liability(LiabilityRubric::LegalReserve),
            Money::from_cents(5_000)
        );
        assert_eq!(
            sheet.liability(LiabilityRubric::RetainedEarnings),
            Money::from_cents(80_000)
        );
        assert_eq!(sheet.liability(LiabilityRubric::Result), Money::ZERO);
    }

    // --- Rapprochement bancaire des dépenses (lot 33). ---

    #[test]
    fn a_reconciled_expense_is_charged_at_its_date_and_paid_at_the_statement_date() {
        // Honoraires de 960 € TTC engagés le 28 décembre 2026, débités le 4 janvier 2027 : la
        // charge (622600 + 445660 contre 401) est dans 2026, le décaissement (401 contre 512)
        // dans 2027 ; au 31 décembre 2026, 401 créditeur est une dette fournisseur (case 166)
        // et la banque n'a pas encore bougé.
        let p = profile(None, None);
        let mut fees = expense(96_000, 16_000, date(2026, TimeMonth::December, 28));
        fees.category = ExpenseCategory::Fees;
        let mut unreconciled = expense(6_000, 0, date(2026, TimeMonth::June, 1));
        unreconciled.label = "Fournitures".to_string();
        let expenses = vec![fees.clone(), unreconciled];
        let debits = vec![debit_for(&fees, date(2027, TimeMonth::January, 4))];

        let y2026 = Ledger::build(LedgerFacts {
            bank_transactions: &debits,
            ..facts(&p, FiscalYear::calendar(2026), &[], &expenses, None)
        })
        .unwrap();
        let purchases: Vec<&LedgerEntry> = y2026
            .entries
            .iter()
            .filter(|e| e.journal == Journal::Purchases)
            .collect();
        assert_eq!(purchases.len(), 2);
        let charged = purchases
            .iter()
            .find(|e| e.label == "Honoraires")
            .expect("la charge des honoraires est dans 2026");
        assert_eq!(charged.lines[0].account, accounts::FEES);
        assert_eq!(charged.lines[0].amount, Money::from_cents(80_000));
        assert_eq!(charged.lines[2].account, accounts::SUPPLIERS);
        assert_eq!(charged.lines[2].amount, Money::from_cents(-96_000));
        let direct = purchases.iter().find(|e| e.label != "Honoraires").unwrap();
        assert_eq!(
            direct.lines[1].account,
            accounts::BANK,
            "une dépense non rapprochée reste réputée payée à sa date"
        );
        assert!(
            y2026.entries.iter().all(|e| e.journal != Journal::Bank),
            "aucun décaissement en 2026 : le relevé le date de 2027"
        );
        let sheet = y2026.balance_sheet();
        assert_eq!(
            sheet.liability(LiabilityRubric::Suppliers),
            Money::from_cents(96_000)
        );
        assert_eq!(sheet.asset_net(AssetRubric::Cash), Money::ZERO);
        assert_eq!(
            sheet.liability(LiabilityRubric::Borrowings),
            Money::from_cents(6_000),
            "seule la dépense non rapprochée a touché la banque (créditrice → concours)"
        );
        assert_eq!(sheet.total_assets_net, sheet.total_liabilities);

        let y2027 = Ledger::build(LedgerFacts {
            bank_transactions: &debits,
            ..facts(&p, FiscalYear::calendar(2027), &[], &expenses, None)
        })
        .unwrap();
        assert!(
            y2027
                .entries
                .iter()
                .all(|e| e.journal != Journal::Purchases),
            "la charge n'est pas dans 2027"
        );
        let paid = y2027
            .entries
            .iter()
            .find(|e| e.journal == Journal::Bank)
            .expect("le décaissement est dans 2027");
        assert_eq!(paid.date, date(2027, TimeMonth::January, 4));
        assert_eq!(paid.piece_ref, charged.piece_ref);
        assert_eq!(paid.lines[0].account, accounts::SUPPLIERS);
        assert_eq!(paid.lines[0].amount, Money::from_cents(96_000));
        assert_eq!(paid.lines[1].account, accounts::BANK);
        assert!(paid.label.contains("CB FOURNISSEUR"));
        assert!(y2027.entries.iter().all(LedgerEntry::is_balanced));
    }

    #[test]
    fn the_new_categories_have_their_own_charge_accounts() {
        assert_eq!(charge_account(ExpenseCategory::Fees).number, "622600");
        assert_eq!(
            charge_account(ExpenseCategory::BankCharges).number,
            "627000"
        );
        for category in ExpenseCategory::ALL {
            assert_eq!(charge_account(category).class(), 6);
        }
    }

    /// Lot 36 : des montants dont la somme absolue dépasse `i64::MAX` centimes sont refusés à
    /// la construction, plutôt que de faire paniquer la première balance.
    #[test]
    fn a_ledger_whose_amounts_cannot_be_totalled_is_refused() {
        let p = profile(None, None);
        let exercise = FiscalYear::new(
            Date::from_calendar_date(2026, TimeMonth::January, 1).unwrap(),
            Date::from_calendar_date(2026, TimeMonth::December, 31).unwrap(),
        );
        let huge = Money::from_cents(i64::MAX / 2 + 1);
        let opening = OpeningLines {
            label: "AN".to_string(),
            lines: vec![
                line(Account::fixed("512000", "Banque"), huge),
                line(Account::fixed("101000", "Capital"), -huge),
            ],
        };
        assert_eq!(
            Ledger::build(facts(&p, exercise, &[], &[], Some(opening))).unwrap_err(),
            LedgerError::Overflow
        );
    }

    // --- Lot 37 : règlements de comptes de bilan, libellés, pièces, report à nouveau net. ---

    fn settled(
        on: Date,
        amount_cents: i64,
        description: &str,
        account: &str,
        label: Option<&str>,
    ) -> BankTransaction {
        BankTransaction {
            id: BankTransactionId::new(),
            occurred_on: on,
            amount_cents,
            description: description.to_string(),
            matched_invoice_id: None,
            matched_expense_id: None,
            settlement_account: Some(account.parse().unwrap()),
            settlement_label: label.map(str::to_string),
            fitid: None,
        }
    }

    /// Le scénario de l'expert-comptable (audit du 2 septembre 2026), posé à la main : bilan
    /// repris au 1er octobre 2025 avec 401 C 600, 444 C 1 200, 455 C 500, 445670 D 210, banque
    /// D 9 540 ; dans l'exercice, le débit de 600 règle le 401, celui de 1 200 le 444, sans
    /// charge. Attendu : 401 = 0, 444 = 0, banque 9 540 − 1 800 = 7 740, résultat nul, deux
    /// écritures `BQ` équilibrées, pièces `BQ-<uuid>`.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn settling_reprised_debts_clears_them_without_any_charge() {
        let p = profile(None, None);
        let exercise = FiscalYear::new(
            date(2025, TimeMonth::October, 1),
            date(2026, TimeMonth::September, 30),
        );
        let opening = OpeningBalance {
            opens_on: date(2025, TimeMonth::October, 1),
            source: Some("bilan au 30/09/2025, cabinet".to_string()),
            lines: vec![
                "101000:Capital social:C:1000.00".parse().unwrap(),
                "106100:Réserve légale:C:100.00".parse().unwrap(),
                "110000:Report à nouveau:C:6350.00".parse().unwrap(),
                "401000:Fournisseurs (honoraires cabinet, facture 09/2025):C:600.00"
                    .parse()
                    .unwrap(),
                "444000:État - IS à payer (solde IS 2025):C:1200.00"
                    .parse()
                    .unwrap(),
                "455000:Compte courant d'associé:C:500.00".parse().unwrap(),
                "445670:Crédit de TVA:D:210.00".parse().unwrap(),
                "512000:Banque:D:9540.00".parse().unwrap(),
            ],
            tax_losses: Money::ZERO,
        };
        let transactions = vec![
            settled(
                date(2025, TimeMonth::October, 15),
                -60_000,
                "VIR CABINET",
                "401000",
                None,
            ),
            settled(
                date(2026, TimeMonth::January, 15),
                -120_000,
                "PRLV DGFIP IS",
                "444000",
                None,
            ),
        ];
        let ledger = Ledger::build(LedgerFacts {
            bank_transactions: &transactions,
            ..facts(
                &p,
                exercise,
                &[],
                &[],
                Some(OpeningLines::from_opening_balance(&opening)),
            )
        })
        .unwrap();
        assert!(ledger.entries.iter().all(LedgerEntry::is_balanced));
        let settlements: Vec<_> = ledger
            .entries
            .iter()
            .filter(|e| e.piece_ref.starts_with("BQ-"))
            .collect();
        assert_eq!(settlements.len(), 2);
        assert!(settlements.iter().all(|e| e.journal == Journal::Bank));
        assert_eq!(
            settlements[0].piece_ref,
            format!("BQ-{}", transactions[0].id)
        );
        assert!(
            settlements[0]
                .label
                .starts_with("Règlement Fournisseurs — relevé du 2025-10-15"),
            "{}",
            settlements[0].label
        );
        let balance = ledger.trial_balance();
        let of = |n: &str| {
            balance
                .rows
                .iter()
                .find(|r| r.account.number == n)
                .map_or(Money::ZERO, |r| r.balance)
        };
        assert_eq!(of("401000"), Money::ZERO);
        assert_eq!(of("444000"), Money::ZERO);
        assert_eq!(of("512000"), Money::from_cents(774_000));
        assert_eq!(of("455000"), Money::from_cents(-50_000));
        assert_eq!(ledger.net_result(), Money::ZERO);
        let sheet = ledger.balance_sheet();
        assert!(sheet.is_balanced());
        assert_eq!(sheet.liability(LiabilityRubric::Suppliers), Money::ZERO);
        assert_eq!(sheet.total_liabilities, Money::from_cents(795_000));

        // Un libellé par compte : celui du plan fixe, jamais celui saisi au bilan d'ouverture.
        let supplier_labels: std::collections::BTreeSet<&str> = ledger
            .entries
            .iter()
            .flat_map(|e| e.lines.iter())
            .filter(|l| l.account.number == "401000")
            .map(|l| l.account.label.as_ref())
            .collect();
        assert_eq!(
            supplier_labels.into_iter().collect::<Vec<_>>(),
            vec!["Fournisseurs"]
        );
        // Un compte hors plan garde son libellé saisi.
        let custom = Account::for_number("467100", Some("Débiteurs divers")).unwrap();
        assert_eq!(custom.label, "Débiteurs divers");
        assert!(Account::for_number("467100", None).is_none());
    }

    /// Un crédit du relevé règle aussi : le remboursement du crédit de TVA repris (445670 D
    /// 210) est `512 / 445670`, et ramène ce compte à zéro.
    #[test]
    fn a_credit_can_settle_a_reprised_receivable() {
        let p = profile(None, None);
        let exercise = FiscalYear::calendar(2026);
        let opening = OpeningBalance {
            opens_on: date(2026, TimeMonth::January, 1),
            source: None,
            lines: vec![
                "101000:Capital:C:1210.00".parse().unwrap(),
                "445670:Crédit de TVA:D:210.00".parse().unwrap(),
                "512000:Banque:D:1000.00".parse().unwrap(),
            ],
            tax_losses: Money::ZERO,
        };
        let transactions = vec![settled(
            date(2026, TimeMonth::February, 3),
            21_000,
            "REMB TVA",
            "445670",
            None,
        )];
        let ledger = Ledger::build(LedgerFacts {
            bank_transactions: &transactions,
            ..facts(
                &p,
                exercise,
                &[],
                &[],
                Some(OpeningLines::from_opening_balance(&opening)),
            )
        })
        .unwrap();
        let balance = ledger.trial_balance();
        let credit = balance
            .rows
            .iter()
            .find(|r| r.account.number == "445670")
            .unwrap();
        assert_eq!(credit.balance, Money::ZERO);
        assert_eq!(credit.account.label, "Crédit de TVA à reporter");
        assert_eq!(
            balance
                .rows
                .iter()
                .find(|r| r.account.number == "512000")
                .unwrap()
                .balance,
            Money::from_cents(121_000)
        );
        assert!(ledger.balance_sheet().is_balanced());
    }

    /// Chaque dépense a sa propre pièce (UUID complet), la même pour la charge et son
    /// décaissement ; le justificatif est nommé dans le libellé, pas dans la pièce.
    #[test]
    fn expense_pieces_are_unique_and_the_receipt_is_named_in_the_label() {
        let p = profile(None, None);
        let exercise = FiscalYear::calendar(2026);
        let mut with_receipt = expense(12_000, 2_000, date(2026, TimeMonth::March, 1));
        with_receipt.receipt_filename = Some("facture-ovh.pdf".to_string());
        let expenses = vec![
            with_receipt.clone(),
            expense(12_000, 2_000, date(2026, TimeMonth::March, 1)),
            expense(12_000, 2_000, date(2026, TimeMonth::March, 1)),
        ];
        let debit = debit_for(&with_receipt, date(2026, TimeMonth::March, 4));
        let ledger = Ledger::build(LedgerFacts {
            bank_transactions: std::slice::from_ref(&debit),
            ..facts(&p, exercise, &[], &expenses, None)
        })
        .unwrap();
        let pieces: std::collections::BTreeSet<&str> = ledger
            .entries
            .iter()
            .filter(|e| e.piece_ref.starts_with("DEP-"))
            .map(|e| e.piece_ref.as_str())
            .collect();
        assert_eq!(
            pieces.len(),
            3,
            "une pièce par dépense, partagée charge/décaissement"
        );
        assert!(pieces.contains(format!("DEP-{}", with_receipt.id).as_str()));
        let charge = ledger
            .entries
            .iter()
            .find(|e| e.journal == Journal::Purchases && e.label.contains("facture-ovh.pdf"))
            .expect("le justificatif est nommé dans le libellé");
        assert_eq!(charge.piece_ref, format!("DEP-{}", with_receipt.id));
    }

    /// Le report à nouveau des à-nouveaux dérivés est **net** : 110 C 6 350 repris et une
    /// perte de 1 226,90 affectée ne donnent pas « 110 C 6 350 + 119 D 1 226,90 » mais
    /// 110 C 5 123,10.
    #[test]
    fn derived_opening_lines_present_the_retained_earnings_net_on_a_single_account() {
        let p = profile(None, None);
        let exercise = FiscalYear::calendar(2027);
        let opening = OpeningLines {
            label: "AN".to_string(),
            lines: vec![
                line(accounts::RETAINED_CREDIT, Money::from_cents(-635_000)),
                line(accounts::RETAINED_DEBIT, Money::from_cents(122_690)),
                line(accounts::BANK, Money::from_cents(512_310)),
            ],
        };
        let ledger = Ledger::build(facts(&p, exercise, &[], &[], Some(opening))).unwrap();
        let next = ledger.closing_opening_lines();
        let retained: Vec<_> = next
            .lines
            .iter()
            .filter(|l| l.account.number.starts_with("11"))
            .collect();
        assert_eq!(retained.len(), 1);
        assert_eq!(retained[0].account, accounts::RETAINED_CREDIT);
        assert_eq!(retained[0].amount, Money::from_cents(-512_310));
        assert_eq!(
            next.lines.iter().map(|l| l.amount).sum::<Money>(),
            Money::ZERO
        );
    }

    proptest! {
        /// Des règlements arbitraires sur des comptes de bilan arbitraires (classes 1 à 5,
        /// hors 512) laissent le bilan équilibré et les à-nouveaux dérivés à somme nulle.
        #[test]
        fn settlements_on_arbitrary_accounts_keep_the_balance_sheet_balanced(
            balances in prop::collection::vec((account_strategy(), -5_000_000i64..5_000_000), 0..8),
            settlements in prop::collection::vec(
                (prop::sample::select(vec!["101000", "164000", "401000", "421000", "444000", "445510", "445670", "455000", "457000", "467100", "580000"]), -300_000i64..300_000),
                0..8,
            ),
        ) {
            let p = profile(None, None);
            let exercise = FiscalYear::calendar(2026);
            let mut lines: Vec<LedgerLine> = balances
                .iter()
                .map(|(n, cents)| line(Account::for_number(n, Some("x")).unwrap(), Money::from_cents(*cents)))
                .collect();
            let sum: Money = lines.iter().map(|l| l.amount).sum();
            lines.push(line(accounts::BANK, -sum));
            let transactions: Vec<BankTransaction> = settlements
                .iter()
                .filter(|(_, cents)| *cents != 0)
                .map(|(account, cents)| settled(date(2026, TimeMonth::May, 2), *cents, "x", account, Some("Compte saisi")))
                .collect();
            let ledger = Ledger::build(LedgerFacts {
                bank_transactions: &transactions,
                ..facts(&p, exercise, &[], &[], Some(OpeningLines { label: "AN".to_string(), lines }))
            }).unwrap();
            prop_assert!(ledger.entries.iter().all(LedgerEntry::is_balanced));
            let balance = ledger.trial_balance();
            prop_assert_eq!(balance.total_debit, balance.total_credit);
            let sheet = ledger.balance_sheet();
            prop_assert!(sheet.is_balanced(), "{:#?}", sheet);
            prop_assert_eq!(ledger.net_result(), Money::ZERO);
            let carried: Money = ledger.closing_opening_lines().lines.iter().map(|l| l.amount).sum();
            prop_assert_eq!(carried, Money::ZERO);
        }
    }
}
