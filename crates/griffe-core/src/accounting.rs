//! Calcul du résultat comptable et de l'impôt sur les sociétés — le premier calcul de *montant*
//! fiscal du projet (lot 18), là où `fiscal.rs` ne produisait jusqu'ici que des *dates*.
//!
//! **Périmètre volontairement simplifié, et indicatif.** Le résultat est
//! `produits (factures émises HT) − charges (dépenses + rémunération du dirigeant)`, **sans
//! amortissements, provisions, variation de stock, ni produits/charges constatés d'avance**. Il
//! converge donc avec le résultat comptable réel pour une activité de prestation sans
//! immobilisations, et s'en écarte dès qu'il y en a. Ce module ne remplace pas la liasse produite
//! par un expert-comptable via EDI-TDFC : il sert au pilotage et à la production des documents de
//! synthèse. Tout est en centimes entiers (`Money`), jamais en flottant.
//!
//! **Déficits (lot 32).** Le résultat *fiscal* n'est pas le résultat comptable : les déficits des
//! exercices antérieurs s'imputent sur le bénéfice avant IS (report en avant, art. 209 I CGI,
//! [`impute_prior_losses`]), et le déficit d'un exercice peut, sur option, s'imputer sur le
//! bénéfice de l'exercice précédent contre une créance d'IS (report en arrière,
//! art. 220 quinquies CGI, [`carry_back`]). Les deux règles sont *pures* ici ; la chaîne des
//! déficits d'un exercice à l'autre vit dans [`crate::fiscal_year`].

use rusqlite::Connection;

use crate::app::AppError;
use crate::billing::{VatBreakdownLine, active_write_off_for, compute_totals, list_invoices};
use crate::company::CompanyProfile;
use crate::domain::{ExpenseId, FiscalYear, Money, VatRate};
use crate::expenses::expenses_between;
use crate::fixed_assets::list_fixed_assets;

/// Plafond de la tranche à taux réduit d'IS : 42 500 € de bénéfice.
const REDUCED_RATE_CEILING: Money = Money::from_cents(4_250_000);
/// Taux réduit d'IS (15 %) en dix-millièmes.
const REDUCED_RATE_BPS: u32 = 1_500;
/// Taux normal d'IS (25 %) en dix-millièmes.
const NORMAL_RATE_BPS: u32 = 2_500;

/// Plafond d'imputation des déficits antérieurs sur le bénéfice d'un exercice : 1 000 000 €,
/// majoré de 50 % de la fraction du bénéfice qui excède ce montant (art. 209 I al. 3 CGI,
/// BOI-IS-DEF-10-30 § 140).
pub const LOSS_CARRY_FORWARD_CAP: Money = Money::from_cents(100_000_000);
/// Part (en dix-millièmes) du bénéfice au-delà du plafond de base qui reste imputable.
const LOSS_CARRY_FORWARD_EXCESS_BPS: u32 = 5_000;
/// Plafond du déficit reportable en arrière sur le bénéfice de l'exercice précédent
/// (art. 220 quinquies I CGI, BOI-IS-DEF-20-10 § 200).
pub const LOSS_CARRY_BACK_CAP: Money = Money::from_cents(100_000_000);

/// Résultat comptable simplifié d'un exercice, et l'IS qui en découle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct AccountingResult {
    pub period: FiscalYear,
    /// Chiffre d'affaires HT : somme des factures émises sur la période (les avoirs, à lignes
    /// négatives, se soustraient naturellement).
    pub revenue_ht: Money,
    /// Charges externes : coût net des dépenses (montant − TVA déductible, récupérée par ailleurs
    /// via la CA3).
    pub expenses: Money,
    /// Coût employeur de la rémunération du président sur la période (brut × mois + cotisations
    /// patronales estimées), à titre indicatif.
    pub director_remuneration: Money,
    /// Dotations aux amortissements de l'exercice (lot 42, ligne 254 du 2033-B) : linéaires,
    /// prorata temporis, sur les immobilisations déclarées — la dépense immobilisée n'est plus
    /// dans `expenses`.
    pub depreciation: Money,
    /// Résultat **comptable** avant impôt : produits − charges.
    pub result_before_tax: Money,
    /// Charges comptabilisées mais non déductibles fiscalement (art. 39-4 CGI : amendes,
    /// dépenses somptuaires…), réintégrées au résultat fiscal — déclarées à la clôture,
    /// mentionnées au PV (art. 223 quater CGI). Lot 41.
    pub non_deductible_expenses: Money,
    /// Déficits des exercices antérieurs encore reportables à l'ouverture de l'exercice.
    pub prior_losses_available: Money,
    /// Fraction de ces déficits imputée sur le bénéfice de l'exercice (report en avant,
    /// ligne 360 du 2033-B) — nulle sur un exercice déficitaire.
    pub losses_imputed: Money,
    /// Résultat **fiscal** : `result_before_tax + non_deductible_expenses − losses_imputed`,
    /// base de l'IS (négatif = déficit fiscal de l'exercice, ligne 372 du 2033-B).
    pub taxable_result: Money,
    pub corporate_tax: Money,
    /// Déficit de l'exercice reporté en arrière sur le bénéfice de l'exercice précédent
    /// (ligne 356 du 2033-B) — nul sans option.
    pub carried_back: Money,
    /// Créance d'IS née du report en arrière (2039-SD), comptabilisée en produit (699 contre
    /// 444) : elle entre dans le résultat net.
    pub carry_back_credit: Money,
    /// `result_before_tax − corporate_tax + carry_back_credit`.
    pub net_result: Money,
}

