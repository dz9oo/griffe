//! Bilan d'ouverture (lot 30) : la reprise, compte par compte, du bilan tenu *avant* l'usage de
//! l'application — typiquement le dernier bilan établi par un expert-comptable. Sans lui, la
//! chaîne du report à nouveau et de la réserve légale (`fiscal_year`) part de zéro, et le FEC du
//! premier exercice n'a pas d'à-nouveaux : deux hypothèses fausses pour toute société qui
//! existait déjà.
//!
//! Types purs, sans IO : un code de compte du plan comptable général ([`AccountCode`]), une
//! ligne de balance ([`OpeningBalanceLine`], débit *ou* crédit, jamais les deux) et le bilan
//! entier ([`OpeningBalance`]), dont l'invariant central est l'**équilibre** — total débit égal
//! au total crédit, comme toute balance. Seuls les comptes de bilan (classes 1 à 5) sont admis :
//! un bilan d'ouverture ne porte ni charge ni produit, le résultat de l'exercice précédent y
//! figure déjà en capitaux propres (120/129, ou 110/119 après affectation).
//!
//! La syntaxe texte d'une ligne (`compte:libellé:D|C:montant`, [`OpeningBalanceLine::from_str`])
//! est le format d'échange partagé par `--line` en CLI, l'outil MCP et le textarea de la fenêtre —
//! un seul parseur, sur le modèle de `Milestone` (lot 16) et `QuoteLine` (lot 23).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::Date;

use super::money::Money;

/// Un numéro de compte du plan comptable général : 3 à 12 chiffres ASCII, sans séparateur.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct AccountCode(String);

#[derive(Debug, Error, PartialEq, Eq, Clone)]
#[error("numéro de compte invalide : {0} (attendu 3 à 12 chiffres, ex. 101000)")]
pub struct AccountCodeError(pub String);

impl AccountCode {
    pub const MIN_LEN: usize = 3;
    pub const MAX_LEN: usize = 12;

    /// # Errors
    ///
    /// Longueur hors de `3..=12` ou caractère non numérique.
    pub fn parse(s: &str) -> Result<Self, AccountCodeError> {
        let s = s.trim();
        let len = s.len();
        if !(Self::MIN_LEN..=Self::MAX_LEN).contains(&len) || !s.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(AccountCodeError(s.to_string()));
        }
        Ok(Self(s.to_string()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// La classe du compte : son premier chiffre (`1` capitaux, `2` immobilisations, …).
    ///
    /// # Panics
    ///
    /// Jamais : un `AccountCode` a au moins [`Self::MIN_LEN`] chiffres par construction.
    #[must_use]
    pub fn class(&self) -> u8 {
        let first = self.0.as_bytes()[0];
        first - b'0'
    }

    /// Compte de bilan (classes 1 à 5), par opposition aux comptes de gestion (6, 7) et aux
    /// comptes spéciaux (8, 9).
    #[must_use]
    pub fn is_balance_sheet(&self) -> bool {
        (1..=5).contains(&self.class())
    }

    /// `true` si le compte commence par `prefix` — `1061` couvre `1061`, `106100`, etc.
    #[must_use]
    pub fn starts_with(&self, prefix: &str) -> bool {
        self.0.starts_with(prefix)
    }
}

impl fmt::Display for AccountCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for AccountCode {
    type Err = AccountCodeError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl TryFrom<String> for AccountCode {
    type Error = AccountCodeError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::parse(&s)
    }
}

impl From<AccountCode> for String {
    fn from(code: AccountCode) -> Self {
        code.0
    }
}

/// Le sens d'une ligne de balance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Debit,
    Credit,
}

impl Side {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Debit => "debit",
            Self::Credit => "credit",
        }
    }

    /// La lettre de la syntaxe texte (`D`/`C`).
    #[must_use]
    pub const fn letter(self) -> char {
        match self {
            Self::Debit => 'D',
            Self::Credit => 'C',
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq, Clone)]
#[error("sens de ligne invalide : {0} (attendu debit ou credit)")]
pub struct UnknownSide(pub String);

impl FromStr for Side {
    type Err = UnknownSide;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "d" | "debit" | "débit" => Ok(Self::Debit),
            "c" | "credit" | "crédit" => Ok(Self::Credit),
            other => Err(UnknownSide(other.to_string())),
        }
    }
}

/// Une ligne de balance d'ouverture : un compte, un libellé, un montant **strictement positif**
/// porté d'un seul côté.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpeningBalanceLine {
    pub account: AccountCode,
    pub label: String,
    pub side: Side,
    pub amount: Money,
}

