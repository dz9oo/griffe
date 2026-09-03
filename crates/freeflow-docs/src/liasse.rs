//! Export des données de la liasse fiscale : les cases principales des formulaires 2065
//! (déclaration de résultats IS) et 2033 (régime simplifié — 2033-B compte de résultat, et
//! depuis le lot 31 le bilan 2033-A dérivé du grand livre), en structure sérialisable JSON —
//! **pas** le PDF Cerfa officiel. Le dépôt réel passe par EDI-TDFC (format XML propriétaire
//! DGFiP, via partenaire agréé), hors périmètre : cet export sert à transmettre les chiffres à
//! l'expert-comptable qui télédéclare.

use freeflow_core::accounting::director_gross;
use freeflow_core::company::CompanyProfile;
use freeflow_core::domain::Money;
use freeflow_core::fiscal_year::FiscalYearRecord;
use freeflow_core::ledger::{BalanceSheet, Ledger};
use serde::Serialize;
use time::Date;

/// Une case de formulaire : le montant en centimes, jamais en flottant (doctrine `Money`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LiasseEntry {
    /// Formulaire d'origine (`"2065"`, `"2033-B"`).
    pub form: &'static str,
    /// Référence de case sur le formulaire.
    pub case: &'static str,
    pub label: String,
    pub amount_cents: i64,
}

/// Composition du capital social (formulaire 2033-F-SD, lot 41) : cadre I, nombre d'associés
/// et de parts détenus par des personnes morales (P1/P3) et physiques (P2/P4) ; cadre II, les
/// personnes physiques détenant au moins 10 % du capital. Une SASU n'a qu'un associé, qui
/// détient tout : renseigné dès que le profil nomme l'associé unique.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CapitalComposition {
    pub form: &'static str,
    /// Case P1 — associés personnes morales.
    pub legal_entity_shareholders: u32,
    /// Case P2 — associés personnes physiques.
    pub individual_shareholders: u32,
    /// Case P3 — parts détenues par des personnes morales.
    pub shares_held_by_legal_entities: u32,
    /// Case P4 — parts détenues par des personnes physiques (`null` si le nombre d'actions
    /// n'est pas renseigné au profil).
    pub shares_held_by_individuals: Option<u32>,
    /// Cadre II — détenteurs personnes physiques d'au moins 10 %.
    pub holders: Vec<CapitalHolder>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CapitalHolder {
    pub name: String,
    pub address: Option<String>,
    pub shares: Option<u32>,
    /// Quote-part du capital en points de base (`10000` = 100 %).
    pub percent_bps: u32,
}

impl CapitalComposition {
    fn from_profile(profile: &CompanyProfile) -> Option<Self> {
        let name = profile
            .sole_shareholder_name
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())?;
        Some(Self {
            form: "2033-F",
            legal_entity_shareholders: 0,
            individual_shareholders: 1,
            shares_held_by_legal_entities: 0,
            shares_held_by_individuals: profile.share_count,
            holders: vec![CapitalHolder {
                name: name.to_string(),
                address: profile.sole_shareholder_address.clone().or_else(|| {
                    let a = &profile.address;
                    Some(format!("{}, {} {}", a.street, a.postal_code, a.city))
                }),
                shares: profile.share_count,
                percent_bps: 10_000,
            }],
        })
    }
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
    /// Composition du capital (2033-F), `null` tant que le profil ne nomme pas l'associé unique.
    pub capital: Option<CapitalComposition>,
    /// Limite de l'export, répétée dans la donnée elle-même pour voyager avec le fichier.
    pub note: &'static str,
}

