//! Résolution du coffre : chemin par défaut, sources de passphrase non interactives, invite TTY
//! masquée en dernier recours. Le cœur (`griffe_core::store`) ne fait jamais d'IO interactive
//! (pas de prompt, pas de sous-process) : c'est ce module — l'adaptateur — qui acquiert la
//! passphrase avant de la remettre au cœur.
//!
//! Aucune variable d'environnement de passphrase n'existe : les seules sources sont un fichier
//! (`--passphrase-file`), une commande externe (`--passphrase-command`), l'entrée standard
//! (`--passphrase-stdin`), ou l'invite TTY masquée si un terminal est disponible.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::Subcommand;
use griffe_core::app::{Actor, ExecutionContext, Executor};
use griffe_core::store::{Passphrase, Store, StoreError, VaultStatus};
use griffe_core::vault::PassphraseChanged;

use crate::error::CliError;
use crate::parsers;

/// Durée par défaut d'une session mise en cache dans le trousseau OS, quand `--remember` est
/// demandé sans `--ttl` explicite.
pub const DEFAULT_SESSION_TTL: Duration = Duration::from_secs(12 * 3600);

#[derive(clap::Args, Debug, Clone, Default)]
pub struct PassphraseOpts {
    /// Lit la passphrase dans ce fichier (refusé si son mode est plus ouvert que 0600).
    #[arg(
        long,
        global = true,
        conflicts_with_all = ["passphrase_command", "passphrase_stdin"]
    )]
    pub passphrase_file: Option<PathBuf>,
    /// Exécute cette commande et prend sa sortie standard comme passphrase — ex.
    /// `pass show freeflow`, `op read op://Perso/freeflow/passphrase`.
    #[arg(
        long,
        global = true,
        conflicts_with_all = ["passphrase_file", "passphrase_stdin"]
    )]
    pub passphrase_command: Option<String>,
    /// Lit la passphrase sur l'entrée standard (une ligne).
    #[arg(
        long,
        global = true,
        conflicts_with_all = ["passphrase_file", "passphrase_command"]
    )]
    pub passphrase_stdin: bool,
    /// N'invite jamais au terminal : échoue proprement si aucune source non interactive n'a
    /// fourni de passphrase, plutôt que de bloquer en attente d'une frappe.
    #[arg(long, global = true)]
    pub non_interactive: bool,
    /// Met la clé en cache dans le trousseau OS pour les appels suivants (jamais automatique).
    #[arg(long, global = true)]
    pub remember: bool,
    /// Durée de la session mise en cache (défaut 12h). Implique `--remember`.
    #[arg(long, global = true, value_parser = parsers::parse_ttl)]
    pub ttl: Option<Duration>,
}

impl PassphraseOpts {
    fn wants_remember(&self) -> bool {
        self.remember || self.ttl.is_some()
    }
}

/// Chemin du coffre : `--db`, sinon `GRIFFE_DB` / `FREEFLOW_DB`, sinon l'emplacement XDG
/// (`~/.local/share/griffe/`), après migration d'un ancien `freeflow/` s'il est seul.
///
/// # Errors
pub fn resolve_db_path(explicit: Option<PathBuf>) -> Result<PathBuf, CliError> {
    if let Some(path) = explicit {
        return Ok(path);
    }
    if let Ok(from_env) = std::env::var("GRIFFE_DB").or_else(|_| std::env::var("FREEFLOW_DB")) {
        return Ok(PathBuf::from(from_env));
    }
    Store::resolve_default_vault_path().map_err(|e| {
        CliError::Unexpected(format!(
            "aucun coffre indiqué et pas d'emplacement par défaut disponible : {e} \
             (utilisez --db, GRIFFE_DB ou FREEFLOW_DB)"
        ))
    })
}

