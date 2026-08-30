//! Clients — référencés par la prospection, les missions, la facturation et les devis, mais
//! dont la création n'appartenait à aucun de ces lots. Découvert en construisant le premier
//! parcours de bout en bout (lot 7) : sans ce module, aucune donnée réelle ne pouvait exister.

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::app::{AppError, Command};
use crate::domain::{Address, Client, ClientId, Siren, VatNumber};

fn conv_err(e: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateClient {
    pub name: String,
    pub siren: Option<Siren>,
    pub vat_number: Option<VatNumber>,
    pub address: Option<Address>,
}

impl Command for CreateClient {
    type Output = ClientId;
    const NAME: &'static str = "clients.create_client";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let client = Client {
            id: ClientId::new(),
            name: self.name.clone(),
            siren: self.siren,
            vat_number: self.vat_number.clone(),
            address: self.address.clone(),
            created_at: OffsetDateTime::now_utc(),
        };
        insert_client(conn, &client)?;
        Ok(client.id)
    }
}

fn insert_client(conn: &Connection, client: &Client) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO clients (id, name, siren, vat_number, address_street, address_postal_code, address_city, address_country, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            client.id.to_string(),
            client.name,
            client.siren.map(|s| s.to_string()),
            client.vat_number.as_ref().map(std::string::ToString::to_string),
            client.address.as_ref().map(|a| a.street.clone()),
            client.address.as_ref().map(|a| a.postal_code.clone()),
            client.address.as_ref().map(|a| a.city.clone()),
            client.address.as_ref().map(|a| a.country.clone()),
            client.created_at.format(&Rfc3339)?,
        ],
    )?;
    Ok(())
}

fn row_to_client(row: &Row) -> rusqlite::Result<Client> {
    let id: String = row.get("id")?;
    let siren: Option<String> = row.get("siren")?;
    let vat_number: Option<String> = row.get("vat_number")?;
    let street: Option<String> = row.get("address_street")?;
    let postal_code: Option<String> = row.get("address_postal_code")?;
    let city: Option<String> = row.get("address_city")?;
    let country: Option<String> = row.get("address_country")?;
    let created_at: String = row.get("created_at")?;

    let address = match (street, postal_code, city, country) {
        (Some(street), Some(postal_code), Some(city), Some(country)) => Some(Address {
            street,
            postal_code,
            city,
            country,
        }),
        _ => None,
    };

    Ok(Client {
        id: id.parse().map_err(conv_err)?,
        name: row.get("name")?,
        siren: siren
            .map(|s| Siren::parse(&s))
            .transpose()
            .map_err(conv_err)?,
        vat_number: vat_number
            .map(|s| VatNumber::parse(&s))
            .transpose()
            .map_err(conv_err)?,
        address,
        created_at: OffsetDateTime::parse(&created_at, &Rfc3339).map_err(conv_err)?,
    })
}

/// # Errors
pub fn client_by_id(conn: &Connection, id: ClientId) -> Result<Option<Client>, AppError> {
    conn.query_row(
        "SELECT * FROM clients WHERE id = ?1",
        [id.to_string()],
        row_to_client,
    )
    .optional()
    .map_err(AppError::from)
}

/// # Errors
pub fn list_clients(conn: &Connection) -> Result<Vec<Client>, AppError> {
    let mut stmt = conn.prepare("SELECT * FROM clients ORDER BY created_at ASC")?;
    let rows = stmt.query_map([], row_to_client)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Actor, ExecutionContext, Executor, Outcome};
    use crate::store::Store;

    fn test_store(label: &str) -> Store {
        let dir = std::env::temp_dir().join(format!(
            "freeflow-clients-test-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ));
        Store::open_with_passphrase(&dir.join("vault.db"), "s3cret").unwrap()
    }

    #[test]
    fn creating_a_client_makes_it_immediately_fetchable_and_listed() {
        let mut store = test_store("create-fetch");
        let cmd = CreateClient {
            name: "Argon Digital".to_string(),
            siren: None,
            vat_number: None,
            address: None,
        };
        let Outcome::Applied(id) = Executor::new(&mut store)
            .execute(&cmd, &ExecutionContext::new(Actor::Human, false))
            .unwrap()
        else {
            panic!("expected Applied")
        };

        let fetched = client_by_id(store.connection(), id).unwrap().unwrap();
        assert_eq!(fetched.name, "Argon Digital");

        let listed = list_clients(store.connection()).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, id);
    }
}
