//! Mise en forme du résultat d'une commande — deux formats seulement : humain et `--json` (le
//! contrat machine que consomme un agent). Aucun format intermédiaire.
//!
//! Ces fonctions renvoient une `String` plutôt que d'imprimer directement : c'est ce qui permet
//! à la console de la GUI (lot 9) de capturer la sortie sans dupliquer le moindre formatage —
//! seul le binaire `freeflow` (`main.rs` / `lib.rs::run`) l'imprime sur stdout.
//!
//! **Plus aucun repli `Debug` (lot 36).** Jusqu'ici, tout ce qui n'avait pas de rendu texte
//! dédié sortait en `{value:?}` — la structure Rust brute (`FiscalDeadline { kind: Ca3, due_on:
//! 2026-10-19, … }`), illisible pour la cible du produit. Le rendu humain passe désormais par
//! [`HumanRender`], que **le compilateur exige** de tout type qui sort en texte : un nouveau type
//! sans rendu ne compile pas, au lieu de fuir en `Debug`. Les branches purement machine
//! (`if json { … }`) utilisent [`format_json`], qui n'exige que `Serialize`.

use freeflow_core::app::Outcome;
use freeflow_core::billing::EmittedInvoice;
use freeflow_core::domain::{
    BankTransactionId, ClientId, ContactId, ExpenseId, FiscalYearId, InteractionId, InvoiceId,
    MissionId, OpportunityId, PaperId, PaymentId, QuoteId, TimeEntryId,
};
use freeflow_core::fiscal_year::Approval;
use serde::Serialize;

/// Le rendu texte d'une valeur, tel qu'un humain le lit dans un terminal ou dans la console de
/// la fenêtre. Multi-lignes autorisé ; sans retour à la ligne final.
pub trait HumanRender {
    fn render_human(&self) -> String;
}

