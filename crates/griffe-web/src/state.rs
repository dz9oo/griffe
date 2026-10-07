//! État partagé du routeur : le coffre, verrouillé ou non, protégé par un mutex — la GUI est
//! mono-process comme la CLI et le serveur MCP, et suit le même modèle de concurrence
//! multi-process décrit dans le plan (WAL + `busy_timeout`, pas de démon).
//!
//! La session de la fenêtre (en mémoire, ici) et la session de la CLI (dans le trousseau OS,
//! `griffe_core::store`) sont deux objets distincts : verrouiller la fenêtre ferme la
//! connexion en mémoire *et* purge la session trousseau (pour que « verrouiller » veuille dire
//! une seule chose), mais un `freeflow lock` tapé dans un terminal pendant que la fenêtre est
//! ouverte ne referme pas cette fenêtre tant qu'elle n'a pas relu son propre état — limite
//! acceptée, documentée dans `CLAUDE.md`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use griffe_core::app::{Actor, ExecutionContext};
use griffe_core::domain::parse_date;
use griffe_core::store::{AUTO_BACKUP_MAX_AGE, Passphrase, Store, StoreError, VaultStatus};
use tokio::sync::Mutex;

/// Lit `GRIFFE_TODAY` (`AAAA-MM-JJ`). `None` ou une chaîne vide : horloge locale. Une valeur
/// illisible est une erreur — le binaire de dev refuse de démarrer plutôt que de photographier
/// « aujourd'hui » en silence.
///
/// # Errors
///
/// Chaîne non vide qui n'est pas une date `AAAA-MM-JJ`.
pub fn parse_today_opt(raw: Option<&str>) -> Result<Option<time::Date>, String> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let s = raw.trim();
    if s.is_empty() {
        return Ok(None);
    }
    parse_date(s)
        .map(Some)
        .map_err(|e| format!("GRIFFE_TODAY invalide ({s}) : {e}"))
}

/// Durée de la session trousseau quand la case des 12 h est cochée. La même durée suspend
/// le verrouillage d'inactivité de la fenêtre, si le trousseau accepte la clé. Même défaut
/// que `griffe unlock --remember` en CLI.
const DEFAULT_SESSION_TTL: Duration = Duration::from_secs(12 * 3600);

/// Sauvegarde périodique après ouverture réussie — même contrat que la CLI (`GRIFFE_TEST_KDF`
/// en debug saute la copie SQLCipher pour le harness). Un échec n'empêche pas l'unlock.
fn maybe_auto_backup(store: &Store, db_path: &Path) {
    if cfg!(debug_assertions) && std::env::var_os("GRIFFE_TEST_KDF").is_some() {
        return;
    }
    if let Err(e) =
        store.auto_backup_if_stale(&db_path.with_file_name("backups"), AUTO_BACKUP_MAX_AGE)
    {
        eprintln!("⚠ sauvegarde automatique échouée : {e}");
    }
}

/// Délai d'inactivité au-delà duquel la fenêtre se reverrouille toute seule, sans démon : c'est
/// le polling du rail d'audit lui-même (`hx-trigger="load, every 2s"`, *exclu* du calcul
/// d'activité ci-dessous) qui garantit qu'une requête arrive assez vite pour déclencher la
/// bascule vers l'écran de déverrouillage.
const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// `true` quand l'inactivité dépasse `timeout`. Une suspension encore en cours
/// (`now` strictement avant `suspended_until`) tient la fenêtre ouverte. Passée cette
/// échéance, le délai repart de `last_activity`, la dernière navigation réelle.
/// `suspended_until` absent : pas de suspension, seul le délai compte.
#[must_use]
pub(crate) fn idle_expired(
    last_activity: Instant,
    now: Instant,
    timeout: Duration,
    suspended_until: Option<Instant>,
) -> bool {
    if let Some(until) = suspended_until
        && now < until
    {
        return false;
    }
    now.saturating_duration_since(last_activity) > timeout
}

/// Temps qui reste avant l'échéance du trousseau. `None` une fois cette échéance atteinte :
/// la règle du quart d'heure reprend.
fn suspension_duration(
    expires_at: time::OffsetDateTime,
    now: time::OffsetDateTime,
) -> Option<Duration> {
    let remaining = expires_at - now;
    if !remaining.is_positive() {
        return None;
    }
    let secs = u64::try_from(remaining.whole_seconds()).ok()?;
    let nanos = u32::try_from(remaining.subsec_nanoseconds()).ok()?;
    Some(Duration::new(secs, nanos))
}

/// Suspension posée au déverrouillage quand le trousseau a accepté la clé. Un échec, ou une
/// case décochée, n'en pose pas : le quart d'heure reste.
fn suspension_if_remembered(now: Instant, ttl: Duration, remembered: bool) -> Option<Instant> {
    remembered.then_some(now + ttl)
}