impl AccountingResult {
    /// Déficit fiscal de l'exercice (positif), ou zéro sur un bénéfice.
    #[must_use]
    pub fn deficit(&self) -> Money {
        (-self.taxable_result).max(Money::ZERO)
    }

    /// Déficits reportables en avant *après* cet exercice : le stock antérieur non imputé, plus
    /// le déficit de l'exercice non reporté en arrière (case 870 du 2033-D).
    #[must_use]
    pub fn losses_carried_forward(&self) -> Money {
        self.prior_losses_available - self.losses_imputed + self.deficit() - self.carried_back
    }

    /// Applique l'option de report en arrière : le déficit reporté sort du stock reportable en
    /// avant, la créance d'IS entre dans le résultat net.
    #[must_use]
    pub fn with_carry_back(mut self, carry_back: CarryBack) -> Self {
        self.carried_back = carry_back.imputed;
        self.carry_back_credit = carry_back.credit;
        self.net_result = self.result_before_tax - self.corporate_tax + carry_back.credit;
        self
    }
}

/// Imputation des déficits antérieurs sur le bénéfice d'un exercice (report en avant).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct LossImputation {
    /// Fraction du stock imputée sur le bénéfice.
    pub imputed: Money,
    /// Résultat fiscal après imputation (négatif = déficit de l'exercice).
    pub taxable_result: Money,
    /// Stock antérieur restant reportable après imputation.
    pub remaining: Money,
}

/// Plafond d'imputation des déficits antérieurs sur un bénéfice donné : 1 000 000 € + 50 % de
/// l'excédent (BOI-IS-DEF-10-30 § 160 : un bénéfice de 1 500 000 € admet 1 250 000 €).
#[must_use]
pub fn loss_carry_forward_ceiling(profit: Money) -> Money {
    if profit <= LOSS_CARRY_FORWARD_CAP {
        return profit.max(Money::ZERO);
    }
    LOSS_CARRY_FORWARD_CAP
        + (profit - LOSS_CARRY_FORWARD_CAP).apply_rate_bps(LOSS_CARRY_FORWARD_EXCESS_BPS)
}

/// Impute le stock `available` de déficits antérieurs sur `result_before_tax`, dans la limite du
/// plafond (art. 209 I al. 3 CGI). Un résultat nul ou déficitaire n'impute rien : le stock
/// reste entier et le déficit de l'exercice s'y ajoutera (voir
/// [`AccountingResult::losses_carried_forward`]).
#[must_use]
pub fn impute_prior_losses(result_before_tax: Money, available: Money) -> LossImputation {
    let available = available.max(Money::ZERO);
    if result_before_tax.cents() <= 0 {
        return LossImputation {
            imputed: Money::ZERO,
            taxable_result: result_before_tax,
            remaining: available,
        };
    }
    let imputed = available.min(loss_carry_forward_ceiling(result_before_tax));
    LossImputation {
        imputed,
        taxable_result: result_before_tax - imputed,
        remaining: available - imputed,
    }
}

/// Le bénéfice de l'exercice précédent sur lequel un déficit peut être reporté en arrière.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CarryBackBase {
    /// Résultat fiscal de l'exercice précédent (bénéfice imposé, après imputation de ses propres
    /// déficits antérieurs — rubrique « résultat fiscal » du cadre C du 2065).
    pub taxable_profit: Money,
    /// Distributions prélevées sur ce bénéfice (dividendes décidés au titre de cet exercice) :
    /// elles en sortent (art. 220 quinquies I, BOI-IS-DEF-20-10 § 120, ligne 3 du 2039-SD).
    pub distributed: Money,
}

/// Le report en arrière d'un déficit : la fraction imputée et la créance d'IS qui en naît.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct CarryBack {
    /// Déficit imputé sur le bénéfice de l'exercice précédent.
    pub imputed: Money,
    /// Créance sur l'État : l'IS effectivement acquitté à raison du bénéfice imputé.
    pub credit: Money,
}

impl CarryBack {
    pub const NONE: Self = Self {
        imputed: Money::ZERO,
        credit: Money::ZERO,
    };
}

