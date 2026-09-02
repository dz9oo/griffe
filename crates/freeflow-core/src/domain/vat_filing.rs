//! Jour limite de télédéclaration de la TVA (CA3, régime réel normal) — la grille officielle
//! (BOI-TVA-DECLA-20-20-10-10, « Date de dépôt des déclarations » ; art. 39 de l'annexe IV au
//! CGI, qui renvoie depuis le 1er janvier 2025 à l'art. D. 161-11 du code des impositions sur les
//! biens et services), et non plus un « 15 du mois » forfaitaire.
//!
//! Contrairement à ce que la feuille de route supposait, la date ne dépend **pas** du dernier
//! chiffre du SIREN. Elle dépend de trois choses :
//! - la **zone du siège** : la ville de Paris et les départements des Hauts-de-Seine, de la
//!   Seine-Saint-Denis et du Val-de-Marne (75/92/93/94) d'un côté, tous les autres départements
//!   de l'autre ;
//! - la **catégorie de redevable** : entreprise individuelle (initiale du nom), société autre
//!   qu'anonyme, société anonyme — assimilation des SAS/SASU aux SA telle que l'administration
//!   fiscale la retient —, autres redevables ;
//! - pour les sociétés de la zone Paris, les **deux premiers chiffres** du SIREN.
//!
//! | Redevable                                        | Paris/92/93/94 | Autres départements |
//! |--------------------------------------------------|----------------|---------------------|
//! | EI, nom A–H                                      | 15             | 16                  |
//! | EI, nom I–Z                                      | 17             | 19                  |
//! | Sociétés hors SA, SIREN 00–68 / 69–78 / 79–99    | 19 / 20 / 21   | 21                  |
//! | SA (SAS, SASU), SIREN 00–74 / 75–99              | 23 / 24        | 24                  |
//! | Autres redevables                                | 24             | 24                  |
//!
//! Ce module ne produit que le **jour du mois** : le report au premier jour ouvré suivant quand
//! la date tombe un samedi, un dimanche ou un jour férié est fait par `fiscal` au moment de dater
//! l'échéance, avec [`super::next_french_business_day_on_or_after`].

use serde::Serialize;

use super::Siren;

/// Zone géographique du siège social, telle que la grille officielle la distingue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum VatFilingZone {
    /// Ville de Paris et départements des Hauts-de-Seine, de la Seine-Saint-Denis et du
    /// Val-de-Marne (codes postaux commençant par 75, 92, 93 ou 94).
    ParisPetiteCouronne,
    /// Tous les autres départements.
    OtherDepartments,
}

impl VatFilingZone {
    /// Zone déduite du code postal du siège (deux premiers chiffres). Tout ce qui n'est pas
    /// 75/92/93/94 — y compris un code postal vide ou étranger — relève des « autres
    /// départements ».
    #[must_use]
    pub fn from_postal_code(postal_code: &str) -> Self {
        let digits: String = postal_code
            .chars()
            .filter(char::is_ascii_digit)
            .take(2)
            .collect();
        match digits.as_str() {
            "75" | "92" | "93" | "94" => Self::ParisPetiteCouronne,
            _ => Self::OtherDepartments,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ParisPetiteCouronne => "siège à Paris/92/93/94",
            Self::OtherDepartments => "siège hors Paris/92/93/94",
        }
    }
}

/// Catégorie de redevable au sens de la grille officielle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum VatFilerCategory {
    /// Entreprise individuelle : la date dépend de l'initiale du nom (A–H ou I–Z). Une initiale
    /// hors alphabet latin compte comme A–H (borne basse, jamais en retard).
    SoleTrader { initial: char },
    /// Toute société autre qu'anonyme (SARL, EURL, SNC, SCI, SEL…), et par défaut toute forme
    /// juridique non reconnue — c'est la tranche la plus précoce des sociétés, donc le choix
    /// prudent quand on ne sait pas.
    Company,
    /// Sociétés anonymes, auxquelles l'administration fiscale assimile les SAS et SASU.
    PublicLimitedCompany,
    /// Autres redevables (associations, collectivités, redevables occasionnels).
    Other,
}

/// Majuscules ASCII sans accents ni ponctuation : « Société par actions simplifiée » →
/// `SOCIETEPARACTIONSSIMPLIFIEE`.
fn normalize(label: &str) -> String {
    label
        .chars()
        .filter_map(|c| {
            let c = match c {
                'à' | 'â' | 'ä' | 'À' | 'Â' | 'Ä' => 'A',
                'é' | 'è' | 'ê' | 'ë' | 'É' | 'È' | 'Ê' | 'Ë' => 'E',
                'î' | 'ï' | 'Î' | 'Ï' => 'I',
                'ô' | 'ö' | 'Ô' | 'Ö' => 'O',
                'ù' | 'û' | 'ü' | 'Ù' | 'Û' | 'Ü' => 'U',
                'ç' | 'Ç' => 'C',
                other => other.to_ascii_uppercase(),
            };
            c.is_ascii_alphanumeric().then_some(c)
        })
        .collect()
}

