//! Encaissements.

use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::Date;

use super::ids::{BankTransactionId, InvoiceId, PaymentId};
use super::money::Money;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PaymentMethod {
    BankTransfer,
    Check,
    Card,
    Other,
}

#[derive(Debug, Error, PartialEq, Eq)]
#[error("mode de paiement inconnu : {0}")]
pub struct UnknownPaymentMethod(pub String);

impl PaymentMethod {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BankTransfer => "bank_transfer",
            Self::Check => "check",
            Self::Card => "card",
            Self::Other => "other",
        }
    }
}

impl std::str::FromStr for PaymentMethod {
    type Err = UnknownPaymentMethod;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "bank_transfer" => Ok(Self::BankTransfer),
            "check" => Ok(Self::Check),
            "card" => Ok(Self::Card),
            "other" => Ok(Self::Other),
            other => Err(UnknownPaymentMethod(other.to_string())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Payment {
    pub id: PaymentId,
    pub invoice_id: InvoiceId,
    pub amount: Money,
    pub received_on: Date,
    pub method: PaymentMethod,
}

/// Une ligne de relevé bancaire importée (CSV/OFX), avant ou après rapprochement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BankTransaction {
    pub id: BankTransactionId,
    pub occurred_on: Date,
    /// Positif pour une entrée d'argent, négatif pour une sortie.
    pub amount_cents: i64,
    pub description: String,
    pub matched_invoice_id: Option<InvoiceId>,
}
