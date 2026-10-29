//! Document d'échange, version 1. L'adaptateur lit le fichier. Le cœur reçoit déjà la valeur.

use serde::{Deserialize, Serialize};

use crate::app::AppError;
use crate::domain::Money;

/// Un texte `12,00` ou `12.00`. Deux décimales, sans espace, sans arrondi.
///
/// # Errors
///
/// Le texte n'est pas un nombre de centimes.
pub fn parse_montant(raw: &str) -> Result<Money, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_whitespace) {
        return Err(format!("montant illisible : {raw}"));
    }
    let normalized = trimmed.replace(',', ".");
    let Some((_, fraction)) = normalized.split_once('.') else {
        return Err(format!("montant illisible : {raw}"));
    };
    if fraction.len() != 2 {
        return Err(format!("montant illisible : {raw}"));
    }
    Money::parse_decimal(trimmed).map_err(|_| format!("montant illisible : {raw}"))
}

/// Le fichier entier. `version` doit valoir 1.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExchangeDocument {
    pub version: u32,
    /// Après un dossier incomplet, les suivants ne sont pas lus.
    #[serde(default)]
    pub stop: bool,
    #[serde(default)]
    pub dossiers: Vec<DossierFile>,
    #[serde(default)]
    pub series: Vec<SeriesFile>,
    /// Liste complète des moments, dans l'ordre. Absente : la cadence du coffre reste.
    #[serde(default)]
    pub cadence: Option<Vec<CadenceMoment>>,
}

/// Une piste et la fiche qui la porte.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DossierFile {
    /// Origine de la piste. Stable, choisie dehors.
    pub origine: String,
    /// Origine de la fiche. Absente : le Qui suffit.
    #[serde(default)]
    pub origine_fiche: Option<String>,
    pub qui: String,
    pub quoi: String,
    /// Écrase travaux, brouillon, date et contact déjà tenus dans Griffe.
    #[serde(default)]
    pub reprendre: bool,
    #[serde(default)]
    pub adresse: Option<AddressFile>,
    #[serde(default)]
    pub courriel: Option<String>,
    #[serde(default)]
    pub telephone: Option<String>,
    #[serde(default)]
    pub siren: Option<String>,
    #[serde(default)]
    pub tva: Option<String>,
    #[serde(default)]
    pub types: Option<Vec<TypeEntry>>,
    #[serde(default)]
    pub travaux: Option<String>,
    #[serde(default)]
    pub contact: Option<ContactFile>,
    #[serde(default)]
    pub probabilite: Option<u8>,
    /// `AAAA-MM-JJ`. Obligatoire à la création.
    #[serde(default)]
    pub prochaine_action: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub lignes: Option<Vec<LineFile>>,
    #[serde(default)]
    pub serie: Option<String>,
    #[serde(default)]
    pub brouillon: Option<String>,
}

/// Rue, code postal et ville ensemble. Pays absent : `FR`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddressFile {
    pub rue: String,
    pub code_postal: String,
    pub ville: String,
    #[serde(default)]
    pub pays: Option<String>,
}

/// Qui répond. Prénom et nom ensemble.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactFile {
    pub prenom: String,
    pub nom: String,
    #[serde(default)]
    pub genre: Option<String>,
    #[serde(default)]
    pub courriel: Option<String>,
    #[serde(default)]
    pub telephone: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
}

/// Un type : un nom, ou un nom avec teinte et note.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TypeEntry {
    Name(String),
    Detail(TypeDetail),
}

/// Teinte et note d'un type du catalogue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeDetail {
    pub nom: String,
    #[serde(default)]
    pub teinte: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

impl TypeEntry {
    #[must_use]
    pub fn nom(&self) -> &str {
        match self {
            Self::Name(nom) | Self::Detail(TypeDetail { nom, .. }) => nom,
        }
    }

    #[must_use]
    pub fn detail(&self) -> Option<&TypeDetail> {
        match self {
            Self::Name(_) => None,
            Self::Detail(detail) => Some(detail),
        }
    }
}

/// Une ligne d'estimation. `montant` est un texte à deux décimales.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineFile {
    pub libelle: String,
    pub montant: String,
}

/// Les mots d'une série. Les clés sont celles de la cadence du coffre.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeriesFile {
    pub nom: String,
    #[serde(default)]
    pub reprendre: bool,
    #[serde(default)]
    pub moments: Vec<SeriesMoment>,
}

/// Sujet et corps d'un moment, jetons `{{…}}` du coffre.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeriesMoment {
    pub cle: String,
    pub sujet: String,
    pub corps: String,
}

/// Un moment de la cadence partagée. Le premier écart est 0.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CadenceMoment {
    pub cle: String,
    pub libelle: String,
    pub ecart_ouvres: i64,
    #[serde(default)]
    pub sujet: Option<String>,
    #[serde(default)]
    pub corps: Option<String>,
}

/// Lit le JSON. La version autre que 1 est refusée.
///
/// # Errors
///
/// JSON illisible, ou `version` différente de 1.
pub fn document_from_str(raw: &str) -> Result<ExchangeDocument, AppError> {
    let document: ExchangeDocument = serde_json::from_str(raw)?;
    if document.version != 1 {
        let version = document.version;
        return Err(AppError::Domain(format!(
            "version d'échange inconnue : {version}"
        )));
    }
    Ok(document)
}

#[cfg(test)]
mod tests {
    use super::parse_montant;
    use crate::domain::Money;

    #[test]
    fn a_cent_amount_needs_two_decimals() {
        assert_eq!(parse_montant("1200,00"), Ok(Money::from_cents(120_000)));
        assert_eq!(parse_montant("400.50"), Ok(Money::from_cents(40_050)));
        assert!(parse_montant("12,345").is_err());
        assert!(parse_montant("1200").is_err());
        assert!(parse_montant("1200,0").is_err());
        assert!(parse_montant("1 200,00").is_err());
    }
}