/// Temps qui reste sur une session trousseau déjà acceptée. `None` si l'échéance est illisible
/// ou déjà passée : la fenêtre s'ouvre, le quart d'heure s'applique.
fn idle_suspension_from_cache(db_path: &Path) -> Option<Instant> {
    let Ok(VaultStatus::Exists {
        session_expires_at: Some(expires_at),
        ..
    }) = Store::status(db_path)
    else {
        return None;
    };
    suspension_duration(expires_at, time::OffsetDateTime::now_utc())
        .map(|left| Instant::now() + left)
}

enum VaultSession {
    Locked,
    // `Store` boxé : sinon `VaultSession` hériterait de sa taille (la connexion SQLite en fait
    // partie) même dans la variante `Locked`, qui n'en a pourtant pas besoin.
    Unlocked {
        store: Box<Store>,
        last_activity: Instant,
        /// Échéance jusqu'à laquelle l'inactivité ne referme pas la fenêtre. `None` : le quart
        /// d'heure s'applique. Posée quand `remember` réussit, ou à la réouverture par le
        /// trousseau pour le temps qui reste.
        suspended_until: Option<Instant>,
    },
}

#[derive(Clone)]
pub struct AppState {
    session: Arc<Mutex<VaultSession>>,
    db_path: Arc<PathBuf>,
    idle_timeout: Duration,
    /// Date du jour figée (tests) ; `None` = l'horloge locale de la machine (lot 36).
    fixed_today: Option<time::Date>,
}

/// Instantané de l'état du coffre pour le rendu et le middleware — ne donne jamais accès au
/// `Store` lui-même.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultSnapshot {
    /// Aucun `.db`/`.kdf` à cet emplacement : rien à déverrouiller, seulement à créer.
    Absent,
    Locked {
        /// Un changement de passphrase a été commencé et interrompu — l'écran de déverrouillage
        /// le signale avant même une première tentative plutôt que de laisser l'utilisateur
        /// découvrir la cause par un premier échec.
        passphrase_change_pending: bool,
    },
    Unlocked,
}

impl AppState {
    #[must_use]
    pub fn new(db_path: PathBuf) -> Self {
        Self {
            session: Arc::new(Mutex::new(VaultSession::Locked)),
            db_path: Arc::new(db_path),
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
            fixed_today: None,
        }
    }

    /// Fige la date du jour que la fenêtre transmet au cœur (clôture, approbation, parcours,
    /// calendrier) — pour les tests, qui rejouent des exercices à des dates choisies. En
    /// production, `today` est l'horloge locale ([`griffe_core::clock::today_local`]).
    #[must_use]
    pub fn with_today(self, today: time::Date) -> Self {
        Self {
            fixed_today: Some(today),
            ..self
        }
    }

    /// La date du jour, fournie par cet adaptateur à toute requête ou commande du cœur qui en
    /// dépend — jamais lue par le cœur lui-même.
    #[must_use]
    pub fn today(&self) -> time::Date {
        self.fixed_today
            .unwrap_or_else(griffe_core::clock::today_local)
    }

    /// Comme [`Self::new`], avec un délai d'inactivité explicite plutôt que le défaut de 15
    /// minutes — pour les tests, ou un adaptateur qui voudrait exposer ce réglage.
    #[must_use]
    pub fn with_idle_timeout(db_path: PathBuf, idle_timeout: Duration) -> Self {
        Self {
            idle_timeout,
            ..Self::new(db_path)
        }
    }

    #[must_use]
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// Toute action déclenchée depuis la GUI est un acte humain direct — contrairement au
    /// serveur MCP, il n'y a pas d'acteur agent ici : la console exécute la même CLI, mais
    /// tapée par la personne qui a la souris.
    #[must_use]
    pub fn human_ctx() -> ExecutionContext {
        ExecutionContext::new(Actor::Human, false)
    }

    /// Tentative silencieuse d'ouverture via une session déjà en cache dans le trousseau OS —
    /// pour sauter l'écran de déverrouillage au démarrage si une session valide existe déjà.
    /// N'échoue jamais bruyamment : à défaut, l'écran de déverrouillage s'affiche normalement.
    /// Le temps qui reste sur le trousseau suspend le verrouillage d'inactivité.
    pub async fn try_open_cached(&self) {
        if let Ok(store) = Store::open_cached(&self.db_path) {
            maybe_auto_backup(&store, &self.db_path);
            let now = Instant::now();
            *self.session.lock().await = VaultSession::Unlocked {
                store: Box::new(store),
                last_activity: now,
                suspended_until: idle_suspension_from_cache(&self.db_path),
            };
        }
    }

