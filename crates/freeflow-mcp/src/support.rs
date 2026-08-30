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
