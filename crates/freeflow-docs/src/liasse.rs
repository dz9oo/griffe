//! Export des données de la liasse fiscale : les cases principales des formulaires 2065
//! (déclaration de résultats IS) et 2033 (régime simplifié), en structure sérialisable JSON —
//! **pas** le PDF Cerfa officiel. Le dépôt réel passe par EDI-TDFC (format XML propriétaire
//! DGFiP, via partenaire agréé), hors périmètre : cet export sert à transmettre les chiffres à
//! l'expert-comptable qui télédéclare.

use freeflow_core::company::CompanyProfile;
use freeflow_core::fiscal_year::FiscalYearRecord;
use serde::Serialize;
use time::Date;

/// Une case de formulaire : le montant en centimes, jamais en flottant (doctrine `Money`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LiasseEntry {
    /// Formulaire d'origine (`"2065"`, `"2033-B"`).
    pub form: &'static str,
    /// Référence de case sur le formulaire.
    pub case: &'static str,
    pub label: &'static str,
    pub amount_cents: i64,
}

/// L'export complet, prêt à être sérialisé en JSON pour l'expert-comptable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LiasseExport {
    pub company: String,
    pub siren: String,
    pub period_start: Date,
    pub period_end: Date,
    /// L'exercice est-il approuvé (chiffres définitifs) ou encore en projet ?
    pub approved: bool,
    pub entries: Vec<LiasseEntry>,
    /// Limite de l'export, répétée dans la donnée elle-même pour voyager avec le fichier.
    pub note: &'static str,
}

/// Construit l'export de liasse depuis le snapshot figé d'un exercice clos.
#[must_use]
pub fn liasse_export(profile: &CompanyProfile, year: &FiscalYearRecord) -> LiasseExport {
    let entries = vec![
        LiasseEntry {
            form: "2033-B",
            case: "210",
            label: "Chiffre d'affaires — prestations de services (HT)",
            amount_cents: year.revenue_ht.cents(),
        },
        LiasseEntry {
            form: "2033-B",
            case: "242",
            label: "Autres charges externes (nettes de TVA déductible)",
            amount_cents: year.expenses.cents(),
        },
        LiasseEntry {
            form: "2033-B",
            case: "250",
            label: "Rémunération du dirigeant (coût employeur)",
            amount_cents: year.director_remuneration.cents(),
        },
        LiasseEntry {
            form: "2033-B",
            case: "310",
            label: "Résultat comptable de l'exercice",
            amount_cents: year.result_before_tax.cents(),
        },
        LiasseEntry {
            form: "2065",
            case: "C1",
            label: "Bénéfice imposable au taux normal et au taux réduit",
            amount_cents: year.result_before_tax.cents(),
        },
        LiasseEntry {
            form: "2065",
            case: "IS",
            label: "Impôt sur les sociétés (barème 15 % / 25 %)",
            amount_cents: year.corporate_tax.cents(),
        },
        LiasseEntry {
            form: "2065",
            case: "NET",
            label: "Résultat net après impôt",
            amount_cents: year.net_result.cents(),
        },
    ];
    LiasseExport {
        company: profile.name.clone(),
        siren: profile.siren.to_string(),
        period_start: year.starts_on,
        period_end: year.ends_on,
        approved: year.is_approved(),
        entries,
        note: "Export indicatif des cases principales (2065/2033) à destination de \
               l'expert-comptable — le dépôt réel de la liasse passe par EDI-TDFC, hors \
               périmètre de FreeFlow.",
    }
}
