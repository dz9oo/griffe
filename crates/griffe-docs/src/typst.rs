//! Socle Typst partagé par les documents de clôture — le patron anti-injection de
//! `griffe-invoice::pdf` recopié à l'identique (voir la note de doctrine là-bas) : toute
//! donnée dynamique est injectée comme littéral de chaîne Typst (`#let v0 = "...";`) puis
//! référencée via `#v0`, jamais interpolée dans le balisage. À la différence des factures, les
//! documents de clôture sont des PDF simples : pas de `--pdf-standard a-3b`, pas de pièce
//! jointe.

use std::process::Command;

use griffe_core::company::CompanyProfile;
use time::Date;

use crate::error::DocsError;

/// Avertissement apposé au pied de chaque document : ce sont des modèles générés depuis les
/// données saisies, pas des actes garantis conformes — même statut que les échéances
/// « indicatives » du calendrier fiscal.
pub(crate) const DISCLAIMER: &str = "Document généré par Griffe à partir des données saisies — \
     modèle indicatif à faire relire (expert-comptable, conseil juridique) avant signature ou \
     transmission.";

pub(crate) fn typst_string(s: &str) -> String {
    let escaped = s
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n");
    format!("\"{escaped}\"")
}

/// Date longue en français (« 31 décembre 2026 ») pour le corps des actes — la forme ISO reste
/// celle des données et des noms de fichiers.
pub(crate) fn fr_date(date: Date) -> String {
    let month = match date.month() {
        time::Month::January => "janvier",
        time::Month::February => "février",
        time::Month::March => "mars",
        time::Month::April => "avril",
        time::Month::May => "mai",
        time::Month::June => "juin",
        time::Month::July => "juillet",
        time::Month::August => "août",
        time::Month::September => "septembre",
        time::Month::October => "octobre",
        time::Month::November => "novembre",
        time::Month::December => "décembre",
    };
    format!("{} {month} {}", date.day(), date.year())
}

/// Accumule des déclarations `#let nom = "valeur";` et retourne leur nom — voir
/// `griffe-invoice::pdf::Bindings`, recopié.
pub(crate) struct Bindings {
    pub(crate) declarations: String,
    counter: usize,
}

impl Bindings {
    pub(crate) fn new() -> Self {
        Self {
            declarations: String::new(),
            counter: 0,
        }
    }

    pub(crate) fn bind(&mut self, value: &str) -> String {
        let name = format!("v{}", self.counter);
        self.counter += 1;
        self.declarations
            .push_str(&format!("#let {name} = {};\n", typst_string(value)));
        name
    }
}

/// En-tête d'identité de la société, partagé par tous les actes : dénomination, forme et
/// capital, SIREN/RCS, siège. Renvoie le fragment de balisage à insérer (les valeurs passent
/// toutes par [`Bindings::bind`]).
pub(crate) fn company_header(b: &mut Bindings, profile: &CompanyProfile) -> String {
    let name_v = b.bind(&profile.name);
    let form_line = profile.share_capital.map_or_else(
        || profile.legal_form.clone(),
        |capital| format!("{} au capital de {capital}", profile.legal_form),
    );
    let form_v = b.bind(&form_line);
    let mut registration = format!("SIREN {}", profile.siren);
    if let Some(city) = &profile.rcs_city {
        registration = format!("{registration} — RCS {city} {}", profile.siren);
    }
    let registration_v = b.bind(&registration);
    let seat_v = b.bind(&format!(
        "Siège social : {}, {} {}, {}",
        profile.address.street,
        profile.address.postal_code,
        profile.address.city,
        profile.address.country
    ));
    format!(
        "#text(weight: \"bold\", size: 14pt)[#{name_v}] \\\n#{form_v} \\\n#{registration_v} \
         \\\n#{seat_v}\n"
    )
}

/// Compile `source` en PDF via le binaire `typst` — même mécanique que
/// `griffe-invoice::render_pdf`, sans profil PDF/A ni fichier joint.
pub(crate) fn compile(source: &str) -> Result<Vec<u8>, DocsError> {
    let workdir = tempfile::tempdir()?;
    std::fs::write(workdir.path().join("doc.typ"), source)?;
    let output_path = workdir.path().join("doc.pdf");

    let output = Command::new("typst")
        .arg("compile")
        .arg("doc.typ")
        .arg(&output_path)
        .current_dir(workdir.path())
        .output()?;

    if !output.status.success() {
        return Err(DocsError::TypstFailed {
            code: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }

    Ok(std::fs::read(&output_path)?)
}
