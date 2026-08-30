//! Parseur de commandes exposé en bibliothèque : la console de la GUI (lot 9) et le binaire
//! `freeflow` (ce crate) empruntent exactement le même chemin de code — [`run`] imprime sur
//! stdout/stderr et renvoie un code de sortie, exactement ce qu'une console capture déjà.

mod backup;
mod client;
mod company;
mod context;
mod error;
mod expense;
mod fiscal;
mod forecast;
mod invoice;
mod mission;
mod output;
mod parsers;
mod pending;
mod prospect;
mod quote;

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use freeflow_core::app::{Actor, ExecutionContext, PendingActionId};
use freeflow_core::store::Store;

use error::CliError;

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
    #[command(subcommand)]
    command: TopCommand,
}

#[derive(Subcommand)]
enum TopCommand {
    /// Déverrouille (ou crée) le coffre à partir de `FREEFLOW_PASSPHRASE`, et met la clé en
    /// cache dans le trousseau OS pour les appels suivants.
    Unlock,
    /// Verrouille le coffre : purge la clé du trousseau OS.
    Lock,
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
        #[arg(long, value_parser = clap::value_parser!(PendingActionId))]
        id: PendingActionId,
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
    /// Sauvegardes : une automatique et silencieuse tourne déjà à chaque commande ; ces
    /// sous-commandes ne servent qu'à en forcer une, ou à restaurer.
    #[command(subcommand)]
    Backup(backup::BackupCommand),
}

/// Durée en-dessous de laquelle une sauvegarde existante est considérée assez fraîche pour que
/// [`dispatch`] n'en écrive pas une nouvelle.
const AUTO_BACKUP_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 3600);

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

    match dispatch(cli) {
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

/// Analyse `args` et exécute la commande correspondante *sans rien imprimer* : la sortie
/// (succès ou message d'erreur, sous la même forme que verrait un terminal) est renvoyée en
/// `String`, accompagnée du code de sortie normalisé. C'est le point d'entrée qu'emprunte la
/// console de la GUI (lot 9) — elle tape verbatim la même commande qu'un terminal, mais
/// l'affiche dans son propre journal au lieu du stdout du process serveur.
///
/// # Panics
///
/// Ne panique jamais : les erreurs de parsing `clap` sont converties en texte, pas en process
/// `exit` (contrairement à [`run`], qui est un vrai binaire et peut se le permettre).
#[must_use]
pub fn run_capturing<I, T>(args: I) -> (String, i32)
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(e) => return (e.render().to_string(), e.exit_code()),
    };

    match dispatch(cli) {
        Ok(output) => (output, 0),
        Err(err) => (format!("✗ {err}"), err.exit_code()),
    }
}

fn dispatch(cli: Cli) -> Result<String, CliError> {
    let Cli {
        db,
        json,
        actor,
        dry_run,
        no_color: _,
        command,
    } = cli;
    let db_path = context::resolve_db_path(db)?;

    if matches!(command, TopCommand::Unlock) {
        let passphrase = std::env::var("FREEFLOW_PASSPHRASE").map_err(|_| {
            CliError::Unexpected("définissez FREEFLOW_PASSPHRASE pour déverrouiller".to_string())
        })?;
        Store::open_with_passphrase(&db_path, &passphrase)?;
        return Ok(format!("✓ coffre déverrouillé : {}", db_path.display()));
    }
    if matches!(command, TopCommand::Lock) {
        let _ = Store::lock(&db_path);
        return Ok("✓ coffre verrouillé".to_string());
    }
    if let TopCommand::Backup(backup::BackupCommand::Restore { from, to }) = &command {
        // Ne touche jamais le coffre par défaut : doit rester possible même si celui-là est
        // justement celui qui est cassé, donc traité ici plutôt qu'après son ouverture ci-dessous.
        return backup::restore(from.clone(), to.clone());
    }

    let mut store = context::open_store(&db_path)?;
    if let Err(e) =
        store.auto_backup_if_stale(&db_path.with_file_name("backups"), AUTO_BACKUP_MAX_AGE)
    {
        eprintln!("⚠ sauvegarde automatique échouée : {e}");
    }
    let ctx = ExecutionContext::new(actor, dry_run);

    match command {
        TopCommand::Unlock | TopCommand::Lock => unreachable!("traités ci-dessus"),
        TopCommand::Backup(cmd) => backup::run(cmd, &store),
        TopCommand::Client(cmd) => client::run(cmd, &mut store, &ctx, json),
        TopCommand::Company(cmd) => company::run(cmd, &mut store, &ctx, json),
        TopCommand::Prospect(cmd) => prospect::run(cmd, &mut store, &ctx, json),
        TopCommand::Mission(cmd) => mission::run(cmd, &mut store, &ctx, json),
        TopCommand::Quote(cmd) => quote::run(cmd, &mut store, &ctx, json),
        TopCommand::Invoice(cmd) => invoice::run_invoice(cmd, &mut store, &ctx, json),
        TopCommand::Payment(cmd) => invoice::run_payment(cmd, &mut store, &ctx, json),
        TopCommand::Bank(cmd) => invoice::run_bank(cmd, &mut store, &ctx, json),
        TopCommand::Pending(cmd) => pending::run_pending(cmd, &mut store, json),
        TopCommand::Audit(cmd) => pending::run_audit(cmd, &mut store, json),
        TopCommand::Confirm { id } => pending::confirm(&mut store, id, json),
        TopCommand::Expense(cmd) => expense::run(cmd, &mut store, &ctx, json),
        TopCommand::Fiscal(cmd) => fiscal::run(cmd, json),
        TopCommand::Forecast(cmd) => forecast::run(cmd, &store, json),
    }
}