/// Les cases du bilan simplifié 2033-A-SD : à l'actif la colonne brut et, si elle n'est pas
/// vide, la colonne amortissements de chaque rubrique, puis les totaux ; au passif chaque
/// rubrique et les totaux. Les cases à zéro sont omises, comme sur un formulaire.
fn balance_sheet_entries(sheet: &BalanceSheet) -> Vec<LiasseEntry> {
    let mut entries = Vec::new();
    let mut push = |case: &'static str, label: &str, cents: i64| {
        if cents != 0 {
            entries.push(LiasseEntry {
                form: "2033-A",
                case,
                label: label.to_string(),
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

/// Les cases de suivi des déficits (lot 32) : sur le 2033-B, le déficit reporté en arrière
/// (356), les déficits antérieurs imputés (360) et le déficit fiscal de l'exercice (372) ; sur
/// le 2033-D, le relevé des déficits reportables (982 restant à reporter au titre de l'exercice
/// précédent, 983 imputés, 984 non imputés, 860 déficit de l'exercice après report en arrière,
/// 870 total restant à reporter). Cases à zéro omises, comme sur un formulaire.
fn tax_loss_entries(year: &FiscalYearRecord) -> Vec<LiasseEntry> {
    let mut entries = Vec::new();
    let mut push = |form: &'static str, case: &'static str, label: &str, cents: i64| {
        if cents != 0 {
            entries.push(LiasseEntry {
                form,
                case,
                label: label.to_string(),
                amount_cents: cents,
            });
        }
    };
    let deficit = year.deficit();
    let available_before = year.losses_available_before();
    push(
        "2033-B",
        "356",
        "Déficit de l'exercice reporté en arrière (art. 220 quinquies CGI)",
        year.carried_back.cents(),
    );
    push(
        "2033-B",
        "360",
        "Déficits antérieurs reportables imputés sur l'exercice",
        year.losses_imputed.cents(),
    );
    push(
        "2033-B",
        "372",
        "Déficit fiscal de l'exercice",
        deficit.cents(),
    );
    push(
        "2033-D",
        "982",
        "Déficits restant à reporter au titre de l'exercice précédent",
        available_before.cents(),
    );
    push(
        "2033-D",
        "983",
        "Déficits imputés",
        year.losses_imputed.cents(),
    );
    push(
        "2033-D",
        "984",
        "Déficits antérieurs non imputés, reportables sans limite de durée",
        (available_before - year.losses_imputed).cents(),
    );
    push(
        "2033-D",
        "860",
        "Déficit de l'exercice restant à reporter (après report en arrière)",
        (deficit - year.carried_back).cents(),
    );
    push(
        "2033-D",
        "870",
        "Total des déficits restant à reporter",
        year.losses_carried_forward.cents(),
    );
    entries
}

/// Construit l'export de liasse depuis le snapshot figé d'un exercice clos, et le bilan dérivé
/// de son grand livre (`balance_sheet`) s'il est fourni.
#[must_use]
pub fn liasse_export(
    profile: &CompanyProfile,
    year: &FiscalYearRecord,
    ledger: Option<&Ledger>,
) -> LiasseExport {
    // Lot 37 : les impôts et taxes (635) sortent des « autres charges externes » pour la case
    // 244 — lus dans la balance du grand livre, le snapshot ne connaissant que le total.
    let taxes = ledger.map_or(Money::ZERO, |l| {
        l.trial_balance()
            .rows
            .iter()
            .filter(|r| r.account.number.starts_with("635"))
            .map(|r| r.balance)
            .sum()
    });
    // Lot 41, vérifié sur la notice 2033-SD : 218 production vendue de services (210 est la
    // vente de marchandises), 250 salaires et traitements, 252 charges sociales, 306 IS. Le
    // brut du dirigeant vient du profil (comme le grand livre, `director_gross`), les charges
    // sociales en sont le complément dans le coût figé au snapshot ; si le profil ne s'y prête
    // plus, tout le coût va en 250.
    let period = year.period();
    let director_gross = director_gross(profile, period)
        .filter(|g| *g <= year.director_remuneration)
        .unwrap_or(year.director_remuneration);
    let director_charges = year.director_remuneration - director_gross;
    let entry = |form: &'static str, case: &'static str, label: &str, amount: Money| LiasseEntry {
        form,
        case,
        label: label.to_string(),
        amount_cents: amount.cents(),
    };
    let mut entries = vec![
        entry(
            "2033-B",
            "218",
            "Production vendue — services (HT)",
            year.revenue_ht,
        ),
        entry(
            "2033-B",
            "242",
            "Autres charges externes (nettes de TVA déductible, hors impôts et taxes)",
            year.expenses - taxes,
        ),
        entry(
            "2033-B",
            "244",
            "Impôts, taxes et versements assimilés",
            taxes,
        ),
        entry(
            "2033-B",
            "250",
            "Salaires et traitements (rémunération brute du dirigeant)",
            director_gross,
        ),
        entry(
            "2033-B",
            "252",
            "Charges sociales (cotisations sur la rémunération du dirigeant)",
            director_charges,
        ),
        entry(
            "2033-B",
            "306",
            "Impôt sur les bénéfices (barème 15 % / 25 %, arrondi à l'euro — art. 1657 CGI)",
            year.corporate_tax,
        ),
        entry(
            "2033-B",
            "310",
            "Bénéfice ou perte — résultat comptable de l'exercice (après IS)",
            year.net_result,
        ),
        entry(
            "2065",
            "C1",
            &format!(
                "Résultat fiscal (bénéfice imposable après réintégration de {} de charges non \
                 déductibles et imputation des déficits antérieurs, négatif = déficit)",
                year.non_deductible_expenses
            ),
            year.taxable_result(),
        ),
    ];
    if !year.dividends.is_zero() {
        entries.push(entry(
            "2065",
            "distributions",
            "Répartition des produits distribués — montant net des dividendes décidés au titre \
             de l'exercice (ouvre la déclaration 2777 le 15 du mois suivant leur paiement)",
            year.dividends,
        ));
    }
    entries.extend(tax_loss_entries(year));
    if let Some(ledger) = ledger {
        entries.extend(balance_sheet_entries(&ledger.balance_sheet()));
    }
    LiasseExport {
        company: profile.name.clone(),
        siren: profile.siren.to_string(),
        period_start: year.starts_on,
        period_end: year.ends_on,
        approved: year.is_approved(),
        entries,
        capital: CapitalComposition::from_profile(profile),
        note: "Export indicatif des cases principales (2065, 2033-A, 2033-B, 2033-D, 2033-F) à \
               destination de l'expert-comptable — le bilan 2033-A est dérivé du grand livre \
               (sans amortissement, provision ni régularisation) ; le dépôt réel de la liasse \
               passe par EDI-TDFC, hors périmètre de FreeFlow.",
    }
}
