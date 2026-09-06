//! Écran `societe` (lot 39) : le profil de la société, **modifiable dans la fenêtre** — jusqu'ici
//! la seule voie était la console (`company set-profile` et ses douze options). Le même
//! formulaire sert à l'assistant de premier lancement (`premiers_pas`) ; chaque champ porte son
//! aide (« la date de clôture est sur vos statuts et votre dernier bilan »), et les régimes de
//! TVA sont expliqués en une phrase.

use freeflow_core::app::AppError;
use freeflow_core::billing::list_bank_transactions;
use freeflow_core::company::{CompanyProfile, company_profile};
use freeflow_core::domain::{Money, format_date, format_date_fr};
use freeflow_core::fiscal::fiscal_calendar;
use freeflow_core::store::Store;
use maud::{Markup, html};
use time::Date;

use crate::layout::{ViewId, view_head};
use crate::views::copy::{deadline_fr, letter_date, year_end_fr};
use crate::views::form;

pub const VAT_REGIME_OPTIONS: [(&str, &str); 5] = [
    ("", "— à choisir —"),
    (
        "real_normal_monthly",
        "réel normal mensuel — une déclaration de TVA (CA3) chaque mois (cas général)",
    ),
    (
        "real_normal_quarterly",
        "réel normal trimestriel — une CA3 par trimestre (TVA annuelle < 4 000 €)",
    ),
    (
        "real_simplified",
        "réel simplifié — deux acomptes et une déclaration annuelle (supprimé à partir de 2027)",
    ),
    (
        "franchise",
        "franchise en base — aucune TVA facturée ni déclarée (sous 37 500 € de prestations)",
    ),
];

#[derive(Default, Clone)]
pub struct ProfileFormValues {
    pub name: String,
    pub legal_form: String,
    pub siren: String,
    pub vat_number: String,
    pub street: String,
    pub postal_code: String,
    pub city: String,
    pub country: String,
    pub share_capital: String,
    pub share_count: String,
    pub rcs_city: String,
    pub iban: String,
    pub fiscal_year_end: String,
    pub vat_regime: String,
    pub president_name: String,
    pub sole_shareholder_name: String,
    pub sole_shareholder_address: String,
    pub director_gross: String,
    pub director_charge_ratio: String,
}

impl ProfileFormValues {
    /// Valeurs par défaut d'un profil vide : SASU, France, exercice civil — ce que la cible
    /// du produit est le plus souvent.
    #[must_use]
    pub fn blank() -> Self {
        Self {
            legal_form: "SASU".to_string(),
            country: "FR".to_string(),
            fiscal_year_end: "31/12".to_string(),
            ..Self::default()
        }
    }
}

fn euros(m: Option<freeflow_core::domain::Money>) -> String {
    m.map_or(
        String::new(),
        freeflow_core::domain::Money::to_decimal_string,
    )
}

impl From<&CompanyProfile> for ProfileFormValues {
    fn from(p: &CompanyProfile) -> Self {
        Self {
            name: p.name.clone(),
            legal_form: p.legal_form.clone(),
            siren: p.siren.to_string(),
            vat_number: p
                .vat_number
                .as_ref()
                .map_or(String::new(), ToString::to_string),
            street: p.address.street.clone(),
            postal_code: p.address.postal_code.clone(),
            city: p.address.city.clone(),
            country: p.address.country.clone(),
            share_capital: euros(p.share_capital),
            share_count: p.share_count.map_or(String::new(), |n| n.to_string()),
            rcs_city: p.rcs_city.clone().unwrap_or_default(),
            iban: p.iban.clone().unwrap_or_default(),
            fiscal_year_end: p.fiscal_year_end.map_or(String::new(), |f| {
                format!("{:02}/{:02}", f.day(), f.month())
            }),
            vat_regime: p
                .vat_regime
                .map_or(String::new(), |r| r.as_str().to_string()),
            president_name: p.president_name.clone().unwrap_or_default(),
            sole_shareholder_name: p.sole_shareholder_name.clone().unwrap_or_default(),
            sole_shareholder_address: p.sole_shareholder_address.clone().unwrap_or_default(),
            director_gross: euros(p.director_monthly_gross),
            director_charge_ratio: p
                .director_charge_ratio_bps
                .map_or(String::new(), |b| format!("{}.{:02}", b / 100, b % 100)),
        }
    }
}

#[derive(Default)]
pub struct ProfileFormErrors {
    pub banner: Option<String>,
    pub siren: Option<String>,
    pub vat_number: Option<String>,
    pub share_capital: Option<String>,
    pub fiscal_year_end: Option<String>,
    pub vat_regime: Option<String>,
    pub director: Option<String>,
    pub share_count: Option<String>,
}

