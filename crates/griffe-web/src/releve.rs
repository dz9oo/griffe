//! Départ de la relève. La socket reste dans le crochet installé par la fenêtre.
//! Chaque lot est rangé dès qu'il arrive. Le corps suit, sans retenir la page.

use std::time::Duration;

use griffe_core::app::{Actor, AppError, ExecutionContext};
use griffe_core::mail::{self, PullEvent, PulledMailbox, RelevePlan};
use griffe_core::mailbox::PendingBody;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::state::{AppState, BodyFetch, ReleveFetch};

/// Lance une relève si le crochet est là, qu'aucune n'est en cours, et que la précédente
/// a atteint la période du compte — ou qu'on l'a demandée. Un échec attend la même période.
pub async fn kick_releve(state: &AppState) {
    if !state.lecture_engaged() || !state.vault_is_open().await {
        return;
    }
    let every = releve_every(state).await;
    if !state.lecture_due(Duration::from_secs(every)) {
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
            eprintln!("⚠ courriel : {error}");
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

async fn releve_every(state: &AppState) -> u64 {
    state
        .with_store(mail::releve_every)
        .await
        .and_then(Result::ok)
        .unwrap_or(mail::RELEVE_EVERY_SECS)
}

async fn run_releve(state: AppState, plan: RelevePlan, fetch: ReleveFetch) {
    let icloud = plan.endpoint.icloud;
    let was_empty = mirror_empty(&state).await;
    let generation = plan.kept_generation;
    let prepared = state
        .with_store(move |store| {
            let mut plan = plan;
            mail::prepare_pull(store, &mut plan).map(|sweep| (sweep, plan))
        })
        .await;
    let (sweep, plan) = match prepared {
        Some(Ok(pair)) => pair,
        Some(Err(error)) => {
            eprintln!("⚠ courriel : {error}");
            note_outcome(&state, false, Some("Le courriel n'a pas pu être lu.")).await;
            state.note_pull_failed();
            return;
        }
        None => {
            state.cancel_lecture();
            return;
        }
    };
    let (tx, mut rx) = tokio::sync::mpsc::channel::<PullEvent>(4);
    let endpoint = plan.endpoint;
    let secret = plan.secret;
    let cursors = plan.cursors;
    let kept = plan.kept;
    let joined = tokio::task::spawn_blocking(move || {
        let scope = mail::PullScope {
            cursors: &cursors,
            addresses: &kept,
            sweep,
        };
        fetch(&endpoint, &secret, &scope, &mut |event| {
            let _ = tx.blocking_send(event);
        })
    });
    let mut absorb_failed = false;
    while let Some(event) = rx.recv().await {
        match event {
            PullEvent::Tick(tick) => state.note_pull(tick),
            PullEvent::Batch(batch) => {
                if !absorb_batch(&state, &batch).await {
                    absorb_failed = true;
                }
            }
        }
    }
    match joined.await {
        Ok(Ok(())) if !absorb_failed => {
            if sweep && !kept_applied(&state, generation).await {
                note_outcome(&state, false, Some("Le courriel n'a pas pu être rangé.")).await;
                state.note_pull_failed();
                return;
            }
            note_outcome(&state, true, None).await;
            state.finish_lecture(was_empty);
            fill_recent(&state).await;
        }
        Ok(Ok(())) => {
            note_outcome(&state, false, Some("Le courriel n'a pas pu être rangé.")).await;
            state.note_pull_failed();
        }
        Ok(Err(error)) => {
            let sentence = mail::french_submit_error(&error, icloud);
            eprintln!("⚠ courriel : {sentence}");
            note_outcome(&state, false, Some(&sentence)).await;
            state.note_pull_failed();
        }
        Err(error) => {
            eprintln!("⚠ courriel : {error}");
            note_outcome(&state, false, Some("Le courriel n'a pas pu être lu.")).await;
            state.note_pull_failed();
        }
    }
}

async fn kept_applied(state: &AppState, generation: i64) -> bool {
    state
        .with_store(|store| mail::note_kept_applied(store, generation))
        .await
        .and_then(Result::ok)
        .is_some()
}

async fn mirror_empty(state: &AppState) -> bool {
    state
        .with_store(|store| -> Result<bool, AppError> {
            if !griffe_core::mailbox::mailbox_file(store.db_path()).exists() {
                return Ok(true);
            }
            Ok(store.open_mailbox()?.count()? == 0)
        })
        .await
        .and_then(Result::ok)
        .unwrap_or(true)
}

async fn absorb_batch(state: &AppState, batch: &PulledMailbox) -> bool {
    let ctx = ExecutionContext::new(Actor::System, false);
    let recorded = state
        .with_store_mut(|store| -> Result<_, AppError> {
            let mut mailbox = store.open_mailbox()?;
            let before = mailbox.count()?;
            let report = mail::absorb(store, &mut mailbox, std::slice::from_ref(batch), &ctx)?;
            let fresh = mailbox.recent_ids(report.stored)?;
            let after = mailbox.count()?;
            let changed = before != after || report.stored > 0;
            Ok((report, changed, fresh))
        })
        .await;
    match recorded {
        Some(Ok((report, changed, fresh))) => {
            state.note_batch(&report, changed, fresh);
            true
        }
        Some(Err(error)) => {
            eprintln!("⚠ courriel : {error}");
            false
        }
        None => false,
    }
}

async fn note_outcome(state: &AppState, ok: bool, error: Option<&str>) {
    let stamp = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| String::new());
    let sentence = error.map(str::to_string);
    state
        .with_store_mut(|store| {
            if ok {
                mail::note_releve_success(store, &stamp)
            } else if let Some(sentence) = sentence.as_deref() {
                mail::note_releve_failure(store, sentence)
            } else {
                Ok(())
            }
        })
        .await;
}

async fn fill_recent(state: &AppState) {
    let Some(fetch) = state.body_fetch() else {
        return;
    };
    let Some(Ok(Some(plan))) = state.with_store(mail::releve_plan).await else {
        return;
    };
    let Some(Ok(pending)) = state
        .with_store(|store| -> Result<_, AppError> {
            store.open_mailbox()?.pending_bodies(mail::RECENT_BODIES)
        })
        .await
    else {
        return;
    };
    let mut queue = pending;
    loop {
        if let Some(rushed) = state.take_preferred() {
            queue.retain(|item| item.mailbox != rushed.mailbox || item.uid != rushed.uid);
            queue.insert(0, rushed);
        }
        if queue.is_empty() {
            break;
        }
        let group = take_group(&mut queue);
        fill_batch(state, &fetch, &plan, &group).await;
    }
}

fn take_group(queue: &mut Vec<PendingBody>) -> Vec<PendingBody> {
    let Some(first) = queue.first().cloned() else {
        return Vec::new();
    };
    let mut group = Vec::new();
    queue.retain(|item| {
        let same = item.mailbox == first.mailbox && item.uidvalidity == first.uidvalidity;
        if same {
            group.push(item.clone());
        }
        !same
    });
    group
}

async fn fill_batch(state: &AppState, fetch: &BodyFetch, plan: &RelevePlan, items: &[PendingBody]) {
    let mut uids = Vec::new();
    for item in items {
        if let Ok(uid) = u32::try_from(item.uid) {
            uids.push(uid);
        }
    }
    if uids.is_empty() {
        return;
    }
    let Some(first) = items.first() else {
        return;
    };
    let endpoint = plan.endpoint.clone();
    let secret = password_of(&plan.secret);
    let mailbox = first.mailbox.clone();
    let uidvalidity = first.uidvalidity;
    let fetch = fetch.clone();
    let asked = mailbox.clone();
    let joined = tokio::task::spawn_blocking(move || {
        let Ok(secret) = mail::MailSecret::new(secret) else {
            return Err(mail::MailSubmitError::Auth);
        };
        fetch(&endpoint, &secret, &asked, &uids)
    })
    .await;
    let Ok(Ok(messages)) = joined else {
        return;
    };
    if messages.is_empty() {
        return;
    }
    let batch = PulledMailbox {
        name: mailbox,
        uidvalidity,
        resync: false,
        role: "inbox".to_string(),
        messages,
        modseq: None,
        examined_uid: 0,
        keep_uids: None,
    };
    if absorb_batch(state, &batch).await {
        state.note_body_landed();
    }
}

/// La page a ouvert une lettre dont le corps n'est pas encore là.
pub fn request_body(state: &AppState, body: PendingBody) {
    state.prefer_body(body.clone());
    let Some(fetch) = state.body_fetch() else {
        return;
    };
    let state = state.clone();
    tokio::spawn(async move {
        let Some(Ok(Some(plan))) = state.with_store(mail::releve_plan).await else {
            return;
        };
        fill_batch(&state, &fetch, &plan, std::slice::from_ref(&body)).await;
    });
}

fn password_of(secret: &mail::MailSecret) -> String {
    secret.expose().to_string()
}

/// Le texte montré quand le corps n'est pas dans le miroir.
#[must_use]
pub fn body_sentence(state: &str) -> Option<&'static str> {
    match state {
        "pending" => Some("Le texte arrive."),
        "too_large" => Some("Cette lettre est trop lourde pour être gardée ici."),
        "unreadable" => Some("Cette lettre n'a pas pu être lue."),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::take_group;
    use griffe_core::mailbox::PendingBody;

    fn item(mailbox: &str, uid: i64, uidvalidity: i64) -> PendingBody {
        PendingBody {
            mailbox: mailbox.to_string(),
            uid,
            uidvalidity,
        }
    }

    #[test]
    fn a_rushed_letter_leads_its_mailbox() {
        let mut queue = vec![
            item("INBOX", 1, 7),
            item("Sent", 2, 7),
            item("INBOX", 3, 7),
            item("INBOX", 4, 8),
        ];
        queue.insert(0, item("Sent", 9, 7));
        let sent = take_group(&mut queue);
        assert_eq!(sent.iter().map(|row| row.uid).collect::<Vec<_>>(), [9, 2]);
        let inbox = take_group(&mut queue);
        assert_eq!(inbox.iter().map(|row| row.uid).collect::<Vec<_>>(), [1, 3]);
        let other = take_group(&mut queue);
        assert_eq!(other.iter().map(|row| row.uid).collect::<Vec<_>>(), [4]);
        assert!(queue.is_empty());
    }
}