impl VatFilerCategory {
    /// Catégorie déduite de la forme juridique libre du profil (« SASU », « EURL », « Société
    /// anonyme »…) et, pour une entreprise individuelle, du nom (dont seule l'initiale compte).
    #[must_use]
    pub fn from_legal_form(legal_form: &str, name: &str) -> Self {
        let form = normalize(legal_form);
        let is_sole_trader = matches!(form.as_str(), "EI" | "EIRL")
            || form.contains("INDIVIDUEL")
            || form.contains("MICRO")
            || form.contains("AUTOENTREPRENEUR");
        if is_sole_trader {
            let initial = normalize(name)
                .chars()
                .find(char::is_ascii_alphabetic)
                .unwrap_or('A');
            return Self::SoleTrader { initial };
        }
        if matches!(form.as_str(), "SA" | "SAS" | "SASU")
            || form.contains("ANONYME")
            || form.contains("ACTIONSSIMPLIFIEE")
        {
            return Self::PublicLimitedCompany;
        }
        if form.starts_with("ASSO") || form.contains("COLLECTIVITE") {
            return Self::Other;
        }
        Self::Company
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SoleTrader { .. } => "entreprise individuelle",
            Self::Company => "société hors SA",
            Self::PublicLimitedCompany => "SA/SAS",
            Self::Other => "autre redevable",
        }
    }
}

/// Règle de date CA3 applicable à une entreprise : ses deux déterminants et le jour du mois qui
/// en résulte (toujours dans `15..=24`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Ca3FilingRule {
    pub zone: VatFilingZone,
    pub category: VatFilerCategory,
    /// Deux premiers chiffres du SIREN (`00..=99`), le seul morceau du numéro qui compte.
    pub siren_leading_pair: u8,
    /// Jour du mois de dépôt, avant report éventuel au jour ouvré suivant.
    pub day: u8,
}

impl Ca3FilingRule {
    /// Jour le plus précoce de toute la grille : la borne basse retenue quand le profil
    /// d'entreprise n'est pas renseigné (on ne peut pas être en retard en visant le 15).
    pub const EARLIEST_DAY: u8 = 15;

    /// Dérive la règle depuis les données du profil d'entreprise.
    #[must_use]
    pub fn derive(legal_form: &str, name: &str, siren: Siren, postal_code: &str) -> Self {
        let zone = VatFilingZone::from_postal_code(postal_code);
        let category = VatFilerCategory::from_legal_form(legal_form, name);
        let siren_leading_pair = siren.leading_pair();
        Self {
            zone,
            category,
            siren_leading_pair,
            day: Self::filing_day(zone, category, siren_leading_pair),
        }
    }

