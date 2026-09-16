//! Encaissements.

use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::{Date, OffsetDateTime};

use super::ids::{BankTransactionId, ExpenseId, InvoiceId, PaymentId};
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
    #[serde(with = "crate::domain::serde_date::date")]
    pub received_on: Date,
    pub method: PaymentMethod,
    /// Lignée du rapprochement bancaire dont cet encaissement est issu (lot 22, migration
    /// `0013`) — `None` pour un encaissement manuel ou antérieur à la migration. Comme
    /// `missions.opportunity_id` : une lignée, jamais un lien éditable.
    pub bank_transaction_id: Option<BankTransactionId>,
    /// Contre-écriture (lot 22) : un encaissement saisi à tort ne se supprime pas, il s'annule —
    /// il reste visible dans l'historique mais sort de tous les calculs (balance âgée,
    /// prévisionnel, statut payé). La colonne existait depuis `0008`, posée par anticipation.
    #[serde(with = "crate::domain::serde_date::datetime::option")]
    pub voided_at: Option<OffsetDateTime>,
}

impl Payment {
    /// Un encaissement annulé ne compte plus dans aucun solde.
    #[must_use]
    pub const fn is_voided(&self) -> bool {
        self.voided_at.is_some()
    }
}

/// Une ligne de relevé bancaire importée (CSV/OFX), avant ou après rapprochement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BankTransaction {
    pub id: BankTransactionId,
    #[serde(with = "crate::domain::serde_date::date")]
    pub occurred_on: Date,
    /// Positif pour une entrée d'argent, négatif pour une sortie.
    pub amount_cents: i64,
    pub description: String,
    pub matched_invoice_id: Option<InvoiceId>,
    /// Dépense rapprochée de ce débit (lot 33, migration `0016`) — exclusif avec
    /// `matched_invoice_id` : un crédit règle une facture, un débit paie une dépense.
    #[serde(default)]
    pub matched_expense_id: Option<ExpenseId>,
    /// Compte de bilan que ce mouvement règle (lot 37, migration `0017`) — la troisième lecture
    /// d'une ligne de relevé, exclusive avec les deux autres : dette reprise au bilan
    /// d'ouverture, compte courant, virement interne… `compte / 512` sans charge.
    #[serde(default)]
    pub settlement_account: Option<super::SettlementAccount>,
    /// Libellé du règlement (celui du plan fixe par défaut, ou saisi pour un compte hors plan).
    #[serde(default)]
    pub settlement_label: Option<String>,
    /// Identifiant unique donné par la banque (lot 38, migration `0018`) — `FITID` en OFX,
    /// « Transaction ID » de certains CSV ; la clé de dédoublonnage quand il existe.
    #[serde(default)]
    pub fitid: Option<String>,
}

impl BankTransaction {
    /// Rapprochée — d'une facture, d'une dépense, ou réglée sur un compte de bilan.
    #[must_use]
    pub const fn is_matched(&self) -> bool {
        self.matched_invoice_id.is_some()
            || self.matched_expense_id.is_some()
            || self.settlement_account.is_some()
    }

    /// Réglée sur un compte de bilan (lot 37).
    #[must_use]
    pub const fn is_settled(&self) -> bool {
        self.settlement_account.is_some()
    }

    /// Une sortie d'argent — ce qui peut payer une dépense.
    #[must_use]
    pub const fn is_debit(&self) -> bool {
        self.amount_cents < 0
    }
}