/// Reporte `deficit` en arrière sur `base` (art. 220 quinquies CGI, notice 2039-SD, ligne 13) :
/// imputation à hauteur du plus faible de 1 000 000 € et du bénéfice d'imputation (bénéfice
/// fiscal hors distributions), **en priorité sur la fraction soumise au taux normal, puis sur
/// celle au taux réduit** (BOI-IS-DEF-20-10 § 100) ; la créance est l'IS acquitté sur chaque
/// fraction imputée. Convention pour ventiler les distributions entre les deux fractions, que
/// le 2039-SD laisse à l'entreprise : au prorata de chaque fraction dans le bénéfice.
///
/// Exemple BOI-IS-DEF-20-10 § 210 : bénéfice 1 020 620 € (42 500 € à 15 %, 978 120 € à 25 %),
/// déficit ≥ 1 M€ → 978 120 € imputés à 25 % (244 530 €) et 21 880 € à 15 % (3 282 €).
///
/// # Panics
///
/// Ne panique jamais : les produits intermédiaires sont calculés en `i128`, et un prorata est
/// borné par sa base.
#[must_use]
pub fn carry_back(deficit: Money, base: CarryBackBase) -> CarryBack {
    let profit = base.taxable_profit;
    if deficit.cents() <= 0 || profit.cents() <= 0 {
        return CarryBack::NONE;
    }
    let imputable = (profit - base.distributed.max(Money::ZERO)).max(Money::ZERO);
    let capped = deficit.min(LOSS_CARRY_BACK_CAP).min(imputable);
    if capped.is_zero() {
        return CarryBack::NONE;
    }
    // Double calcul (§ 190) : la part au taux réduit du bénéfice, ramenée au prorata du
    // bénéfice d'imputation (les distributions en sortent proportionnellement).
    let reduced_fraction = profit.min(REDUCED_RATE_CEILING);
    let reduced_base = Money::from_cents(
        i64::try_from(
            i128::from(reduced_fraction.cents()) * i128::from(imputable.cents())
                / i128::from(profit.cents()),
        )
        .unwrap_or(reduced_fraction.cents()),
    );
    let normal_base = imputable - reduced_base;
    let on_normal = capped.min(normal_base);
    let on_reduced = (capped - on_normal).min(reduced_base);
    CarryBack {
        imputed: on_normal + on_reduced,
        credit: on_normal.apply_rate_bps(NORMAL_RATE_BPS)
            + on_reduced.apply_rate_bps(REDUCED_RATE_BPS),
    }
}

/// Impôt sur les sociétés d'un bénéfice, barème 2025+ : **15 %** jusqu'à 42 500 € (PME éligible —
/// hypothèse : CA < 10 M€, capital entièrement libéré détenu ≥ 75 % par des personnes physiques,
/// non vérifiée ici), **25 %** au-delà. Un résultat nul ou déficitaire ne produit aucun IS.
///
/// # Panics
///
/// Ne panique jamais : arithmétique `Money` bornée (tranches ≤ résultat, résultat un `i64`).
#[must_use]
pub fn corporate_income_tax(result_before_tax: Money) -> Money {
    if result_before_tax.cents() <= 0 {
        return Money::ZERO;
    }
    // Art. 1657 CGI (lot 41) : les bases d'imposition et les cotisations d'impôts directs sont
    // arrondies à l'euro le plus proche, la fraction d'euro égale à 0,50 comptée pour 1 — le
    // résultat fiscal du 2065 s'écrit en euros, l'IS aussi (1 315,96 € affiché avant ce lot pour
    // un résultat de 8 773,10 € : 1 316 € en réalité).
    let base = round_to_euro(result_before_tax);
    let reduced_base = base.min(REDUCED_RATE_CEILING);
    let mut tax = reduced_base.apply_rate_bps(REDUCED_RATE_BPS);
    if base > REDUCED_RATE_CEILING {
        let upper_base = base - REDUCED_RATE_CEILING;
        tax += upper_base.apply_rate_bps(NORMAL_RATE_BPS);
    }
    round_to_euro(tax)
}

/// Arrondi à l'euro le plus proche, 0,50 € compté pour 1 (art. 1657 CGI) ; symétrique pour un
/// montant négatif.
#[must_use]
pub fn round_to_euro(amount: Money) -> Money {
    let cents = amount.cents();
    let sign = cents.signum();
    let magnitude = cents.abs();
    let euros = (magnitude + 50) / 100;
    Money::from_cents(sign * euros * 100)
}

/// Nombre de mois entiers couverts par un exercice (12 pour un exercice plein).
fn months_in(period: FiscalYear) -> u32 {
    let start = period.start();
    let end = period.end();
    let months = (i32::from(u8::from(end.month())) - i32::from(u8::from(start.month())))
        + (end.year() - start.year()) * 12
        + 1;
    u32::try_from(months.max(0)).unwrap_or(0)
}

/// Rémunération brute du président sur `period` : brut mensuel × nombre de mois entiers de
/// l'exercice — `None` s'il n'est pas rémunéré. Partagée avec le grand livre dérivé
/// (`crate::ledger`), qui sépare le brut (641) des cotisations patronales (645).
#[must_use]
pub fn director_gross(profile: &CompanyProfile, period: FiscalYear) -> Option<Money> {
    let gross = profile.director_monthly_gross?;
    let months = months_in(period);
    Some(gross.multiply_by_quantity(f64::from(months)))
}

