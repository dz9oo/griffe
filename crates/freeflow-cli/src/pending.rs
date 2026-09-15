//! `freeflow confirm`, `freeflow pending list`, `freeflow audit verify-chain`
//!
//! Le registre de confirmation (`confirm`) ne liste que les commandes qui déclarent
//! `requires_confirmation() == true` — à étendre au fil des lots suivants s'il y en a
//! d'autres (envoi d'email, par exemple, quand ce canal existera).

use clap::Subcommand;
use freeflow_core::app::{self, Actor, Command, ExecutionContext, Executor, PendingActionId};
use freeflow_core::billing::{
    DeleteBankTransaction, EmitInvoice, IssueCreditNote, ReconcileTransaction, RecordPayment,
    SettleBankTransaction, UnreconcileTransaction, UnsettleBankTransaction, VoidPayment,
};
use freeflow_core::clients::{DeleteClient, DeleteContact};
use freeflow_core::expenses::{DeleteExpense, ReconcileExpense, RecordExpense};
use freeflow_core::fiscal_year::{ApproveFiscalYear, CloseFiscalYear, DeleteFiscalYear};
use freeflow_core::fixed_assets::DeleteFixedAsset;
use freeflow_core::follow_up::MarkFollowUpSent;
use freeflow_core::missions::{DeleteMission, DeleteTimeEntry};
use freeflow_core::opening_balance::{
    DeleteOpeningBalance, RecordOpeningBalance, UpdateOpeningBalance,
};
use freeflow_core::papers::PurgePaper;
use freeflow_core::prospection::{DeleteInteraction, DeleteOpportunity};
use freeflow_core::society::{
    DeleteVatCarryIn, MarkCatchUpFiled, MarkDutyFiled, RecordVatCarryIn, RecordVatReversal,
    RequestVatRefund, RetractDutyFiled, RetractVatRefund, RetractVatReversal, UpdateVatCarryIn,
};
use freeflow_core::store::Store;

use crate::error::CliError;
use crate::output::{format_json, format_outcome};
use crate::table;

fn human_ctx() -> ExecutionContext {
    ExecutionContext::new(Actor::Human, false)
}

#[derive(Debug, Subcommand)]
pub enum PendingCommand {
    /// Liste les actions en attente de confirmation humaine.
    List,
}

#[derive(Debug, Subcommand)]
pub enum AuditCommand {
    /// Revérifie la chaîne de hash du journal d'audit.
    VerifyChain,
}

pub fn run_pending(cmd: PendingCommand, store: &mut Store, json: bool) -> Result<String, CliError> {
    let PendingCommand::List = cmd;
    let actions = app::list_pending_actions(store.connection())?;
    if json {
        return Ok(format_json(&actions));
    }
    if actions.is_empty() {
        return Ok("aucune action en attente de confirmation".to_string());
    }
    let rows: Vec<Vec<String>> = actions
        .iter()
        .map(|a| {
            vec![
                a.id.to_string(),
                a.command_name.clone(),
                a.actor_session.clone(),
                a.status.as_str().to_string(),
            ]
        })
        .collect();
    Ok(format!(
        "{}\nConfirmer : freeflow confirm <id>",
        table::render(&["id", "commande", "session", "statut"], &rows)
    ))
}

pub fn run_audit(cmd: AuditCommand, store: &mut Store, json: bool) -> Result<String, CliError> {
    let AuditCommand::VerifyChain = cmd;
    let status = app::verify_chain(store.connection())?;
    let broken = matches!(status, app::ChainStatus::BrokenAt(_));
    let rendered = match status {
        app::ChainStatus::Intact if json => format_json(&serde_json::json!({"status": "intact"})),
        app::ChainStatus::Intact => {
            "✓ journal d'audit intact : chaque entrée chaîne la précédente".to_string()
        }
        app::ChainStatus::BrokenAt(sequence) if json => {
            format_json(&serde_json::json!({"status": "broken", "broken_at_sequence": sequence}))
        }
        app::ChainStatus::BrokenAt(sequence) => {
            format!("✗ chaîne rompue à la séquence {sequence}")
        }
    };
    if broken {
        return Err(CliError::Domain(format!(
            "{rendered}\nle journal d'audit est rompu"
        )));
    }
    Ok(rendered)
}

