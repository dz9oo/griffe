//! Export des données de la liasse fiscale : les cases principales des formulaires 2065
//! (déclaration de résultats IS) et 2033 (régime simplifié — 2033-B compte de résultat, et
//! depuis le lot 31 le bilan 2033-A dérivé du grand livre), en structure sérialisable JSON.
//! Ce n'est pas un PDF Cerfa et ce n'est pas un EDI : on recopie, rien n'est télétransmis.
//!
//! Quand un livre est fourni, les cases du compte de résultat se lisent sur sa balance.
//! Sans livre, elles ne sont pas remplies depuis le snapshot. La case C1 du 2065 reste le
//! résultat fiscal du snapshot (`taxable_result`), pas un compte. Chaque case gardée porte
//! les centimes du livre et l'euro entier à recopier (art. 1657 CGI). Une case dont cet
//! euro est 0 est omise. Le 2033-E et le 2033-G sont déclarés néant. Pas d'autoliquidation.

use griffe_core::accounting::round_to_euro;
use griffe_core::company::CompanyProfile;
use griffe_core::domain::{AssetRow, ExpenseCategory, Money};
use griffe_core::fiscal_year::FiscalYearRecord;
use griffe_core::ledger::{BalanceSheet, Journal, Ledger, charge_account};
use serde::Serialize;
use time::Date;

/// Une case de formulaire à recopier.
///
/// `amount_cents` est le montant du livre, non arrondi. `amount_euros` est l'euro entier
/// du formulaire : `round_to_euro` (art. 1657 CGI), la fraction égale à 0,50 comptée pour 1.
/// Une case dont cet euro est 0 est omise. Les centimes d'une case gardée ne sont pas réécrits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LiasseEntry {
    /// Formulaire d'origine (`"2065"`, `"2033-A"`, `"2033-B"`, `"2033-C"`, `"2033-D"`).
    pub form: &'static str,
    /// Référence de case sur le formulaire.
    pub case: &'static str,
    pub label: String,
    /// Centimes du livre, non arrondis.
    pub amount_cents: i64,
    /// Euros entiers à recopier sur le formulaire.
    pub amount_euros: i64,
}

/// Euro entier à recopier sur le formulaire (art. 1657 CGI).
///
/// `round_to_euro(Money::from_cents(amount_cents)).cents() / 100`. Le montant du livre
/// reste dans [`LiasseEntry::amount_cents`].
fn euros_to_copy(amount_cents: i64) -> i64 {
    round_to_euro(Money::from_cents(amount_cents)).cents() / 100
}

fn push_case(
    entries: &mut Vec<LiasseEntry>,
    form: &'static str,
    case: &'static str,
    label: &str,
    amount_cents: i64,
) {
    let amount_euros = euros_to_copy(amount_cents);
    if amount_euros != 0 {
        entries.push(LiasseEntry {
            form,
            case,
            label: label.to_string(),
            amount_cents,
            amount_euros,
        });
    }
}

/// Formulaire coché néant. Le 2033-E-SD (effectifs et valeur ajoutée) et
/// le 2033-G-SD (filiales) portent la mention néant. Pas d'effectif,
/// pas de valeur ajoutée, pas de liste de filiales, pas de moteur de CVAE, même si un 641 existe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct LiasseMention {
    /// `"2033-E"` ou `"2033-G"`.
    pub form: &'static str,
    /// Toujours `"néant"`.
    pub text: &'static str,
}

const NIL_MENTIONS: [LiasseMention; 2] = [
    LiasseMention {
        form: "2033-E",
        text: "néant",
    },
    LiasseMention {
        form: "2033-G",
        text: "néant",
    },
];

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

/// Un chiffre que le snapshot de clôture et le livre ne disent pas de la même façon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LiasseDiscrepancy {
    /// « chiffre d'affaires », « coût du dirigeant », « résultat » ou « impôt sur les bénéfices ».
    pub subject: &'static str,
    /// Ce que le snapshot dit, en centimes.
    pub snapshot_cents: i64,
    /// Ce que le livre dit, en centimes.
    pub ledger_cents: i64,
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
    /// Chiffres où le snapshot et le livre divergent. Vide sans livre, ou quand ils concordent.
    /// Centimes seulement : l'écart compare le snapshot et le livre, pas le formulaire.
    pub discrepancies: Vec<LiasseDiscrepancy>,
    /// 2033-E et 2033-G, toujours néant pour cette SASU.
    pub mentions: [LiasseMention; 2],
    /// Limite de l'export, répétée dans la donnée elle-même pour voyager avec le fichier.
    pub note: &'static str,
}

