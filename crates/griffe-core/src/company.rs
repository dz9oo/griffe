//! Identité légale de l'émetteur (SASU/EURL) — nécessaire aux mentions obligatoires d'une
//! facture (lot 10 : SIREN, TVA intra, forme sociale, capital social, immatriculation RCS).
//! Une seule ligne en base (`id = 1`) : ce n'est pas une entité métier qu'on liste, c'est un
//! profil qu'on configure une fois puis met à jour rarement.

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::app::{AppError, Command};
use crate::domain::{Address, FiscalYearEnd, Money, Siren, VatNumber, VatRegime};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompanyProfile {
    pub name: String,
    /// Ex. `"SASU"`, `"EURL"`.
    pub legal_form: String,
    pub siren: Siren,
    pub vat_number: Option<VatNumber>,
    pub address: Address,
    pub share_capital: Option<Money>,
    /// Ville du greffe d'immatriculation (ex. `"Paris"`), pour la mention RCS.
    pub rcs_city: Option<String>,
    pub iban: Option<String>,
    /// Date de clôture d'exercice récurrente (lot 17) — dont dérive tout le calendrier fiscal.
    /// `None` tant qu'elle n'est pas configurée : le calendrier retombe alors sur l'année civile.
    pub fiscal_year_end: Option<FiscalYearEnd>,
    /// Régime de TVA déclaré (pilote la périodicité CA3).
    pub vat_regime: Option<VatRegime>,
    /// Rémunération mensuelle brute du président (assimilé salarié). `None` = non rémunéré, donc
    /// aucune DSN ni cotisation sociale.
    pub director_monthly_gross: Option<Money>,
    /// Ratio charges/net paramétrable (dix-millièmes, ex. `8000` = 80 %) pour estimer, à titre
    /// indicatif, les cotisations sociales sur la rémunération du président.
    pub director_charge_ratio_bps: Option<u32>,
    /// Nom du président (lot 39) — signataire du PV et des comptes.
    #[serde(default)]
    pub president_name: Option<String>,
    /// Identité de l'associé unique (lot 39) : nom et adresse, pour le PV des décisions
    /// (lot 41) et le tableau 2033-F. Souvent la même personne que le président.
    #[serde(default)]
    pub sole_shareholder_name: Option<String>,
    #[serde(default)]
    pub sole_shareholder_address: Option<String>,
    /// Nombre d'actions ou de parts composant le capital (2033-F).
    #[serde(default)]
    pub share_count: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetCompanyProfile {
    pub name: String,
    pub legal_form: String,
    pub siren: Siren,
    pub vat_number: Option<VatNumber>,
    pub address: Address,
    pub share_capital: Option<Money>,
    pub rcs_city: Option<String>,
    pub iban: Option<String>,
    pub fiscal_year_end: Option<FiscalYearEnd>,
    pub vat_regime: Option<VatRegime>,
    pub director_monthly_gross: Option<Money>,
    pub director_charge_ratio_bps: Option<u32>,
    #[serde(default)]
    pub president_name: Option<String>,
    #[serde(default)]
    pub sole_shareholder_name: Option<String>,
    #[serde(default)]
    pub sole_shareholder_address: Option<String>,
    #[serde(default)]
    pub share_count: Option<u32>,
}

impl Command for SetCompanyProfile {
    type Output = ();
    const NAME: &'static str = "company.set_profile";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        conn.execute(
            "INSERT INTO company_profile
                (id, name, legal_form, siren, vat_number, address_street, address_postal_code,
                 address_city, address_country, share_capital_cents, rcs_city, iban,
                 fiscal_year_end_month, fiscal_year_end_day, vat_regime,
                 director_monthly_gross_cents, director_charge_ratio_bps, updated_at,
                 president_name, sole_shareholder_name, sole_shareholder_address, share_count)
             VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17,
                     ?18, ?19, ?20, ?21)
             ON CONFLICT (id) DO UPDATE SET
                name = excluded.name,
                legal_form = excluded.legal_form,
                siren = excluded.siren,
                vat_number = excluded.vat_number,
                address_street = excluded.address_street,
                address_postal_code = excluded.address_postal_code,
                address_city = excluded.address_city,
                address_country = excluded.address_country,
                share_capital_cents = excluded.share_capital_cents,
                rcs_city = excluded.rcs_city,
                iban = excluded.iban,
                fiscal_year_end_month = excluded.fiscal_year_end_month,
                fiscal_year_end_day = excluded.fiscal_year_end_day,
                vat_regime = excluded.vat_regime,
                director_monthly_gross_cents = excluded.director_monthly_gross_cents,
                director_charge_ratio_bps = excluded.director_charge_ratio_bps,
                updated_at = excluded.updated_at,
                president_name = excluded.president_name,
                sole_shareholder_name = excluded.sole_shareholder_name,
                sole_shareholder_address = excluded.sole_shareholder_address,
                share_count = excluded.share_count",
            params![
                self.name,
                self.legal_form,
                self.siren.to_string(),
                self.vat_number
                    .as_ref()
                    .map(std::string::ToString::to_string),
                self.address.street,
                self.address.postal_code,
                self.address.city,
                self.address.country,
                self.share_capital.map(Money::cents),
                self.rcs_city,
                self.iban,
                self.fiscal_year_end.map(|f| i64::from(f.month())),
                self.fiscal_year_end.map(|f| i64::from(f.day())),
                self.vat_regime.map(VatRegime::as_str),
                self.director_monthly_gross.map(Money::cents),
                self.director_charge_ratio_bps.map(i64::from),
                OffsetDateTime::now_utc().format(&Rfc3339)?,
                self.president_name,
                self.sole_shareholder_name,
                self.sole_shareholder_address,
                self.share_count.map(i64::from),
            ],
        )?;
        Ok(())
    }
}

