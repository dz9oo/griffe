//! `freeflow client ...`

use clap::Subcommand;
use freeflow_core::app::{ExecutionContext, Executor};
use freeflow_core::clients::{self, client_by_id, list_clients};
use freeflow_core::domain::{Address, ClientId, Siren, VatNumber};
use freeflow_core::store::Store;

use crate::error::CliError;
use crate::output::{format_outcome, format_value};
use crate::parsers::{parse_siren, parse_vat_number};

#[derive(Debug, Subcommand)]
pub enum ClientCommand {
    /// Crée un client.
    Create {
        #[arg(long)]
        name: String,
        #[arg(long, value_parser = parse_siren)]
        siren: Option<Siren>,
        #[arg(long, value_parser = parse_vat_number)]
        vat_number: Option<VatNumber>,
        #[arg(long, requires_all = ["postal_code", "city", "country"])]
        street: Option<String>,
        #[arg(long)]
        postal_code: Option<String>,
        #[arg(long)]
        city: Option<String>,
        /// Code pays ISO 3166-1 alpha-2, ex. `FR`.
        #[arg(long)]
        country: Option<String>,
    },
    /// Affiche un client.
    Show {
        #[arg(long, value_parser = clap::value_parser!(ClientId))]
        id: ClientId,
    },
    /// Liste tous les clients.
    List,
}

pub fn run(
    cmd: ClientCommand,
    store: &mut Store,
    ctx: &ExecutionContext,
    json: bool,
) -> Result<String, CliError> {
    let output = match cmd {
        ClientCommand::Create {
            name,
            siren,
            vat_number,
            street,
            postal_code,
            city,
            country,
        } => {
            let address = match (street, postal_code, city, country) {
                (Some(street), Some(postal_code), Some(city), Some(country)) => Some(Address {
                    street,
                    postal_code,
                    city,
                    country,
                }),
                _ => None,
            };
            let command = clients::CreateClient {
                name,
                siren,
                vat_number,
                address,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        ClientCommand::Show { id } => {
            let client = client_by_id(store.connection(), id)?;
            format_value(&client, json)
        }
        ClientCommand::List => {
            let clients = list_clients(store.connection())?;
            format_value(&clients, json)
        }
    };
    Ok(output)
}
