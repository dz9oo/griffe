//! Mise en forme du résultat d'une commande — deux formats seulement : humain (une ligne) et
//! `--json` (le contrat machine que consomme un agent). Aucun format intermédiaire.
//!
//! Ces fonctions renvoient une `String` plutôt que d'imprimer directement : c'est ce qui permet
//! à la console de la GUI (lot 9) de capturer la sortie sans dupliquer le moindre formatage —
//! seul le binaire `freeflow` (`main.rs` / `lib.rs::run`) l'imprime sur stdout.

use std::fmt::Debug;

use freeflow_core::app::Outcome;
use serde::Serialize;

pub fn format_outcome<T: Serialize + Debug>(outcome: &Outcome<T>, json: bool) -> String {
    if json {
        let value = match outcome {
            Outcome::Applied(v) => serde_json::json!({"status": "applied", "result": v}),
            Outcome::DryRun => serde_json::json!({"status": "dry_run"}),
            Outcome::AlreadyApplied(v) => {
                serde_json::json!({"status": "already_applied", "result": v})
            }
            Outcome::PendingConfirmation(id) => {
                serde_json::json!({"status": "pending_confirmation", "pending_action_id": id.to_string()})
            }
        };
        serde_json::to_string_pretty(&value).expect("une Value serde_json se sérialise toujours")
    } else {
        match outcome {
            Outcome::Applied(v) => format!("✓ {v:?}"),
            Outcome::DryRun => "(dry-run) aucune écriture".to_string(),
            Outcome::AlreadyApplied(v) => format!("= déjà appliqué : {v:?}"),
            Outcome::PendingConfirmation(id) => {
                format!("⏸ en attente de confirmation humaine — id : {id}")
            }
        }
    }
}

pub fn format_value<T: Serialize + Debug>(value: &T, json: bool) -> String {
    if json {
        serde_json::to_string_pretty(value).expect("la valeur se sérialise toujours")
    } else {
        format!("{value:?}")
    }
}
