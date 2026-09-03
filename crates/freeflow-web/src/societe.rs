//! Routes de l'écran `societe` (lot 39) : `POST /societe` enregistre le profil depuis le
//! formulaire de la fenêtre — la même commande `SetCompanyProfile` que la CLI et le serveur MCP,
//! aucune validation propre au-delà de l'analyse des champs (SIREN, TVA, montants, date).

use axum::Form;
use axum::extract::State;
use axum::http::HeaderValue;
use axum::response::{Html, IntoResponse, Response};
use freeflow_core::app::Executor;
use freeflow_core::company::SetCompanyProfile;
use freeflow_core::domain::{Address, FiscalYearEnd, Money, Siren, VatNumber, VatRegime};
use serde::Deserialize;

use crate::state::AppState;
use crate::views;
use crate::views::societe::{ProfileFormErrors, ProfileFormValues};

#[derive(Debug, Deserialize, Default)]
pub struct ProfileForm {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub legal_form: String,
    #[serde(default)]
    pub siren: String,
    #[serde(default)]
    pub vat_number: String,
    #[serde(default)]
    pub street: String,
    #[serde(default)]
    pub postal_code: String,
    #[serde(default)]
    pub city: String,
    #[serde(default)]
    pub country: String,
    #[serde(default)]
    pub share_capital: String,
    #[serde(default)]
    pub share_count: String,
    #[serde(default)]
    pub rcs_city: String,
    #[serde(default)]
    pub iban: String,
    #[serde(default)]
    pub fiscal_year_end: String,
    #[serde(default)]
    pub vat_regime: String,
    #[serde(default)]
    pub president_name: String,
    #[serde(default)]
    pub sole_shareholder_name: String,
    #[serde(default)]
    pub sole_shareholder_address: String,
    #[serde(default)]
    pub director_gross: String,
    #[serde(default)]
    pub director_charge_ratio: String,
}

impl From<&ProfileForm> for ProfileFormValues {
    fn from(f: &ProfileForm) -> Self {
        Self {
            name: f.name.clone(),
            legal_form: f.legal_form.clone(),
            siren: f.siren.clone(),
            vat_number: f.vat_number.clone(),
            street: f.street.clone(),
            postal_code: f.postal_code.clone(),
            city: f.city.clone(),
            country: f.country.clone(),
            share_capital: f.share_capital.clone(),
            share_count: f.share_count.clone(),
            rcs_city: f.rcs_city.clone(),
            iban: f.iban.clone(),
            fiscal_year_end: f.fiscal_year_end.clone(),
            vat_regime: f.vat_regime.clone(),
            president_name: f.president_name.clone(),
            sole_shareholder_name: f.sole_shareholder_name.clone(),
            sole_shareholder_address: f.sole_shareholder_address.clone(),
            director_gross: f.director_gross.clone(),
            director_charge_ratio: f.director_charge_ratio.clone(),
        }
    }
}

