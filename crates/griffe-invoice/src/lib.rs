//! Rendu de facture : XML CII EN 16931 ([`cii`]) et PDF/A-3b Factur-X ([`pdf`]), tous deux
//! dérivés uniquement des types du domaine (`griffe_core`) — aucune logique métier propre à
//! ce crate, seulement la mise en forme.

mod cii;
mod error;
mod pdf;

pub use cii::cii_xml;
pub use error::InvoiceError;
pub use pdf::render_pdf;
