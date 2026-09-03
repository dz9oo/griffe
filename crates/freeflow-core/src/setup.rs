//! Premier lancement (lot 39) : **où en est la configuration du coffre**, et quel est le
//! prochain geste — une requête, consommée telle quelle par le tableau de bord, la palette, le
//! parcours de clôture, `freeflow setup status` et l'outil MCP `setup.status`. Aucune façade
//! ne devine ce qui manque : c'est ici, une fois.
//!
//! Trois prérequis : le **profil** (nom, SIREN, adresse — et pour que le calendrier et la
//! clôture soient justes : date de clôture, régime de TVA, capital, associé unique, président),
//! le **point de départ** (un bilan d'ouverture repris du cabinet, ou le choix explicite
//! « société nouvelle » — sans l'un ni l'autre, l'assistant ne sait pas si la reprise a été
//! oubliée), et un **relevé bancaire** importé. Le cœur ne rend rien obligatoire (la date de
//! clôture et le régime de TVA restent facultatifs dans `SetCompanyProfile`) : il dit ce qui
//! est incomplet, en clair.

use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::app::{AppError, Command};
use crate::billing::list_bank_transactions;
use crate::company::{CompanyProfile, company_profile};
use crate::opening_balance::opening_balance;

/// L'état d'un prérequis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Prerequisite {
    Done,
    Missing,
    /// Présent mais incomplet : les champs manquants, nommés pour l'utilisateur.
    Incomplete {
        fields: Vec<String>,
    },
}

impl Prerequisite {
    #[must_use]
    pub const fn is_done(&self) -> bool {
        matches!(self, Self::Done)
    }
}

/// Le prochain geste attendu, dans l'ordre de l'assistant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SetupStep {
    /// Renseigner (ou compléter) le profil de la société.
    Profile,
    /// Dire d'où l'on vient : bilan d'ouverture du cabinet, ou société nouvelle.
    Origin,
    /// Importer le premier relevé bancaire.
    Bank,
    /// Tout est en place : le parcours de clôture prend le relais.
    Done,
}