/// Ouvre le coffre à `db_path` : session déjà en cache en priorité, sinon acquisition d'une
/// passphrase (source déclarée, puis invite TTY) et déverrouillage. Met la session en cache si
/// `--remember`/`--ttl` a été demandé.
///
/// # Errors
pub fn open_or_prompt(db_path: &Path, opts: &PassphraseOpts) -> Result<Store, CliError> {
    match Store::open_cached(db_path) {
        Ok(store) => return Ok(store),
        Err(StoreError::Locked) => {}
        Err(e) => return Err(map_store_err(db_path, e)),
    }

    let passphrase = resolve_passphrase(opts, "Passphrase du coffre FreeFlow : ")?;
    let store =
        Store::open_with_passphrase(db_path, &passphrase).map_err(|e| map_store_err(db_path, e))?;
    if opts.wants_remember()
        && let Err(e) = store.remember(opts.ttl.unwrap_or(DEFAULT_SESSION_TTL))
    {
        eprintln!("⚠ {e}");
    }
    Ok(store)
}

fn map_store_err(db_path: &Path, err: StoreError) -> CliError {
    match err {
        StoreError::VaultNotFound(_) => CliError::NoVault(db_path.to_path_buf()),
        StoreError::VaultBusy => CliError::VaultBusy,
        other => other.into(),
    }
}

/// Résout une passphrase dans l'ordre : source déclarée en ligne de commande, puis invite TTY
/// masquée si un terminal est disponible et que `--non-interactive` n'a pas été demandé. Ne
/// consulte jamais le trousseau OS — c'est le rôle de [`open_or_prompt`] avant d'en arriver là.
///
/// # Errors
pub fn resolve_passphrase(opts: &PassphraseOpts, prompt: &str) -> Result<Passphrase, CliError> {
    if let Some(passphrase) = read_declared_source(opts)? {
        return Ok(passphrase);
    }
    if !opts.non_interactive && std::io::stderr().is_terminal() {
        return prompt_tty(prompt);
    }
    Err(CliError::Locked)
}

fn read_declared_source(opts: &PassphraseOpts) -> Result<Option<Passphrase>, CliError> {
    if let Some(path) = &opts.passphrase_file {
        return Ok(Some(read_from_file(path)?));
    }
    if let Some(cmd) = &opts.passphrase_command {
        return Ok(Some(read_from_command(cmd)?));
    }
    if opts.passphrase_stdin {
        return Ok(Some(read_from_stdin()?));
    }
    Ok(None)
}

fn read_from_file(path: &Path) -> Result<Passphrase, CliError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path)
            .map_err(|e| CliError::Unexpected(format!("lecture de {} : {e}", path.display())))?
            .permissions()
            .mode();
        if mode & 0o077 != 0 {
            return Err(CliError::Unexpected(format!(
                "le fichier de passphrase {} est lisible par d'autres (mode {:o}) : chmod 600 {}",
                path.display(),
                mode & 0o777,
                path.display()
            )));
        }
    }
    let raw = std::fs::read_to_string(path)
        .map_err(|e| CliError::Unexpected(format!("lecture de {} : {e}", path.display())))?;
    Ok(Passphrase::from(trim_one_newline(&raw)))
}

