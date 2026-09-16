//! Aides partagées par tous les outils MCP : conversion `Outcome`/`AppError` en contenu de
//! réponse, et remontée des erreurs de validation (parsing) comme erreurs *outil* — visibles
//! par l'agent — plutôt que comme erreurs *protocole* (`rmcp::ErrorData`), qui sont le plus
//! souvent opaques côté client.

use griffe_core::app::Outcome;
use rmcp::model::{CallToolResult, ContentBlock};
use serde::Serialize;
use serde_json::json;

pub(crate) fn ok_json(value: impl Serialize) -> CallToolResult {
    match serde_json::to_string_pretty(&value) {
        Ok(text) => CallToolResult::success(vec![ContentBlock::text(text)]),
        Err(e) => CallToolResult::error(vec![ContentBlock::text(format!(
            "échec de sérialisation : {e}"
        ))]),
    }
}

pub(crate) fn err_text(msg: impl Into<String>) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(msg.into())])
}

pub(crate) fn outcome_json<T: Serialize>(outcome: &Outcome<T>) -> serde_json::Value {
    match outcome {
        Outcome::Applied(v) => json!({"status": "applied", "result": v}),
        Outcome::DryRun => json!({"status": "dry_run"}),
        Outcome::AlreadyApplied(v) => json!({"status": "already_applied", "result": v}),
        Outcome::PendingConfirmation(id) => {
            json!({"status": "pending_confirmation", "pending_action_id": id.to_string()})
        }
    }
}

/// Résout une référence texte (uuid, préfixe, nom — voir `griffe_core::reference`) vers un
/// identifiant de client, en un message d'erreur listant les candidats en cas d'ambiguïté :
/// même comportement que `griffe_cli::refs::resolve_client`, dupliqué ici plutôt que partagé
/// entre les deux crates (le format d'erreur — texte pour un humain vs `CallToolResult` pour un
/// agent — diverge assez pour ne pas valoir une dépendance croisée CLI -> MCP).
pub(crate) fn resolve_client(
    store: &griffe_core::store::Store,
    needle: &str,
) -> Result<griffe_core::domain::ClientId, String> {
    use griffe_core::reference::{self, RefMatch};
    match reference::resolve_client(store.connection(), needle) {
        Ok(RefMatch::Unique(id)) => Ok(id),
        Ok(RefMatch::NotFound) => Err(format!("aucun client ne correspond à « {needle} »")),
        Ok(RefMatch::Ambiguous(candidates)) => {
            let list = candidates
                .iter()
                .map(|(id, name)| format!("  {id} — {name}"))
                .collect::<Vec<_>>()
                .join("\n");
            Err(format!(
                "« {needle} » désigne plusieurs clients, précisez lequel :\n{list}"
            ))
        }
        Err(e) => Err(e.to_string()),
    }
}

/// Enveloppe commune à `resolve_client`/`resolve_opportunity`/`resolve_mission` : traduit un
/// `RefMatch` en `Result<T, String>`, avec l'accord grammatical (« aucun client »/« clients »,
/// « aucune mission »/« missions ») passé explicitement plutôt que dérivé — pas assez régulier
/// en français pour l'être.
fn resolve_ref<T: std::fmt::Display>(
    result: Result<griffe_core::reference::RefMatch<T>, griffe_core::app::AppError>,
    no_match: &str,
    plural: &str,
) -> Result<T, String> {
    use griffe_core::reference::RefMatch;
    match result {
        Ok(RefMatch::Unique(id)) => Ok(id),
        Ok(RefMatch::NotFound) => Err(format!("{no_match} : aucune correspondance")),
        Ok(RefMatch::Ambiguous(candidates)) => {
            let list = candidates
                .iter()
                .map(|(id, label)| format!("  {id} — {label}"))
                .collect::<Vec<_>>()
                .join("\n");
            Err(format!(
                "désigne plusieurs {plural}, précisez lequel :\n{list}"
            ))
        }
        Err(e) => Err(e.to_string()),
    }
}

