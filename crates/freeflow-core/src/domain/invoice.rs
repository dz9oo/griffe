//! Factures. La numérotation séquentielle, l'immuabilité et le chaînage cryptographique sont
//! imposés par la couche de persistance (lot 5) — ce module ne fixe que la forme des données.

use serde::{Deserialize, Serialize};
use time::Date;

use super::ids::{ClientId, InvoiceId, MissionId};
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
    pub issued_on: Date,
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
