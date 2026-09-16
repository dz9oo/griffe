//! Prévisionnel de trésorerie sur 12 mois : factures émises non payées, missions signées non
//! encore facturées, pipeline pondéré, moins les charges connues.
//!
//! Ce module est délibérément pur (aucun accès à la base) : [`forecast_12_months`] prend des
//! données déjà interrogées ([`ForecastInputs`]) et produit une projection mois par mois — ce
//! qui rend l'invariant central directement testable en proptest, sans base SQLite en jeu :
//! **ajouter une facture impayée au prévisionnel ne peut jamais faire baisser la trésorerie
//! projetée d'un mois donné, quel que soit son échéance.** [`build_forecast_inputs`] est la
//! (fine) couche qui va chercher ces données réelles dans le store.

use rusqlite::Connection;
use time::Date;

use crate::app::AppError;
use crate::billing::{aged_balance, compute_totals, list_invoices};
use crate::domain::{ExpensePaidBy, Money, Month};
use crate::expenses::expenses_between;
use crate::missions::{BillingSchedule, billing_schedule, list_active_missions};
use crate::prospection::weighted_pipeline;

const FORECAST_WINDOW_MONTHS: u32 = 12;
/// Nombre de mois d'historique moyennés pour estimer les charges récurrentes futures — trop
/// court et un mois inhabituel fausse toute la projection, trop long et on efface les tendances
/// récentes.
const EXPENSE_HISTORY_MONTHS: u32 = 3;

