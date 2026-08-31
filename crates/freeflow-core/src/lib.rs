//! Domaine métier, persistance chiffrée et couche applicative de `FreeFlow`.
//!
//! `domain` ne fait aucune IO. `store` porte la persistance `SQLCipher`. `app` expose les
//! `Command`/`Query` : c'est l'unique surface que consomment la CLI, le serveur MCP et la GUI.

pub mod app;
pub mod billing;
pub mod clients;
pub mod company;
pub mod domain;
pub mod expenses;
pub mod fiscal;
pub mod forecast;
pub mod missions;
pub mod prospection;
pub mod quotes;
pub mod reference;
pub mod store;
pub mod vault;
