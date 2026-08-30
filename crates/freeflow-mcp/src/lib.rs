//! Adaptateur MCP : expose la même couche applicative que la CLI (lot 7) sous forme d'outils
//! `rmcp`, en transport stdio. Aucune logique métier ici — uniquement le mapping arguments JSON
//! ⇄ `Command`/`Query` du domaine, et la mise en forme de la réponse.

mod server;
mod support;
mod tools;

pub use server::FreeflowServer;
