//! Outils `company.*` — miroir de `freeflow company ...` (CLI). Le profil est un singleton en
//! état complet (doctrine du lot 15 : jamais un patch) : `company.set_profile` remplace tout,
//! `company.show` lit tout — voir aussi la ressource `freeflow://company`.

use freeflow_core::app::Executor;
use freeflow_core::company::{self, company_profile};
use freeflow_core::domain::{Address, FiscalYearEnd, Money, Siren, VatNumber, VatRegime};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::server::FreeflowServer;
use crate::support::{err_text, ok_json, ok_or_return, outcome_json};

/// Analyse une date de clôture récurrente au format `JJ/MM` (ex. `31/12`) — dupliqué depuis
/// `freeflow_cli::company` pour la même raison que les résolveurs de `support` (format d'erreur
/// divergent entre adaptateurs).
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

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct SetProfileArgs {
    name: String,
    /// Ex. `SASU`, `EURL`.
    legal_form: String,
    /// SIREN à 9 chiffres.
    siren: String,
    /// Numéro de TVA intracommunautaire (ex. `FR32552100554`).
    vat_number: Option<String>,
    street: String,
    postal_code: String,
    city: String,
    country: String,
    /// Capital social, en centimes.
    share_capital_cents: Option<i64>,
    /// Ville du greffe d'immatriculation, pour la mention RCS.
    rcs_city: Option<String>,
    iban: Option<String>,
    /// Date de clôture d'exercice récurrente, au format `JJ/MM` (ex. `31/12`) — socle du
    /// calendrier fiscal.
    fiscal_year_end: Option<String>,
    /// Régime de TVA : `real_normal_monthly`, `real_normal_quarterly`, `real_simplified`, ou
    /// `franchise`.
    vat_regime: Option<String>,
    /// Rémunération mensuelle brute du président (assimilé salarié), en centimes. Absent = non
    /// rémunéré.
    director_monthly_gross_cents: Option<i64>,
    /// Ratio charges/net du dirigeant, en dix-millièmes (ex. `8000` = 80 %), pour estimer les
    /// cotisations.
    director_charge_ratio_bps: Option<u32>,
    #[serde(default)]
    dry_run: bool,
}

#[tool_router(router = company_router, vis = "pub(crate)")]
impl FreeflowServer {
    /// Affiche l'identité légale de l'émetteur (mentions obligatoires des factures), ou `null`
    /// si aucun profil n'est encore défini.
    #[tool(
        name = "company.show",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn company_show(&self) -> CallToolResult {
        let store = self.store.lock().await;
        match company_profile(store.connection()) {
            Ok(profile) => ok_json(profile),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Définit (ou remplace entièrement) l'identité légale de l'émetteur — nécessaire aux
    /// mentions obligatoires d'une facture (`invoice.render`) et au calendrier fiscal chiffré.
    /// État complet : les champs optionnels absents effacent la valeur existante.
    #[tool(
        name = "company.set_profile",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true
        )
    )]
    async fn company_set_profile(
        &self,
        Parameters(args): Parameters<SetProfileArgs>,
    ) -> CallToolResult {
        let siren: Siren = ok_or_return!("siren", Siren::parse(&args.siren));
        let vat_number: Option<VatNumber> = match &args.vat_number {
            Some(s) => Some(ok_or_return!("vat_number", VatNumber::parse(s))),
            None => None,
        };
        let fiscal_year_end: Option<FiscalYearEnd> = match &args.fiscal_year_end {
            Some(s) => Some(ok_or_return!("fiscal_year_end", parse_fiscal_year_end(s))),
            None => None,
        };
        let vat_regime: Option<VatRegime> = match &args.vat_regime {
            Some(s) => Some(ok_or_return!("vat_regime", s.parse::<VatRegime>())),
            None => None,
        };
        let cmd = company::SetCompanyProfile {
            name: args.name,
            legal_form: args.legal_form,
            siren,
            vat_number,
            address: Address {
                street: args.street,
                postal_code: args.postal_code,
                city: args.city,
                country: args.country,
            },
            share_capital: args.share_capital_cents.map(Money::from_cents),
            rcs_city: args.rcs_city,
            iban: args.iban,
            fiscal_year_end,
            vat_regime,
            director_monthly_gross: args.director_monthly_gross_cents.map(Money::from_cents),
            director_charge_ratio_bps: args.director_charge_ratio_bps,
        };
        let mut store = self.store.lock().await;
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }
}