/// Résout une référence texte (uuid, préfixe, nom — voir `griffe_core::reference`) vers un
/// identifiant d'opportunité, dupliqué depuis `griffe_cli::refs` pour la même raison que
/// [`resolve_client`] (format d'erreur MCP divergent).
pub(crate) fn resolve_opportunity(
    store: &griffe_core::store::Store,
    needle: &str,
) -> Result<griffe_core::domain::OpportunityId, String> {
    resolve_ref(
        griffe_core::reference::resolve_opportunity(store.connection(), needle),
        "aucune opportunité",
        "opportunités",
    )
}

/// Résout une référence texte vers un identifiant de mission — voir [`resolve_opportunity`].
pub(crate) fn resolve_mission(
    store: &griffe_core::store::Store,
    needle: &str,
) -> Result<griffe_core::domain::MissionId, String> {
    resolve_ref(
        griffe_core::reference::resolve_mission(store.connection(), needle),
        "aucune mission",
        "missions",
    )
}

/// Résout une référence texte vers un identifiant de dépense — voir [`resolve_opportunity`].
pub(crate) fn resolve_expense(
    store: &griffe_core::store::Store,
    needle: &str,
) -> Result<griffe_core::domain::ExpenseId, String> {
    resolve_ref(
        griffe_core::reference::resolve_expense(store.connection(), needle),
        "aucune dépense",
        "dépenses",
    )
}

/// Résout une référence texte vers un identifiant d'immobilisation — voir [`resolve_opportunity`].
pub(crate) fn resolve_fixed_asset(
    store: &griffe_core::store::Store,
    needle: &str,
) -> Result<griffe_core::domain::FixedAssetId, String> {
    resolve_ref(
        griffe_core::reference::resolve_fixed_asset(store.connection(), needle),
        "aucune immobilisation",
        "immobilisations",
    )
}

/// Résout une référence texte vers un identifiant de devis (le libellé cherché est le nom du
/// client porteur) — voir [`resolve_opportunity`].
pub(crate) fn resolve_paper(
    store: &griffe_core::store::Store,
    needle: &str,
) -> Result<griffe_core::domain::PaperId, String> {
    resolve_ref(
        griffe_core::reference::resolve_paper(store.connection(), needle),
        "aucune pièce",
        "pièces",
    )
}

pub(crate) fn resolve_quote(
    store: &griffe_core::store::Store,
    needle: &str,
) -> Result<griffe_core::domain::QuoteId, String> {
    resolve_ref(
        griffe_core::reference::resolve_quote(store.connection(), needle),
        "aucun devis",
        "devis",
    )
}

pub(crate) fn parse_loss_reason(s: &str) -> Result<griffe_core::domain::LossReason, String> {
    use griffe_core::domain::LossReason;
    match s {
        "budget" => Ok(LossReason::Budget),
        "timing" => Ok(LossReason::Timing),
        "competitor" => Ok(LossReason::Competitor),
        "no-response" => Ok(LossReason::NoResponse),
        "scope-mismatch" => Ok(LossReason::ScopeMismatch),
        other => other
            .strip_prefix("other:")
            .map(|detail| LossReason::Other(detail.to_string()))
            .ok_or_else(|| format!("motif de perte invalide : {other}")),
    }
}

/// Évalue une expression `Result<T, E: Display>` ; en cas d'erreur, retourne immédiatement de
/// la fonction appelante une `CallToolResult::error` portant le message (préfixé par `label`)
/// — c'est l'agent qui voit l'erreur, pas le transport MCP.
macro_rules! ok_or_return {
    ($label:literal, $expr:expr) => {
        match $expr {
            Ok(v) => v,
            Err(e) => return crate::support::err_text(format!("{} : {e}", $label)),
        }
    };
}
pub(crate) use ok_or_return;
