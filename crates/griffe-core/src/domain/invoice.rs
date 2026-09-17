//! Factures. La numérotation séquentielle, l'immuabilité et le chaînage cryptographique sont
//! imposés par la couche de persistance (lot 5) — ce module ne fixe que la forme des données.

use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::Date;

use super::ids::{ClientId, InvoiceId, MissionId, WriteOffId};
use super::money::Money;
use super::vat::VatRate;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InvoiceStatus {
    Issued,
    PartiallyPaid,
    Paid,
    Overdue,
    Cancelled,
}

/// Provenance d'une facture de vente : émise localement, ou importée d'une PA / d'un outil tiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum InvoiceOrigin {
    #[default]
    Issued,
    Imported,
}

#[derive(Debug, Error, PartialEq, Eq)]
#[error("origine de facture inconnue : {0}")]
pub struct UnknownInvoiceOrigin(pub String);

impl InvoiceOrigin {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Issued => "issued",
            Self::Imported => "imported",
        }
    }
}

impl std::str::FromStr for InvoiceOrigin {
    type Err = UnknownInvoiceOrigin;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "issued" => Ok(Self::Issued),
            "imported" => Ok(Self::Imported),
            other => Err(UnknownInvoiceOrigin(other.to_string())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InvoiceLine {
    pub description: String,
    pub quantity: f64,
    pub unit_price: Money,
    pub vat_rate: VatRate,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Invoice {
    pub id: InvoiceId,
    /// Numéro séquentiel définitif (ex. `FA-2026-0001`), alloué par le store au moment de
    /// l'émission (lot 5).
    pub number: String,
    pub client_id: ClientId,
    pub mission_id: Option<MissionId>,
    pub lines: Vec<InvoiceLine>,
    pub status: InvoiceStatus,
    /// Provenance : émise ici (`Issued`) ou importée d'ailleurs (`Imported`). Les JSON /
    /// audits anciens sans champ valent `Issued`.
    #[serde(default)]
    pub origin: InvoiceOrigin,
    #[serde(with = "crate::domain::serde_date::date")]
    pub issued_on: Date,
    #[serde(with = "crate::domain::serde_date::date")]
    pub due_on: Date,
    /// Hash SHA-256 de la facture précédente dans la chaîne globale (`None` pour la toute
    /// première facture jamais émise).
    pub previous_hash: Option<String>,
    /// Hash SHA-256 de cette facture, calculé au moment de l'émission.
    pub hash: String,
    /// `Some` si cette facture est un avoir : l'id de la facture qu'elle annule. Une facture
    /// normale ne peut jamais être modifiée pour être annulée — seul un avoir, une nouvelle
    /// facture à part entière, en a le droit.
    pub credited_invoice_id: Option<InvoiceId>,
}

/// Perte sur créance : le reste dû d'une facture figé en centimes, sans avoir.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InvoiceWriteOff {
    pub id: WriteOffId,
    pub invoice_id: InvoiceId,
    #[serde(with = "crate::domain::serde_date::date")]
    pub written_off_on: Date,
    pub ht: Money,
    pub vat: Money,
    pub ttc: Money,
    pub recovers_vat: bool,
    #[serde(with = "crate::domain::serde_date::date::option")]
    pub retracted_on: Option<Date>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invoice_origin_roundtrips_as_snake_case() {
        assert_eq!(InvoiceOrigin::Issued.as_str(), "issued");
        assert_eq!(InvoiceOrigin::Imported.as_str(), "imported");
        assert_eq!(
            "imported".parse::<InvoiceOrigin>().unwrap(),
            InvoiceOrigin::Imported
        );
        let json = serde_json::to_string(&InvoiceOrigin::Imported).unwrap();
        assert_eq!(json, "\"imported\"");
    }
}