    /// La grille officielle elle-même (voir le tableau en tête de module).
    #[must_use]
    pub const fn filing_day(
        zone: VatFilingZone,
        category: VatFilerCategory,
        siren_leading_pair: u8,
    ) -> u8 {
        use VatFilerCategory::{Company, Other, PublicLimitedCompany, SoleTrader};
        use VatFilingZone::{OtherDepartments, ParisPetiteCouronne};
        match (category, zone) {
            (SoleTrader { initial }, ParisPetiteCouronne) => {
                if initial <= 'H' {
                    15
                } else {
                    17
                }
            }
            (SoleTrader { initial }, OtherDepartments) => {
                if initial <= 'H' {
                    16
                } else {
                    19
                }
            }
            (Company, ParisPetiteCouronne) => match siren_leading_pair {
                0..=68 => 19,
                69..=78 => 20,
                _ => 21,
            },
            (Company, OtherDepartments) => 21,
            (PublicLimitedCompany, ParisPetiteCouronne) => {
                if siren_leading_pair <= 74 {
                    23
                } else {
                    24
                }
            }
            (PublicLimitedCompany | Other, _) => 24,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn siren(s: &str) -> Siren {
        Siren::parse(s).unwrap()
    }

    #[test]
    fn a_sasu_in_paris_files_on_the_23rd_or_24th_by_siren_leading_pair() {
        // 552 100 554 commence par 55 → tranche 00–74 → le 23.
        let rule = Ca3FilingRule::derive("SASU", "Argon Digital", siren("552100554"), "75002");
        assert_eq!(rule.category, VatFilerCategory::PublicLimitedCompany);
        assert_eq!(rule.zone, VatFilingZone::ParisPetiteCouronne);
        assert_eq!(rule.siren_leading_pair, 55);
        assert_eq!(rule.day, 23);
        // 732 829 320 commence par 73 → encore 00–74 ; 910 000 009 commence par 91 → 75–99 → le 24.
        assert_eq!(
            Ca3FilingRule::derive("SAS", "X", siren("732829320"), "92100").day,
            23
        );
        assert_eq!(
            Ca3FilingRule::derive("SAS", "X", siren("910000009"), "93200").day,
            24
        );
    }

    #[test]
    fn outside_paris_the_siren_does_not_matter() {
        assert_eq!(
            Ca3FilingRule::derive("SASU", "X", siren("552100554"), "69001").day,
            24
        );
        assert_eq!(
            Ca3FilingRule::derive("EURL", "X", siren("552100554"), "31000").day,
            21
        );
        assert_eq!(
            Ca3FilingRule::derive("SARL", "X", siren("910000009"), "13001").day,
            21
        );
    }

    #[test]
    fn other_companies_in_paris_are_spread_over_19_20_and_21() {
        assert_eq!(
            Ca3FilingRule::derive("SARL", "X", siren("552100554"), "75001").day,
            19
        );
        assert_eq!(
            Ca3FilingRule::derive("EURL", "X", siren("732829320"), "94000").day,
            20
        );
        assert_eq!(
            Ca3FilingRule::derive("SNC", "X", siren("910000009"), "92000").day,
            21
        );
    }

    #[test]
    fn sole_traders_depend_on_the_name_initial_and_zone() {
        let paris = |name: &str| Ca3FilingRule::derive("EI", name, siren("552100554"), "75010");
        assert_eq!(paris("Hugo Martin").day, 15);
        assert_eq!(paris("Éric Martin").day, 15, "É est déaccentué en E");
        assert_eq!(paris("Inès Martin").day, 17);
        let lyon = |name: &str| {
            Ca3FilingRule::derive("Entreprise individuelle", name, siren("552100554"), "69003")
        };
        assert_eq!(lyon("Alice Durand").day, 16);
        assert_eq!(lyon("Zoé Durand").day, 19);
        assert_eq!(
            paris("東京 Consulting").day,
            15,
            "sans initiale latine, la borne basse A–H s'applique"
        );
        assert_eq!(
            paris("12 Rue").day,
            17,
            "la première lettre compte, pas le premier caractère"
        );
    }

    #[test]
    fn legal_form_recognition_tolerates_free_text() {
        let cat = |form: &str| VatFilerCategory::from_legal_form(form, "X");
        assert_eq!(
            cat("Société anonyme"),
            VatFilerCategory::PublicLimitedCompany
        );
        assert_eq!(
            cat("Société par actions simplifiée unipersonnelle"),
            VatFilerCategory::PublicLimitedCompany
        );
        assert_eq!(cat("s.a.s."), VatFilerCategory::PublicLimitedCompany);
        assert_eq!(cat("SARL"), VatFilerCategory::Company);
        assert_eq!(cat("SELARL"), VatFilerCategory::Company);
        assert_eq!(cat("Association loi 1901"), VatFilerCategory::Other);
        assert_eq!(
            cat("micro-entreprise"),
            VatFilerCategory::SoleTrader { initial: 'X' }
        );
        assert_eq!(
            cat("forme inconnue"),
            VatFilerCategory::Company,
            "une forme non reconnue tombe dans la tranche la plus précoce des sociétés"
        );
    }

    #[test]
    fn the_zone_is_read_from_the_first_two_digits_of_the_postal_code() {
        assert_eq!(
            VatFilingZone::from_postal_code("75008"),
            VatFilingZone::ParisPetiteCouronne
        );
        assert_eq!(
            VatFilingZone::from_postal_code(" 94 300 "),
            VatFilingZone::ParisPetiteCouronne
        );
        assert_eq!(
            VatFilingZone::from_postal_code("77000"),
            VatFilingZone::OtherDepartments,
            "la grande couronne (77/78/91/95) relève des autres départements"
        );
        assert_eq!(
            VatFilingZone::from_postal_code(""),
            VatFilingZone::OtherDepartments
        );
    }

    #[test]
    fn every_cell_of_the_grid_is_between_15_and_24() {
        use VatFilerCategory::{Company, Other, PublicLimitedCompany, SoleTrader};
        use VatFilingZone::{OtherDepartments, ParisPetiteCouronne};
        for zone in [ParisPetiteCouronne, OtherDepartments] {
            for category in [
                SoleTrader { initial: 'A' },
                SoleTrader { initial: 'Z' },
                Company,
                PublicLimitedCompany,
                Other,
            ] {
                for pair in 0..=99u8 {
                    let day = Ca3FilingRule::filing_day(zone, category, pair);
                    assert!((Ca3FilingRule::EARLIEST_DAY..=24).contains(&day));
                }
            }
        }
    }
}
