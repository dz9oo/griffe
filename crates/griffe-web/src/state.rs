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
use griffe_core::store::{AUTO_BACKUP_MAX_AGE, Passphrase, Store, StoreError, VaultStatus};
use tokio::sync::Mutex;

/// Durée par défaut d'une session mise en cache dans le trousseau OS quand l'utilisateur coche
/// « se souvenir de moi » — même valeur par défaut que `freeflow unlock --remember` en CLI.
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

enum VaultSession {
    Locked,
    // `Store` boxé : sinon `VaultSession` hériterait de sa taille (la connexion SQLite en fait
    // partie) même dans la variante `Locked`, qui n'en a pourtant pas besoin.
    Unlocked {
        store: Box<Store>,
        last_activity: Instant,
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
    pub async fn try_open_cached(&self) {
        if let Ok(store) = Store::open_cached(&self.db_path) {
            maybe_auto_backup(&store, &self.db_path);
            *self.session.lock().await = VaultSession::Unlocked {
                store: Box::new(store),
                last_activity: Instant::now(),
            };
        }
    }

    /// Déverrouille avec `passphrase`. `remember` met la clé en cache dans le trousseau OS pour
    /// les prochains lancements de l'application (pas seulement le reste de cette session
    /// fenêtre, déjà couverte tant que la fenêtre reste ouverte).
    ///
    /// # Errors
    pub async fn unlock(&self, passphrase: &Passphrase, remember: bool) -> Result<(), StoreError> {
        let store = Store::open_with_passphrase(&self.db_path, passphrase)?;
        if remember {
            // Best-effort : la fenêtre reste utilisable même si le trousseau OS est
            // indisponible, seule la mise en cache pour la prochaine ouverture est perdue.
            let _ = store.remember(DEFAULT_SESSION_TTL);
        }
        maybe_auto_backup(&store, &self.db_path);
        // Lot 39 : les justificatifs en clair d'avant sont chiffrés au premier déverrouillage
        // (idempotent, silencieux — une pièce qui résiste sera reprise la prochaine fois).
        let _ = griffe_core::receipts::migrate_legacy(&store);
        *self.session.lock().await = VaultSession::Unlocked {
            store: Box::new(store),
            last_activity: Instant::now(),
        };
        Ok(())
    }

    /// Crée le coffre puis le déverrouille immédiatement.
    ///
    /// # Errors
    pub async fn create(&self, passphrase: &Passphrase, remember: bool) -> Result<(), StoreError> {
        let store = Store::create(&self.db_path, passphrase)?;
        if remember {
            let _ = store.remember(DEFAULT_SESSION_TTL);
        }
        *self.session.lock().await = VaultSession::Unlocked {
            store: Box::new(store),
            last_activity: Instant::now(),
        };
        Ok(())
    }

    /// Ferme la connexion et purge la session du trousseau OS — pour que « verrouiller » veuille
    /// dire une seule chose, que ce soit le bouton de la fenêtre ou `freeflow lock` tapé dans la
    /// console intégrée.
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
        if let VaultSession::Unlocked { last_activity, .. } = &*session
            && last_activity.elapsed() > self.idle_timeout
        {
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
