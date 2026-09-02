//! Export des données de la liasse fiscale : les cases principales des formulaires 2065
//! (déclaration de résultats IS) et 2033 (régime simplifié — 2033-B compte de résultat, et
//! depuis le lot 31 le bilan 2033-A dérivé du grand livre), en structure sérialisable JSON —
//! **pas** le PDF Cerfa officiel. Le dépôt réel passe par EDI-TDFC (format XML propriétaire
//! DGFiP, via partenaire agréé), hors périmètre : cet export sert à transmettre les chiffres à
//! l'expert-comptable qui télédéclare.

use freeflow_core::company::CompanyProfile;
use freeflow_core::fiscal_year::FiscalYearRecord;
use freeflow_core::ledger::BalanceSheet;
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

/// Les cases du bilan simplifié 2033-A-SD : à l'actif la colonne brut et, si elle n'est pas
/// vide, la colonne amortissements de chaque rubrique, puis les totaux ; au passif chaque
/// rubrique et les totaux. Les cases à zéro sont omises, comme sur un formulaire.
fn balance_sheet_entries(sheet: &BalanceSheet) -> Vec<LiasseEntry> {
    let mut entries = Vec::new();
    let mut push = |case: &'static str, label: &'static str, cents: i64| {
        if cents != 0 {
            entries.push(LiasseEntry {
                form: "2033-A",
                case,
                label,
                amount_cents: cents,
            });
        }
    };
    for a in &sheet.assets {
        push(a.case_gross, a.label, a.gross.cents());
        push(a.case_depreciation, a.label, a.depreciation.cents());
    }
    push(
        "044",
        "Total I — actif immobilisé (net)",
        sheet.fixed_assets_net.cents(),
    );
    push(
        "096",
        "Total II — actif circulant (net)",
        sheet.current_assets_net.cents(),
    );
    push(
        "110",
        "Total général de l'actif (brut)",
        sheet.total_assets_gross.cents(),
    );
    push(
        "112",
        "Total général de l'actif (net)",
        sheet.total_assets_net.cents(),
    );
    for l in &sheet.liabilities {
        push(l.case, l.label, l.amount.cents());
    }
    push(
        "142",
        "Total I — capitaux propres",
        sheet.total_equity.cents(),
    );
    push(
        "154",
        "Total II — provisions",
        sheet.total_provisions.cents(),
    );
    push(
        "169",
        "dont comptes courants d'associés",
        sheet.shareholder_current_accounts.cents(),
    );
    push(
        "180",
        "Total général du passif",
        sheet.total_liabilities.cents(),
    );
    entries
}

/// Construit l'export de liasse depuis le snapshot figé d'un exercice clos, et le bilan dérivé
/// de son grand livre (`balance_sheet`) s'il est fourni.
#[must_use]
pub fn liasse_export(
    profile: &CompanyProfile,
    year: &FiscalYearRecord,
    balance_sheet: Option<&BalanceSheet>,
) -> LiasseExport {
    let mut entries = vec![
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
    if let Some(sheet) = balance_sheet {
        entries.extend(balance_sheet_entries(sheet));
    }
    LiasseExport {
        company: profile.name.clone(),
        siren: profile.siren.to_string(),
        period_start: year.starts_on,
        period_end: year.ends_on,
        approved: year.is_approved(),
        entries,
        note: "Export indicatif des cases principales (2065, 2033-A, 2033-B) à destination de \
               l'expert-comptable — le bilan 2033-A est dérivé du grand livre (sans \
               amortissement, provision ni régularisation) ; le dépôt réel de la liasse passe \
               par EDI-TDFC, hors périmètre de FreeFlow.",
    }
}