/// Coût employeur indicatif de la rémunération du président sur `period` : brut mensuel × nombre
/// de mois, majoré des cotisations patronales estimées par le ratio du profil.
#[must_use]
pub fn director_cost(profile: &CompanyProfile, period: FiscalYear) -> Money {
    let Some(gross_annual) = director_gross(profile, period) else {
        return Money::ZERO;
    };
    match profile.director_charge_ratio_bps {
        Some(ratio_bps) => gross_annual + gross_annual.apply_rate_bps(ratio_bps),
        None => gross_annual,
    }
}

/// Calcule le résultat comptable simplifié et l'IS d'un exercice, les déficits antérieurs
/// reportables étant lus dans la chaîne des exercices clos
/// ([`crate::fiscal_year::tax_losses_available`]) — sans option de report en arrière, qui est
/// une décision de clôture ([`crate::fiscal_year::CloseFiscalYear`]).
///
/// # Errors
///
/// Erreur de lecture SQLite.
pub fn compute_result(
    conn: &Connection,
    period: FiscalYear,
    profile: &CompanyProfile,
) -> Result<AccountingResult, AppError> {
    let prior_losses = crate::fiscal_year::tax_losses_available(conn, period.start())?;
    compute_result_with_losses(conn, period, profile, prior_losses)
}

/// [`compute_result`] avec un stock de déficits antérieurs donné par l'appelant.
///
/// # Errors
///
/// Erreur de lecture SQLite.
pub fn compute_result_with_losses(
    conn: &Connection,
    period: FiscalYear,
    profile: &CompanyProfile,
    prior_losses_available: Money,
) -> Result<AccountingResult, AppError> {
    compute_result_with(conn, period, profile, prior_losses_available, Money::ZERO)
}

/// [`compute_result_with_losses`] avec, en plus, les charges non déductibles à réintégrer au
/// résultat fiscal (lot 41) : le résultat comptable ne change pas, la base de l'IS augmente
/// d'autant — les déficits antérieurs s'imputent sur la base réintégrée.
///
/// # Errors
///
/// Erreur de lecture SQLite.
pub fn compute_result_with(
    conn: &Connection,
    period: FiscalYear,
    profile: &CompanyProfile,
    prior_losses_available: Money,
    non_deductible_expenses: Money,
) -> Result<AccountingResult, AppError> {
    let revenue_ht: Money = list_invoices(conn)?
        .iter()
        .filter(|inv| period.contains(inv.issued_on))
        .map(|inv| compute_totals(&inv.lines).subtotal_ht)
        .sum();

    // Lot 42 : une dépense immobilisée n'est plus une charge (elle entre à l'actif) ; sa
    // dotation l'est. Les charges constatées d'avance reprises au bilan d'ouverture (486) sont
    // extournées au premier jour de l'exercice qui s'ouvre sur ce bilan : une charge de plus.
    let assets = crate::fixed_assets::list_fixed_assets(conn)?;
    let immobilized = crate::fixed_assets::assets_by_expense(&assets);
    let prepaid: Money = crate::opening_balance::opening_balance(conn)?
        .filter(|o| o.balance.opens_on == period.start())
        .map_or(Money::ZERO, |o| prepaid_expenses_of(&o.balance.lines));
    let expenses: Money = expenses_between(conn, period.start(), period.end())?
        .iter()
        .filter(|e| !immobilized.contains_key(&e.id))
        .map(|e| e.amount - e.vat_deductible)
        .sum::<Money>()
        + prepaid;
    let depreciation = crate::fixed_assets::depreciation_of(&assets, period);

    let director_remuneration = director_cost(profile, period);

    let result_before_tax = revenue_ht - expenses - depreciation - director_remuneration;
    let non_deductible_expenses = non_deductible_expenses.max(Money::ZERO);
    let imputation = impute_prior_losses(
        result_before_tax + non_deductible_expenses,
        prior_losses_available,
    );
    let corporate_tax = corporate_income_tax(imputation.taxable_result);
    let net_result = result_before_tax - corporate_tax;

    Ok(AccountingResult {
        period,
        revenue_ht,
        expenses,
        director_remuneration,
        depreciation,
        result_before_tax,
        non_deductible_expenses,
        prior_losses_available: prior_losses_available.max(Money::ZERO),
        losses_imputed: imputation.imputed,
        taxable_result: imputation.taxable_result,
        corporate_tax,
        carried_back: Money::ZERO,
        carry_back_credit: Money::ZERO,
        net_result,
    })
}

/// Les charges constatées d'avance reprises au bilan d'ouverture (comptes 486, au débit) :
/// des charges de l'exercice précédent payées d'avance pour celui-ci, extournées au premier
/// jour (lot 42). Pure, partagée par le résultat et le grand livre.
#[must_use]
pub fn prepaid_expenses_of(lines: &[crate::domain::OpeningBalanceLine]) -> Money {
    lines
        .iter()
        .filter(|l| l.account.starts_with("486") && l.side == crate::domain::Side::Debit)
        .map(|l| l.amount)
        .sum()
}