fn read_from_command(cmd: &str) -> Result<Passphrase, CliError> {
    let output = shell_command(cmd)
        .output()
        .map_err(|e| CliError::Unexpected(format!("exécution de --passphrase-command : {e}")))?;
    if !output.status.success() {
        return Err(CliError::Unexpected(format!(
            "--passphrase-command a échoué ({}) : {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let raw = String::from_utf8(output.stdout)
        .map_err(|_| CliError::Unexpected("--passphrase-command : sortie non UTF-8".to_string()))?;
    Ok(Passphrase::from(trim_one_newline(&raw)))
}

#[cfg(unix)]
fn shell_command(cmd: &str) -> std::process::Command {
    let mut command = std::process::Command::new("sh");
    command.arg("-c").arg(cmd);
    command
}

#[cfg(windows)]
fn shell_command(cmd: &str) -> std::process::Command {
    let mut command = std::process::Command::new("cmd");
    command.arg("/C").arg(cmd);
    command
}

fn read_from_stdin() -> Result<Passphrase, CliError> {
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| CliError::Unexpected(format!("lecture de --passphrase-stdin : {e}")))?;
    Ok(Passphrase::from(trim_one_newline(&line)))
}

fn prompt_tty(prompt: &str) -> Result<Passphrase, CliError> {
    let raw = rpassword::prompt_password(prompt)
        .map_err(|e| CliError::Unexpected(format!("lecture de la passphrase : {e}")))?;
    Ok(Passphrase::from(raw))
}

/// Retire un unique `\n` (ou `\r\n`) final — la convention d'un fichier ou d'une sortie de
/// commande écrit par un éditeur — sans toucher au reste du contenu.
fn trim_one_newline(raw: &str) -> &str {
    raw.strip_suffix('\n')
        .map_or(raw, |s| s.strip_suffix('\r').unwrap_or(s))
}

/// `freeflow init` : crée le coffre. Échoue s'il existe déjà — contrairement à l'ancien
/// `unlock`, qui créait silencieusement un coffre vide sur une faute de frappe de chemin.
///
/// # Errors
pub fn init(db_path: &Path, opts: &PassphraseOpts) -> Result<String, CliError> {
    let passphrase = match read_declared_source(opts)? {
        Some(p) => p,
        None if opts.non_interactive || !std::io::stderr().is_terminal() => {
            return Err(CliError::Locked);
        }
        None => {
            eprintln!(
                "⚠ Cette passphrase est la seule clé de vos données. Il n'existe aucune \
                 récupération : si vous la perdez, le coffre est définitivement illisible."
            );
            let first = prompt_tty("Nouvelle passphrase : ")?;
            let second = prompt_tty("Confirmez la passphrase : ")?;
            if first.expose() != second.expose() {
                return Err(CliError::Unexpected(
                    "les deux saisies ne correspondent pas".to_string(),
                ));
            }
            first
        }
    };
    if passphrase.is_empty() {
        return Err(CliError::Unexpected(
            "la passphrase ne peut pas être vide".to_string(),
        ));
    }

    let store = Store::create(db_path, &passphrase)?;
    if opts.wants_remember()
        && let Err(e) = store.remember(opts.ttl.unwrap_or(DEFAULT_SESSION_TTL))
    {
        eprintln!("⚠ {e}");
    }
    Ok(format!("✓ coffre créé : {}", db_path.display()))
}

/// `freeflow unlock` : vérifie la passphrase et, seulement si `--remember`/`--ttl` est demandé,
/// met la clé en cache dans le trousseau OS. Ne crée jamais le coffre.
///
/// # Errors
pub fn unlock(db_path: &Path, opts: &PassphraseOpts) -> Result<String, CliError> {
    if let Ok(store) = Store::open_cached(db_path) {
        if opts.wants_remember() {
            let ttl = opts.ttl.unwrap_or(DEFAULT_SESSION_TTL);
            if let Err(e) = store.remember(ttl) {
                eprintln!("⚠ {e}");
            }
        }
        return Ok(format!("✓ session déjà active pour {}", db_path.display()));
    }

    let passphrase = resolve_passphrase(opts, "Passphrase du coffre FreeFlow : ")?;
    let store =
        Store::open_with_passphrase(db_path, &passphrase).map_err(|e| map_store_err(db_path, e))?;

    if !opts.wants_remember() {
        return Ok(format!(
            "✓ passphrase correcte pour {} — relancez avec --remember pour garder une session active",
            db_path.display()
        ));
    }
    let ttl = opts.ttl.unwrap_or(DEFAULT_SESSION_TTL);
    match store.remember(ttl) {
        Ok(()) => Ok(format!(
            "✓ coffre déverrouillé : {} (session valide {} min)",
            db_path.display(),
            ttl.as_secs() / 60
        )),
        Err(e) => {
            eprintln!("⚠ {e}");
            Ok(format!(
                "✓ passphrase correcte pour {} (session non mise en cache)",
                db_path.display()
            ))
        }
    }
}

/// `freeflow lock` : purge la session en cache. Propage l'échec au lieu de le passer sous
/// silence — contrairement au comportement précédent, qui annonçait toujours un succès.
///
/// # Errors
pub fn lock(db_path: &Path) -> Result<String, CliError> {
    if Store::lock(db_path) {
        Ok("✓ coffre verrouillé".to_string())
    } else {
        Err(CliError::Unexpected(
            "échec de la purge de la session dans le trousseau du système".to_string(),
        ))
    }
}

/// `freeflow vault status` : chemin résolu, existence, version du sidecar, session en cache —
/// sans jamais déverrouiller le coffre.
///
/// # Errors
pub fn status(db_path: &Path, json: bool) -> Result<String, CliError> {
    let status = Store::status(db_path)?;
    let expires_at_rfc3339 = |status: &VaultStatus| match status {
        VaultStatus::Exists {
            session_expires_at: Some(t),
            ..
        } => t
            .format(&time::format_description::well_known::Rfc3339)
            .ok(),
        _ => None,
    };

    if json {
        let value = match &status {
            VaultStatus::Absent => serde_json::json!({"path": db_path, "exists": false}),
            VaultStatus::Exists {
                sidecar_version,
                passphrase_change_pending,
                ..
            } => serde_json::json!({
                "path": db_path,
                "exists": true,
                "sidecar_version": sidecar_version,
                "session_expires_at": expires_at_rfc3339(&status),
                "passphrase_change_pending": passphrase_change_pending,
            }),
        };
        return Ok(serde_json::to_string_pretty(&value).expect("Value se sérialise toujours"));
    }

    Ok(match &status {
        VaultStatus::Absent => format!(
            "aucun coffre à {} — lancez `freeflow init`",
            db_path.display()
        ),
        VaultStatus::Exists {
            sidecar_version,
            passphrase_change_pending,
            ..
        } => {
            let base = match expires_at_rfc3339(&status) {
                Some(expires) => format!(
                    "coffre {} (sidecar v{sidecar_version}) — session active jusqu'à {expires}",
                    db_path.display()
                ),
                None => format!(
                    "coffre {} (sidecar v{sidecar_version}) — verrouillé",
                    db_path.display()
                ),
            };
            if *passphrase_change_pending {
                format!(
                    "{base}\n⚠ un changement de passphrase a été interrompu : réessayez \
                     `freeflow unlock`/toute commande avec la NOUVELLE passphrase, ou restaurez \
                     la sauvegarde `pre-passphrase-change-*` écrite juste avant"
                )
            } else {
                base
            }
        }
    })
}

/// `freeflow passphrase change`.
#[derive(Debug, Subcommand)]
pub enum PassphraseCommand {
    /// Ré-chiffre l'intégralité du coffre sous une clé neuve. L'ancienne passphrase est exigée
    /// même si une session est déjà active. Écrit d'abord une sauvegarde obligatoire — qui,
    /// comme toute sauvegarde antérieure, reste chiffrée avec l'ANCIENNE passphrase.
    Change(ChangeArgs),
}

#[derive(Debug, Clone, clap::Args)]
pub struct ChangeArgs {
    /// Lit la NOUVELLE passphrase dans ce fichier (refusé si son mode est plus ouvert que 0600).
    #[arg(
        long,
        conflicts_with_all = ["new_passphrase_command", "new_passphrase_stdin"]
    )]
    pub new_passphrase_file: Option<PathBuf>,
    /// Exécute cette commande et prend sa sortie standard comme NOUVELLE passphrase.
    #[arg(
        long,
        conflicts_with_all = ["new_passphrase_file", "new_passphrase_stdin"]
    )]
    pub new_passphrase_command: Option<String>,
    /// Lit la NOUVELLE passphrase sur l'entrée standard. Avec `--passphrase-stdin`, l'ancienne
    /// est la première ligne lue et la nouvelle la seconde.
    #[arg(
        long,
        conflicts_with_all = ["new_passphrase_file", "new_passphrase_command"]
    )]
    pub new_passphrase_stdin: bool,
}

