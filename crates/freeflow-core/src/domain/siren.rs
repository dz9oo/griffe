//! Identifiants d'entreprise français : SIREN (clé de Luhn) et numéro de TVA intracommunautaire
//! (clé de contrôle modulo 97 pour la France).

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Un identifiant SIREN (9 chiffres), validé par sa clé de Luhn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Siren([u8; 9]);

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SirenError {
    #[error("le SIREN doit contenir exactement 9 chiffres (reçu {0})")]
    WrongLength(usize),
    #[error("le SIREN {0} ne respecte pas la clé de Luhn")]
    InvalidChecksum(String),
}

impl Siren {
    /// Valide et construit un SIREN à partir de sa représentation textuelle (espaces tolérés).
    ///
    /// # Errors
    ///
    /// Retourne une erreur si la chaîne ne contient pas exactement 9 chiffres, ou si la clé de
    /// Luhn ne correspond pas.
    pub fn parse(input: &str) -> Result<Self, SirenError> {
        let cleaned: String = input.chars().filter(|c| !c.is_whitespace()).collect();
        if cleaned.len() != 9 || !cleaned.bytes().all(|b| b.is_ascii_digit()) {
            let digit_count = cleaned.chars().filter(char::is_ascii_digit).count();
            return Err(SirenError::WrongLength(digit_count));
        }
        let mut digits = [0u8; 9];
        for (slot, b) in digits.iter_mut().zip(cleaned.bytes()) {
            *slot = b - b'0';
        }
        if luhn_valid(&digits) {
            Ok(Self(digits))
        } else {
            Err(SirenError::InvalidChecksum(cleaned))
        }
    }

    #[must_use]
    pub const fn digits(self) -> [u8; 9] {
        self.0
    }

    /// Valeur numérique du SIREN, utilisée pour le calcul de la clé de contrôle du numéro de
    /// TVA intracommunautaire français.
    #[must_use]
    pub fn as_u32(self) -> u32 {
        self.0.iter().fold(0u32, |acc, &d| acc * 10 + u32::from(d))
    }
}

impl fmt::Display for Siren {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for d in self.0 {
            write!(f, "{d}")?;
        }
        Ok(())
    }
}

impl TryFrom<String> for Siren {
    type Error = SirenError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<Siren> for String {
    fn from(value: Siren) -> Self {
        value.to_string()
    }
}

fn luhn_valid(digits: &[u8; 9]) -> bool {
    let sum: u32 = digits
        .iter()
        .rev()
        .enumerate()
        .map(|(i, &d)| {
            let mut v = u32::from(d);
            if i % 2 == 1 {
                v *= 2;
                if v > 9 {
                    v -= 9;
                }
            }
            v
        })
        .sum();
    sum.is_multiple_of(10)
}

/// Un numéro de TVA intracommunautaire (code pays ISO + identifiant local).
///
/// Seule la clé de contrôle française (formule modulo 97) est vérifiée : chaque pays de l'UE a
/// son propre algorithme, hors périmètre tant qu'aucun besoin réel ne le justifie. Pour les
/// autres pays, seule la structure (2 lettres + 2 à 12 caractères alphanumériques) est validée.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct VatNumber {
    country: [u8; 2],
    rest: String,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum VatNumberError {
    #[error("le numéro de TVA intracommunautaire est trop court")]
    TooShort,
    #[error("format invalide : {0}")]
    InvalidFormat(String),
    #[error("le SIREN encodé dans le numéro de TVA français est invalide : {0}")]
    InvalidFrenchSiren(#[from] SirenError),
    #[error("clé de contrôle française invalide : attendue {expected:02}, reçue {actual:02}")]
    InvalidFrenchKey { expected: u32, actual: u32 },
}

impl VatNumber {
    /// Valide et construit un numéro de TVA intracommunautaire.
    ///
    /// # Errors
    ///
    /// Retourne une erreur si le format est invalide, ou si la clé de contrôle française ne
    /// correspond pas.
    pub fn parse(input: &str) -> Result<Self, VatNumberError> {
        let cleaned: String = input.chars().filter(|c| !c.is_whitespace()).collect();
        if cleaned.len() < 4 {
            return Err(VatNumberError::TooShort);
        }
        let upper = cleaned.to_uppercase();
        let (country_str, rest) = upper.split_at(2);
        let rest_len = rest.chars().count();
        if !country_str.bytes().all(|b| b.is_ascii_uppercase())
            || !(2..=12).contains(&rest_len)
            || !rest.bytes().all(|b| b.is_ascii_alphanumeric())
        {
            return Err(VatNumberError::InvalidFormat(cleaned));
        }

        let mut country = [0u8; 2];
        country.copy_from_slice(country_str.as_bytes());

        if country_str == "FR" {
            validate_french_key(rest)?;
        }

        Ok(Self {
            country,
            rest: rest.to_string(),
        })
    }

    /// Code pays ISO 3166-1 alpha-2 (ex. `"FR"`).
    ///
    /// # Panics
    ///
    /// Ne panique jamais en pratique : le code pays est toujours composé de deux lettres ASCII
    /// majuscules, garanti par [`Self::parse`].
    #[must_use]
    pub fn country(&self) -> &str {
        std::str::from_utf8(&self.country).expect("le code pays est toujours ASCII")
    }

    #[must_use]
    pub fn is_french(&self) -> bool {
        self.country() == "FR"
    }
}

fn validate_french_key(rest: &str) -> Result<(), VatNumberError> {
    // Schéma standard uniquement (2 chiffres de clé + 9 chiffres de SIREN) ; les anciens
    // schémas alphanumériques français ne sont pas vérifiés au-delà de leur structure.
    if rest.len() != 11 || !rest.bytes().all(|b| b.is_ascii_digit()) {
        return Ok(());
    }
    let (key_str, siren_str) = rest.split_at(2);
    let siren = Siren::parse(siren_str)?;
    let expected = (12 + 3 * (siren.as_u32() % 97)) % 97;
    let actual: u32 = key_str.parse().unwrap_or(u32::MAX);
    if expected == actual {
        Ok(())
    } else {
        Err(VatNumberError::InvalidFrenchKey { expected, actual })
    }
}

impl fmt::Display for VatNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.country(), self.rest)
    }
}