fn opt(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

fn money_field(raw: &str, slot: &mut Option<String>) -> Option<Money> {
    let t = raw.trim();
    if t.is_empty() {
        return None;
    }
    match Money::parse_decimal(t) {
        Ok(m) => Some(m),
        Err(_) => {
            *slot = Some("montant invalide (ex. 1000 ou 1000,00)".to_string());
            None
        }
    }
}

/// Analyse le formulaire : la commande du cœur, ou les erreurs de champ.
///
/// # Errors
///
/// Les erreurs de champ, à re-rendre dans le formulaire.
pub fn parse_profile_form(form: &ProfileForm) -> Result<SetCompanyProfile, Box<ProfileFormErrors>> {
    let mut errors = ProfileFormErrors::default();
    let siren = Siren::parse(form.siren.trim())
        .map_err(|e| errors.siren = Some(e.to_string()))
        .ok();
    let vat_number = match form.vat_number.trim() {
        "" => None,
        raw => VatNumber::parse(raw)
            .map_err(|e| errors.vat_number = Some(e.to_string()))
            .ok(),
    };
    let share_capital = money_field(&form.share_capital, &mut errors.share_capital);
    let director_monthly_gross = money_field(&form.director_gross, &mut errors.director);
    let fiscal_year_end = match form.fiscal_year_end.trim() {
        "" => None,
        raw => raw
            .split_once('/')
            .and_then(|(d, m)| Some((d.trim().parse::<u8>().ok()?, m.trim().parse::<u8>().ok()?)))
            .and_then(|(d, m)| FiscalYearEnd::new(m, d).ok())
            .or_else(|| {
                errors.fiscal_year_end = Some("format attendu JJ/MM, ex. 31/12".to_string());
                None
            }),
    };
    let vat_regime = match form.vat_regime.trim() {
        "" => None,
        raw => raw
            .parse::<VatRegime>()
            .map_err(|_| errors.vat_regime = Some("régime inconnu".to_string()))
            .ok(),
    };
    let share_count = match form.share_count.trim() {
        "" => None,
        raw => raw
            .parse::<u32>()
            .map_err(|_| errors.share_count = Some("nombre entier attendu".to_string()))
            .ok(),
    };
    let director_charge_ratio_bps = match form.director_charge_ratio.trim() {
        "" => None,
        raw => raw
            .replace(',', ".")
            .parse::<f64>()
            .ok()
            .filter(|p| p.is_finite() && (0.0..=1000.0).contains(p))
            .map(|p| (p * 100.0).round() as u32),
    };
    let has_error = errors.siren.is_some()
        || errors.vat_number.is_some()
        || errors.share_capital.is_some()
        || errors.fiscal_year_end.is_some()
        || errors.vat_regime.is_some()
        || errors.share_count.is_some()
        || errors.director.is_some();
    if form.name.trim().is_empty() || form.legal_form.trim().is_empty() {
        errors.banner = Some("la dénomination et la forme juridique sont obligatoires".into());
    }
    if has_error || errors.banner.is_some() {
        return Err(Box::new(errors));
    }
    Ok(SetCompanyProfile {
        name: form.name.trim().to_string(),
        legal_form: form.legal_form.trim().to_string(),
        siren: siren.expect("vérifié ci-dessus"),
        vat_number,
        address: Address {
            street: form.street.trim().to_string(),
            postal_code: form.postal_code.trim().to_string(),
            city: form.city.trim().to_string(),
            country: form.country.trim().to_string(),
        },
        share_capital,
        rcs_city: opt(&form.rcs_city),
        iban: opt(&form.iban),
        fiscal_year_end,
        vat_regime,
        director_monthly_gross,
        director_charge_ratio_bps,
        president_name: opt(&form.president_name),
        sole_shareholder_name: opt(&form.sole_shareholder_name),
        sole_shareholder_address: opt(&form.sole_shareholder_address),
        share_count,
    })
}

/// `POST /societe` : enregistre, puis re-rend le formulaire avec « profil enregistré » et
/// `HX-Trigger: freeflow:saved` (les autres écrans se rafraîchissent).
pub async fn save(State(state): State<AppState>, Form(form): Form<ProfileForm>) -> Response {
    let values = ProfileFormValues::from(&form);
    let cmd = match parse_profile_form(&form) {
        Ok(cmd) => cmd,
        Err(errors) => {
            return Html(
                views::societe::profile_form("/societe", &values, &errors, None, "Enregistrer")
                    .into_string(),
            )
            .into_response();
        }
    };
    let outcome = state
        .with_store_mut(|store| Executor::new(store).execute(&cmd, &AppState::human_ctx()))
        .await;
    match outcome {
        None => Html("coffre verrouillé — rechargez la page".to_string()).into_response(),
        Some(Err(e)) => {
            let errors = ProfileFormErrors {
                banner: Some(views::errors::message(&e)),
                ..Default::default()
            };
            Html(
                views::societe::profile_form("/societe", &values, &errors, None, "Enregistrer")
                    .into_string(),
            )
            .into_response()
        }
        Some(Ok(_)) => {
            let mut response = Html(
                views::societe::profile_form(
                    "/societe",
                    &values,
                    &ProfileFormErrors::default(),
                    Some("profil enregistré"),
                    "Enregistrer",
                )
                .into_string(),
            )
            .into_response();
            response
                .headers_mut()
                .insert("HX-Trigger", HeaderValue::from_static("freeflow:saved"));
            response
        }
    }
}