    /// Déverrouille avec `passphrase`. Si `remember` est vrai et que le trousseau accepte la
    /// clé, elle y reste 12 h et l'inactivité ne referme pas la fenêtre avant cette échéance.
    /// Si le trousseau refuse, la fenêtre s'ouvre et le quart d'heure s'applique.
    ///
    /// # Errors
    pub async fn unlock(&self, passphrase: &Passphrase, remember: bool) -> Result<(), StoreError> {
        let store = Store::open_with_passphrase(&self.db_path, passphrase)?;
        // Le trousseau est best-effort : un échec n'empêche pas d'ouvrir la fenêtre, il
        // empêche seulement la promesse des 12 h.
        let remembered = remember && store.remember(DEFAULT_SESSION_TTL).is_ok();
        maybe_auto_backup(&store, &self.db_path);
        // Lot 39 : les justificatifs en clair d'avant sont chiffrés au premier déverrouillage
        // (idempotent, silencieux — une pièce qui résiste sera reprise la prochaine fois).
        let _ = griffe_core::receipts::migrate_legacy(&store);
        let now = Instant::now();
        *self.session.lock().await = VaultSession::Unlocked {
            store: Box::new(store),
            last_activity: now,
            suspended_until: suspension_if_remembered(now, DEFAULT_SESSION_TTL, remembered),
        };
        Ok(())
    }

    /// Crée le coffre puis le déverrouille immédiatement. La case des 12 h suit la même règle
    /// que [`Self::unlock`].
    ///
    /// # Errors
    pub async fn create(&self, passphrase: &Passphrase, remember: bool) -> Result<(), StoreError> {
        let store = Store::create(&self.db_path, passphrase)?;
        let remembered = remember && store.remember(DEFAULT_SESSION_TTL).is_ok();
        let now = Instant::now();
        *self.session.lock().await = VaultSession::Unlocked {
            store: Box::new(store),
            last_activity: now,
            suspended_until: suspension_if_remembered(now, DEFAULT_SESSION_TTL, remembered),
        };
        Ok(())
    }

    /// Pose une suspension d'inactivité sur la session déjà ouverte, sans écrire dans le
    /// trousseau. Les tests s'en servent à la place de `remember = true`.
    #[doc(hidden)]
    pub async fn suspend_idle_for(&self, duration: Duration) {
        if let VaultSession::Unlocked {
            suspended_until, ..
        } = &mut *self.session.lock().await
        {
            *suspended_until = Some(Instant::now() + duration);
        }
    }

    /// Ferme la connexion et purge la session du trousseau OS — pour que « verrouiller » veuille
    /// dire une seule chose, que ce soit le bouton de la fenêtre ou `freeflow lock` tapé dans la
    /// console intégrée. La fermeture est immédiate, même pendant la suspension des 12 h.
    pub async fn lock(&self) {
        if !Store::lock(&self.db_path) {
            eprintln!("⚠ échec de la purge de la session dans le trousseau du système");
        }
        *self.session.lock().await = VaultSession::Locked;
    }

    /// Vérifie l'expiration d'inactivité (verrouille si dépassée) puis rapporte l'état résultant.
    /// Appelé par le middleware avant toute décision de routage.
    pub async fn snapshot(&self) -> VaultSnapshot {
        if !self.db_path.exists() {
            return VaultSnapshot::Absent;
        }
        let mut session = self.session.lock().await;
        if let VaultSession::Unlocked {
            last_activity,
            suspended_until,
            ..
        } = &*session
            && idle_expired(
                *last_activity,
                Instant::now(),
                self.idle_timeout,
                *suspended_until,
            )
        {
            // L'inactivité referme la fenêtre en mémoire. Le trousseau n'est purgé que par `lock`.
            *session = VaultSession::Locked;
        }
        match &*session {
            VaultSession::Locked => VaultSnapshot::Locked {
                passphrase_change_pending: matches!(
                    Store::status(&self.db_path),
                    Ok(VaultStatus::Exists {
                        passphrase_change_pending: true,
                        ..
                    })
                ),
            },
            VaultSession::Unlocked { .. } => VaultSnapshot::Unlocked,
        }
    }

    /// Marque une activité humaine réelle, prolongeant la session jusqu'au prochain
    /// `idle_timeout`. Le middleware l'appelle pour toute route protégée *sauf*
    /// `/audit/recent` : son polling toutes les deux secondes n'est pas de l'activité humaine,
    /// sans quoi la session n'expirerait jamais.
    pub async fn touch(&self) {
        if let VaultSession::Unlocked { last_activity, .. } = &mut *self.session.lock().await {
            *last_activity = Instant::now();
        }
    }

