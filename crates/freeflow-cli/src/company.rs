//! `freeflow company ...`

use clap::{Args, Subcommand};
use freeflow_core::app::{ExecutionContext, Executor};
use freeflow_core::company;
use freeflow_core::domain::{Address, FiscalYearEnd, Money, Siren, VatNumber, VatRegime};
use freeflow_core::fiscal::{CompanyProfileWithVatFiling, company_profile_with_vat_filing};
use freeflow_core::store::Store;

use crate::error::CliError;
use crate::output::{HumanRender, format_outcome_as, format_value, key_values, or_dash};
use crate::parsers::{parse_money, parse_siren, parse_vat_number};

impl HumanRender for CompanyProfileWithVatFiling {
    fn render_human(&self) -> String {
        let p = &self.profile;
        let director = match (p.director_monthly_gross, p.director_charge_ratio_bps) {
            (None, _) => "non rémunéré".to_string(),
            (Some(gross), ratio) => format!(
                "{gross} brut / mois{}",
                ratio.map_or(String::new(), |r| format!(
                    ", charges {},{:02} %",
                    r / 100,
                    r % 100
                ))
            ),
        };
        key_values(&[
            ("Dénomination", format!("{} ({})", p.name, p.legal_form)),
            ("SIREN", p.siren.to_string()),
            ("TVA intracom.", or_dash(p.vat_number.as_ref())),
            (
                "Adresse",
                format!(
                    "{}, {} {}, {}",
                    p.address.street, p.address.postal_code, p.address.city, p.address.country
                ),
            ),
            ("Capital social", or_dash(p.share_capital)),
            ("RCS", or_dash(p.rcs_city.as_deref())),
            ("IBAN", or_dash(p.iban.as_deref())),
            (
                "Clôture d'exercice",
                p.fiscal_year_end.map_or_else(
                    || "non renseignée (année civile supposée)".to_string(),
                    |f| format!("{:02}/{:02}", f.day(), f.month()),
                ),
            ),
            (
                "Régime de TVA",
                p.vat_regime.map_or_else(
                    || "non renseigné (mensuel supposé)".to_string(),
                    |r| r.to_string(),
                ),
            ),
            ("Dirigeant", director),
            ("Président", or_dash(p.president_name.as_deref())),
            (
                "Associé unique",
                match (&p.sole_shareholder_name, &p.sole_shareholder_address) {
                    (None, _) => "—".to_string(),
                    (Some(n), None) => n.clone(),
                    (Some(n), Some(a)) => format!("{n}, {a}"),
                },
            ),
            ("Actions", or_dash(p.share_count)),
            ("Télédéclaration TVA", self.vat_filing.note.clone()),
        ])
    }
}

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
    /// Ratio (charges patronales + salariales) / brut du dirigeant, en pourcentage (ex. `45`),
    /// pour estimer les cotisations : le coût employeur vaut brut × (1 + ratio), la DSN affiche
    /// brut × ratio.
    #[arg(long, value_parser = parse_charge_ratio_bps)]
    director_charge_ratio: Option<u32>,
    /// Nom du président (signataire du PV et des comptes).
    #[arg(long)]
    president: Option<String>,
    /// Nom de l'associé unique (souvent le président lui-même).
    #[arg(long)]
    sole_shareholder: Option<String>,
    /// Adresse de l'associé unique, pour le PV des décisions.
    #[arg(long)]
    sole_shareholder_address: Option<String>,
    /// Nombre d'actions (ou de parts) composant le capital.
    #[arg(long)]
    share_count: Option<u32>,
}

#[derive(Debug, Subcommand)]
pub enum CompanyCommand {
    /// Définit (ou remplace) l'identité légale de l'émetteur — nécessaire aux mentions
    /// obligatoires d'une facture (`freeflow invoice render`).
    ///
    /// La date de clôture est sur vos statuts et votre dernier bilan (`--fiscal-year-end 30/09`).
    /// Régimes de TVA (`--vat-regime`) : `real_normal_monthly`, une déclaration CA3 par mois
    /// (le cas général au-delà de 254 000 € de prestations, ou sur option) ;
    /// `real_normal_quarterly`, une CA3 par trimestre (TVA annuelle inférieure à 4 000 €) ;
    /// `real_simplified`, deux acomptes (juillet, décembre) et une déclaration annuelle CA12 —
    /// régime supprimé pour les exercices ouverts à compter de 2027 ; `franchise`, aucune TVA
    /// facturée ni déclarée (sous 37 500 € de prestations). L'associé unique et le président
    /// (`--sole-shareholder`, `--president`) figurent sur le PV d'approbation des comptes.
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
                president_name: args.president,
                sole_shareholder_name: args.sole_shareholder,
                sole_shareholder_address: args.sole_shareholder_address,
                share_count: args.share_count,
            };
            let outcome = Executor::new(store).execute(&command, ctx)?;
            format_outcome_as(&outcome, json, |()| "profil enregistré".to_string())
        }
        CompanyCommand::Show => {
            let profile = company_profile_with_vat_filing(
                store.connection(),
                freeflow_core::clock::today_local(),
            )?;
            format_value(&profile, json)
        }
    };
    Ok(output)
}
