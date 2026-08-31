//! Échéancier de facturation dérivé du type de mission — logique pure, aucune IO. C'est la
//! grille que le lot Facturation consommera pour émettre les factures réelles.

use crate::domain::{Milestone, Mission, MissionKind, Money};

#[derive(Debug, Clone, PartialEq)]
pub enum BillingSchedule {
    /// Facturé chaque mois sur la base des jours facturables réellement saisis à ce TJM —
    /// aucun montant fixe connu à l'avance, contrairement au forfait et au récurrent.
    Regie { daily_rate: Money },
    /// Échéancier fixe connu à l'avance : une ligne par jalon, dont la somme vaut exactement le
    /// budget (répartition sans perte de centime via [`Money::allocate_proportionally`]).
    Forfait {
        installments: Vec<(Milestone, Money)>,
    },
    /// Même montant chaque mois, tant que la mission reste active.
    Recurrent { monthly_amount: Money },
}

#[must_use]
pub fn billing_schedule(mission: &Mission) -> BillingSchedule {
    match &mission.kind {
        MissionKind::Regie { daily_rate } => BillingSchedule::Regie {
            daily_rate: *daily_rate,
        },
        MissionKind::Forfait { budget } => BillingSchedule::Forfait {
            installments: forfait_installments(*budget, &mission.milestones),
        },
        MissionKind::Recurrent { monthly_amount } => BillingSchedule::Recurrent {
            monthly_amount: *monthly_amount,
        },
    }
}

/// Répartit le budget sur les jalons donnés. Sans jalon, le budget entier forme une seule
/// échéance implicite : un forfait n'a pas besoin d'être découpé pour être valide.
fn forfait_installments(budget: Money, milestones: &[Milestone]) -> Vec<(Milestone, Money)> {
    if milestones.is_empty() {
        return vec![(
            Milestone {
                label: "Solde".to_string(),
                share_bps: 10_000,
                due_on: None,
            },
            budget,
        )];
    }
    let shares: Vec<u32> = milestones.iter().map(|m| m.share_bps).collect();
    let amounts = budget.allocate_proportionally(&shares);
    milestones.iter().cloned().zip(amounts).collect()
}

#[cfg(test)]
mod tests {
    use time::{Date, Month};

    use super::*;
    use crate::domain::{ClientId, MissionId};

    fn mission(kind: MissionKind, milestones: Vec<Milestone>) -> Mission {
        Mission {
            id: MissionId::new(),
            client_id: ClientId::new(),
            quote_id: None,
            name: "Refonte dashboard IoT".to_string(),
            kind,
            milestones,
            started_on: Date::from_calendar_date(2026, Month::September, 1).unwrap(),
            ended_on: None,
            revision: 1,
            archived_at: None,
        }
    }

    #[test]
    fn regie_schedule_carries_the_daily_rate_without_a_fixed_amount() {
        let m = mission(
            MissionKind::Regie {
                daily_rate: Money::from_cents(65_000),
            },
            Vec::new(),
        );
        assert_eq!(
            billing_schedule(&m),
            BillingSchedule::Regie {
                daily_rate: Money::from_cents(65_000)
            }
        );
    }

    #[test]
    fn recurrent_schedule_carries_the_monthly_amount() {
        let m = mission(
            MissionKind::Recurrent {
                monthly_amount: Money::from_cents(350_000),
            },
            Vec::new(),
        );
        assert_eq!(
            billing_schedule(&m),
            BillingSchedule::Recurrent {
                monthly_amount: Money::from_cents(350_000)
            }
        );
    }

    #[test]
    fn forfait_without_milestones_is_a_single_installment_for_the_full_budget() {
        let budget = Money::from_cents(4_500_000);
        let m = mission(MissionKind::Forfait { budget }, Vec::new());
        let BillingSchedule::Forfait { installments } = billing_schedule(&m) else {
            panic!("expected Forfait")
        };
        assert_eq!(installments.len(), 1);
        assert_eq!(installments[0].1, budget);
    }

    #[test]
    fn forfait_with_acompte_solde_milestones_splits_30_40_30_without_losing_a_cent() {
        // Cas classique indépendant français : 30 % à la commande, 40 % à mi-parcours, 30 % au solde.
        let budget = Money::from_cents(4_500_000);
        let milestones = vec![
            Milestone {
                label: "Acompte".to_string(),
                share_bps: 3_000,
                due_on: None,
            },
            Milestone {
                label: "Mi-parcours".to_string(),
                share_bps: 4_000,
                due_on: None,
            },
            Milestone {
                label: "Solde".to_string(),
                share_bps: 3_000,
                due_on: None,
            },
        ];
        let m = mission(MissionKind::Forfait { budget }, milestones);
        let BillingSchedule::Forfait { installments } = billing_schedule(&m) else {
            panic!("expected Forfait")
        };

        assert_eq!(installments.len(), 3);
        assert_eq!(installments[0].1, Money::from_cents(1_350_000));
        assert_eq!(installments[1].1, Money::from_cents(1_800_000));
        assert_eq!(installments[2].1, Money::from_cents(1_350_000));

        let total: Money = installments.iter().map(|(_, amount)| *amount).sum();
        assert_eq!(
            total, budget,
            "la somme des échéances doit toujours retomber exactement sur le budget"
        );
    }
}