fn conv_err(e: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
}

fn row_to_profile(row: &Row) -> rusqlite::Result<CompanyProfile> {
    let siren: String = row.get("siren")?;
    let vat_number: Option<String> = row.get("vat_number")?;
    let share_capital_cents: Option<i64> = row.get("share_capital_cents")?;
    let fy_month: Option<i64> = row.get("fiscal_year_end_month")?;
    let fy_day: Option<i64> = row.get("fiscal_year_end_day")?;
    let vat_regime: Option<String> = row.get("vat_regime")?;
    let director_monthly_gross_cents: Option<i64> = row.get("director_monthly_gross_cents")?;
    let director_charge_ratio_bps: Option<i64> = row.get("director_charge_ratio_bps")?;
    let share_count: Option<i64> = row.get("share_count")?;
    // Mois et jour ne portent une clôture que présents tous les deux ; la migration les pose
    // ensemble, donc un seul renseigné signalerait une base incohérente — on le traite comme
    // « non configuré » plutôt que de deviner.
    let fiscal_year_end = match (fy_month, fy_day) {
        (Some(m), Some(d)) => Some(
            FiscalYearEnd::new(
                u8::try_from(m).map_err(conv_err)?,
                u8::try_from(d).map_err(conv_err)?,
            )
            .map_err(conv_err)?,
        ),
        _ => None,
    };
    Ok(CompanyProfile {
        name: row.get("name")?,
        legal_form: row.get("legal_form")?,
        siren: Siren::parse(&siren).map_err(conv_err)?,
        vat_number: vat_number
            .map(|s| VatNumber::parse(&s))
            .transpose()
            .map_err(conv_err)?,
        address: Address {
            street: row.get("address_street")?,
            postal_code: row.get("address_postal_code")?,
            city: row.get("address_city")?,
            country: row.get("address_country")?,
        },
        share_capital: share_capital_cents.map(Money::from_cents),
        rcs_city: row.get("rcs_city")?,
        iban: row.get("iban")?,
        fiscal_year_end,
        vat_regime: vat_regime
            .map(|s| s.parse::<VatRegime>())
            .transpose()
            .map_err(conv_err)?,
        director_monthly_gross: director_monthly_gross_cents.map(Money::from_cents),
        director_charge_ratio_bps: director_charge_ratio_bps
            .map(u32::try_from)
            .transpose()
            .map_err(conv_err)?,
        president_name: row.get("president_name")?,
        sole_shareholder_name: row.get("sole_shareholder_name")?,
        sole_shareholder_address: row.get("sole_shareholder_address")?,
        share_count: share_count
            .map(u32::try_from)
            .transpose()
            .map_err(conv_err)?,
    })
}