impl TryFrom<String> for VatNumber {
    type Error = VatNumberError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<VatNumber> for String {
    fn from(value: VatNumber) -> Self {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn valid_siren_for(prefix: [u8; 8]) -> Option<[u8; 9]> {
        (0..10u8).find_map(|check| {
            let mut digits = [0u8; 9];
            digits[..8].copy_from_slice(&prefix);
            digits[8] = check;
            luhn_valid(&digits).then_some(digits)
        })
    }

    fn digits_to_string(digits: [u8; 9]) -> String {
        digits.iter().map(|d| char::from(b'0' + d)).collect()
    }

    #[test]
    fn wrong_length_is_rejected() {
        assert_eq!(Siren::parse("12345"), Err(SirenError::WrongLength(5)));
    }

    #[test]
    fn french_vat_number_with_correct_key_parses() {
        // Formule : clé = (12 + 3 × (SIREN mod 97)) mod 97, exemple construit à la main.
        let digits = valid_siren_for([5, 5, 2, 1, 0, 0, 5, 5]).expect("clé de Luhn trouvée");
        let siren = Siren::parse(&digits_to_string(digits)).unwrap();
        let key = (12 + 3 * (siren.as_u32() % 97)) % 97;
        let vat = VatNumber::parse(&format!("FR{key:02}{siren}")).unwrap();
        assert!(vat.is_french());
    }

    #[test]
    fn french_vat_number_with_wrong_key_is_rejected() {
        let digits = valid_siren_for([5, 5, 2, 1, 0, 0, 5, 5]).expect("clé de Luhn trouvée");
        let siren = Siren::parse(&digits_to_string(digits)).unwrap();
        let correct_key = (12 + 3 * (siren.as_u32() % 97)) % 97;
        let wrong_key = (correct_key + 1) % 97;
        let err = VatNumber::parse(&format!("FR{wrong_key:02}{siren}")).unwrap_err();
        assert!(matches!(err, VatNumberError::InvalidFrenchKey { .. }));
    }

    #[test]
    fn non_french_vat_number_only_checks_structure() {
        assert!(VatNumber::parse("DE123456789").is_ok());
        assert_eq!(VatNumber::parse("D"), Err(VatNumberError::TooShort));
    }

    proptest! {
        #[test]
        fn valid_siren_round_trips(prefix in proptest::array::uniform8(0u8..10)) {
            if let Some(digits) = valid_siren_for(prefix) {
                let text = digits_to_string(digits);
                let siren = Siren::parse(&text).expect("checksum construite comme valide");
                prop_assert_eq!(siren.to_string(), text);
            }
        }

        #[test]
        fn mutated_check_digit_is_rejected(prefix in proptest::array::uniform8(0u8..10)) {
            if let Some(mut digits) = valid_siren_for(prefix) {
                digits[8] = (digits[8] + 1) % 10;
                let text = digits_to_string(digits);
                prop_assert!(Siren::parse(&text).is_err());
            }
        }
    }
}
