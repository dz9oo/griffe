//! Une liaison IMAP pour la fenêtre. La relève et les corps passent par le même fil.
//! Marquer lu reste une liaison courte : la page n'attend pas la fin de l'index.

use std::sync::mpsc::{self, Receiver, Sender, SyncSender};

use griffe_core::mail::{
    ImapEndpoint, MailSecret, MailSubmitError, PullEvent, PullScope, PulledMessage,
};
use griffe_core::mailbox::MailboxCursor;
use zeroize::Zeroize;

use crate::imap::{self, Session};

enum Job {
    Sync {
        endpoint: ImapEndpoint,
        password: String,
        cursors: Vec<MailboxCursor>,
        addresses: Vec<String>,
        sweep: bool,
        reply: SyncSender<SyncNote>,
    },
    Body {
        endpoint: ImapEndpoint,
        password: String,
        mailbox: String,
        uid: u32,
        reply: Sender<Result<PulledMessage, MailSubmitError>>,
    },
}

enum SyncNote {
    Event(PullEvent),
    Done(Result<(), MailSubmitError>),
}

struct Live {
    session: Session,
    host: String,
    port: u16,
    username: String,
}

/// File d'une seule liaison. Cloner ne fait que partager la file.
#[derive(Clone)]
pub struct MailHub {
    jobs: Sender<Job>,
}

impl MailHub {
    /// Démarre le fil `griffe-imap`. Il vit autant que le processus.
    ///
    /// # Panics
    ///
    /// Le système refuse de créer le fil.
    #[must_use]
    pub fn start() -> Self {
        let (jobs, incoming) = mpsc::channel();
        // Abandonner le `JoinHandle` détache le fil. Il s'arrête avec le processus.
        drop(
            std::thread::Builder::new()
                .name("griffe-imap".to_string())
                .spawn(move || serve(incoming))
                .expect("le fil de la relève démarre"),
        );
        Self { jobs }
    }

    /// Relève les en-têtes. `on` est appelé depuis le fil qui a demandé la relève.
    ///
    /// # Errors
    ///
    /// La liaison ou la lecture échoue. Le mot de passe n'est pas dans le message.
    pub fn sync(
        &self,
        endpoint: &ImapEndpoint,
        secret: &MailSecret,
        scope: &PullScope<'_>,
        on: &mut dyn FnMut(PullEvent),
    ) -> Result<(), MailSubmitError> {
        let (reply, incoming) = mpsc::sync_channel(4);
        self.jobs
            .send(Job::Sync {
                endpoint: endpoint.clone(),
                password: secret.expose().to_string(),
                cursors: scope.cursors.to_vec(),
                addresses: scope.addresses.to_vec(),
                sweep: scope.sweep,
                reply,
            })
            .map_err(|_| broken())?;
        while let Ok(note) = incoming.recv() {
            match note {
                SyncNote::Event(event) => on(event),
                SyncNote::Done(result) => return result,
            }
        }
        Err(broken())
    }

    /// Corps d'un message déjà indexé.
    ///
    /// # Errors
    ///
    /// La liaison ou la lecture échoue.
    pub fn body(
        &self,
        endpoint: &ImapEndpoint,
        secret: &MailSecret,
        mailbox: &str,
        uid: u32,
    ) -> Result<PulledMessage, MailSubmitError> {
        let (reply, incoming) = mpsc::channel();
        self.jobs
            .send(Job::Body {
                endpoint: endpoint.clone(),
                password: secret.expose().to_string(),
                mailbox: mailbox.to_string(),
                uid,
                reply,
            })
            .map_err(|_| broken())?;
        incoming.recv().unwrap_or_else(|_| Err(broken()))
    }
}

fn serve(incoming: Receiver<Job>) {
    let mut live = None;
    while let Ok(job) = incoming.recv() {
        match job {
            Job::Sync {
                endpoint,
                mut password,
                cursors,
                addresses,
                sweep,
                reply,
            } => {
                let outcome = secret_of(&password).and_then(|secret| {
                    let scope = PullScope {
                        cursors: &cursors,
                        addresses: &addresses,
                        sweep,
                    };
                    sync_with(&mut live, &endpoint, &secret, &scope, &reply)
                });
                password.zeroize();
                let _ = reply.send(SyncNote::Done(outcome));
            }
            Job::Body {
                endpoint,
                mut password,
                mailbox,
                uid,
                reply,
            } => {
                let outcome = secret_of(&password).and_then(|secret| {
                    let session = ensure(&mut live, &endpoint, &secret)?;
                    session.read_one(secret.expose(), &mailbox, uid)
                });
                password.zeroize();
                let _ = reply.send(outcome);
            }
        }
    }
}

fn sync_with(
    live: &mut Option<Live>,
    endpoint: &ImapEndpoint,
    secret: &MailSecret,
    scope: &PullScope<'_>,
    reply: &SyncSender<SyncNote>,
) -> Result<(), MailSubmitError> {
    let session = ensure(live, endpoint, secret)?;
    session.read_folders(endpoint, secret, scope, &mut |event| {
        let _ = reply.send(SyncNote::Event(event));
    })
}

fn ensure<'a>(
    live: &'a mut Option<Live>,
    endpoint: &ImapEndpoint,
    secret: &MailSecret,
) -> Result<&'a mut Session, MailSubmitError> {
    let reusable = live.as_mut().is_some_and(|current| {
        current.host == endpoint.host
            && current.port == endpoint.port
            && current.username == endpoint.username
            && current.session.still_there()
    });
    if reusable {
        let Some(current) = live.as_mut() else {
            return Err(broken());
        };
        return Ok(&mut current.session);
    }
    if let Some(mut current) = live.take() {
        current.session.close();
    }
    let session = imap::open_session(endpoint, secret)?;
    *live = Some(Live {
        session,
        host: endpoint.host.clone(),
        port: endpoint.port,
        username: endpoint.username.clone(),
    });
    let Some(current) = live.as_mut() else {
        return Err(broken());
    };
    Ok(&mut current.session)
}

fn secret_of(password: &str) -> Result<MailSecret, MailSubmitError> {
    MailSecret::new(password.to_string()).map_err(|_| broken())
}

fn broken() -> MailSubmitError {
    MailSubmitError::Other("Le courriel n'a pas pu être lu.".into())
}