/// # Errors
pub fn company_profile(conn: &Connection) -> Result<Option<CompanyProfile>, AppError> {
    conn.query_row(
        "SELECT * FROM company_profile WHERE id = 1",
        [],
        row_to_profile,
    )
    .optional()
    .map_err(AppError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Actor, ExecutionContext, Executor, Outcome};
    use crate::store::{Passphrase, Store};

    fn test_store(label: &str) -> Store {
        let dir = std::env::temp_dir().join(format!(
            "freeflow-company-test-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ));
        Store::create(&dir.join("vault.db"), &Passphrase::from("s3cret")).unwrap()
    }

    fn sample() -> SetCompanyProfile {
        SetCompanyProfile {
            name: "Argon Digital".to_string(),
            legal_form: "SASU".to_string(),
            siren: Siren::parse("552100554").unwrap(),
            vat_number: None,
            address: Address {
                street: "12 rue de la Paix".to_string(),
                postal_code: "75002".to_string(),
                city: "Paris".to_string(),
                country: "FR".to_string(),
            },
            share_capital: Some(Money::from_cents(100_000)),
            rcs_city: Some("Paris".to_string()),
            iban: Some("FR7630006000011234567890189".to_string()),
            fiscal_year_end: Some(crate::domain::FiscalYearEnd::CALENDAR),
            vat_regime: Some(crate::domain::VatRegime::RealNormalMonthly),
            director_monthly_gross: Some(Money::from_cents(300_000)),
            director_charge_ratio_bps: Some(8_000),
            president_name: None,
            sole_shareholder_name: None,
            sole_shareholder_address: None,
            share_count: None,
        }
    }

    #[test]
    fn setting_the_profile_makes_it_immediately_readable() {
        let mut store = test_store("set-read");
        let cmd = sample();
        let Outcome::Applied(()) = Executor::new(&mut store)
            .execute(&cmd, &ExecutionContext::new(Actor::Human, false))
            .unwrap()
        else {
            panic!("expected Applied")
        };

        let profile = company_profile(store.connection()).unwrap().unwrap();
        assert_eq!(profile.name, "Argon Digital");
        assert_eq!(profile.legal_form, "SASU");
        assert_eq!(profile.share_capital, Some(Money::from_cents(100_000)));
        // Les champs du socle fiscal (lot 17) font l'aller-retour en base.
        assert_eq!(
            profile.fiscal_year_end,
            Some(crate::domain::FiscalYearEnd::CALENDAR)
        );
        assert_eq!(
            profile.vat_regime,
            Some(crate::domain::VatRegime::RealNormalMonthly)
        );
        assert_eq!(
            profile.director_monthly_gross,
            Some(Money::from_cents(300_000))
        );
        assert_eq!(profile.director_charge_ratio_bps, Some(8_000));
    }

    #[test]
    fn setting_the_profile_twice_replaces_it_rather_than_duplicating() {
        let mut store = test_store("set-twice");
        Executor::new(&mut store)
            .execute(&sample(), &ExecutionContext::new(Actor::Human, false))
            .unwrap();

        let mut updated = sample();
        updated.name = "Argon Digital SASU".to_string();
        Executor::new(&mut store)
            .execute(&updated, &ExecutionContext::new(Actor::Human, false))
            .unwrap();

        let count: i64 = store
            .connection()
            .query_row("SELECT count(*) FROM company_profile", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            count, 1,
            "un profil met à jour la même ligne, n'en crée pas une seconde"
        );

        let profile = company_profile(store.connection()).unwrap().unwrap();
        assert_eq!(profile.name, "Argon Digital SASU");
    }

    #[test]
    fn no_profile_set_reads_as_none() {
        let store = test_store("unset");
        assert_eq!(company_profile(store.connection()).unwrap(), None);
    }
}