/// Vue jetable de [`ChangeArgs`] sous la forme d'un [`PassphraseOpts`], pour réutiliser
/// [`read_declared_source`] (permissions du fichier comprises) plutôt que de dupliquer sa
/// logique pour une seconde passphrase.
fn new_passphrase_opts(args: &ChangeArgs) -> PassphraseOpts {
    PassphraseOpts {
        passphrase_file: args.new_passphrase_file.clone(),
        passphrase_command: args.new_passphrase_command.clone(),
        passphrase_stdin: args.new_passphrase_stdin,
        non_interactive: false,
        remember: false,
        ttl: None,
    }
}

/// Acquiert la nouvelle passphrase : source déclarée en priorité (jamais confirmée par double
/// saisie, comme pour `init`), sinon double saisie masquée au terminal — jamais interactive si
/// `opts.non_interactive` ou si stderr n'est pas un terminal, exactement comme `init`.
fn resolve_new_passphrase(
    args: &ChangeArgs,
    opts: &PassphraseOpts,
) -> Result<Passphrase, CliError> {
    if let Some(passphrase) = read_declared_source(&new_passphrase_opts(args))? {
        return Ok(passphrase);
    }
    if opts.non_interactive || !std::io::stderr().is_terminal() {
        return Err(CliError::Locked);
    }
    eprintln!(
        "⚠ Le coffre entier va être ré-chiffré sous une clé neuve. Une sauvegarde est écrite \
         avant toute modification, mais elle — comme toutes les sauvegardes existantes — \
         restera chiffrée avec l'ANCIENNE passphrase : ne la détruisez pas."
    );
    let first = prompt_tty("Nouvelle passphrase : ")?;
    let second = prompt_tty("Confirmez la nouvelle passphrase : ")?;
    if first.expose() != second.expose() {
        return Err(CliError::Unexpected(
            "les deux saisies ne correspondent pas".to_string(),
        ));
    }
    Ok(first)
}