    /// Exécute `f` avec une référence au `Store` s'il est déverrouillé, `None` sinon. Un
    /// accesseur par fermeture plutôt qu'un garde retourné : plus simple à raisonner sur la
    /// durée de vie du verrou tokio, et suffisant pour tous les usages de ce crate (rendu d'un
    /// écran, exécution d'une commande de console).
    pub async fn with_store<T>(&self, f: impl FnOnce(&Store) -> T) -> Option<T> {
        match &*self.session.lock().await {
            VaultSession::Unlocked { store, .. } => Some(f(store)),
            VaultSession::Locked => None,
        }
    }

    /// Variante mutable de [`Self::with_store`] — pour la console, qui exécute des commandes
    /// d'écriture contre le coffre déjà ouvert.
    pub async fn with_store_mut<T>(&self, f: impl FnOnce(&mut Store) -> T) -> Option<T> {
        match &mut *self.session.lock().await {
            VaultSession::Unlocked { store, .. } => Some(f(store)),
            VaultSession::Locked => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{idle_expired, parse_today_opt, suspension_duration, suspension_if_remembered};
    use time::{Date, Month, OffsetDateTime};

    #[test]
    fn idle_expired_follows_the_timeout_without_a_suspension() {
        let start = Instant::now();
        let timeout = Duration::from_secs(15 * 60);
        let at_limit = start + timeout;
        assert!(!idle_expired(start, at_limit, timeout, None));
        assert!(idle_expired(
            start,
            at_limit + Duration::from_nanos(1),
            timeout,
            None
        ));
    }

    #[test]
    fn a_live_suspension_holds_the_window_past_the_idle_timeout() {
        let start = Instant::now();
        let timeout = Duration::from_secs(15 * 60);
        let until = start + Duration::from_secs(12 * 3600);
        let now = start + Duration::from_secs(3600);
        assert!(
            !idle_expired(start, now, timeout, Some(until)),
            "une suspension encore en cours tient la fenêtre, même après le quart d'heure"
        );
    }

    #[test]
    fn once_the_suspension_ends_an_abandoned_window_locks() {
        let start = Instant::now();
        let timeout = Duration::from_secs(15 * 60);
        let until = start + Duration::from_secs(12 * 3600);
        assert!(idle_expired(start, until, timeout, Some(until)));
        assert!(idle_expired(
            start,
            until + Duration::from_secs(1),
            timeout,
            Some(until)
        ));
    }

    #[test]
    fn once_the_suspension_ends_a_recent_navigation_keeps_the_window() {
        let start = Instant::now();
        let timeout = Duration::from_secs(15 * 60);
        let until = start + Duration::from_secs(12 * 3600);
        let last = until - Duration::from_secs(5 * 60);
        let now = until + Duration::from_secs(5 * 60);
        assert!(!idle_expired(last, now, timeout, Some(until)));
        let later = last + timeout + Duration::from_secs(1);
        assert!(idle_expired(last, later, timeout, Some(until)));
    }

    #[test]
    fn suspension_duration_is_the_time_left_on_the_keyring() {
        let now = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        let expires = now + time::Duration::hours(10) + time::Duration::milliseconds(250);
        assert_eq!(
            suspension_duration(expires, now),
            Some(Duration::from_millis(10 * 3_600 * 1_000 + 250))
        );
    }

    #[test]
    fn suspension_duration_is_absent_once_the_keyring_time_is_over() {
        let now = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        assert_eq!(suspension_duration(now, now), None);
        assert_eq!(
            suspension_duration(now - time::Duration::seconds(1), now),
            None
        );
    }

    #[test]
    fn a_successful_remember_suspends_for_the_ttl() {
        let now = Instant::now();
        let ttl = Duration::from_secs(12 * 3600);
        assert_eq!(suspension_if_remembered(now, ttl, true), Some(now + ttl));
    }

    #[test]
    fn a_failed_or_unchecked_remember_does_not_suspend() {
        let now = Instant::now();
        let ttl = Duration::from_secs(12 * 3600);
        assert_eq!(suspension_if_remembered(now, ttl, false), None);
    }

    #[test]
    fn parse_today_opt_reads_iso_date() {
        let expected = Date::from_calendar_date(2026, Month::September, 5).unwrap();
        assert_eq!(parse_today_opt(Some("2026-09-05")).unwrap(), Some(expected));
    }

    #[test]
    fn parse_today_opt_treats_blank_as_clock() {
        assert_eq!(parse_today_opt(Some("")).unwrap(), None);
        assert_eq!(parse_today_opt(Some("   ")).unwrap(), None);
        assert_eq!(parse_today_opt(None).unwrap(), None);
    }

    #[test]
    fn parse_today_opt_rejects_garbage() {
        let err = parse_today_opt(Some("hier")).unwrap_err();
        assert!(
            err.contains("GRIFFE_TODAY") && err.contains("hier"),
            "{err}"
        );
    }
}