impl OpeningBalanceLine {
    /// Montant signé — positif au débit, négatif au crédit — la convention des lignes du FEC.
    #[must_use]
    pub fn signed(&self) -> Money {
        match self.side {
            Side::Debit => self.amount,
            Side::Credit => -self.amount,
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq, Clone)]
#[error(
    "ligne de bilan invalide : {0} (attendu `compte:libellé:D|C:montant`, ex. \
     `101000:Capital social:C:1000.00`)"
)]
pub struct OpeningBalanceLineParseError(pub String);

impl FromStr for OpeningBalanceLine {
    type Err = OpeningBalanceLineParseError;

    /// `compte:libellé:D|C:montant`. L'analyse part de la droite (montant, puis sens) et de la
    /// gauche (compte), pour que le libellé au milieu puisse lui-même contenir des `:`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || OpeningBalanceLineParseError(s.to_string());
        let (rest, amount_str) = s.rsplit_once(':').ok_or_else(err)?;
        let (rest, side_str) = rest.rsplit_once(':').ok_or_else(err)?;
        let (account_str, label) = rest.split_once(':').ok_or_else(err)?;
        let account = AccountCode::parse(account_str).map_err(|_| err())?;
        let label = label.trim();
        if label.is_empty() {
            return Err(err());
        }
        let side: Side = side_str.parse().map_err(|_| err())?;
        let amount = Money::parse_decimal(amount_str.trim()).map_err(|_| err())?;
        Ok(Self {
            account,
            label: label.to_string(),
            side,
            amount,
        })
    }
}

impl fmt::Display for OpeningBalanceLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}:{}:{}",
            self.account,
            self.label,
            self.side.letter(),
            self.amount.to_decimal_string()
        )
    }
}

/// Le bilan d'ouverture : la balance des comptes de bilan au premier jour du premier exercice
/// suivi dans l'application.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpeningBalance {
    /// Premier jour de l'exercice qui s'ouvre sur ce bilan (lendemain de la clôture reprise).
    pub opens_on: Date,
    /// Provenance, libre — ex. « bilan au 30/09/2025 établi par le cabinet X ».
    pub source: Option<String>,
    pub lines: Vec<OpeningBalanceLine>,
    /// Déficits fiscaux antérieurs encore reportables en avant à l'ouverture (art. 209 I CGI —
    /// la case 870 du dernier tableau 2033-D déposé), repris **hors bilan** : ce n'est pas un
    /// compte, mais le maillon zéro de la chaîne de déficits que [`crate::fiscal_year`] impute
    /// sur les bénéfices des exercices clos ici (lot 32). Jamais négatif ; zéro par défaut.
    #[serde(default)]
    pub tax_losses: Money,
}

#[derive(Debug, Error, PartialEq, Eq, Clone)]
pub enum OpeningBalanceError {
    #[error("un bilan d'ouverture doit comporter au moins une ligne")]
    Empty,

    #[error("bilan déséquilibré : total débit {debit}, total crédit {credit}")]
    Unbalanced { debit: String, credit: String },

    #[error(
        "le compte {0} n'est pas un compte de bilan (classes 1 à 5) : un bilan d'ouverture ne \
         porte ni charge ni produit"
    )]
    NotABalanceSheetAccount(AccountCode),

    #[error("le compte {0} apparaît deux fois : une balance a une ligne par compte")]
    DuplicateAccount(AccountCode),

    #[error(
        "montant nul ou négatif sur le compte {0} : une ligne de balance porte un montant strictement positif"
    )]
    NonPositiveAmount(AccountCode),

    #[error("déficits reportables négatifs ({0}) : un stock de déficits est nul ou positif")]
    NegativeTaxLosses(Money),
}

/// Ce que la chaîne de clôture (`fiscal_year`) lit dans un bilan d'ouverture : les capitaux
/// propres reconstitués depuis les comptes de classe 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct OpeningEquity {
    /// Solde créditeur de `101*` (capital), signé.
    pub share_capital: Money,
    /// Solde créditeur de `1061*` (réserve légale), signé.
    pub legal_reserve: Money,
    /// Report à nouveau : solde créditeur cumulé de `110*`, `119*`, `120*` et `129*`, signé
    /// (un 119 ou un 129, débiteurs, le rendent négatif = report débiteur).
    /// Un résultat de l'exercice précédent encore en 120/129 est réputé affecté en report à
    /// nouveau — l'affectation la plus courante pour une société unipersonnelle sans dividende.
    pub retained_earnings: Money,
}