/// Déclaration de TVA d'une période (CA3) : TVA collectée sur les factures émises, TVA déductible
/// sur les dépenses, et le solde à reverser (positif) ou le crédit de TVA (négatif).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct VatReturn {
    #[serde(with = "crate::domain::serde_date::date")]
    pub period_start: time::Date,
    #[serde(with = "crate::domain::serde_date::date")]
    pub period_end: time::Date,
    pub collected: Money,
    pub deductible: Money,
    /// `collected − deductible` : à reverser si positif, crédit de TVA reportable si négatif.
    pub due: Money,
    /// Bases HT et TVA collectée, un élément par taux réellement présent (lot 56, cases CA3).
    pub collected_by_rate: Vec<VatBreakdownLine>,
    /// TVA déductible des dépenses immobilisées (CA3 ligne 19).
    pub deductible_assets: Money,
    /// TVA déductible des autres biens et services (CA3 ligne 20).
    pub deductible_other: Money,
    /// Total HT des factures émises de la période (CA3 cadre A, prestations).
    pub taxable_ht: Money,
}

/// TVA due sur une période `[start, end]` (bornes inclusives) — premier calcul de montant de la
/// déclaration CA3 du projet, depuis les factures émises et les dépenses de la période.
///
/// # Errors
///
/// Erreur de lecture SQLite.
pub fn vat_due_for_period(
    conn: &Connection,
    start: time::Date,
    end: time::Date,
) -> Result<VatReturn, AppError> {
    let mut invoices = Vec::new();
    for inv in list_invoices(conn)? {
        if inv.issued_on < start || inv.issued_on > end {
            continue;
        }
        if let Some(write_off) = active_write_off_for(conn, inv.id)?
            && !write_off.recovers_vat
        {
            continue;
        }
        invoices.push(inv);
    }
    let totals: Vec<_> = invoices
        .iter()
        .map(|inv| compute_totals(&inv.lines))
        .collect();
    let collected: Money = totals.iter().map(|t| t.total_vat).sum();
    let taxable_ht: Money = totals.iter().map(|t| t.subtotal_ht).sum();
    let collected_by_rate: Vec<VatBreakdownLine> = VatRate::ALL
        .into_iter()
        .filter_map(|rate| {
            let taxable_amount: Money = totals
                .iter()
                .flat_map(|t| t.vat_breakdown.iter())
                .filter(|b| b.rate == rate)
                .map(|b| b.taxable_amount)
                .sum();
            let vat_amount: Money = totals
                .iter()
                .flat_map(|t| t.vat_breakdown.iter())
                .filter(|b| b.rate == rate)
                .map(|b| b.vat_amount)
                .sum();
            (!taxable_amount.is_zero() || !vat_amount.is_zero()).then_some(VatBreakdownLine {
                rate,
                taxable_amount,
                vat_amount,
            })
        })
        .collect();

    let asset_expense_ids: Vec<ExpenseId> = list_fixed_assets(conn)?
        .into_iter()
        .filter_map(|a| a.expense_id)
        .collect();
    let expenses = expenses_between(conn, start, end)?;
    let mut deductible_assets = Money::ZERO;
    let mut deductible_other = Money::ZERO;
    for expense in &expenses {
        if asset_expense_ids.contains(&expense.id) {
            deductible_assets += expense.vat_deductible;
        } else {
            deductible_other += expense.vat_deductible;
        }
    }
    let deductible = deductible_assets + deductible_other;

    Ok(VatReturn {
        period_start: start,
        period_end: end,
        collected,
        deductible,
        due: collected - deductible,
        collected_by_rate,
        deductible_assets,
        deductible_other,
        taxable_ht,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn is_is_zero_on_a_loss_or_break_even() {
        assert_eq!(
            corporate_income_tax(Money::from_cents(-100_000)),
            Money::ZERO
        );
        assert_eq!(corporate_income_tax(Money::ZERO), Money::ZERO);
    }

    #[test]
    fn is_applies_the_reduced_rate_up_to_the_ceiling() {
        // 42 500 € pile : 15 % = 6 375 €.
        assert_eq!(
            corporate_income_tax(Money::from_cents(4_250_000)),
            Money::from_cents(637_500)
        );
        // 10 000 € : 15 % = 1 500 €.
        assert_eq!(
            corporate_income_tax(Money::from_cents(1_000_000)),
            Money::from_cents(150_000)
        );
    }

    #[test]
    fn is_applies_the_normal_rate_above_the_ceiling() {
        // 100 000 € : 15 % × 42 500 (6 375 €) + 25 % × 57 500 (14 375 €) = 20 750 €.
        assert_eq!(
            corporate_income_tax(Money::from_cents(10_000_000)),
            Money::from_cents(2_075_000)
        );
    }

    // --- Déficits : report en avant plafonné, report en arrière (lot 32). ---

    #[test]
    fn carry_forward_is_capped_at_one_million_plus_half_the_excess() {
        // BOI-IS-DEF-10-30 § 160 : déficit de 2 M€, bénéfice de 1,5 M€ → 1 250 000 € imputés,
        // 250 000 € taxables, 750 000 € encore reportables.
        let r = impute_prior_losses(
            Money::from_cents(150_000_000),
            Money::from_cents(200_000_000),
        );
        assert_eq!(r.imputed, Money::from_cents(125_000_000));
        assert_eq!(r.taxable_result, Money::from_cents(25_000_000));
        assert_eq!(r.remaining, Money::from_cents(75_000_000));
        // § 170 : sous le plafond, le déficit s'impute en entier.
        let r = impute_prior_losses(
            Money::from_cents(150_000_000),
            Money::from_cents(90_000_000),
        );
        assert_eq!(r.imputed, Money::from_cents(90_000_000));
        assert_eq!(r.remaining, Money::ZERO);
    }

    #[test]
    fn a_loss_year_imputes_nothing_and_keeps_the_stock_whole() {
        let r = impute_prior_losses(Money::from_cents(-80_000), Money::from_cents(300_000));
        assert_eq!(r.imputed, Money::ZERO);
        assert_eq!(r.taxable_result, Money::from_cents(-80_000));
        assert_eq!(r.remaining, Money::from_cents(300_000));
        // Un stock négatif (impossible par validation, mais reçu tel quel) vaut zéro.
        let r = impute_prior_losses(Money::from_cents(100), Money::from_cents(-5));
        assert_eq!(r.imputed, Money::ZERO);
        assert_eq!(r.taxable_result, Money::from_cents(100));
    }

    #[test]
    fn carry_back_imputes_the_normal_rate_fraction_first() {
        // BOI-IS-DEF-20-10 § 210 : 42 500 € à 15 % + 978 120 € à 25 %, déficit ≥ 1 M€ →
        // 978 120 € imputés à 25 % (244 530 €) puis 21 880 € à 15 % (3 282 €).
        let cb = carry_back(
            Money::from_cents(150_000_000),
            CarryBackBase {
                taxable_profit: Money::from_cents(102_062_000),
                distributed: Money::ZERO,
            },
        );
        assert_eq!(cb.imputed, Money::from_cents(100_000_000));
        assert_eq!(cb.credit, Money::from_cents(24_781_200));

        // Un petit bénéfice entièrement au taux réduit : créance à 15 % du déficit imputé.
        let cb = carry_back(
            Money::from_cents(80_000),
            CarryBackBase {
                taxable_profit: Money::from_cents(537_500),
                distributed: Money::ZERO,
            },
        );
        assert_eq!(cb.imputed, Money::from_cents(80_000));
        assert_eq!(cb.credit, Money::from_cents(12_000));
    }

    #[test]
    fn carry_back_excludes_distributed_profit_and_needs_a_deficit_and_a_profit() {
        // 5 375 € de bénéfice dont 5 000 € distribués : 375 € imputables seulement.
        let cb = carry_back(
            Money::from_cents(80_000),
            CarryBackBase {
                taxable_profit: Money::from_cents(537_500),
                distributed: Money::from_cents(500_000),
            },
        );
        assert_eq!(cb.imputed, Money::from_cents(37_500));
        assert_eq!(cb.credit, Money::from_cents(5_625));
        // Tout distribué, ou pas de déficit, ou pas de bénéfice : rien.
        let base = CarryBackBase {
            taxable_profit: Money::from_cents(537_500),
            distributed: Money::from_cents(600_000),
        };
        assert_eq!(carry_back(Money::from_cents(80_000), base), CarryBack::NONE);
        let base = CarryBackBase {
            taxable_profit: Money::from_cents(537_500),
            distributed: Money::ZERO,
        };
        assert_eq!(carry_back(Money::ZERO, base), CarryBack::NONE);
        let base = CarryBackBase {
            taxable_profit: Money::from_cents(-100),
            distributed: Money::ZERO,
        };
        assert_eq!(carry_back(Money::from_cents(80_000), base), CarryBack::NONE);
    }

    proptest! {
        /// L'imputation ne dépasse ni le stock, ni le plafond, ni le bénéfice ; le résultat
        /// fiscal d'un bénéfice reste positif ou nul ; stock + déficit se conservent.
        #[test]
        fn carry_forward_never_exceeds_stock_ceiling_or_profit(
            profit in -1_000_000_000_000i64..1_000_000_000_000,
            stock in 0i64..1_000_000_000_000,
        ) {
            let (profit, stock) = (Money::from_cents(profit), Money::from_cents(stock));
            let r = impute_prior_losses(profit, stock);
            prop_assert!(r.imputed.cents() >= 0);
            prop_assert!(r.imputed <= stock);
            prop_assert!(r.imputed <= loss_carry_forward_ceiling(profit));
            prop_assert!(r.imputed <= profit.max(Money::ZERO));
            prop_assert_eq!(r.remaining + r.imputed, stock);
            prop_assert_eq!(r.taxable_result + r.imputed, profit);
            if profit.cents() > 0 {
                prop_assert!(r.taxable_result.cents() >= 0);
            }
        }

        /// La créance de report en arrière ne dépasse jamais l'IS du bénéfice d'imputation, et
        /// l'imputation reste sous le déficit, le plafond et le bénéfice non distribué.
        #[test]
        fn carry_back_credit_never_exceeds_the_tax_paid(
            deficit in 0i64..1_000_000_000_000,
            profit in 0i64..1_000_000_000_000,
            distributed in 0i64..1_000_000_000_000,
        ) {
            let base = CarryBackBase {
                taxable_profit: Money::from_cents(profit),
                distributed: Money::from_cents(distributed),
            };
            let cb = carry_back(Money::from_cents(deficit), base);
            prop_assert!(cb.imputed.cents() >= 0);
            prop_assert!(cb.imputed <= Money::from_cents(deficit));
            prop_assert!(cb.imputed <= LOSS_CARRY_BACK_CAP);
            prop_assert!(cb.imputed <= (base.taxable_profit - base.distributed).max(Money::ZERO));
            prop_assert!(cb.credit.cents() >= 0);
            prop_assert!(cb.credit <= corporate_income_tax(base.taxable_profit));
            // Au moins le taux réduit sur ce qui est imputé, au plus le taux normal.
            prop_assert!(cb.credit >= cb.imputed.apply_rate_bps(REDUCED_RATE_BPS));
            prop_assert!(cb.credit <= cb.imputed.apply_rate_bps(NORMAL_RATE_BPS));
        }

        /// L'IS est monotone (un bénéfice plus élevé n'est jamais moins taxé) et ne dépasse
        /// jamais 25 % du bénéfice.
        #[test]
        fn is_is_monotone_and_bounded_by_the_normal_rate(cents in 0i64..1_000_000_000_000) {
            let result = Money::from_cents(cents);
            let tax = corporate_income_tax(result);
            prop_assert!(tax.cents() >= 0);
            prop_assert!(tax.cents() <= result.apply_rate_bps(NORMAL_RATE_BPS).cents());
            if cents > 0 {
                let more = corporate_income_tax(Money::from_cents(cents + 100));
                prop_assert!(more.cents() >= tax.cents());
            }
        }
    }

    // --- Intégration sur base réelle : facture émise + dépense → résultat et IS. ---

    use crate::app::{Actor, ExecutionContext, Executor};
    use crate::billing::EmitInvoice;
    use crate::company::SetCompanyProfile;
    use crate::domain::{
        Address, ExpenseCategory, FiscalYearEnd, InvoiceLine, Siren, VatRate, VatRegime,
    };
    use crate::expenses::RecordExpense;
    use crate::store::testing::test_store;
    use time::{Date, Month as TimeMonth};

    fn human() -> ExecutionContext {
        ExecutionContext::new(Actor::Human, false)
    }

    fn date(y: i32, m: TimeMonth, d: u8) -> Date {
        Date::from_calendar_date(y, m, d).unwrap()
    }

    fn profile_without_director() -> SetCompanyProfile {
        SetCompanyProfile {
            name: "Argon Digital".to_string(),
            legal_form: "SASU".to_string(),
            siren: Siren::parse("552100554").unwrap(),
            vat_number: None,
            address: Address {
                street: "12 rue de la Paix".to_string(),
                postal_code: "75002".to_string(),
                city: "Paris".to_string(),
                country: "FR".to_string(),
            },
            share_capital: Some(Money::from_cents(100_000)),
            rcs_city: Some("Paris".to_string()),
            iban: None,
            fiscal_year_end: Some(FiscalYearEnd::CALENDAR),
            vat_regime: Some(VatRegime::RealNormalMonthly),
            director_monthly_gross: None,
            director_charge_ratio_bps: None,
            president_name: None,
            sole_shareholder_name: None,
            sole_shareholder_address: None,
            share_count: None,
        }
    }

    #[test]
    fn compute_result_combines_invoices_and_expenses_then_applies_is() {
        let mut store = test_store("accounting-result");
        let client_id = crate::domain::ClientId::new();
        store
            .connection()
            .execute(
                "INSERT INTO clients (id, name, created_at) \
                 VALUES (?1, 'Argon Digital', '2026-01-01T00:00:00Z')",
                [client_id.to_string()],
            )
            .unwrap();

        Executor::new(&mut store)
            .execute(&profile_without_director(), &human())
            .unwrap();

        // Facture : 9,5 j × 650 € = 6 175,00 € HT, émise dans l'exercice civil 2026.
        Executor::new(&mut store)
            .execute(
                &EmitInvoice {
                    client_id,
                    mission_id: None,
                    lines: vec![InvoiceLine {
                        description: "Prestation".to_string(),
                        quantity: 9.5,
                        unit_price: Money::from_cents(65_000),
                        vat_rate: VatRate::Standard,
                    }],
                    issued_on: date(2026, TimeMonth::September, 30),
                    payment_terms_days: 30,
                },
                &human(),
            )
            .unwrap();

        // Dépense : 1 000 € dont 200 € de TVA déductible → charge nette de 800 €.
        Executor::new(&mut store)
            .execute(
                &RecordExpense {
                    label: "Matériel".to_string(),
                    category: ExpenseCategory::Equipment,
                    amount: Money::from_cents(96_000),
                    vat_rate: VatRate::Standard,
                    vat_deductible: Money::from_cents(16_000),
                    incurred_on: date(2026, TimeMonth::October, 5),
                    receipt_hash: None,
                    receipt_filename: None,
                    supplier: None,
                    bank_transaction_id: None,
                    paid_by: crate::domain::ExpensePaidBy::Company,
                },
                &human(),
            )
            .unwrap();

        let profile = crate::company::company_profile(store.connection())
            .unwrap()
            .unwrap();
        let period = profile
            .fiscal_year_end
            .unwrap()
            .current(date(2026, TimeMonth::December, 31));
        let result = compute_result(store.connection(), period, &profile).unwrap();

        assert_eq!(result.revenue_ht, Money::from_cents(617_500));
        assert_eq!(result.expenses, Money::from_cents(80_000));
        assert_eq!(result.director_remuneration, Money::ZERO);
        assert_eq!(result.result_before_tax, Money::from_cents(537_500));
        // 15 % de 5 375 € = 806,25 € → 806 € (art. 1657 CGI, lot 41).
        assert_eq!(result.corporate_tax, Money::from_cents(80_600));
        assert_eq!(result.net_result, Money::from_cents(456_900));

        // TVA de l'exercice : 20 % de 6 175 € collectés (1 235 €) − 160 € déductibles = 1 075 €.
        let vat = vat_due_for_period(store.connection(), period.start(), period.end()).unwrap();
        assert_eq!(vat.collected, Money::from_cents(123_500));
        assert_eq!(vat.deductible, Money::from_cents(16_000));
        assert_eq!(vat.due, Money::from_cents(107_500));
        assert_eq!(vat.taxable_ht, Money::from_cents(617_500));
        assert_eq!(vat.deductible_assets, Money::ZERO);
        assert_eq!(vat.deductible_other, Money::from_cents(16_000));
        assert_eq!(vat.collected_by_rate.len(), 1);
        assert_eq!(vat.collected_by_rate[0].rate, VatRate::Standard);
        assert_eq!(
            vat.collected_by_rate[0].vat_amount,
            Money::from_cents(123_500)
        );
    }

    // --- Lot 41 : arrondi à l'euro (art. 1657 CGI). ---

    #[test]
    fn corporate_income_tax_is_rounded_to_the_euro_like_the_2065() {
        // 5 375 € × 15 % = 806,25 € → 806 € (fraction < 0,50 abandonnée).
        assert_eq!(
            corporate_income_tax(Money::from_cents(537_500)),
            Money::from_cents(80_600)
        );
        // Base 5 375,50 € → arrondie à 5 376 € d'abord ; 15 % = 806,40 € → 806 €.
        assert_eq!(
            corporate_income_tax(Money::from_cents(537_550)),
            Money::from_cents(80_600)
        );
        // 10 003,33 € → 10 003 € → 1 500,45 € → 1 500 €.
        assert_eq!(
            corporate_income_tax(Money::from_cents(1_000_333)),
            Money::from_cents(150_000)
        );
        // La fraction égale à 0,50 compte pour 1 : 0,50 € → 1 € ; 0,49 € → 0 ; −0,50 € → −1 €.
        assert_eq!(round_to_euro(Money::from_cents(50)), Money::from_cents(100));
        assert_eq!(round_to_euro(Money::from_cents(49)), Money::ZERO);
        assert_eq!(
            round_to_euro(Money::from_cents(-50)),
            Money::from_cents(-100)
        );
        assert_eq!(
            round_to_euro(Money::from_cents(123_449)),
            Money::from_cents(123_400)
        );
    }

    proptest! {
        #[test]
        fn the_rounded_tax_is_a_whole_number_of_euros_within_half_a_euro_of_the_exact_one(
            cents in 0_i64..1_000_000_000
        ) {
            let tax = corporate_income_tax(Money::from_cents(cents));
            prop_assert_eq!(tax.cents() % 100, 0);
            let base = round_to_euro(Money::from_cents(cents)).cents();
            let exact = if base <= REDUCED_RATE_CEILING.cents() {
                base * 15 / 100
            } else {
                REDUCED_RATE_CEILING.cents() * 15 / 100 + (base - REDUCED_RATE_CEILING.cents()) * 25 / 100
            };
            prop_assert!((tax.cents() - exact).abs() <= 50, "{tax} vs {exact}");
        }
    }
}
