//! `freeflow company ...`

use clap::{Args, Subcommand};
use freeflow_core::app::{ExecutionContext, Executor};
use freeflow_core::company;
use freeflow_core::domain::{Address, FiscalYearEnd, Money, Siren, VatNumber, VatRegime};
use freeflow_core::fiscal::company_profile_with_vat_filing;
use freeflow_core::store::Store;

use crate::error::CliError;
use crate::output::{format_outcome, format_value};
use crate::parsers::{parse_money, parse_siren, parse_vat_number};

/// Analyse une date de clôture récurrente au format `JJ/MM` (ex. `31/12`).
fn parse_fiscal_year_end(s: &str) -> Result<FiscalYearEnd, String> {
    let (day, month) = s
        .split_once('/')
        .ok_or_else(|| format!("format attendu JJ/MM (ex. 31/12), reçu : {s}"))?;
    let day: u8 = day
        .trim()
        .parse()
        .map_err(|_| format!("jour invalide : {day}"))?;
    let month: u8 = month
        .trim()
        .parse()
        .map_err(|_| format!("mois invalide : {month}"))?;
    FiscalYearEnd::new(month, day).map_err(|e| e.to_string())
}

/// Analyse un régime de TVA (`real_normal_monthly`, `real_normal_quarterly`, `real_simplified`,
/// `franchise`).
fn parse_vat_regime(s: &str) -> Result<VatRegime, String> {
    s.parse().map_err(|_| {
        format!(
            "régime inconnu : {s} (attendu : real_normal_monthly, real_normal_quarterly, \
             real_simplified, franchise)"
        )
    })
}

/// Analyse un ratio de charges exprimé en pourcentage (ex. `80` ou `80.5`) vers des dix-millièmes.
fn parse_charge_ratio_bps(s: &str) -> Result<u32, String> {
    let percent: f64 = s
        .trim()
        .replace(',', ".")
        .parse()
        .map_err(|_| format!("pourcentage invalide : {s}"))?;
    if !percent.is_finite() || percent < 0.0 || percent > 1000.0 {
        return Err(format!("pourcentage hors bornes (0..=1000) : {s}"));
    }
    // Arrondi au dix-millième le plus proche, sans dépendre d'un cast tronquant.
    Ok((percent * 100.0).round() as u32)
}

#[derive(Debug, Args)]
pub struct SetProfileArgs {
    #[arg(long)]
    name: String,
    /// Ex. `SASU`, `EURL`.
    #[arg(long)]
    legal_form: String,
    #[arg(long, value_parser = parse_siren)]
    siren: Siren,
    #[arg(long, value_parser = parse_vat_number)]
    vat_number: Option<VatNumber>,
    #[arg(long)]
    street: String,
    #[arg(long)]
    postal_code: String,
    #[arg(long)]
    city: String,
    #[arg(long)]
    country: String,
    #[arg(long, value_parser = parse_money)]
    share_capital: Option<Money>,
    /// Ville du greffe d'immatriculation, pour la mention RCS.
    #[arg(long)]
    rcs_city: Option<String>,
    #[arg(long)]
    iban: Option<String>,
    /// Date de clôture d'exercice récurrente, au format `JJ/MM` (ex. `31/12`) — socle du
    /// calendrier fiscal.
    #[arg(long, value_parser = parse_fiscal_year_end)]
    fiscal_year_end: Option<FiscalYearEnd>,
    /// Régime de TVA : `real_normal_monthly`, `real_normal_quarterly`, `real_simplified`,
    /// `franchise`.
    #[arg(long, value_parser = parse_vat_regime)]
    vat_regime: Option<VatRegime>,
    /// Rémunération mensuelle brute du président (assimilé salarié). Absent = non rémunéré.
    #[arg(long, value_parser = parse_money)]
    director_gross: Option<Money>,
    /// Ratio charges/net du dirigeant, en pourcentage (ex. `80`), pour estimer les cotisations.
    #[arg(long, value_parser = parse_charge_ratio_bps)]
    director_charge_ratio: Option<u32>,
}

#[derive(Debug, Subcommand)]
pub enum CompanyCommand {
    /// Définit (ou remplace) l'identité légale de l'émetteur — nécessaire aux mentions
    /// obligatoires d'une facture (`freeflow invoice render`).
    SetProfile(Box<SetProfileArgs>),
    /// Affiche l'identité légale actuellement configurée, et la règle de télédéclaration de TVA
    /// qui en découle (`vat_filing` : schéma déclaratif, jour de la grille officielle).
    Show,
}

pub fn run(
    cmd: CompanyCommand,
    store: &mut Store,
    ctx: &ExecutionContext,
    json: bool,
) -> Result<String, CliError> {
    let output = match cmd {
        CompanyCommand::SetProfile(args) => {
            let command = company::SetCompanyProfile {
                name: args.name,
                legal_form: args.legal_form,
                siren: args.siren,
                vat_number: args.vat_number,
                address: Address {
                    street: args.street,
                    postal_code: args.postal_code,
                    city: args.city,
                    country: args.country,
                },
                share_capital: args.share_capital,
                rcs_city: args.rcs_city,
                iban: args.iban,
                fiscal_year_end: args.fiscal_year_end,
                vat_regime: args.vat_regime,
                director_monthly_gross: args.director_gross,
                director_charge_ratio_bps: args.director_charge_ratio,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        CompanyCommand::Show => {
            let today = time::OffsetDateTime::now_utc().date();
            let profile = company_profile_with_vat_filing(store.connection(), today)?;
            format_value(&profile, json)
        }
    };
    Ok(output)
}
