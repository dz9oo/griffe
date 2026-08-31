//! Aides partagées par tous les outils MCP : conversion `Outcome`/`AppError` en contenu de
//! réponse, et remontée des erreurs de validation (parsing) comme erreurs *outil* — visibles
//! par l'agent — plutôt que comme erreurs *protocole* (`rmcp::ErrorData`), qui sont le plus
//! souvent opaques côté client.

use freeflow_core::app::Outcome;
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

/// Résout une référence texte (uuid, préfixe, nom — voir `freeflow_core::reference`) vers un
/// identifiant de client, en un message d'erreur listant les candidats en cas d'ambiguïté :
/// même comportement que `freeflow_cli::refs::resolve_client`, dupliqué ici plutôt que partagé
/// entre les deux crates (le format d'erreur — texte pour un humain vs `CallToolResult` pour un
/// agent — diverge assez pour ne pas valoir une dépendance croisée CLI -> MCP).
pub(crate) fn resolve_client(
    store: &freeflow_core::store::Store,
    needle: &str,
) -> Result<freeflow_core::domain::ClientId, String> {
    use freeflow_core::reference::{self, RefMatch};
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
    result: Result<freeflow_core::reference::RefMatch<T>, freeflow_core::app::AppError>,
    no_match: &str,
    plural: &str,
) -> Result<T, String> {
    use freeflow_core::reference::RefMatch;
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

/// Résout une référence texte (uuid, préfixe, nom — voir `freeflow_core::reference`) vers un
/// identifiant d'opportunité, dupliqué depuis `freeflow_cli::refs` pour la même raison que
/// [`resolve_client`] (format d'erreur MCP divergent).
pub(crate) fn resolve_opportunity(
    store: &freeflow_core::store::Store,
    needle: &str,
) -> Result<freeflow_core::domain::OpportunityId, String> {
    resolve_ref(
        freeflow_core::reference::resolve_opportunity(store.connection(), needle),
        "aucune opportunité",
        "opportunités",
    )
}

/// Résout une référence texte vers un identifiant de mission — voir [`resolve_opportunity`].
pub(crate) fn resolve_mission(
    store: &freeflow_core::store::Store,
    needle: &str,
) -> Result<freeflow_core::domain::MissionId, String> {
    resolve_ref(
        freeflow_core::reference::resolve_mission(store.connection(), needle),
        "aucune mission",
        "missions",
    )
}

pub(crate) fn parse_loss_reason(s: &str) -> Result<freeflow_core::domain::LossReason, String> {
    use freeflow_core::domain::LossReason;
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