/// Le formulaire de profil, posté sur `action` ; `notice` est un bandeau d'information
/// (« profil enregistré »).
pub fn profile_form(
    action: &str,
    values: &ProfileFormValues,
    errors: &ProfileFormErrors,
    notice: Option<&str>,
    submit_label: &str,
) -> Markup {
    html! {
        form hx-post=(action) hx-target="#societe-form" hx-swap="outerHTML" id="societe-form" {
            @if let Some(msg) = &errors.banner {
                (form::error_banner(msg))
            }
            @if let Some(msg) = notice {
                div class="detail-note" { span class="badge ok" { "✓" } " " (msg) }
            }
            h3 { "Identité" }
            (form::text("name", "Dénomination sociale", &values.name, None))
            (form::text("legal_form", "Forme juridique (SASU, EURL…)", &values.legal_form, None))
            (form::text("siren", "SIREN (9 chiffres)", &values.siren, errors.siren.as_deref()))
            (form::field_help("Sur votre extrait Kbis et vos factures. Le numéro de TVA intracommunautaire en découle (FR + clé + SIREN)."))
            (form::text("vat_number", "Numéro de TVA intracommunautaire (facultatif)", &values.vat_number, errors.vat_number.as_deref()))
            (form::text("street", "Adresse du siège", &values.street, None))
            (form::text("postal_code", "Code postal", &values.postal_code, None))
            (form::text("city", "Ville", &values.city, None))
            (form::text("country", "Pays (code, ex. FR)", &values.country, None))
            (form::text("rcs_city", "Ville du greffe (mention RCS, facultatif)", &values.rcs_city, None))
            (form::text("iban", "IBAN (facultatif, imprimé sur les factures)", &values.iban, None))

            h3 { "Exercice et TVA" }
            (form::text("fiscal_year_end", "Date de clôture d'exercice (JJ/MM)", &values.fiscal_year_end, errors.fiscal_year_end.as_deref()))
            (form::field_help("Elle est sur vos statuts et sur votre dernier bilan (souvent 31/12, parfois 30/09 ou 30/06). Tout le calendrier fiscal en dépend."))
            (form::select("vat_regime", "Régime de TVA", &VAT_REGIME_OPTIONS, &values.vat_regime, errors.vat_regime.as_deref()))
            (form::field_help("Indiqué sur votre attestation de régime fiscal (impots.gouv.fr, espace professionnel) ou par votre cabinet. En cas de doute, le réel normal mensuel est le cas le plus courant pour une prestation intellectuelle."))

            h3 { "Capital et personnes" }
            (form::number("share_capital", "Capital social (€)", &values.share_capital, "0.01", errors.share_capital.as_deref()))
            (form::number("share_count", "Nombre d'actions ou de parts", &values.share_count, "1", errors.share_count.as_deref()))
            (form::text("president_name", "Président (signataire des comptes)", &values.president_name, None))
            (form::text("sole_shareholder_name", "Associé unique", &values.sole_shareholder_name, None))
            (form::field_help("En SASU, souvent la même personne que le président. Son nom et son adresse figurent sur le PV d'approbation des comptes."))
            (form::text("sole_shareholder_address", "Adresse de l'associé unique", &values.sole_shareholder_address, None))
            (form::number("director_gross", "Rémunération mensuelle brute du président (€, vide = non rémunéré)", &values.director_gross, "0.01", errors.director.as_deref()))
            (form::number("director_charge_ratio", "Charges sociales, en % du brut (ex. 80)", &values.director_charge_ratio, "0.01", None))
            (form::actions(submit_label))
        }
    }
}