/// Les cases du bilan simplifié 2033-A-SD : à l'actif la colonne brut et, si elle n'est pas
/// vide, la colonne amortissements de chaque rubrique, puis les totaux ; au passif chaque
/// rubrique et les totaux. Une case dont l'euro recopié est 0 est omise.
fn balance_sheet_entries(sheet: &BalanceSheet) -> Vec<LiasseEntry> {
    let mut entries = Vec::new();
    let mut push = |case: &'static str, label: &str, cents: i64| {
        push_case(&mut entries, "2033-A", case, label, cents);
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
/// 870 total restant à reporter). Une case dont l'euro recopié est 0 est omise.
fn tax_loss_entries(year: &FiscalYearRecord) -> Vec<LiasseEntry> {
    let mut entries = Vec::new();
    let mut push = |form: &'static str, case: &'static str, label: &str, cents: i64| {
        push_case(&mut entries, form, case, label, cents);
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

/// Tableau 2033-C (cadres I et II) : brut et amortissements par rubrique, dérivés du grand
/// livre — à-nouveaux = début, acquisitions (journal AC ou BQ sur un 2xx) = augmentations, dotations
/// (`681`) = charges d'amortissement. Pas de cession dans le domaine : diminutions à zéro.
fn asset_table_entries(ledger: &Ledger) -> Vec<LiasseEntry> {
    #[derive(Clone, Copy, Default)]
    struct Mov {
        start: i64,
        increase: i64,
        charge: i64,
    }
    let mut gross = [Mov::default(); 9];
    let mut dep = [Mov::default(); 9];
    let idx = |row: AssetRow| AssetRow::ALL.iter().position(|r| *r == row).unwrap_or(8);
    for entry in &ledger.entries {
        match entry.journal {
            Journal::Opening | Journal::Purchases | Journal::Bank => {
                for line in &entry.lines {
                    let number = line.account.number.as_ref();
                    let i = idx(AssetRow::from_account(number));
                    let cents = line.amount.cents();
                    let is_dep = number.starts_with("28") || number.starts_with("29");
                    if entry.journal == Journal::Opening {
                        if is_dep {
                            dep[i].start += -cents;
                        } else if number.starts_with('2') {
                            gross[i].start += cents;
                        }
                    } else if number.starts_with('2') && !is_dep {
                        gross[i].increase += cents;
                    }
                }
            }
            Journal::Misc => {
                if let (Some(charge), Some(counter)) = (
                    entry
                        .lines
                        .iter()
                        .find(|l| l.account.number.starts_with("681")),
                    entry.lines.iter().find(|l| {
                        l.account.number.starts_with("28") || l.account.number.starts_with("29")
                    }),
                ) {
                    let j = idx(AssetRow::from_account(counter.account.number.as_ref()));
                    dep[j].charge += charge.amount.cents();
                }
            }
            _ => {}
        }
    }
    let mut entries = Vec::new();
    let mut push = |case: &'static str, label: &str, cents: i64| {
        push_case(&mut entries, "2033-C", case, label, cents);
    };
    let mut tot_g = Mov::default();
    let mut tot_d = Mov::default();
    for (i, row) in AssetRow::ALL.iter().enumerate() {
        let g = gross[i];
        let end_g = g.start + g.increase;
        let [s, inc, dec, e] = row.gross_cases();
        push(s, &format!("{} — brut début", row.label()), g.start);
        push(inc, &format!("{} — augmentations", row.label()), g.increase);
        push(dec, &format!("{} — diminutions", row.label()), 0);
        push(e, &format!("{} — brut fin", row.label()), end_g);
        tot_g.start += g.start;
        tot_g.increase += g.increase;
        if let Some([ds, dc, dd, de]) = row.depreciation_cases() {
            let d = dep[i];
            let end_d = d.start + d.charge;
            push(
                ds,
                &format!("{} — amortissements début", row.label()),
                d.start,
            );
            push(
                dc,
                &format!("{} — dotations de l'exercice", row.label()),
                d.charge,
            );
            push(
                dd,
                &format!("{} — diminutions d'amortissements", row.label()),
                0,
            );
            push(de, &format!("{} — amortissements fin", row.label()), end_d);
            tot_d.start += d.start;
            tot_d.charge += d.charge;
        }
    }
    let [gs, gi, gd, ge] = AssetRow::GROSS_TOTAL_CASES;
    push(gs, "Total immobilisations — brut début", tot_g.start);
    push(gi, "Total immobilisations — augmentations", tot_g.increase);
    push(gd, "Total immobilisations — diminutions", 0);
    push(
        ge,
        "Total immobilisations — brut fin",
        tot_g.start + tot_g.increase,
    );
    let [ds, di, dd, de] = AssetRow::DEPRECIATION_TOTAL_CASES;
    push(ds, "Total amortissements — début", tot_d.start);
    push(di, "Total amortissements — dotations", tot_d.charge);
    push(dd, "Total amortissements — diminutions", 0);
    push(de, "Total amortissements — fin", tot_d.start + tot_d.charge);
    entries
}

/// Solde du livre (`débit − crédit`) des comptes dont le numéro commence par `prefix`.
fn signed_balance(ledger: &Ledger, prefix: &str) -> Money {
    ledger
        .trial_balance()
        .rows
        .iter()
        .filter(|row| row.account.number.starts_with(prefix))
        .map(|row| row.balance)
        .sum()
}

/// Case de produit : un crédit du livre est un montant positif.
fn product_case(ledger: &Ledger, prefix: &str) -> Money {
    -signed_balance(ledger, prefix)
}

/// Case de charge : un débit du livre est un montant positif.
fn charge_case(ledger: &Ledger, prefix: &str) -> Money {
    signed_balance(ledger, prefix)
}

/// Case 242 : les comptes de charge déjà visés par [`charge_account`], hors 635 (case 244).
fn external_charges(ledger: &Ledger) -> Money {
    ExpenseCategory::ALL
        .into_iter()
        .map(charge_account)
        .filter(|account| !account.number.starts_with("635"))
        .map(|account| charge_case(ledger, account.number.as_ref()))
        .sum()
}

fn push_income_case(
    entries: &mut Vec<LiasseEntry>,
    case: &'static str,
    label: &str,
    amount: Money,
) {
    push_case(entries, "2033-B", case, label, amount.cents());
}

fn push_gap(
    gaps: &mut Vec<LiasseDiscrepancy>,
    subject: &'static str,
    snapshot: Money,
    book: Money,
) {
    if snapshot != book {
        gaps.push(LiasseDiscrepancy {
            subject,
            snapshot_cents: snapshot.cents(),
            ledger_cents: book.cents(),
        });
    }
}

/// Cases 2033-B lues sur la balance. La case 210 (ventes de marchandises) n'est pas émise.
/// La case 230 « Autres produits » lit le compte 758. L'export garde les
/// centimes du livre et ajoute l'euro à recopier.
fn income_statement_entries(ledger: &Ledger) -> Vec<LiasseEntry> {
    let mut entries = Vec::new();
    push_income_case(
        &mut entries,
        "218",
        "Production vendue — services (HT)",
        product_case(ledger, "706"),
    );
    push_income_case(
        &mut entries,
        "230",
        "Autres produits",
        product_case(ledger, "758"),
    );
    push_income_case(
        &mut entries,
        "242",
        "Autres charges externes (nettes de TVA déductible, hors impôts et taxes)",
        external_charges(ledger),
    );
    push_income_case(
        &mut entries,
        "244",
        "Impôts, taxes et versements assimilés",
        charge_case(ledger, "635"),
    );
    push_income_case(
        &mut entries,
        "250",
        "Salaires et traitements",
        charge_case(ledger, "641"),
    );
    push_income_case(
        &mut entries,
        "252",
        "Charges sociales",
        charge_case(ledger, "645"),
    );
    push_income_case(
        &mut entries,
        "254",
        "Dotations aux amortissements et aux provisions",
        charge_case(ledger, "681"),
    );
    push_income_case(
        &mut entries,
        "306",
        "Impôt sur les bénéfices (compte 695 du livre)",
        charge_case(ledger, "695"),
    );
    push_income_case(
        &mut entries,
        "310",
        "Bénéfice ou perte — résultat comptable du livre (classes 6 et 7)",
        ledger.net_result(),
    );
    entries
}

fn book_discrepancies(year: &FiscalYearRecord, ledger: &Ledger) -> Vec<LiasseDiscrepancy> {
    let mut gaps = Vec::new();
    push_gap(
        &mut gaps,
        "chiffre d'affaires",
        year.revenue_ht,
        product_case(ledger, "706"),
    );
    push_gap(
        &mut gaps,
        "coût du dirigeant",
        year.director_remuneration,
        charge_case(ledger, "641") + charge_case(ledger, "645"),
    );
    push_gap(&mut gaps, "résultat", year.net_result, ledger.net_result());
    push_gap(
        &mut gaps,
        "impôt sur les bénéfices",
        year.corporate_tax,
        charge_case(ledger, "695"),
    );
    gaps
}

const NOTE: &str = "Export indicatif pour recopier les cases (2065, 2033-A, 2033-B, 2033-C, \
    2033-D, 2033-F). On recopie, rien n’est télétransmis. On recopie les euros ; les centimes \
    du livre restent dans le JSON. Pas d’EDI, pas de PDF Cerfa. Le 2033-E et le 2033-G sont \
    néant. Pas d’autoliquidation. Les cases du compte de résultat se lisent sur le livre \
    lorsqu’il est fourni ; une case dont l’euro recopié est 0 est omise. L’impôt de la case \
    306 est celui du compte 695 du livre. La case C1 du 2065 reste le résultat fiscal du \
    snapshot (taxable_result). Le résultat de clôture lit le livre. L’écart entre le snapshot \
    et le livre (chiffre d’affaires, coût du dirigeant, résultat, impôt) reste en centimes, \
    sans euro, quand les deux chiffres diffèrent.";

/// Construit l'export de liasse depuis le snapshot figé d'un exercice clos, et le bilan dérivé
/// de son grand livre s'il est fourni.
///
/// Les cases du 2033-B viennent de la balance de ce livre. Le snapshot n'en est pas la source.
/// S'il ne dit pas le même chiffre d'affaires, le même coût du dirigeant, le même résultat ou
/// le même impôt, l'écart est listé en centimes dans [`LiasseExport::discrepancies`] et le
/// snapshot n'est pas modifié. La case C1 reste [`FiscalYearRecord::taxable_result`] ; elle
/// gagne seulement l'euro à recopier. Le 2033-E et le 2033-G sont néant.
#[must_use]
pub fn liasse_export(
    profile: &CompanyProfile,
    year: &FiscalYearRecord,
    ledger: Option<&Ledger>,
) -> LiasseExport {
    let mut entries = Vec::new();
    let discrepancies = ledger.map_or_else(Vec::new, |book| {
        entries.extend(income_statement_entries(book));
        book_discrepancies(year, book)
    });
    let taxable = year.taxable_result();
    push_case(
        &mut entries,
        "2065",
        "C1",
        &format!(
            "Résultat fiscal (bénéfice imposable après réintégration de {} de charges non \
             déductibles et imputation des déficits antérieurs, négatif = déficit). Ce \
             n'est pas un compte du livre : c'est le résultat fiscal du snapshot.",
            year.non_deductible_expenses
        ),
        taxable.cents(),
    );
    entries.extend(tax_loss_entries(year));
    if let Some(ledger) = ledger {
        entries.extend(balance_sheet_entries(&ledger.balance_sheet()));
        entries.extend(asset_table_entries(ledger));
    }
    LiasseExport {
        company: profile.name.clone(),
        siren: profile.siren.to_string(),
        period_start: year.starts_on,
        period_end: year.ends_on,
        approved: year.is_approved(),
        entries,
        capital: CapitalComposition::from_profile(profile),
        discrepancies,
        mentions: NIL_MENTIONS,
        note: NOTE,
    }
}
