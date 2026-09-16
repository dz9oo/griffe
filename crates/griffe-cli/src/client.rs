//! `freeflow client ...` — les références (`--id`, `<RÉFÉRENCE>`) acceptent un UUID complet, un
//! préfixe d'UUID d'au moins 4 caractères hexadécimaux (à la manière d'un hash git court), ou un
//! nom de client (insensible à la casse et aux accents, exact puis par préfixe) : voir
//! `griffe_core::reference`.

use clap::Subcommand;
use griffe_core::app::{ExecutionContext, Executor};
use griffe_core::clients::{
    self, ClientFilter, client_by_id, contact_by_id, list_clients_with, list_contacts,
};
use griffe_core::domain::{Address, ContactId, Siren, VatNumber};
use griffe_core::store::Store;

use crate::error::CliError;
use crate::output::{HumanRender, format_json, format_outcome, format_value, key_values, or_dash};

impl HumanRender for griffe_core::domain::Client {
    fn render_human(&self) -> String {
        key_values(&[
            ("Client", self.name.clone()),
            ("id", self.id.to_string()),
            ("SIREN", or_dash(self.siren.as_ref())),
            ("TVA intracom.", or_dash(self.vat_number.as_ref())),
            (
                "adresse",
                self.address.as_ref().map_or_else(
                    || "—".to_string(),
                    |a| format!("{}, {} {}, {}", a.street, a.postal_code, a.city, a.country),
                ),
            ),
            (
                "statut",
                if self.archived_at.is_some() {
                    "archivé"
                } else {
                    "actif"
                }
                .to_string(),
            ),
            ("révision", self.revision.to_string()),
        ])
    }
}
use crate::parsers::{parse_siren, parse_vat_number};
use crate::refs;

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
        #[arg(value_name = "RÉFÉRENCE")]
        reference: String,
    },
    /// Liste les clients actifs (ajoutez `--archived` pour inclure les archivés).
    List {
        #[arg(long)]
        archived: bool,
    },
    /// Modifie un client existant — seuls les champs fournis changent, le reste est conservé
    /// tel quel.
    ///
    /// `--clear-siren`/`--clear-vat-number`/`--clear-address` effacent un champ optionnel sans en
    /// fournir un nouveau.
    Edit {
        #[arg(value_name = "RÉFÉRENCE")]
        reference: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long, value_parser = parse_siren, conflicts_with = "clear_siren")]
        siren: Option<Siren>,
        /// Efface le SIREN, sans en fournir un nouveau.
        #[arg(long)]
        clear_siren: bool,
        #[arg(long, value_parser = parse_vat_number, conflicts_with = "clear_vat_number")]
        vat_number: Option<VatNumber>,
        /// Efface le numéro de TVA intracommunautaire, sans en fournir un nouveau.
        #[arg(long)]
        clear_vat_number: bool,
        #[arg(long, requires_all = ["postal_code", "city", "country"], conflicts_with = "clear_address")]
        street: Option<String>,
        #[arg(long, conflicts_with = "clear_address")]
        postal_code: Option<String>,
        #[arg(long, conflicts_with = "clear_address")]
        city: Option<String>,
        #[arg(long, conflicts_with = "clear_address")]
        country: Option<String>,
        /// Efface l'adresse postale, sans en fournir une nouvelle.
        #[arg(long)]
        clear_address: bool,
    },
    /// Retire un client des listes actives sans le supprimer — les factures et devis passés
    /// gardent une référence valide.
    Archive {
        #[arg(value_name = "RÉFÉRENCE")]
        reference: String,
    },
    /// Réintègre un client archivé dans les listes actives.
    Unarchive {
        #[arg(value_name = "RÉFÉRENCE")]
        reference: String,
    },
    /// Supprime un client pour de bon — refusé s'il est encore référencé par une opportunité,
    /// un devis, une mission ou une facture (archivez-le dans ce cas).
    Rm {
        #[arg(value_name = "RÉFÉRENCE")]
        reference: String,
    },
    /// Contacts d'un client.
    #[command(subcommand)]
    Contact(ContactCommand),
}

#[derive(Debug, Subcommand)]
pub enum ContactCommand {
    /// Ajoute un contact à un client.
    Add {
        #[arg(long, value_name = "RÉFÉRENCE")]
        client: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        email: Option<String>,
        #[arg(long)]
        phone: Option<String>,
        #[arg(long)]
        role: Option<String>,
    },
    /// Liste les contacts d'un client.
    List {
        #[arg(long, value_name = "RÉFÉRENCE")]
        client: String,
    },
    /// Modifie un contact existant — seuls les champs fournis changent.
    ///
    /// `--clear-email`/`--clear-phone`/`--clear-role` effacent un champ optionnel sans en fournir
    /// un nouveau.
    Edit {
        #[arg(value_parser = clap::value_parser!(ContactId))]
        id: ContactId,
        #[arg(long)]
        name: Option<String>,
        #[arg(long, conflicts_with = "clear_email")]
        email: Option<String>,
        /// Efface l'adresse e-mail, sans en fournir une nouvelle.
        #[arg(long)]
        clear_email: bool,
        #[arg(long, conflicts_with = "clear_phone")]
        phone: Option<String>,
        /// Efface le téléphone, sans en fournir un nouveau.
        #[arg(long)]
        clear_phone: bool,
        #[arg(long, conflicts_with = "clear_role")]
        role: Option<String>,
        /// Efface la fonction, sans en fournir une nouvelle.
        #[arg(long)]
        clear_role: bool,
    },
    /// Supprime un contact.
    Rm {
        #[arg(value_parser = clap::value_parser!(ContactId))]
        id: ContactId,
    },
}

