//! Départ de la relève. La socket reste dans le crochet installé par la fenêtre.

use griffe_core::app::{Actor, AppError, ExecutionContext};
use griffe_core::mail::{self, PulledMailbox, RelevePlan};

use crate::state::{AppState, ReleveFetch};

/// Lance une relève si le crochet est là, qu'aucune n'est en cours, et que la précédente
/// a plus de [`mail::RELEVE_EVERY_SECS`] ou n'a jamais eu lieu. Un échec attend le même délai.
pub async fn kick_releve(state: &AppState) {
    if !state.lecture_engaged() || !state.vault_is_open().await || !state.lecture_due() {
        return;
    }
    let prepared = state.with_store(mail::releve_plan).await;
    let Some(prepared) = prepared else {
        return;
    };
    let plan = match prepared {
        Ok(Some(plan)) => plan,
        Ok(None) => return,
        Err(error) => {
            eprintln!("⚠ boîte : {error}");
            return;
        }
    };
    if !state.begin_lecture(&plan.address) {
        return;
    }
    let Some(fetch) = state.releve_fetch() else {
        state.cancel_lecture();
        return;
    };
    let state = state.clone();
    tokio::spawn(async move {
        run_releve(state, plan, fetch).await;
    });
}

async fn run_releve(state: AppState, plan: RelevePlan, fetch: ReleveFetch) {
    let icloud = plan.endpoint.icloud;
    let endpoint = plan.endpoint;
    let secret = plan.secret;
    let cursors = plan.cursors;
    let ticking = state.clone();
    let pulled = tokio::task::spawn_blocking(move || {
        fetch(&endpoint, &secret, &cursors, &mut |tick| {
            ticking.note_pull(tick);
        })
    })
    .await;
    match pulled {
        Ok(Ok(batch)) => absorb(&state, batch).await,
        Ok(Err(error)) => {
            eprintln!("⚠ boîte : {}", mail::french_submit_error(&error, icloud));
            state.note_pull_failed();
        }
        Err(error) => {
            eprintln!("⚠ boîte : {error}");
            state.note_pull_failed();
        }
    }
}

async fn absorb(state: &AppState, batch: Vec<PulledMailbox>) {
    let ctx = ExecutionContext::new(Actor::System, false);
    let recorded = state
        .with_store_mut(|store| -> Result<_, AppError> {
            let mut mailbox = store.open_mailbox()?;
            let before = mailbox.count()?;
            let report = mail::absorb(store, &mut mailbox, &batch, &ctx)?;
            let fresh = mailbox.recent_ids(report.stored)?;
            let after = mailbox.count()?;
            let changed = before != after || report.stored > 0;
            Ok((report, before == 0, changed, fresh))
        })
        .await;
    match recorded {
        Some(Ok((report, was_empty, changed, fresh))) => {
            state.note_pull_absorbed(&report, was_empty, changed, fresh);
        }
        Some(Err(error)) => {
            eprintln!("⚠ boîte : {error}");
            state.note_pull_failed();
        }
        None => state.note_pull_failed(),
    }
}
