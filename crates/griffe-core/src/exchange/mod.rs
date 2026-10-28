//! Document d'échange v1 : une commande, un dossier à la fois, une origine stable.

mod apply;
mod document;
mod report;

pub use apply::{ApplyExchange, preview};
pub use document::{
    AddressFile, CadenceMoment, ContactFile, DossierFile, ExchangeDocument, LineFile, SeriesFile,
    SeriesMoment, TypeDetail, TypeEntry, document_from_str, parse_montant,
};
pub use report::{CadenceOutcome, DossierOutcome, ExchangeReport, ExchangeStatus, SeriesOutcome};

#[cfg(test)]
mod tests;
