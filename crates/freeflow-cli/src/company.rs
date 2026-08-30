//! `freeflow company ...`

use clap::{Args, Subcommand};
use freeflow_core::app::{ExecutionContext, Executor};
use freeflow_core::company::{self, company_profile};
use freeflow_core::domain::{Address, Money, Siren, VatNumber};
use freeflow_core::store::Store;

use crate::error::CliError;
use crate::output::{format_outcome, format_value};
use crate::parsers::{parse_money, parse_siren, parse_vat_number};

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
}

#[derive(Debug, Subcommand)]
pub enum CompanyCommand {
    /// Définit (ou remplace) l'identité légale de l'émetteur — nécessaire aux mentions
    /// obligatoires d'une facture (`freeflow invoice render`).
    SetProfile(Box<SetProfileArgs>),
    /// Affiche l'identité légale actuellement configurée.
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
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        CompanyCommand::Show => {
            let profile = company_profile(store.connection())?;
            format_value(&profile, json)
        }
    };
    Ok(output)
}
