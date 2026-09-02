//! Parseur de commandes exposé en bibliothèque : la console de la GUI (lot 9) et le binaire
//! `freeflow` (ce crate) empruntent exactement le même chemin de code — [`run`] imprime sur
//! stdout/stderr et renvoie un code de sortie, exactement ce qu'une console capture déjà.

mod backup;
mod client;
mod company;
mod error;
mod expense;
mod fec;
mod fiscal;
mod forecast;
mod invoice;
mod mission;
mod output;
mod parsers;
mod pending;
mod prospect;
mod quote;
mod refs;
mod table;
mod vault;
mod year;

use std::path::{Path, PathBuf};

pub use freeflow_core::clock::today_local as today;

use clap::{Parser, Subcommand};
use freeflow_core::app::{Actor, ExecutionContext, PendingActionId};
use freeflow_core::store::Store;

use error::CliError;
use vault::PassphraseOpts;

pub use expense::{ArchivedReceipt, archive_receipt_bytes};

#[derive(Parser)]
#[command(
    name = "freeflow",
    version,
    about = "Gestion pour indépendant — pilotable en CLI et par un agent LLM (MCP)."
)]
struct Cli {
    /// Chemin du coffre chiffré. Sinon, lu depuis `FREEFLOW_DB`.
    #[arg(long, global = true)]
    db: Option<PathBuf>,
    /// Imprime le résultat en JSON plutôt qu'en texte lisible — le contrat que consomme un agent.
    #[arg(long, global = true)]
    json: bool,
    /// Qui déclenche la commande : `human` (défaut), `system`, ou `agent:<session>`.
    #[arg(long, global = true, default_value = "human", value_parser = parsers::parse_actor)]
    actor: Actor,
    /// N'écrit rien : montre ce qui serait fait.
    #[arg(long, global = true)]
    dry_run: bool,
    /// Désactive la coloration (déjà minimale) de la sortie humaine.
    #[arg(long, global = true)]
    no_color: bool,
    #[command(flatten)]
    passphrase: PassphraseOpts,
    #[command(subcommand)]
    command: TopCommand,
}

#[derive(Subcommand)]
enum TopCommand {
    /// Crée le coffre. Échoue s'il existe déjà — double saisie masquée en mode interactif.
    Init,
    /// Vérifie la passphrase et, avec `--remember`, met la clé en cache dans le trousseau OS.
    /// Ne crée jamais le coffre (utilisez `freeflow init`).
    Unlock,
    /// Verrouille le coffre : purge la session du trousseau OS.
    Lock,
    /// État du coffre (existence, version du sidecar, session en cache), sans le déverrouiller.
    #[command(subcommand)]
    Vault(VaultCommand),
    /// Passphrase du coffre : changement en place, ré-chiffrement complet, sauvegarde préalable
    /// obligatoire.
    #[command(subcommand)]
    Passphrase(vault::PassphraseCommand),
    /// Clients.
    #[command(subcommand)]
    Client(client::ClientCommand),
    /// Identité légale de l'émetteur (mentions obligatoires des factures).
    #[command(subcommand)]
    Company(company::CompanyCommand),
    /// Prospection : opportunités, pipeline, interactions.
    #[command(subcommand)]
    Prospect(prospect::ProspectCommand),
    /// Missions : création, saisie de temps, TJM effectif, capacité.
    #[command(subcommand)]
    Mission(mission::MissionCommand),
    /// Devis : création, révision, envoi, acceptation.
    #[command(subcommand)]
    Quote(quote::QuoteCommand),
    /// Facturation : émission, avoir, vérification de la chaîne, balance âgée.
    #[command(subcommand)]
    Invoice(invoice::InvoiceCommand),
    /// Encaissements.
    #[command(subcommand)]
    Payment(invoice::PaymentCommand),
    /// Relevés bancaires : import et rapprochement.
    #[command(subcommand)]
    Bank(invoice::BankCommand),
    /// Actions en attente de confirmation humaine (proposées par un agent).
    #[command(subcommand)]
    Pending(pending::PendingCommand),
    /// Journal d'audit.
    #[command(subcommand)]
    Audit(pending::AuditCommand),
    /// Confirme une action en attente : retrouve son type de commande et l'applique.
    Confirm {
        /// Identifiant de l'action (voir `freeflow pending list`).
        #[arg(value_name = "ID", value_parser = clap::value_parser!(PendingActionId), required_unless_present = "id_flag")]
        id: Option<PendingActionId>,
        /// Ancienne forme `--id <ID>` (lot 36 : conservée un lot comme alias, puis retirée).
        #[arg(long = "id", hide = true, value_parser = clap::value_parser!(PendingActionId), conflicts_with = "id")]
        id_flag: Option<PendingActionId>,
    },
    /// Dépenses professionnelles et TVA déductible.
    #[command(subcommand)]
    Expense(expense::ExpenseCommand),
    /// Échéances fiscales indicatives (CA3, IS, CFE).
    #[command(subcommand)]
    Fiscal(fiscal::FiscalCommand),
    /// Prévisionnel de trésorerie sur 12 mois.
    #[command(subcommand)]
    Forecast(forecast::ForecastCommand),
    /// Clôture d'exercice : snapshot du résultat, affectation, approbation, documents.
    #[command(subcommand)]
    Year(year::YearCommand),
    /// Fichier des Écritures Comptables (FEC) d'un exercice, pour l'expert-comptable.
    #[command(subcommand)]
    Fec(fec::FecCommand),
    /// Sauvegardes : une automatique et silencieuse tourne déjà à chaque commande ; ces
    /// sous-commandes ne servent qu'à en forcer une, ou à restaurer.
    #[command(subcommand)]
    Backup(backup::BackupCommand),
}

