//! Rail d'audit persistant : `GET /audit/recent` rend les dernières entrées du journal — appelé
//! au chargement d'un écran puis à intervalle régulier par un polling htmx, y compris pour une
//! écriture faite par un *autre* process (CLI, serveur MCP) sur le même coffre.
//!
//! Le plan envisageait un flux SSE piloté par `PRAGMA data_version`. La coque desktop (lot 9)
//! pont son routeur Axum vers un protocole URI custom Tauri dont l'API de réponse
//! (`UriSchemeResponder::respond<T: Into<Cow<'static, [u8]>>>`) est un buffer complet, pas un
//! flux — SSE y est donc irréalisable. Comme la même surface doit se comporter identiquement
//! dans les deux contextes (dev-serveur HTTP et coque Tauri), le polling htmx l'emporte partout :
//! chaque battement remplace intégralement le rail (`hx-swap="innerHTML"`), ce qui évite toute
//! logique de dédoublonnage — le coût d'un `SELECT ... LIMIT 30` toutes les deux secondes est
//! négligeable pour une app mono-utilisateur locale.

use axum::extract::State;
use axum::response::Html;
use freeflow_core::app::{AuditEntry, recent_audit_entries};
use maud::{Markup, html};
use time::format_description::well_known::Rfc3339;

use crate::state::AppState;

const RAIL_PAGE_SIZE: i64 = 30;

fn outcome_label(outcome: &str) -> &'static str {
    match outcome {
        "applied" => "appliqué",
        "confirmed" => "confirmé",
        "already_applied" => "déjà appliqué",
        "pending_confirmation" => "en attente",
        _ => "?",
    }
}

fn occurred_at_label(entry: &AuditEntry) -> String {
    time::OffsetDateTime::parse(&entry.occurred_at, &Rfc3339)
        .map(|t| format!("{:02}:{:02}:{:02}", t.hour(), t.minute(), t.second()))
        .unwrap_or_else(|_| entry.occurred_at.clone())
}

fn item(entry: &AuditEntry) -> Markup {
    let actor_class = match entry.actor_kind.as_str() {
        "agent" => "agent",
        "system" => "system",
        _ => "human",
    };
    let actor_label = match entry.actor_kind.as_str() {
        "agent" => "agent",
        "system" => "système",
        _ => "vous",
    };
    html! {
        div class="audit-item" {
            span class={ "audit-actor " (actor_class) } { (actor_label) }
            span class="audit-time" { (occurred_at_label(entry)) }
            div class="audit-desc" { code { (entry.command_name) } " — " (outcome_label(&entry.outcome)) }
        }
    }
}

/// Rendu du rail : déclenché par `hx-trigger="load, every 2s"` sur `#audit-items`, en
/// remplacement intégral (`hx-swap="innerHTML"`).
pub async fn recent(State(state): State<AppState>) -> Html<String> {
    let store = state.store.lock().await;
    let entries = recent_audit_entries(store.connection(), 0, RAIL_PAGE_SIZE).unwrap_or_default();
    drop(store);

    let markup = if entries.is_empty() {
        html! { div class="audit-empty" { "aucune activité pour l'instant" } }
    } else {
        html! { @for entry in &entries { (item(entry)) } }
    };
    Html(markup.into_string())
}
