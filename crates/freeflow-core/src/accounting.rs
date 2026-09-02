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

use rusqlite::Connection;

use crate::app::AppError;
use crate::billing::{compute_totals, list_invoices};
use crate::company::CompanyProfile;
use crate::domain::{FiscalYear, Money};
use crate::expenses::expenses_between;

/// Plafond de la tranche à taux réduit d'IS : 42 500 € de bénéfice.
const REDUCED_RATE_CEILING: Money = Money::from_cents(4_250_000);
/// Taux réduit d'IS (15 %) en dix-millièmes.
const REDUCED_RATE_BPS: u32 = 1_500;
/// Taux normal d'IS (25 %) en dix-millièmes.
const NORMAL_RATE_BPS: u32 = 2_500;

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
    pub result_before_tax: Money,
    pub corporate_tax: Money,
    pub net_result: Money,
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
    let reduced_base = result_before_tax.min(REDUCED_RATE_CEILING);
    let mut tax = reduced_base.apply_rate_bps(REDUCED_RATE_BPS);
    if result_before_tax > REDUCED_RATE_CEILING {
        let upper_base = result_before_tax - REDUCED_RATE_CEILING;
        tax += upper_base.apply_rate_bps(NORMAL_RATE_BPS);
    }
    tax
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

/// Calcule le résultat comptable simplifié et l'IS d'un exercice.
///
/// # Errors
///
/// Erreur de lecture SQLite.
pub fn compute_result(
    conn: &Connection,
    period: FiscalYear,
    profile: &CompanyProfile,
) -> Result<AccountingResult, AppError> {
    let revenue_ht: Money = list_invoices(conn)?
        .iter()
        .filter(|inv| period.contains(inv.issued_on))
        .map(|inv| compute_totals(&inv.lines).subtotal_ht)
        .sum();

    let expenses: Money = expenses_between(conn, period.start(), period.end())?
        .iter()
        .map(|e| e.amount - e.vat_deductible)
        .sum();

    let director_remuneration = director_cost(profile, period);

    let result_before_tax = revenue_ht - expenses - director_remuneration;
    let corporate_tax = corporate_income_tax(result_before_tax);
    let net_result = result_before_tax - corporate_tax;

    Ok(AccountingResult {
        period,
        revenue_ht,
        expenses,
        director_remuneration,
        result_before_tax,
        corporate_tax,
        net_result,
    })
}

/// Déclaration de TVA d'une période (CA3) : TVA collectée sur les factures émises, TVA déductible
/// sur les dépenses, et le solde à reverser (positif) ou le crédit de TVA (négatif).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct VatReturn {
    pub period_start: time::Date,
    pub period_end: time::Date,
    pub collected: Money,
    pub deductible: Money,
    /// `collected − deductible` : à reverser si positif, crédit de TVA reportable si négatif.
    pub due: Money,
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
    let collected: Money = list_invoices(conn)?
        .iter()
        .filter(|inv| inv.issued_on >= start && inv.issued_on <= end)
        .map(|inv| compute_totals(&inv.lines).total_vat)
        .sum();

    let deductible: Money = expenses_between(conn, start, end)?
        .iter()
        .map(|e| e.vat_deductible)
        .sum();

    Ok(VatReturn {
        period_start: start,
        period_end: end,
        collected,
        deductible,
        due: collected - deductible,
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

    proptest! {
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
    use crate::store::{Passphrase, Store};
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
        }
    }

    #[test]
    fn compute_result_combines_invoices_and_expenses_then_applies_is() {
        let dir = std::env::temp_dir().join(format!(
            "freeflow-accounting-{}-{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ));
        let mut store = Store::create(&dir.join("vault.db"), &Passphrase::from("s3cret")).unwrap();
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
                    amount: Money::from_cents(100_000),
                    vat_rate: VatRate::Standard,
                    vat_deductible: Money::from_cents(20_000),
                    incurred_on: date(2026, TimeMonth::October, 5),
                    receipt_hash: None,
                    receipt_filename: None,
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
        // 15 % de 5 375 € = 806,25 €.
        assert_eq!(result.corporate_tax, Money::from_cents(80_625));
        assert_eq!(result.net_result, Money::from_cents(456_875));

        // TVA de l'exercice : 20 % de 6 175 € collectés (1 235 €) − 200 € déductibles = 1 035 €.
        let vat = vat_due_for_period(store.connection(), period.start(), period.end()).unwrap();
        assert_eq!(vat.collected, Money::from_cents(123_500));
        assert_eq!(vat.deductible, Money::from_cents(20_000));
        assert_eq!(vat.due, Money::from_cents(103_500));
    }
}
