//! Calcul de prix des devis — logique pure, aucune IO. Réutilise volontairement les primitives
//! déjà éprouvées de [`crate::domain::Money`] (arithmétique par centimes, allocation sans
//! perte) plutôt que de réimplémenter un mécanisme de répartition équivalent.

use crate::domain::{
    Discount, LineKind, Milestone, Mission, MissionId, MissionKind, Money, Quote, QuoteId,
    QuoteLine,
};
use time::Date;

use super::error::QuoteError;

fn line_gross_ht(line: &QuoteLine) -> Money {
    match &line.kind {
        LineKind::Regie { daily_rate, days } => daily_rate.multiply_by_quantity(*days),
        LineKind::Forfait { amount } => *amount,
        LineKind::Recurrent {
            monthly_amount,
            months,
        } => monthly_amount.multiply_by_quantity(f64::from(*months)),
    }
}

/// Montant HT brut (avant remise) et remise allouée, pour chaque ligne, dans leur ordre
/// d'origine. La remise globale est toujours calculée une fois sur le total puis répartie
/// proportionnellement — jamais ligne par ligne — pour ne perdre ni créer aucun centime.
#[must_use]
pub fn priced_lines(lines: &[QuoteLine], discount: Option<Discount>) -> Vec<(Money, Money)> {
    let gross: Vec<Money> = lines.iter().map(line_gross_ht).collect();
    let total_gross: Money = gross.iter().copied().sum();

    let Some(discount) = discount else {
        return gross.into_iter().map(|g| (g, Money::ZERO)).collect();
    };
    if total_gross.cents() <= 0 {
        return gross.into_iter().map(|g| (g, Money::ZERO)).collect();
    }

    let total_discount = match discount {
        Discount::Percentage(bps) => total_gross.apply_rate_bps(bps),
        Discount::FixedAmount(amount) => amount.min(total_gross),
    };
    if total_discount.cents() <= 0 {
        return gross.into_iter().map(|g| (g, Money::ZERO)).collect();
    }

    let weights: Vec<u32> = gross
        .iter()
        .map(|g| u32::try_from(g.cents().max(0)).unwrap_or(u32::MAX))
        .collect();
    let discount_amounts = total_discount.allocate_proportionally(&weights);
    gross.into_iter().zip(discount_amounts).collect()
}

/// Dérive la mission résultant de l'acceptation d'un devis : les lignes doivent toutes porter
/// le même type de facturation (une mission n'a qu'un seul [`MissionKind`]), et une mission
/// régie ou récurrente doit tenir sur une seule ligne — mélanger plusieurs TJM ou plusieurs
/// montants mensuels sur un même devis n'a pas de traduction unique en une seule mission.
///
/// # Errors
pub fn derive_mission(quote: &Quote, started_on: Date) -> Result<Mission, QuoteError> {
    if quote.lines.is_empty() {
        return Err(QuoteError::EmptyQuote);
    }
    let priced = priced_lines(&quote.lines, quote.discount);
    let net_ht: Vec<Money> = priced
        .iter()
        .map(|(gross, discount)| *gross - *discount)
        .collect();

    let kind = match &quote.lines[0].kind {
        LineKind::Forfait { .. } => {
            if !quote
                .lines
                .iter()
                .all(|l| matches!(l.kind, LineKind::Forfait { .. }))
            {
                return Err(QuoteError::CannotDeriveMission(quote.id));
            }
            MissionKind::Forfait {
                budget: net_ht.iter().copied().sum(),
            }
        }
        LineKind::Regie { daily_rate, .. } => {
            if quote.lines.len() != 1 {
                return Err(QuoteError::CannotDeriveMission(quote.id));
            }
            MissionKind::Regie {
                daily_rate: *daily_rate,
            }
        }
        LineKind::Recurrent { monthly_amount, .. } => {
            if quote.lines.len() != 1 {
                return Err(QuoteError::CannotDeriveMission(quote.id));
            }
            MissionKind::Recurrent {
                monthly_amount: *monthly_amount,
            }
        }
    };

    let milestones = match &kind {
        MissionKind::Forfait { .. } => forfait_milestones(quote.id, &quote.lines, &net_ht)?,
        MissionKind::Regie { .. } | MissionKind::Recurrent { .. } => Vec::new(),
    };

    Ok(Mission {
        id: MissionId::new(),
        client_id: quote.client_id,
        quote_id: Some(quote.id),
        name: quote
            .lines
            .first()
            .map(|l| l.description.clone())
            .unwrap_or_default(),
        kind,
        milestones,
        started_on,
        ended_on: None,
    })
}

fn forfait_milestones(
    quote_id: QuoteId,
    lines: &[QuoteLine],
    net_ht: &[Money],
) -> Result<Vec<Milestone>, QuoteError> {
    let total: Money = net_ht.iter().copied().sum();
    if total.cents() <= 0 {
        return Err(QuoteError::CannotDeriveMission(quote_id));
    }
    let weights: Vec<u32> = net_ht
        .iter()
        .map(|m| u32::try_from(m.cents().max(0)).unwrap_or(u32::MAX))
        .collect();
    // Astuce : allouer 10 000 « centimes » avec la même méthode du plus fort reste donne
    // directement des dix-millièmes qui somment exactement à 10 000, sans dupliquer
    // l'algorithme de répartition déjà éprouvé sur `Money`.
    let shares_bps: Vec<u32> = Money::from_cents(10_000)
        .allocate_proportionally(&weights)
        .into_iter()
        .map(|m| u32::try_from(m.cents()).unwrap_or(0))
        .collect();

    Ok(lines
        .iter()
        .zip(shares_bps)
        .map(|(line, share_bps)| Milestone {
            label: line.description.clone(),
            share_bps,
            due_on: None,
        })
        .collect())
}