#[derive(Subcommand)]
enum VaultCommand {
    /// Chemin résolu, existence du coffre, version du sidecar, session en cache.
    Status,
}

/// Durée en-dessous de laquelle une sauvegarde existante est considérée assez fraîche pour que
/// [`dispatch`] n'en écrive pas une nouvelle.
const AUTO_BACKUP_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 3600);

/// Comment [`dispatch`] obtient le `Store` sur lequel exécuter la commande.
pub enum VaultAccess<'a> {
    /// Process CLI ordinaire : session déjà en cache en priorité, sinon sources de passphrase
    /// déclarées puis invite TTY masquée en dernier recours. C'est le *seul* chemin qui peut
    /// prompter — voir [`vault::open_or_prompt`].
    Process,
    /// Coffre déjà ouvert et détenu par l'appelant — la console de la fenêtre. Aucune invite
    /// possible, aucune autre source de passphrase lue : `vault::open_or_prompt` n'est jamais
    /// appelée sur ce chemin, donc le prompt TTY y est inatteignable par construction, pas
    /// seulement par convention.
    Borrowed {
        store: &'a mut Store,
        db_path: &'a Path,
    },
}

/// Analyse `args` et exécute la commande correspondante — imprime sur stdout/stderr, renvoie
/// un code de sortie normalisé (voir [`error::CliError::exit_code`]). C'est le point d'entrée
/// du binaire `freeflow`.
pub fn run<I, T>(args: I) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(e) => {
            let _ = e.print();
            return e.exit_code();
        }
    };

    match dispatch(cli, VaultAccess::Process) {
        Ok(output) => {
            println!("{output}");
            0
        }
        Err(err) => {
            eprintln!("✗ {err}");
            err.exit_code()
        }
    }
}

/// Analyse `args` et exécute la commande correspondante *sans rien imprimer*, contre `access` —
/// la variante générale qu'emprunte [`run_capturing`], et celle qu'utilise la console de la GUI
/// pour agir sur son coffre déjà ouvert plutôt que d'en rouvrir un second. La sortie (succès ou
/// message d'erreur, sous la même forme que verrait un terminal) est renvoyée en `String`,
/// accompagnée du code de sortie normalisé.
///
/// # Panics
///
/// Ne panique jamais : les erreurs de parsing `clap` sont converties en texte, pas en process
/// `exit` (contrairement à [`run`], qui est un vrai binaire et peut se le permettre).
#[must_use]
pub fn run_capturing_with_vault<I, T>(args: I, access: VaultAccess<'_>) -> (String, i32)
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(e) => return (e.render().to_string(), e.exit_code()),
    };

    match dispatch(cli, access) {
        Ok(output) => (output, 0),
        Err(err) => (format!("✗ {err}"), err.exit_code()),
    }
}

/// Équivalent à [`run_capturing_with_vault`] avec [`VaultAccess::Process`] — le chemin d'un
/// process CLI ordinaire, qui peut ouvrir son propre coffre (et prompter si besoin).
///
/// # Panics
///
/// Voir [`run_capturing_with_vault`].
#[must_use]
pub fn run_capturing<I, T>(args: I) -> (String, i32)
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    run_capturing_with_vault(args, VaultAccess::Process)
}