impl OpeningBalance {
    /// # Errors
    ///
    /// Voir [`OpeningBalanceError`] : vide, déséquilibré, compte hors bilan, compte en double,
    /// montant non positif.
    pub fn validate(&self) -> Result<(), OpeningBalanceError> {
        if self.lines.is_empty() {
            return Err(OpeningBalanceError::Empty);
        }
        if self.tax_losses.is_negative() {
            return Err(OpeningBalanceError::NegativeTaxLosses(self.tax_losses));
        }
        let mut seen = std::collections::HashSet::new();
        for line in &self.lines {
            if !line.account.is_balance_sheet() {
                return Err(OpeningBalanceError::NotABalanceSheetAccount(
                    line.account.clone(),
                ));
            }
            if line.amount.cents() <= 0 {
                return Err(OpeningBalanceError::NonPositiveAmount(line.account.clone()));
            }
            if !seen.insert(&line.account) {
                return Err(OpeningBalanceError::DuplicateAccount(line.account.clone()));
            }
        }
        let (debit, credit) = (self.total_debit(), self.total_credit());
        if debit != credit {
            return Err(OpeningBalanceError::Unbalanced {
                debit: debit.to_string(),
                credit: credit.to_string(),
            });
        }
        Ok(())
    }

    #[must_use]
    pub fn total_debit(&self) -> Money {
        self.lines
            .iter()
            .filter(|l| l.side == Side::Debit)
            .map(|l| l.amount)
            .sum()
    }

    #[must_use]
    pub fn total_credit(&self) -> Money {
        self.lines
            .iter()
            .filter(|l| l.side == Side::Credit)
            .map(|l| l.amount)
            .sum()
    }

    /// Solde **créditeur** cumulé des comptes commençant par `prefix` (crédit − débit).
    #[must_use]
    pub fn credit_balance(&self, prefix: &str) -> Money {
        self.lines
            .iter()
            .filter(|l| l.account.starts_with(prefix))
            .map(|l| -l.signed())
            .sum()
    }