#[derive(Debug, Clone)]
pub struct ForecastInputs {
    pub starting_cash: Money,
    /// `(date d'échéance, solde restant dû)` de chaque facture émise non intégralement payée.
    pub unpaid_invoices: Vec<(Date, Money)>,
    /// `(date d'échéance estimée, montant)` des jalons de missions actives pas encore facturés
    /// (issus de [`crate::missions::billing_schedule`]) — une approximation : rien ne garantit
    /// qu'une facture sera bien émise exactement à cette date.
    pub pending_mission_revenue: Vec<(Date, Money)>,
    /// Pipeline pondéré (montant × probabilité) des opportunités ouvertes — réparti à parts
    /// égales sur les 12 mois plutôt que sur une date de clôture attendue, que le domaine ne
    /// porte pas encore.
    pub weighted_pipeline: Money,
    /// Estimation des charges mensuelles récurrentes, appliquée identiquement à chaque mois de
    /// la fenêtre — voir [`build_forecast_inputs`] pour son calcul par moyenne historique.
    pub monthly_known_expenses: Money,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonthlyForecast {
    pub month: Month,
    pub inflow: Money,
    pub outflow: Money,
    /// Trésorerie cumulée projetée à la fin de ce mois.
    pub projected_cash: Money,
}

/// Projette la trésorerie sur les [`FORECAST_WINDOW_MONTHS`] mois à partir de celui de `today`
/// inclus.
///
/// # Panics
///
/// Ne panique jamais en pratique : `today.month()` est toujours un mois `1..=12` valide pour
/// une `Date` réelle.
#[must_use]
pub fn forecast_12_months(today: Date, inputs: &ForecastInputs) -> Vec<MonthlyForecast> {
    let first_month =
        Month::new(today.year(), u8::from(today.month())).expect("mois courant valide");
    let months: Vec<Month> = std::iter::successors(Some(first_month), |m| Some(m.succ()))
        .take(FORECAST_WINDOW_MONTHS as usize)
        .collect();

    let pipeline_shares = inputs
        .weighted_pipeline
        .split_equally(FORECAST_WINDOW_MONTHS);

    let mut running_cash = inputs.starting_cash;
    months
        .iter()
        .zip(pipeline_shares)
        .map(|(&month, pipeline_share)| {
            let invoice_inflow: Money = inputs
                .unpaid_invoices
                .iter()
                .filter(|(due_on, _)| month_of(*due_on) == month)
                .map(|(_, amount)| *amount)
                .sum();
            let mission_inflow: Money = inputs
                .pending_mission_revenue
                .iter()
                .filter(|(due_on, _)| month_of(*due_on) == month)
                .map(|(_, amount)| *amount)
                .sum();
            let inflow = invoice_inflow + mission_inflow + pipeline_share;
            let outflow = inputs.monthly_known_expenses;

            running_cash = running_cash + inflow - outflow;
            MonthlyForecast {
                month,
                inflow,
                outflow,
                projected_cash: running_cash,
            }
        })
        .collect()
}

fn month_of(date: Date) -> Month {
    Month::new(date.year(), u8::from(date.month()))
        .expect("mois toujours valide pour une date réelle")
}

/// Premier mois de la projection dont la trésorerie cumulée est négative — `None` si elle reste
/// positive sur toute la fenêtre de 12 mois.
#[must_use]
pub fn first_shortfall_month(forecast: &[MonthlyForecast]) -> Option<Month> {
    forecast
        .iter()
        .find(|f| f.projected_cash.cents() < 0)
        .map(|f| f.month)
}

/// Rassemble les données réelles du store pour [`forecast_12_months`] — la seule fonction de ce
/// module qui touche la base.
///
/// # Errors
pub fn build_forecast_inputs(
    conn: &Connection,
    today: Date,
    starting_cash: Money,
) -> Result<ForecastInputs, AppError> {
    let unpaid_invoices = aged_balance(conn, today)?
        .into_iter()
        .map(|a| {
            // `aged_balance` ne porte pas la date d'échéance, seulement le retard déjà connu —
            // on la reconstruit : `today - days_overdue` (une facture non encore échue a un
            // `days_overdue` négatif, ce qui la replace correctement dans le futur).
            let due_on = today - time::Duration::days(a.days_overdue);
            (due_on, a.outstanding)
        })
        .collect();

    let all_invoices = list_invoices(conn)?;
    let mut pending_mission_revenue = Vec::new();
    for mission in list_active_missions(conn)? {
        let already_invoiced: Money = all_invoices
            .iter()
            .filter(|i| i.mission_id == Some(mission.id))
            .map(|i| compute_totals(&i.lines).subtotal_ht)
            .sum();

        match billing_schedule(&mission) {
            // Le CA d'une mission en régie dépend de jours pas encore travaillés : aucune
            // hypothèse de rythme futur ne serait mieux qu'une approximation inventée — omise
            // volontairement plutôt que projetée sur une base fictive.
            BillingSchedule::Regie { .. } => {}
            BillingSchedule::Forfait { installments } => {
                let budget: Money = installments.iter().map(|(_, amount)| *amount).sum();
                let remaining = budget - already_invoiced;
                if remaining.cents() > 0 {
                    let due_on = installments
                        .iter()
                        .filter_map(|(m, _)| m.due_on)
                        .max()
                        .unwrap_or_else(|| today + time::Duration::days(90));
                    pending_mission_revenue.push((due_on.max(today), remaining));
                }
            }
            BillingSchedule::Recurrent { monthly_amount } => {
                for offset in 0..FORECAST_WINDOW_MONTHS {
                    let month = (0..offset).fold(month_of(today), |m, _| m.succ());
                    pending_mission_revenue.push((month.first_day(), monthly_amount));
                }
            }
        }
    }

    let pipeline = weighted_pipeline(conn)?;

    let history_start = (0..EXPENSE_HISTORY_MONTHS)
        .fold(month_of(today), |m, _| m.pred())
        .first_day();
    let recent_expenses = expenses_between(conn, history_start, today)?;
    let total_recent: Money = recent_expenses
        .iter()
        .filter(|e| e.paid_by != ExpensePaidBy::Associate)
        .map(|e| e.amount)
        .sum();
    let monthly_known_expenses = total_recent.divide_by_days(f64::from(EXPENSE_HISTORY_MONTHS));

    Ok(ForecastInputs {
        starting_cash,
        unpaid_invoices,
        pending_mission_revenue,
        weighted_pipeline: pipeline,
        monthly_known_expenses,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use time::Month as TimeMonth;

    fn date(year: i32, month: TimeMonth, day: u8) -> Date {
        Date::from_calendar_date(year, month, day).unwrap()
    }

    fn base_inputs() -> ForecastInputs {
        ForecastInputs {
            starting_cash: Money::from_cents(1_000_000),
            unpaid_invoices: vec![(
                date(2026, TimeMonth::October, 15),
                Money::from_cents(500_000),
            )],
            pending_mission_revenue: vec![],
            weighted_pipeline: Money::ZERO,
            monthly_known_expenses: Money::from_cents(200_000),
        }
    }

    #[test]
    fn projects_exactly_twelve_consecutive_months_starting_from_today() {
        let forecast = forecast_12_months(date(2026, TimeMonth::September, 1), &base_inputs());
        assert_eq!(forecast.len(), 12);
        assert_eq!(forecast[0].month, Month::new(2026, 9).unwrap());
        assert_eq!(forecast[11].month, Month::new(2027, 8).unwrap());
    }

    #[test]
    fn an_unpaid_invoice_lands_in_the_month_of_its_due_date_only() {
        let forecast = forecast_12_months(date(2026, TimeMonth::September, 1), &base_inputs());
        let september = &forecast[0];
        let october = &forecast[1];
        assert_eq!(september.inflow, Money::ZERO);
        assert_eq!(october.inflow, Money::from_cents(500_000));
    }

    #[test]
    fn a_flat_monthly_expense_reduces_projected_cash_every_month() {
        let forecast = forecast_12_months(date(2026, TimeMonth::September, 1), &base_inputs());
        for f in &forecast {
            assert_eq!(f.outflow, Money::from_cents(200_000));
        }
    }

    #[test]
    fn weighted_pipeline_is_spread_across_all_twelve_months_without_losing_a_cent() {
        let mut inputs = base_inputs();
        inputs.unpaid_invoices.clear();
        inputs.monthly_known_expenses = Money::ZERO;
        inputs.weighted_pipeline = Money::from_cents(1_000_001); // ne se divise pas rond par 12
        let forecast = forecast_12_months(date(2026, TimeMonth::September, 1), &inputs);
        let total_inflow: Money = forecast.iter().map(|f| f.inflow).sum();
        assert_eq!(total_inflow, Money::from_cents(1_000_001));
    }

    proptest! {
        /// L'invariant central du lot : ajouter une facture impayée au prévisionnel ne peut
        /// jamais faire baisser la trésorerie projetée d'un mois donné, quelle que soit son
        /// échéance (dans la fenêtre de 12 mois ou après).
        #[test]
        fn adding_an_unpaid_invoice_never_decreases_any_months_projected_cash(
            extra_cents in 1i64..10_000_000,
            due_offset_days in 0i64..730,
        ) {
            let today = date(2026, TimeMonth::September, 1);
            let before = base_inputs();
            let forecast_before = forecast_12_months(today, &before);

            let mut after = before;
            after.unpaid_invoices.push((
                today + time::Duration::days(due_offset_days),
                Money::from_cents(extra_cents),
            ));
            let forecast_after = forecast_12_months(today, &after);

            for (b, a) in forecast_before.iter().zip(forecast_after.iter()) {
                prop_assert!(a.projected_cash.cents() >= b.projected_cash.cents());
            }
        }
    }
}
