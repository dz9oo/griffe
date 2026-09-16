//! Le compte de bilan qu'un mouvement du relevé **règle** (lot 37).
//!
//! Une ligne de relevé était jusqu'ici soit l'encaissement d'une facture, soit le paiement d'une
//! dépense. Or une SASU reprise d'un cabinet paie, dans l'exercice, des dettes qui figuraient
//! déjà au bilan d'ouverture — honoraires de septembre (401), solde d'IS (444), TVA à décaisser
//! (4455) — ou bouge des comptes qui ne sont pas des charges : compte courant d'associé (455),
//! dividendes (457), virement entre ses propres comptes (580), emprunt (164). Saisir ces débits
//! en dépense compte deux fois la charge et laisse la dette au passif ; ne pas les saisir laisse
//! la banque surestimée. Un règlement est la troisième lecture d'un débit : `compte / 512`, sans
//! charge ni produit.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::opening::AccountCode;

/// Un compte de bilan (classes 1 à 5) **hors 512** : un règlement contre la banque elle-même n'a
/// pas de sens — le virement interne passe par 580, le compte d'attente.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "AccountCode", into = "AccountCode")]
pub struct SettlementAccount(AccountCode);

#[derive(Debug, Error, PartialEq, Eq, Clone)]
pub enum SettlementAccountError {
    #[error("{0}")]
    Code(#[from] super::opening::AccountCodeError),

    #[error(
        "le compte {0} est un compte de gestion (classe {1}) : un règlement ne porte que sur un \
         compte de bilan (classes 1 à 5) — une charge se saisit en dépense"
    )]
    NotBalanceSheet(String, u8),

    #[error(
        "le compte {0} est la banque elle-même : un règlement solde un autre compte contre 512 \
         (un virement entre vos comptes passe par 580)"
    )]
    IsBank(String),
}

impl SettlementAccount {
    /// # Errors
    ///
    /// Code invalide, compte de gestion (classes 6 à 9), ou compte 512.
    pub fn new(code: AccountCode) -> Result<Self, SettlementAccountError> {
        if !code.is_balance_sheet() {
            return Err(SettlementAccountError::NotBalanceSheet(
                code.to_string(),
                code.class(),
            ));
        }
        if code.starts_with("512") {
            return Err(SettlementAccountError::IsBank(code.to_string()));
        }
        Ok(Self(code))
    }

    #[must_use]
    pub fn code(&self) -> &AccountCode {
        &self.0
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl TryFrom<AccountCode> for SettlementAccount {
    type Error = SettlementAccountError;
    fn try_from(code: AccountCode) -> Result<Self, Self::Error> {
        Self::new(code)
    }
}

impl From<SettlementAccount> for AccountCode {
    fn from(account: SettlementAccount) -> Self {
        account.0
    }
}

impl FromStr for SettlementAccount {
    type Err = SettlementAccountError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(AccountCode::parse(s)?)
    }
}

impl fmt::Display for SettlementAccount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_balance_sheet_account_other_than_the_bank_is_accepted() {
        for code in [
            "401000", "444000", "455000", "580000", "164000", "445670", "457",
        ] {
            assert!(code.parse::<SettlementAccount>().is_ok(), "{code}");
        }
    }

    #[test]
    fn income_statement_accounts_and_the_bank_are_refused() {
        assert!(matches!(
            "622600".parse::<SettlementAccount>(),
            Err(SettlementAccountError::NotBalanceSheet(_, 6))
        ));
        assert!(matches!(
            "706000".parse::<SettlementAccount>(),
            Err(SettlementAccountError::NotBalanceSheet(_, 7))
        ));
        assert!(matches!(
            "512000".parse::<SettlementAccount>(),
            Err(SettlementAccountError::IsBank(_))
        ));
        assert!(matches!(
            "51".parse::<SettlementAccount>(),
            Err(SettlementAccountError::Code(_))
        ));
    }

    #[test]
    fn it_serializes_as_the_bare_account_number_and_validates_on_read() {
        let account: SettlementAccount = "401000".parse().unwrap();
        assert_eq!(serde_json::to_string(&account).unwrap(), "\"401000\"");
        assert_eq!(
            serde_json::from_str::<SettlementAccount>("\"401000\"").unwrap(),
            account
        );
        assert!(serde_json::from_str::<SettlementAccount>("\"512000\"").is_err());
    }
}
