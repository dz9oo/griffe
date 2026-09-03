//! Domaine métier, persistance chiffrée et couche applicative de `FreeFlow`.
//!
//! `domain` ne fait aucune IO. `store` porte la persistance `SQLCipher`. `app` expose les
//! `Command`/`Query` : c'est l'unique surface que consomment la CLI, le serveur MCP et la GUI.

pub mod accounting;
pub mod app;
pub mod billing;
pub mod clients;
pub mod clock;
pub mod closing;
pub mod company;
pub mod domain;
pub mod expenses;
pub mod fec;
pub mod fiscal;
pub mod fiscal_year;
pub mod forecast;
pub mod ledger;
pub mod missions;
pub mod opening_balance;
pub mod prospection;
pub mod quotes;
pub mod receipts;
pub mod reference;
pub mod setup;
pub mod store;
pub mod vault;
