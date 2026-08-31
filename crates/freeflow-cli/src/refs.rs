//! Résolution de références lisibles par un humain (uuid, préfixe, nom) — appelle
//! `freeflow_core::reference` et traduit son ambiguïté ou son absence de résultat en
//! [`CliError::Domain`] listant les candidats, pour qu'un script ou un humain sache quoi taper
//! ensuite plutôt que de recevoir un simple « introuvable ».

use freeflow_core::domain::ClientId;
use freeflow_core::reference::{self, RefMatch};
use freeflow_core::store::Store;

use crate::error::CliError;

/// # Errors
pub fn resolve_client(store: &Store, needle: &str) -> Result<ClientId, CliError> {
    match reference::resolve_client(store.connection(), needle)? {
        RefMatch::Unique(id) => Ok(id),
        RefMatch::NotFound => Err(CliError::Domain(format!(
            "aucun client ne correspond à « {needle} »"
        ))),
        RefMatch::Ambiguous(candidates) => {
            let list = candidates
                .iter()
                .map(|(id, name)| format!("  {id} — {name}"))
                .collect::<Vec<_>>()
                .join("\n");
            Err(CliError::Domain(format!(
                "« {needle} » désigne plusieurs clients, précisez lequel :\n{list}"
            )))
        }
    }
}