fn dispatch(cli: Cli, access: VaultAccess<'_>) -> Result<String, CliError> {
    let Cli {
        db,
        json,
        actor,
        dry_run,
        no_color: _,
        passphrase,
        command,
    } = cli;
    let requested_db_path = vault::resolve_db_path(db)?;

    match access {
        VaultAccess::Borrowed { store, db_path } => {
            // La session de la CLI (trousseau OS) et celle de la fenêtre (mémoire) ne sont pas
            // le même objet : la console ne peut agir que sur le coffre déjà ouvert par la
            // fenêtre, jamais en ouvrir ou verrouiller un autre.
            if requested_db_path != *db_path {
                return Err(CliError::Unexpected(
                    "--db ne peut pas pointer ailleurs que le coffre déjà ouvert par la fenêtre"
                        .to_string(),
                ));
            }
            match &command {
                TopCommand::Init | TopCommand::Unlock | TopCommand::Lock => {
                    return Err(CliError::Unexpected(
                        "utilisez le bouton de verrouillage de la fenêtre plutôt que cette commande"
                            .to_string(),
                    ));
                }
                TopCommand::Passphrase(_) => {
                    return Err(CliError::Unexpected(
                        "`passphrase change` ré-chiffre l'intégralité du coffre : lancez-la \
                         depuis un terminal, fenêtre fermée"
                            .to_string(),
                    ));
                }
                // Lecture seule, sans effet sur la session de la fenêtre : contrairement aux
                // trois commandes ci-dessus, rien n'empêche de la laisser passer.
                TopCommand::Vault(VaultCommand::Status) => return vault::status(db_path, json),
                TopCommand::Backup(backup::BackupCommand::Restore { .. }) => {
                    return Err(CliError::Unexpected(
                        "`backup restore` est un geste de reprise après sinistre : lancez-la depuis un terminal".to_string(),
                    ));
                }
                _ => {}
            }
            let ctx = ExecutionContext::new(actor, dry_run);
            run_command(command, store, &ctx, json)
        }
        VaultAccess::Process => {
            let db_path = requested_db_path;
            match &command {
                TopCommand::Init => return vault::init(&db_path, &passphrase),
                TopCommand::Unlock => return vault::unlock(&db_path, &passphrase),
                TopCommand::Lock => return vault::lock(&db_path),
                TopCommand::Vault(VaultCommand::Status) => return vault::status(&db_path, json),
                TopCommand::Passphrase(vault::PassphraseCommand::Change(args)) => {
                    return vault::change_passphrase(
                        &db_path,
                        &passphrase,
                        args,
                        actor,
                        dry_run,
                        json,
                    );
                }
                TopCommand::Backup(backup::BackupCommand::Restore { from, to }) => {
                    // Ne touche jamais le coffre par défaut : doit rester possible même si
                    // celui-là est justement celui qui est cassé, donc traité ici plutôt qu'après
                    // son ouverture ci-dessous.
                    return backup::restore(from.clone(), to.clone(), &passphrase);
                }
                _ => {}
            }

            let mut store = vault::open_or_prompt(&db_path, &passphrase)?;
            if let Err(e) =
                store.auto_backup_if_stale(&db_path.with_file_name("backups"), AUTO_BACKUP_MAX_AGE)
            {
                eprintln!("⚠ sauvegarde automatique échouée : {e}");
            }
            let ctx = ExecutionContext::new(actor, dry_run);
            run_command(command, &mut store, &ctx, json)
        }
    }
}

fn run_command(
    command: TopCommand,
    store: &mut Store,
    ctx: &ExecutionContext,
    json: bool,
) -> Result<String, CliError> {
    match command {
        TopCommand::Init
        | TopCommand::Unlock
        | TopCommand::Lock
        | TopCommand::Vault(_)
        | TopCommand::Passphrase(_) => {
            unreachable!("traitées avant l'ouverture du coffre, dans dispatch")
        }
        TopCommand::Backup(cmd) => backup::run(cmd, store),
        TopCommand::Client(cmd) => client::run(cmd, store, ctx, json),
        TopCommand::Company(cmd) => company::run(cmd, store, ctx, json),
        TopCommand::Prospect(cmd) => prospect::run(cmd, store, ctx, json),
        TopCommand::Mission(cmd) => mission::run(cmd, store, ctx, json),
        TopCommand::Quote(cmd) => quote::run(cmd, store, ctx, json),
        TopCommand::Invoice(cmd) => invoice::run_invoice(cmd, store, ctx, json),
        TopCommand::Payment(cmd) => invoice::run_payment(cmd, store, ctx, json),
        TopCommand::Bank(cmd) => invoice::run_bank(cmd, store, ctx, json),
        TopCommand::Pending(cmd) => pending::run_pending(cmd, store, json),
        TopCommand::Audit(cmd) => pending::run_audit(cmd, store, json),
        TopCommand::Confirm { id, id_flag } => {
            let id = id
                .or(id_flag)
                .expect("clap exige l'un des deux (required_unless_present)");
            pending::confirm(store, id, json)
        }
        TopCommand::Expense(cmd) => expense::run(cmd, store, ctx, json),
        TopCommand::Fiscal(cmd) => fiscal::run(cmd, store, json),
        TopCommand::Forecast(cmd) => forecast::run(cmd, store, json),
        TopCommand::Year(cmd) => year::run(cmd, store, ctx, json),
        TopCommand::Fec(cmd) => fec::run(cmd, store, json),
    }
}