fn client_or_not_found(
    store: &Store,
    id: griffe_core::domain::ClientId,
) -> Result<griffe_core::domain::Client, CliError> {
    client_by_id(store.connection(), id)?
        .ok_or_else(|| CliError::Domain(format!("client introuvable : {id}")))
}

fn contact_or_not_found(
    store: &Store,
    id: ContactId,
) -> Result<griffe_core::domain::Contact, CliError> {
    contact_by_id(store.connection(), id)?
        .ok_or_else(|| CliError::Domain(format!("contact introuvable : {id}")))
}

fn client_table(clients: &[griffe_core::domain::Client]) -> String {
    let rows = clients
        .iter()
        .map(|c| {
            vec![
                c.id.to_string(),
                c.name.clone(),
                c.siren.map_or_else(|| "—".to_string(), |s| s.to_string()),
                c.address
                    .as_ref()
                    .map_or_else(|| "—".to_string(), |a| a.city.clone()),
                if c.archived_at.is_some() {
                    "archivé".to_string()
                } else {
                    "actif".to_string()
                },
            ]
        })
        .collect::<Vec<_>>();
    crate::table::render(&["id", "nom", "siren", "ville", "statut"], &rows)
}

fn contact_table(contacts: &[griffe_core::domain::Contact]) -> String {
    let rows = contacts
        .iter()
        .map(|c| {
            vec![
                c.id.to_string(),
                c.name.clone(),
                c.role.clone().unwrap_or_else(|| "—".to_string()),
                c.email.clone().unwrap_or_else(|| "—".to_string()),
                c.phone.clone().unwrap_or_else(|| "—".to_string()),
            ]
        })
        .collect::<Vec<_>>();
    crate::table::render(&["id", "nom", "rôle", "email", "téléphone"], &rows)
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
        ClientCommand::Show { reference } => {
            let id = refs::resolve_client(store, &reference)?;
            let client = client_or_not_found(store, id)?;
            format_value(&client, json)
        }
        ClientCommand::List { archived } => {
            let filter = if archived {
                ClientFilter::All
            } else {
                ClientFilter::ActiveOnly
            };
            let clients = list_clients_with(store.connection(), filter)?;
            if json {
                format_json(&clients)
            } else {
                client_table(&clients)
            }
        }
        ClientCommand::Edit {
            reference,
            name,
            siren,
            clear_siren,
            vat_number,
            clear_vat_number,
            street,
            postal_code,
            city,
            country,
            clear_address,
        } => {
            let id = refs::resolve_client(store, &reference)?;
            let current = client_or_not_found(store, id)?;
            // Le patron `champ.or(current.champ)` ne sait jamais effacer un optionnel : chaque
            // `--clear-*` est l'échappatoire explicite (même trou comblé au lot 16 pour
            // `prospect edit --clear-source`/`--clear-note`).
            let address = match (street, postal_code, city, country) {
                _ if clear_address => None,
                (Some(street), Some(postal_code), Some(city), Some(country)) => Some(Address {
                    street,
                    postal_code,
                    city,
                    country,
                }),
                _ => current.address,
            };
            let command = clients::UpdateClient {
                id,
                revision: current.revision,
                name: name.unwrap_or(current.name),
                siren: if clear_siren {
                    None
                } else {
                    siren.or(current.siren)
                },
                vat_number: if clear_vat_number {
                    None
                } else {
                    vat_number.or(current.vat_number)
                },
                address,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        ClientCommand::Archive { reference } => {
            let id = refs::resolve_client(store, &reference)?;
            let current = client_or_not_found(store, id)?;
            let command = clients::ArchiveClient {
                id,
                revision: current.revision,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        ClientCommand::Unarchive { reference } => {
            let id = refs::resolve_client(store, &reference)?;
            let current = client_or_not_found(store, id)?;
            let command = clients::UnarchiveClient {
                id,
                revision: current.revision,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        ClientCommand::Rm { reference } => {
            let id = refs::resolve_client(store, &reference)?;
            let current = client_or_not_found(store, id)?;
            let command = clients::DeleteClient {
                id,
                revision: current.revision,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        ClientCommand::Contact(cmd) => run_contact(cmd, store, ctx, json)?,
    };
    Ok(output)
}

fn run_contact(
    cmd: ContactCommand,
    store: &mut Store,
    ctx: &ExecutionContext,
    json: bool,
) -> Result<String, CliError> {
    let output = match cmd {
        ContactCommand::Add {
            client,
            name,
            email,
            phone,
            role,
        } => {
            let client_id = refs::resolve_client(store, &client)?;
            let command = clients::CreateContact {
                client_id,
                name,
                email,
                phone,
                role,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        ContactCommand::List { client } => {
            let client_id = refs::resolve_client(store, &client)?;
            let contacts = list_contacts(store.connection(), client_id)?;
            if json {
                format_json(&contacts)
            } else {
                contact_table(&contacts)
            }
        }
        ContactCommand::Edit {
            id,
            name,
            email,
            clear_email,
            phone,
            clear_phone,
            role,
            clear_role,
        } => {
            let current = contact_or_not_found(store, id)?;
            let keep_unless_cleared =
                |clear: bool, new: Option<String>, current: Option<String>| {
                    if clear { None } else { new.or(current) }
                };
            let command = clients::UpdateContact {
                id,
                revision: current.revision,
                name: name.unwrap_or(current.name),
                email: keep_unless_cleared(clear_email, email, current.email),
                phone: keep_unless_cleared(clear_phone, phone, current.phone),
                role: keep_unless_cleared(clear_role, role, current.role),
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
        ContactCommand::Rm { id } => {
            let current = contact_or_not_found(store, id)?;
            let command = clients::DeleteContact {
                id,
                revision: current.revision,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome(&outcome, json)
        }
    };
    Ok(output)
}