/// Confirme une action en attente : retrouve son type de commande d'origine par son nom
/// stocké, puis rejoue [`Executor::confirm`] avec le type concret correspondant.
pub fn confirm(store: &mut Store, id: PendingActionId, json: bool) -> Result<String, CliError> {
    let action = app::pending_action_by_id(store.connection(), id)?
        .ok_or_else(|| CliError::PendingAction(id.to_string()))?;

    if action.command_name == EmitInvoice::NAME {
        let outcome = Executor::new(store).confirm::<EmitInvoice>(id)?;
        let rendered = format_outcome(&outcome, json);
        let note = match &outcome {
            freeflow_core::app::Outcome::Applied(emitted) => crate::papers::invoice_capture_note(
                crate::papers::capture_invoice(store, &human_ctx(), emitted.id),
            ),
            _ => None,
        };
        Ok(crate::papers::append_capture_note(rendered, json, note))
    } else if action.command_name == IssueCreditNote::NAME {
        let outcome = Executor::new(store).confirm::<IssueCreditNote>(id)?;
        let rendered = format_outcome(&outcome, json);
        let note = match &outcome {
            freeflow_core::app::Outcome::Applied(emitted) => crate::papers::invoice_capture_note(
                crate::papers::capture_invoice(store, &human_ctx(), emitted.id),
            ),
            _ => None,
        };
        Ok(crate::papers::append_capture_note(rendered, json, note))
    } else if action.command_name == DeleteClient::NAME {
        let outcome = Executor::new(store).confirm::<DeleteClient>(id)?;
        Ok(format_outcome(&outcome, json))
    } else if action.command_name == DeleteOpportunity::NAME {
        let outcome = Executor::new(store).confirm::<DeleteOpportunity>(id)?;
        Ok(format_outcome(&outcome, json))
    } else if action.command_name == DeleteMission::NAME {
        let outcome = Executor::new(store).confirm::<DeleteMission>(id)?;
        Ok(format_outcome(&outcome, json))
    } else if action.command_name == RecordPayment::NAME {
        let outcome = Executor::new(store).confirm::<RecordPayment>(id)?;
        Ok(format_outcome(&outcome, json))
    } else if action.command_name == ReconcileTransaction::NAME {
        let outcome = Executor::new(store).confirm::<ReconcileTransaction>(id)?;
        Ok(format_outcome(&outcome, json))
    } else if action.command_name == DeleteContact::NAME {
        let outcome = Executor::new(store).confirm::<DeleteContact>(id)?;
        Ok(format_outcome(&outcome, json))
    } else if action.command_name == DeleteTimeEntry::NAME {
        let outcome = Executor::new(store).confirm::<DeleteTimeEntry>(id)?;
        Ok(format_outcome(&outcome, json))
    } else if action.command_name == DeleteInteraction::NAME {
        let outcome = Executor::new(store).confirm::<DeleteInteraction>(id)?;
        Ok(format_outcome(&outcome, json))
    } else if action.command_name == CloseFiscalYear::NAME {
        let outcome = Executor::new(store).confirm::<CloseFiscalYear>(id)?;
        Ok(format_outcome(&outcome, json))
    } else if action.command_name == ApproveFiscalYear::NAME {
        // Même filet que `year approve` : l'approbation rend l'exercice immuable, une sauvegarde
        // est écrite d'abord (lot 36) — son échec abandonne la confirmation.
        let backup = crate::year::pre_approve_backup_for_action(store, &action.command_json)?;
        let cmd: ApproveFiscalYear = serde_json::from_str(&action.command_json)
            .map_err(|e| CliError::Unexpected(format!("action en attente illisible : {e}")))?;
        let period = freeflow_core::fiscal_year::fiscal_year_by_id(store.connection(), cmd.id)?
            .map_or(0, |r| r.ends_on.year());
        let outcome = Executor::new(store).confirm::<ApproveFiscalYear>(id)?;
        let rendered = crate::year::with_backup_note(format_outcome(&outcome, json), &backup, json);
        let note = if matches!(outcome, freeflow_core::app::Outcome::Applied(_)) {
            crate::papers::capture_year(store, &human_ctx(), period)
                .ok()
                .and_then(|r| r.human_note())
        } else {
            None
        };
        Ok(crate::papers::append_capture_note(rendered, json, note))
    } else if action.command_name == DeleteFiscalYear::NAME {
        let outcome = Executor::new(store).confirm::<DeleteFiscalYear>(id)?;
        Ok(format_outcome(&outcome, json))
    } else if action.command_name == DeleteExpense::NAME {
        let outcome = Executor::new(store).confirm::<DeleteExpense>(id)?;
        Ok(format_outcome(&outcome, json))
    } else if action.command_name == ReconcileExpense::NAME {
        let outcome = Executor::new(store).confirm::<ReconcileExpense>(id)?;
        Ok(format_outcome(&outcome, json))
    } else if action.command_name == RecordExpense::NAME {
        // Une dépense n'est en attente que si un agent l'a proposée *rapprochée* d'un débit du
        // relevé (`bank_transaction_id`) — le seul cas où `RecordExpense` exige confirmation.
        let outcome = Executor::new(store).confirm::<RecordExpense>(id)?;
        if let freeflow_core::app::Outcome::Applied(expense_id) = &outcome
            && let Some(expense) =
                freeflow_core::expenses::expense_by_id(store.connection(), *expense_id)?
            && let (Some(filename), Some(hash)) = (
                expense.receipt_filename.as_deref(),
                expense.receipt_hash.as_deref(),
            )
        {
            let _ = crate::papers::capture_expense_receipt(
                store,
                &human_ctx(),
                *expense_id,
                filename,
                hash,
            );
        }
        Ok(format_outcome(&outcome, json))
    } else if action.command_name == VoidPayment::NAME {
        let outcome = Executor::new(store).confirm::<VoidPayment>(id)?;
        Ok(format_outcome(&outcome, json))
    } else if action.command_name == UnreconcileTransaction::NAME {
        let outcome = Executor::new(store).confirm::<UnreconcileTransaction>(id)?;
        Ok(format_outcome(&outcome, json))
    } else if action.command_name == SettleBankTransaction::NAME {
        let outcome = Executor::new(store).confirm::<SettleBankTransaction>(id)?;
        Ok(format_outcome(&outcome, json))
    } else if action.command_name == UnsettleBankTransaction::NAME {
        let outcome = Executor::new(store).confirm::<UnsettleBankTransaction>(id)?;
        Ok(format_outcome(&outcome, json))
    } else if action.command_name == DeleteBankTransaction::NAME {
        let outcome = Executor::new(store).confirm::<DeleteBankTransaction>(id)?;
        Ok(format_outcome(&outcome, json))
    } else if action.command_name == RecordOpeningBalance::NAME {
        let outcome = Executor::new(store).confirm::<RecordOpeningBalance>(id)?;
        Ok(format_outcome(&outcome, json))
    } else if action.command_name == UpdateOpeningBalance::NAME {
        let outcome = Executor::new(store).confirm::<UpdateOpeningBalance>(id)?;
        Ok(format_outcome(&outcome, json))
    } else if action.command_name == DeleteOpeningBalance::NAME {
        let outcome = Executor::new(store).confirm::<DeleteOpeningBalance>(id)?;
        Ok(format_outcome(&outcome, json))
    } else if action.command_name == DeleteFixedAsset::NAME {
        let outcome = Executor::new(store).confirm::<DeleteFixedAsset>(id)?;
        Ok(format_outcome(&outcome, json))
    } else if action.command_name == MarkFollowUpSent::NAME {
        let outcome = Executor::new(store).confirm::<MarkFollowUpSent>(id)?;
        Ok(format_outcome(&outcome, json))
    } else if action.command_name == MarkDutyFiled::NAME {
        let outcome = Executor::new(store).confirm::<MarkDutyFiled>(id)?;
        Ok(format_outcome(&outcome, json))
    } else if action.command_name == MarkCatchUpFiled::NAME {
        let outcome = Executor::new(store).confirm::<MarkCatchUpFiled>(id)?;
        Ok(format_outcome(&outcome, json))
    } else if action.command_name == RetractDutyFiled::NAME {
        let outcome = Executor::new(store).confirm::<RetractDutyFiled>(id)?;
        Ok(format_outcome(&outcome, json))
    } else if action.command_name == RecordVatCarryIn::NAME {
        let outcome = Executor::new(store).confirm::<RecordVatCarryIn>(id)?;
        Ok(format_outcome(&outcome, json))
    } else if action.command_name == UpdateVatCarryIn::NAME {
        let outcome = Executor::new(store).confirm::<UpdateVatCarryIn>(id)?;
        Ok(format_outcome(&outcome, json))
    } else if action.command_name == DeleteVatCarryIn::NAME {
        let outcome = Executor::new(store).confirm::<DeleteVatCarryIn>(id)?;
        Ok(format_outcome(&outcome, json))
    } else if action.command_name == RequestVatRefund::NAME {
        let outcome = Executor::new(store).confirm::<RequestVatRefund>(id)?;
        Ok(format_outcome(&outcome, json))
    } else if action.command_name == RetractVatRefund::NAME {
        let outcome = Executor::new(store).confirm::<RetractVatRefund>(id)?;
        Ok(format_outcome(&outcome, json))
    } else if action.command_name == RecordVatReversal::NAME {
        let outcome = Executor::new(store).confirm::<RecordVatReversal>(id)?;
        Ok(format_outcome(&outcome, json))
    } else if action.command_name == RetractVatReversal::NAME {
        let outcome = Executor::new(store).confirm::<RetractVatReversal>(id)?;
        Ok(format_outcome(&outcome, json))
    } else if action.command_name == PurgePaper::NAME {
        let outcome = Executor::new(store).confirm::<PurgePaper>(id)?;
        Ok(format_outcome(&outcome, json))
    } else {
        Err(CliError::UnknownConfirmableCommand(action.command_name))
    }
}