    #[must_use]
    pub fn equity(&self) -> OpeningEquity {
        OpeningEquity {
            share_capital: self.credit_balance("101"),
            legal_reserve: self.credit_balance("1061"),
            retained_earnings: ["110", "119", "120", "129"]
                .iter()
                .map(|prefix| self.credit_balance(prefix))
                .sum(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::Month;

    fn line(spec: &str) -> OpeningBalanceLine {
        spec.parse().unwrap()
    }

    fn balance(specs: &[&str]) -> OpeningBalance {
        OpeningBalance {
            opens_on: Date::from_calendar_date(2025, Month::October, 1).unwrap(),
            source: None,
            lines: specs.iter().map(|s| line(s)).collect(),
            tax_losses: Money::ZERO,
        }
    }

    #[test]
    fn negative_carried_losses_are_refused() {
        let mut b = balance(&["101000:Capital:C:10.00", "512000:Banque:D:10.00"]);
        b.tax_losses = Money::from_cents(-1);
        assert_eq!(
            b.validate(),
            Err(OpeningBalanceError::NegativeTaxLosses(Money::from_cents(
                -1
            )))
        );
        b.tax_losses = Money::from_cents(250_000);
        assert_eq!(b.validate(), Ok(()));
    }

    #[test]
    fn account_codes_are_three_to_twelve_digits() {
        assert!(AccountCode::parse("101").is_ok());
        assert!(AccountCode::parse("512000").is_ok());
        assert!(AccountCode::parse(" 411000 ").is_ok());
        assert!(AccountCode::parse("10").is_err());
        assert!(AccountCode::parse("1234567890123").is_err());
        assert!(AccountCode::parse("51A000").is_err());
        assert!(AccountCode::parse("").is_err());
        assert_eq!(AccountCode::parse("512000").unwrap().class(), 5);
        assert!(AccountCode::parse("512000").unwrap().is_balance_sheet());
        assert!(!AccountCode::parse("706000").unwrap().is_balance_sheet());
        assert!(!AccountCode::parse("890000").unwrap().is_balance_sheet());
    }

    #[test]
    fn a_line_parses_from_the_right_so_the_label_may_contain_colons() {
        let l = line("101000:Capital : social:C:1000.00");
        assert_eq!(l.account.as_str(), "101000");
        assert_eq!(l.label, "Capital : social");
        assert_eq!(l.side, Side::Credit);
        assert_eq!(l.amount, Money::from_cents(100_000));
        assert_eq!(l.signed(), Money::from_cents(-100_000));

        let d = line("512000:Banque:debit:2345,67");
        assert_eq!(d.side, Side::Debit);
        assert_eq!(d.amount, Money::from_cents(234_567));
    }

    #[test]
    fn a_line_round_trips_through_display() {
        for spec in [
            "101000:Capital social:C:1000.00",
            "512000:Banque BNP:D:2345.67",
            "119000:Report à nouveau (solde débiteur):D:0.50",
        ] {
            let l = line(spec);
            assert_eq!(l.to_string(), spec);
            assert_eq!(l.to_string().parse::<OpeningBalanceLine>().unwrap(), l);
        }
    }

    #[test]
    fn malformed_lines_are_refused() {
        for bad in [
            "",
            "101000",
            "101000:Capital",
            "101000:Capital:C",
            "101000::C:10",
            "101000:Capital:X:10",
            "101000:Capital:C:abc",
            "10:Capital:C:10",
            "706000:Ventes:C:10.001",
        ] {
            assert!(bad.parse::<OpeningBalanceLine>().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_balanced_balance_sheet_validates_and_yields_its_equity() {
        let b = balance(&[
            "101000:Capital social:C:1000.00",
            "106100:Réserve légale:C:100.00",
            "110000:Report à nouveau:C:2500.00",
            "120000:Résultat de l'exercice:C:400.00",
            "455000:Compte courant d'associé:C:1200.00",
            "512000:Banque:D:5000.00",
            "445660:TVA déductible:D:200.00",
        ]);
        b.validate().unwrap();
        assert_eq!(b.total_debit(), Money::from_cents(520_000));
        assert_eq!(b.total_credit(), Money::from_cents(520_000));
        let equity = b.equity();
        assert_eq!(equity.share_capital, Money::from_cents(100_000));
        assert_eq!(equity.legal_reserve, Money::from_cents(10_000));
        // 2 500 de report + 400 de résultat non encore affecté.
        assert_eq!(equity.retained_earnings, Money::from_cents(290_000));
    }

    #[test]
    fn a_debit_retained_earnings_account_yields_a_negative_carry() {
        let b = balance(&[
            "101000:Capital:C:1000.00",
            "119000:Report à nouveau débiteur:D:300.00",
            "129000:Perte de l'exercice:D:200.00",
            "512000:Banque:D:500.00",
        ]);
        b.validate().unwrap();
        assert_eq!(b.equity().retained_earnings, Money::from_cents(-50_000));
    }

    #[test]
    fn an_unbalanced_balance_sheet_is_refused() {
        let b = balance(&["101000:Capital:C:1000.00", "512000:Banque:D:999.99"]);
        assert_eq!(
            b.validate(),
            Err(OpeningBalanceError::Unbalanced {
                debit: Money::from_cents(99_999).to_string(),
                credit: Money::from_cents(100_000).to_string(),
            })
        );
    }

    #[test]
    fn income_statement_accounts_duplicates_and_empty_balances_are_refused() {
        assert_eq!(balance(&[]).validate(), Err(OpeningBalanceError::Empty));
        let b = balance(&["706000:Ventes:C:10.00", "512000:Banque:D:10.00"]);
        assert_eq!(
            b.validate(),
            Err(OpeningBalanceError::NotABalanceSheetAccount(
                AccountCode::parse("706000").unwrap()
            ))
        );
        let b = balance(&[
            "512000:Banque:D:10.00",
            "512000:Banque bis:D:5.00",
            "101000:Capital:C:15.00",
        ]);
        assert_eq!(
            b.validate(),
            Err(OpeningBalanceError::DuplicateAccount(
                AccountCode::parse("512000").unwrap()
            ))
        );
        let zero = OpeningBalance {
            opens_on: Date::from_calendar_date(2025, Month::October, 1).unwrap(),
            source: None,
            lines: vec![OpeningBalanceLine {
                account: AccountCode::parse("512000").unwrap(),
                label: "Banque".to_string(),
                side: Side::Debit,
                amount: Money::ZERO,
            }],
            tax_losses: Money::ZERO,
        };
        assert_eq!(
            zero.validate(),
            Err(OpeningBalanceError::NonPositiveAmount(
                AccountCode::parse("512000").unwrap()
            ))
        );
    }
}
