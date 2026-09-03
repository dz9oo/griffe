//! Résolution de références lisibles par un humain (uuid, préfixe, nom) — appelle
//! `freeflow_core::reference` et traduit son ambiguïté ou son absence de résultat en
//! [`CliError::Domain`] listant les candidats, pour qu'un script ou un humain sache quoi taper
//! ensuite plutôt que de recevoir un simple « introuvable ».

use std::fmt::Display;

use freeflow_core::domain::{ClientId, ExpenseId, FixedAssetId, MissionId, OpportunityId, QuoteId};
use freeflow_core::reference::{self, RefMatch};
use freeflow_core::store::Store;

use crate::error::CliError;

/// Traduit un `RefMatch` en résultat CLI — factorisé une fois, chaque résolveur concret n'est
/// plus qu'un appel à `freeflow_core::reference` suivi de cet appel. `no_match` et `plural`
/// portent l'accord grammatical correct (« aucun client »/« clients », « aucune mission »/
/// « missions ») — pas assez régulier en français pour être dérivé d'une seule chaîne.
fn translate<T: Display>(
    needle: &str,
    no_match: &str,
    plural: &str,
    result: RefMatch<T>,
) -> Result<T, CliError> {
    match result {
        RefMatch::Unique(id) => Ok(id),
        RefMatch::NotFound => Err(CliError::Domain(format!(
            "{no_match} ne correspond à « {needle} »"
        ))),
        RefMatch::Ambiguous(candidates) => {
            let list = candidates
                .iter()
                .map(|(id, label)| format!("  {id} — {label}"))
                .collect::<Vec<_>>()
                .join("\n");
            Err(CliError::Domain(format!(
                "« {needle} » désigne plusieurs {plural}, précisez lequel :\n{list}"
            )))
        }
    }
}

/// # Errors
pub fn resolve_client(store: &Store, needle: &str) -> Result<ClientId, CliError> {
    translate(
        needle,
        "aucun client",
        "clients",
        reference::resolve_client(store.connection(), needle)?,
    )
}

/// # Errors
pub fn resolve_opportunity(store: &Store, needle: &str) -> Result<OpportunityId, CliError> {
    translate(
        needle,
        "aucune opportunité",
        "opportunités",
        reference::resolve_opportunity(store.connection(), needle)?,
    )
}

/// # Errors
pub fn resolve_mission(store: &Store, needle: &str) -> Result<MissionId, CliError> {
    translate(
        needle,
        "aucune mission",
        "missions",
        reference::resolve_mission(store.connection(), needle)?,
    )
}

/// # Errors
pub fn resolve_fixed_asset(store: &Store, needle: &str) -> Result<FixedAssetId, CliError> {
    translate(
        needle,
        "aucune immobilisation",
        "immobilisations",
        reference::resolve_fixed_asset(store.connection(), needle)?,
    )
}

/// # Errors
pub fn resolve_expense(store: &Store, needle: &str) -> Result<ExpenseId, CliError> {
    translate(
        needle,
        "aucune dépense",
        "dépenses",
        reference::resolve_expense(store.connection(), needle)?,
    )
}

/// # Errors
pub fn resolve_quote(store: &Store, needle: &str) -> Result<QuoteId, CliError> {
    translate(
        needle,
        "aucun devis",
        "devis",
        reference::resolve_quote(store.connection(), needle)?,
    )
}