impl SetupStep {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Profile => "profile",
            Self::Origin => "origin",
            Self::Bank => "bank",
            Self::Done => "done",
        }
    }

    /// Une phrase pour l'utilisateur, la même dans toutes les façades.
    #[must_use]
    pub const fn text(self) -> &'static str {
        match self {
            Self::Profile => {
                "Dites qui vous êtes : nom, SIREN, adresse, date de clôture, régime de TVA, \
                 capital, associé unique — tout le reste en dépend."
            }
            Self::Origin => {
                "Dites d'où vous venez : recopiez le dernier bilan de votre cabinet (bilan \
                 d'ouverture), ou indiquez que la société vient d'être créée."
            }
            Self::Bank => {
                "Importez le relevé de votre banque : c'est lui qui vous dira quoi saisir."
            }
            Self::Done => {
                "Tout est en place : le parcours de clôture de l'exercice vous guide pour la \
                 suite."
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupStatus {
    pub profile: Prerequisite,
    /// Bilan d'ouverture repris, ou société déclarée nouvelle.
    pub origin: Prerequisite,
    pub bank: Prerequisite,
    /// `true` si l'utilisateur a déclaré une société nouvelle (pas de bilan à reprendre).
    pub declared_new_company: bool,
    pub next_step: SetupStep,
}

impl SetupStatus {
    #[must_use]
    pub const fn is_done(&self) -> bool {
        matches!(self.next_step, SetupStep::Done)
    }
}

/// Les champs du profil dont le calendrier, la clôture et les documents ont besoin — nommés
/// comme dans les formulaires, pour que le message se lise sans traduction.
fn missing_profile_fields(profile: &CompanyProfile) -> Vec<String> {
    let mut missing = Vec::new();
    if profile.fiscal_year_end.is_none() {
        missing.push("date de clôture d'exercice".to_string());
    }
    if profile.vat_regime.is_none() {
        missing.push("régime de TVA".to_string());
    }
    if profile.share_capital.is_none() {
        missing.push("capital social".to_string());
    }
    if profile
        .sole_shareholder_name
        .as_deref()
        .is_none_or(|n| n.trim().is_empty())
    {
        missing.push("associé unique".to_string());
    }
    if profile
        .president_name
        .as_deref()
        .is_none_or(|n| n.trim().is_empty())
    {
        missing.push("président".to_string());
    }
    missing
}

/// # Errors
///
/// Erreur de lecture SQLite.
pub fn declared_new_company(conn: &Connection) -> Result<bool, AppError> {
    Ok(conn
        .query_row(
            "SELECT declared_new_company FROM setup WHERE id = 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .is_some_and(|v| v != 0))
}

/// L'état de la configuration du coffre — voir le commentaire de module.
///
/// # Errors
///
/// Erreur de lecture SQLite.
pub fn setup_status(conn: &Connection) -> Result<SetupStatus, AppError> {
    let profile = match company_profile(conn)? {
        None => Prerequisite::Missing,
        Some(p) => {
            let fields = missing_profile_fields(&p);
            if fields.is_empty() {
                Prerequisite::Done
            } else {
                Prerequisite::Incomplete { fields }
            }
        }
    };
    let declared_new_company = declared_new_company(conn)?;
    let origin = if opening_balance(conn)?.is_some() || declared_new_company {
        Prerequisite::Done
    } else {
        Prerequisite::Missing
    };
    let bank = if list_bank_transactions(conn)?.is_empty() {
        Prerequisite::Missing
    } else {
        Prerequisite::Done
    };
    let next_step = if profile == Prerequisite::Missing {
        SetupStep::Profile
    } else if !origin.is_done() {
        SetupStep::Origin
    } else if !bank.is_done() {
        SetupStep::Bank
    } else if !profile.is_done() {
        SetupStep::Profile
    } else {
        SetupStep::Done
    };
    Ok(SetupStatus {
        profile,
        origin,
        bank,
        declared_new_company,
        next_step,
    })
}

/// Déclare que la société vient d'être créée : il n'y a pas de bilan de cabinet à reprendre, la
/// chaîne part de zéro en connaissance de cause. Réversible (`declared: false`) ; sans effet
/// sur un bilan d'ouverture existant, qui prime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeclareNewCompany {
    pub declared: bool,
}

impl Command for DeclareNewCompany {
    type Output = ();
    const NAME: &'static str = "setup.declare_new_company";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        conn.execute(
            "INSERT INTO setup (id, declared_new_company, updated_at) VALUES (1, ?1, ?2)
             ON CONFLICT (id) DO UPDATE SET
                declared_new_company = excluded.declared_new_company,
                updated_at = excluded.updated_at",
            rusqlite::params![
                i64::from(self.declared),
                OffsetDateTime::now_utc().format(&Rfc3339)?
            ],
        )?;
        Ok(())
    }
}

/// La vue JSON partagée par `freeflow setup status --json` et `setup.status`.
#[must_use]
pub fn setup_json(status: &SetupStatus) -> serde_json::Value {
    serde_json::json!({
        "profile": status.profile,
        "origin": status.origin,
        "bank": status.bank,
        "declared_new_company": status.declared_new_company,
        "next_step": status.next_step.as_str(),
        "next_step_text": status.next_step.text(),
        "done": status.is_done(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Actor, ExecutionContext, Executor};
    use crate::billing::{ImportBankTransactions, ParsedTransaction};
    use crate::company::SetCompanyProfile;
    use crate::domain::{Address, FiscalYearEnd, Money, Siren, VatRegime};
    use crate::opening_balance::RecordOpeningBalance;
    use crate::store::{Passphrase, Store};

    fn test_store(label: &str) -> Store {
        let dir = std::env::temp_dir().join(format!(
            "freeflow-setup-test-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ));
        Store::create(&dir.join("vault.db"), &Passphrase::from("s3cret")).unwrap()
    }

    fn human() -> ExecutionContext {
        ExecutionContext::new(Actor::Human, false)
    }

    fn profile(complete: bool) -> SetCompanyProfile {
        SetCompanyProfile {
            name: "Nova Dev".to_string(),
            legal_form: "SASU".to_string(),
            siren: Siren::parse("889112348").unwrap(),
            vat_number: None,
            address: Address {
                street: "3 allée des Tanneurs".to_string(),
                postal_code: "44000".to_string(),
                city: "Nantes".to_string(),
                country: "FR".to_string(),
            },
            share_capital: complete.then_some(Money::from_cents(100_000)),
            rcs_city: None,
            iban: None,
            fiscal_year_end: complete.then(|| FiscalYearEnd::new(9, 30).unwrap()),
            vat_regime: complete.then_some(VatRegime::RealSimplified),
            director_monthly_gross: None,
            director_charge_ratio_bps: None,
            president_name: complete.then(|| "Nova Martin".to_string()),
            sole_shareholder_name: complete.then(|| "Nova Martin".to_string()),
            sole_shareholder_address: None,
            share_count: complete.then_some(100),
        }
    }

    /// L'ordre de l'assistant : profil absent → origine → banque → profil incomplet → fait.
    #[test]
    fn the_next_step_follows_the_assistant_order_and_names_what_is_missing() {
        let mut store = test_store("order");
        let status = setup_status(store.connection()).unwrap();
        assert_eq!(status.next_step, SetupStep::Profile);
        assert_eq!(status.profile, Prerequisite::Missing);

        Executor::new(&mut store)
            .execute(&profile(false), &human())
            .unwrap();
        let status = setup_status(store.connection()).unwrap();
        assert_eq!(
            status.profile,
            Prerequisite::Incomplete {
                fields: vec![
                    "date de clôture d'exercice".into(),
                    "régime de TVA".into(),
                    "capital social".into(),
                    "associé unique".into(),
                    "président".into(),
                ]
            }
        );
        assert_eq!(
            status.next_step,
            SetupStep::Origin,
            "l'origine passe avant les détails"
        );

        Executor::new(&mut store)
            .execute(&DeclareNewCompany { declared: true }, &human())
            .unwrap();
        let status = setup_status(store.connection()).unwrap();
        assert!(status.declared_new_company);
        assert!(status.origin.is_done());
        assert_eq!(status.next_step, SetupStep::Bank);

        Executor::new(&mut store)
            .execute(
                &ImportBankTransactions {
                    transactions: vec![ParsedTransaction {
                        occurred_on: time::macros::date!(2026 - 01 - 15),
                        amount_cents: -1_250,
                        description: "FRAIS".to_string(),
                        fitid: None,
                    }],
                },
                &human(),
            )
            .unwrap();
        let status = setup_status(store.connection()).unwrap();
        assert!(status.bank.is_done());
        assert_eq!(
            status.next_step,
            SetupStep::Profile,
            "il reste à compléter le profil"
        );

        Executor::new(&mut store)
            .execute(&profile(true), &human())
            .unwrap();
        let status = setup_status(store.connection()).unwrap();
        assert!(status.is_done());
        assert_eq!(setup_json(&status)["next_step"], "done");
        assert_eq!(setup_json(&status)["profile"]["state"], "done");
    }

    /// Un bilan d'ouverture vaut origine, société déclarée nouvelle ou non.
    #[test]
    fn an_opening_balance_settles_the_origin() {
        let mut store = test_store("opening");
        Executor::new(&mut store)
            .execute(&profile(true), &human())
            .unwrap();
        Executor::new(&mut store)
            .execute(
                &RecordOpeningBalance {
                    opens_on: time::macros::date!(2025 - 10 - 01),
                    source: None,
                    lines: vec![
                        "101000:Capital:C:1000.00".parse().unwrap(),
                        "512000:Banque:D:1000.00".parse().unwrap(),
                    ],
                    tax_losses: Money::ZERO,
                },
                &human(),
            )
            .unwrap();
        let status = setup_status(store.connection()).unwrap();
        assert!(status.origin.is_done());
        assert!(!status.declared_new_company);
        assert_eq!(status.next_step, SetupStep::Bank);
        let json = setup_json(&status);
        assert_eq!(json["origin"]["state"], "done");
        assert_eq!(json["bank"]["state"], "missing");
    }
}