/// La pièce « La société » (lot 48) : identité courte + chapitres. Le paysage de trésorerie
/// et « te payer » chiffré arrivent au lot 51 ; ici on pointe les écrans déjà là.
pub fn piece(store: &Store, today: Date) -> Result<Markup, AppError> {
    let conn = store.connection();
    let profile = company_profile(conn)?;
    let unmatched = list_bank_transactions(conn)?
        .into_iter()
        .filter(|t| !t.is_matched())
        .count();
    let calendar = fiscal_calendar(conn, today)?;
    let next = calendar.first();

    let title = profile.as_ref().map_or("La société.", |p| p.name.as_str());
    let lede = match &profile {
        Some(p) => {
            let form = p.legal_form.as_str();
            let capital = p
                .share_capital
                .filter(|c| *c != Money::ZERO)
                .map(|c| format!(" au capital de {c}"))
                .unwrap_or_default();
            match p.fiscal_year_end {
                Some(end) => format!(
                    "{form}{capital}. L'exercice se clôt le {}.",
                    year_end_fr(end)
                ),
                None => format!("{form}{capital}. La date de clôture n'est pas encore posée."),
            }
        }
        None => "Dites d'abord qui vous êtes : nom, forme, clôture. Tout le reste en dépend."
            .to_string(),
    };

    let taxes_sub = match next {
        Some(d) => {
            let amount = d
                .amount
                .filter(|m| *m != Money::ZERO)
                .map(|m| format!(" · {m}"))
                .unwrap_or_default();
            format!(
                "{} le {}{amount}",
                deadline_fr(d.kind),
                format_date_fr(d.due_on)
            )
        }
        None => "Aucune échéance dans l'horizon.".to_string(),
    };
    let releve_sub = match unmatched {
        0 => "Tout est lu.".to_string(),
        1 => "1 mouvement sans lecture".to_string(),
        n => format!("{n} mouvements sans lecture"),
    };
    let identite_sub = profile.as_ref().map_or_else(
        || "à renseigner".to_string(),
        |p| {
            format!(
                "{} · {}",
                p.legal_form,
                p.fiscal_year_end
                    .map(year_end_fr)
                    .unwrap_or_else(|| "clôture à poser".to_string())
            )
        },
    );

    Ok(html! {
        div class="letter" data-view=(ViewId::Societe.slug()) {
            div class="date" { "La société · " (letter_date(today)) }
            h1 { (title) }
            p class="lede" { (lede) }

            ul class="chapters" {
                li {
                    a href="/view/societe"
                      hx-get="/view/societe" hx-target="#content" hx-push-url="true" {
                        div {
                            strong { "Te payer" }
                            span { "Le montant possible sans casser la piste n'est pas encore calculé — la rémunération du dirigeant se pose dans l'identité." }
                        }
                        span class="go" { "→" }
                    }
                }
                li {
                    a href="#impots" {
                        div {
                            strong { "Ce que tu dois à l'État" }
                            span { (taxes_sub) }
                        }
                        span class="go" { "→" }
                    }
                }
                li {
                    a href=(ViewId::Cloture.path())
                      hx-get=(ViewId::Cloture.path()) hx-target="#content" hx-push-url="true" {
                        div {
                            strong { "Clore l'exercice" }
                            span { "Le parcours, en phrases." }
                        }
                        span class="go" { "→" }
                    }
                }
                li {
                    a href=(ViewId::Depenses.path())
                      hx-get=(ViewId::Depenses.path()) hx-target="#content" hx-push-url="true" {
                        div {
                            strong { "Le relevé" }
                            span { (releve_sub) }
                        }
                        span class="go" { "→" }
                    }
                }
                li {
                    a href="/view/societe"
                      hx-get="/view/societe" hx-target="#content" hx-push-url="true" {
                        div {
                            strong { "L'identité" }
                            span { (identite_sub) }
                        }
                        span class="go" { "→" }
                    }
                }
            }

            @if !calendar.is_empty() {
                div class="block" id="impots" {
                    h3 { "Les prochaines échéances" }
                    ul class="chapters" {
                        @for d in calendar.iter().take(5) {
                            li {
                                div class="id-line" {
                                    span class="k" { (format_date_fr(d.due_on)) }
                                    span class="val" {
                                        (deadline_fr(d.kind))
                                        @if let Some(amount) = d.amount {
                                            @if amount != Money::ZERO { " · " (amount) }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    p class="prose" { "Les chiffres sont dans le coffre. Le dépôt se fait sur le site des impôts — pas ici." }
                }
            }
        }
    })
}

/// L'écran `societe` : le formulaire pré-rempli, ou vide avec ses défauts.
pub fn render(store: &Store) -> Result<Markup, AppError> {
    let profile = company_profile(store.connection())?;
    let values = profile
        .as_ref()
        .map_or_else(ProfileFormValues::blank, ProfileFormValues::from);
    let subtitle = profile.as_ref().map_or_else(
        || "aucun profil : tout le reste en dépend".to_string(),
        |p| format!("{} — SIREN {}", p.name, p.siren),
    );
    let today = freeflow_core::clock::today_local();
    Ok(html! {
        (view_head(ViewId::Societe, &subtitle))
        div class="panel bordered" {
            div class="detail-note" {
                "Ce que FreeFlow sait de votre société : factures, calendrier fiscal, clôture et \
                 documents en dépendent. Modifiable à tout moment (vu le " (format_date(today)) ")."
            }
            (profile_form("/societe", &values, &ProfileFormErrors::default(), None, "Enregistrer"))
        }
    })
}
