//! `freeflow confirm`, `freeflow pending list`, `freeflow audit verify-chain`
//!
//! Le registre de confirmation (`confirm`) ne liste que les commandes qui déclarent
//! `requires_confirmation() == true` — à étendre au fil des lots suivants s'il y en a
//! d'autres (envoi d'email, par exemple, quand ce canal existera).

use clap::Subcommand;
use freeflow_core::app::{self, Command, Executor, PendingActionId};
use freeflow_core::billing::{EmitInvoice, IssueCreditNote, ReconcileTransaction, RecordPayment};
use freeflow_core::clients::{DeleteClient, DeleteContact};
use freeflow_core::expenses::DeleteExpense;
use freeflow_core::fiscal_year::{ApproveFiscalYear, CloseFiscalYear, DeleteFiscalYear};
use freeflow_core::missions::{DeleteMission, DeleteTimeEntry};
use freeflow_core::prospection::{DeleteInteraction, DeleteOpportunity};
use freeflow_core::store::Store;

use crate::error::CliError;
use crate::output::{format_outcome, format_value};

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
    Ok(format_value(&actions, json))
}

pub fn run_audit(cmd: AuditCommand, store: &mut Store, json: bool) -> Result<String, CliError> {
    let AuditCommand::VerifyChain = cmd;
    let status = app::verify_chain(store.connection())?;
    let broken = matches!(status, app::ChainStatus::BrokenAt(_));
    let payload = match status {
        app::ChainStatus::Intact => serde_json::json!({"status": "intact"}),
        app::ChainStatus::BrokenAt(sequence) => {
            serde_json::json!({"status": "broken", "broken_at_sequence": sequence})
        }
    };
    let rendered = format_value(&payload, json);
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
        Ok(format_outcome(&outcome, json))
    } else if action.command_name == IssueCreditNote::NAME {
        let outcome = Executor::new(store).confirm::<IssueCreditNote>(id)?;
        Ok(format_outcome(&outcome, json))
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
        let outcome = Executor::new(store).confirm::<ApproveFiscalYear>(id)?;
        Ok(format_outcome(&outcome, json))
    } else if action.command_name == DeleteFiscalYear::NAME {
        let outcome = Executor::new(store).confirm::<DeleteFiscalYear>(id)?;
        Ok(format_outcome(&outcome, json))
    } else if action.command_name == DeleteExpense::NAME {
        let outcome = Executor::new(store).confirm::<DeleteExpense>(id)?;
        Ok(format_outcome(&outcome, json))
    } else {
        Err(CliError::UnknownConfirmableCommand(action.command_name))
    }
}