/// Une liste de couples `libellé : valeur`, un par ligne, libellés alignés — le rendu de base
/// d'une fiche (`client show`, `expense show`, `company show`…).
pub fn key_values(pairs: &[(&str, String)]) -> String {
    let width = pairs
        .iter()
        .map(|(k, _)| k.chars().count())
        .max()
        .unwrap_or(0);
    pairs
        .iter()
        .map(|(k, v)| format!("{k:<width$} : {v}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Une valeur facultative en texte : `—` quand elle manque.
pub fn or_dash<T: std::fmt::Display>(value: Option<T>) -> String {
    value.map_or_else(|| "—".to_string(), |v| v.to_string())
}

pub fn format_json<T: Serialize>(value: &T) -> String {
    serde_json::to_string_pretty(value).expect("la valeur se sérialise toujours")
}

pub fn format_value<T: Serialize + HumanRender>(value: &T, json: bool) -> String {
    if json {
        format_json(value)
    } else {
        value.render_human()
    }
}

fn outcome_json<T: Serialize>(outcome: &Outcome<T>) -> String {
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
}

fn outcome_text(outcome: &Outcome<impl Serialize>, applied: impl Fn() -> String) -> String {
    match outcome {
        Outcome::Applied(_) => {
            let text = applied();
            if text.is_empty() {
                "✓".to_string()
            } else {
                format!("✓ {text}")
            }
        }
        Outcome::DryRun => "(dry-run) aucune écriture".to_string(),
        Outcome::AlreadyApplied(_) => {
            let text = applied();
            if text.is_empty() {
                "= déjà appliqué".to_string()
            } else {
                format!("= déjà appliqué : {text}")
            }
        }
        Outcome::PendingConfirmation(id) => {
            format!("⏸ en attente de confirmation humaine — id : {id}")
        }
    }
}

/// Le résultat d'une commande : en texte, `✓` suivi du rendu de la valeur produite (un
/// identifiant, une révision, un numéro de facture…).
pub fn format_outcome<T: Serialize + HumanRender>(outcome: &Outcome<T>, json: bool) -> String {
    if json {
        outcome_json(outcome)
    } else {
        let value = match outcome {
            Outcome::Applied(v) | Outcome::AlreadyApplied(v) => Some(v),
            Outcome::DryRun | Outcome::PendingConfirmation(_) => None,
        };
        outcome_text(outcome, || {
            value.map(HumanRender::render_human).unwrap_or_default()
        })
    }
}

/// Comme [`format_outcome`], avec un texte dédié à la place du rendu de la valeur — pour les
/// commandes dont la sortie du cœur est muette (`()`) ou trop technique pour dire ce qui vient
/// de se passer (« profil enregistré », « bilan d'ouverture enregistré (révision 2) »).
pub fn format_outcome_as<T: Serialize>(
    outcome: &Outcome<T>,
    json: bool,
    applied: impl Fn(&T) -> String,
) -> String {
    if json {
        outcome_json(outcome)
    } else {
        let value = match outcome {
            Outcome::Applied(v) | Outcome::AlreadyApplied(v) => Some(v),
            Outcome::DryRun | Outcome::PendingConfirmation(_) => None,
        };
        outcome_text(outcome, || value.map(&applied).unwrap_or_default())
    }
}

impl HumanRender for () {
    fn render_human(&self) -> String {
        String::new()
    }
}

/// Toutes les commandes `Update*`/`Archive*` du cœur rendent la révision résultante.
impl HumanRender for i64 {
    fn render_human(&self) -> String {
        format!("révision {self}")
    }
}

/// La seule sortie `u32` du cœur est le nombre de transactions importées (`bank import`).
impl HumanRender for u32 {
    fn render_human(&self) -> String {
        format!("{self} nouvelle(s) transaction(s) importée(s)")
    }
}

impl HumanRender for String {
    fn render_human(&self) -> String {
        self.clone()
    }
}

impl<T: HumanRender> HumanRender for Option<T> {
    fn render_human(&self) -> String {
        self.as_ref()
            .map_or_else(|| "—".to_string(), T::render_human)
    }
}

macro_rules! render_by_display {
    ($($ty:ty),* $(,)?) => {
        $(impl HumanRender for $ty {
            fn render_human(&self) -> String {
                self.to_string()
            }
        })*
    };
}

render_by_display!(
    BankTransactionId,
    ClientId,
    ContactId,
    ExpenseId,
    FiscalYearId,
    InteractionId,
    InvoiceId,
    MissionId,
    OpportunityId,
    PaperId,
    PaymentId,
    QuoteId,
    TimeEntryId,
);

impl HumanRender for EmittedInvoice {
    fn render_human(&self) -> String {
        format!("{} ({})", self.number, self.id)
    }
}

impl HumanRender for Approval {
    fn render_human(&self) -> String {
        match self.late_by_days {
            None => format!("approuvé (révision {})", self.revision),
            Some(days) => format!(
                "approuvé (révision {}) — ⚠ {days} jour(s) après l'échéance légale de six \
                 mois : le dépôt au greffe est lui aussi en retard, faites-le sans attendre",
                self.revision
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use freeflow_core::app::PendingActionId;

    #[test]
    fn an_applied_unit_outcome_is_a_bare_check_mark() {
        assert_eq!(format_outcome(&Outcome::<()>::Applied(()), false), "✓");
        assert_eq!(
            format_outcome(&Outcome::<()>::AlreadyApplied(()), false),
            "= déjà appliqué"
        );
    }

    #[test]
    fn an_applied_revision_and_a_pending_action_read_as_sentences() {
        assert_eq!(
            format_outcome(&Outcome::Applied(3i64), false),
            "✓ révision 3"
        );
        let id = PendingActionId::new();
        assert_eq!(
            format_outcome(&Outcome::<i64>::PendingConfirmation(id), false),
            format!("⏸ en attente de confirmation humaine — id : {id}")
        );
    }

    #[test]
    fn a_dedicated_text_replaces_the_value() {
        assert_eq!(
            format_outcome_as(&Outcome::Applied(()), false, |()| "profil enregistré"
                .into()),
            "✓ profil enregistré"
        );
        assert_eq!(
            format_outcome_as(&Outcome::<()>::DryRun, false, |()| "jamais".into()),
            "(dry-run) aucune écriture"
        );
    }

    #[test]
    fn a_late_approval_carries_its_warning() {
        let text = Approval {
            revision: 2,
            late_by_days: Some(20),
        }
        .render_human();
        assert!(
            text.starts_with("approuvé (révision 2) — ⚠ 20 jour(s)"),
            "{text}"
        );
    }

    #[test]
    fn key_values_align_the_labels() {
        assert_eq!(
            key_values(&[("nom", "Acme".into()), ("SIREN", "—".into())]),
            "nom   : Acme\nSIREN : —"
        );
    }
}