/// `freeflow passphrase change` : voir [`PassphraseCommand::Change`]. `--dry-run` acquiert et
/// valide les deux passphrases (l'ancienne vérifiée contre le coffre, la nouvelle non vide et
/// différente de l'ancienne) et affiche ce qui serait fait, sans rien écrire — ni sauvegarde, ni
/// sidecar en attente, ni entrée d'audit.
///
/// # Errors
pub fn change_passphrase(
    db_path: &Path,
    opts: &PassphraseOpts,
    args: &ChangeArgs,
    actor: Actor,
    dry_run: bool,
    json: bool,
) -> Result<String, CliError> {
    let current = resolve_passphrase(opts, "Passphrase actuelle du coffre : ")?;
    let new = resolve_new_passphrase(args, opts)?;
    if new.is_empty() {
        return Err(CliError::Unexpected(
            "la passphrase ne peut pas être vide".to_string(),
        ));
    }

    let mut store =
        Store::open_with_passphrase(db_path, &current).map_err(|e| map_store_err(db_path, e))?;
    let backups_dir = db_path.with_file_name("backups");

    if dry_run {
        // Le régime dépend du format du sidecar : un coffre v3 (clé maître enveloppée) ne
        // ré-chiffre rien — seule l'enveloppe est réécrite ; un coffre v1/v2 ré-chiffre tout et
        // migre vers v3 au passage.
        let sidecar_version = match Store::status(db_path).map_err(|e| map_store_err(db_path, e))? {
            griffe_core::store::VaultStatus::Exists {
                sidecar_version, ..
            } => sidecar_version,
            griffe_core::store::VaultStatus::Absent => {
                return Err(CliError::NoVault(db_path.to_path_buf()));
            }
        };
        let reencrypts_database = sidecar_version < 3;
        let bytes_to_reencrypt = if reencrypts_database {
            let page_count: i64 = store
                .connection()
                .query_row("PRAGMA page_count", [], |row| row.get(0))
                .map_err(|e| CliError::Unexpected(format!("lecture de PRAGMA page_count : {e}")))?;
            // `SQLCipher` répond à `PRAGMA page_size` par une colonne TEXTE nommée
            // `cipher_page_size` plutôt que l'entier habituel de SQLite (voir sa surcharge de la
            // pragma `page_size`/`cipher_page_size` dans son propre `pragma.c`) : lire une chaîne
            // puis la parser, pas un entier direct.
            let page_size: i64 = store
                .connection()
                .query_row("PRAGMA page_size", [], |row| row.get::<_, String>(0))
                .map_err(|e| CliError::Unexpected(format!("lecture de PRAGMA page_size : {e}")))?
                .parse()
                .map_err(|e| {
                    CliError::Unexpected(format!("PRAGMA page_size non numérique : {e}"))
                })?;
            page_count * page_size
        } else {
            0
        };
        if json {
            let value = serde_json::json!({
                "path": db_path,
                "dry_run": true,
                "changed": false,
                "reencrypts_database": reencrypts_database,
                "bytes_to_reencrypt": bytes_to_reencrypt,
            });
            return Ok(serde_json::to_string_pretty(&value).expect("Value se sérialise toujours"));
        }
        if reencrypts_database {
            return Ok(format!(
                "(dry-run) passphrase inchangée pour {} — {bytes_to_reencrypt} octets seraient \
                 ré-chiffrés (le coffre migrerait au format v3, clé maître enveloppée), une \
                 sauvegarde préalable serait écrite dans {}",
                db_path.display(),
                backups_dir.display()
            ));
        }
        return Ok(format!(
            "(dry-run) passphrase inchangée pour {} — la base ne serait pas ré-chiffrée, seule \
             l'enveloppe de la clé maître (sidecar .kdf) serait réécrite ; une sauvegarde \
             préalable serait écrite dans {}",
            db_path.display(),
            backups_dir.display()
        ));
    }

    let report = store
        .change_passphrase(&current, &new, &backups_dir)
        .map_err(|e| map_store_err(db_path, e))?;

    if opts.wants_remember()
        && let Err(e) = store.remember(opts.ttl.unwrap_or(DEFAULT_SESSION_TTL))
    {
        eprintln!("⚠ {e}");
    }

    let event = PassphraseChanged {
        backup_path: report.backup_path.display().to_string(),
        argon2_m_cost: report.argon2_m_cost,
        argon2_t_cost: report.argon2_t_cost,
        argon2_p_cost: report.argon2_p_cost,
        reencrypted: report.reencrypted,
    };
    let ctx = ExecutionContext::new(actor, false);
    if let Err(e) = Executor::new(&mut store).execute(&event, &ctx) {
        eprintln!("⚠ passphrase changée mais non consignée dans le journal d'audit : {e}");
    }

    if json {
        let value = serde_json::json!({
            "path": db_path,
            "dry_run": false,
            "changed": true,
            "backup": report.backup_path,
            "reencrypted": report.reencrypted,
            "argon2": {
                "m_cost": report.argon2_m_cost,
                "t_cost": report.argon2_t_cost,
                "p_cost": report.argon2_p_cost,
            },
        });
        return Ok(serde_json::to_string_pretty(&value).expect("Value se sérialise toujours"));
    }

    let regime = if report.reencrypted {
        "\n  le coffre est passé au format v3 (clé maître enveloppée) : les prochains \
         changements ne ré-chiffreront plus la base."
    } else {
        "\n  base non modifiée — seule l'enveloppe de la clé maître (sidecar) a été réécrite."
    };
    Ok(format!(
        "✓ passphrase changée : {}\n  sauvegarde préalable : {}{regime}\n  ⚠ cette sauvegarde et \
         toutes les précédentes s'ouvrent avec l'ANCIENNE passphrase.",
        db_path.display(),
        report.backup_path.display()
    ))
}
