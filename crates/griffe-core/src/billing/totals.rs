//! Calcul des totaux TVA — pur, sans IO. Règle non négociable : l'arrondi se fait **par
//! taux**, sur le total HT de ce taux, jamais ligne par ligne (une facture à 50 lignes au même
//! taux ne doit pas dériver de plusieurs centimes par accumulation d'arrondis individuels).

use serde::Serialize;

use crate::domain::{InvoiceLine, Money, VatRate};

fn line_amount_ht(line: &InvoiceLine) -> Money {
    line.unit_price.multiply_by_quantity(line.quantity)
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct VatBreakdownLine {
    pub rate: VatRate,
    pub taxable_amount: Money,
    pub vat_amount: Money,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InvoiceTotals {
    pub subtotal_ht: Money,
    pub vat_breakdown: Vec<VatBreakdownLine>,
    pub total_vat: Money,
    pub total_ttc: Money,
}

#[must_use]
pub fn compute_totals(lines: &[InvoiceLine]) -> InvoiceTotals {
    let subtotal_ht: Money = lines.iter().map(line_amount_ht).sum();

    let vat_breakdown: Vec<VatBreakdownLine> = VatRate::ALL
        .into_iter()
        .filter(|&rate| lines.iter().any(|l| l.vat_rate == rate))
        .map(|rate| {
            let taxable_amount: Money = lines
                .iter()
                .filter(|l| l.vat_rate == rate)
                .map(line_amount_ht)
                .sum();
            let vat_amount = taxable_amount.apply_rate_bps(rate.basis_points());
            VatBreakdownLine {
                rate,
                taxable_amount,
                vat_amount,
            }
        })
        .collect();

    let total_vat: Money = vat_breakdown.iter().map(|b| b.vat_amount).sum();

    InvoiceTotals {
        subtotal_ht,
        vat_breakdown,
        total_vat,
        total_ttc: subtotal_ht + total_vat,
    }
}

/// Représentation stable d'une facture pour le calcul de son hash — un type dédié, distinct de
/// [`crate::domain::Invoice`], pour que l'évolution future du type domaine (nouveaux champs
/// d'affichage, etc.) ne fasse pas dériver silencieusement des hash déjà émis.
#[derive(Serialize)]
pub(super) struct CanonicalInvoice<'a> {
    pub number: &'a str,
    pub client_id: String,
    pub mission_id: Option<String>,
    pub lines: &'a [InvoiceLine],
    pub issued_on: String,
    pub due_on: String,
    pub credited_invoice_id: Option<String>,
}

#[must_use]
pub(super) fn compute_invoice_hash(
    previous_hash: Option<&str>,
    canonical: &CanonicalInvoice<'_>,
) -> String {
    use sha2::{Digest, Sha256};
    let payload =
        serde_json::to_string(canonical).expect("une facture canonique se sérialise toujours");
    let mut hasher = Sha256::new();
    hasher.update(previous_hash.unwrap_or_default().as_bytes());
    hasher.update([0u8]);
    hasher.update(payload.as_bytes());
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn line(amount_cents: i64, quantity: f64, rate: VatRate) -> InvoiceLine {
        InvoiceLine {
            description: "Prestation".to_string(),
            quantity,
            unit_price: Money::from_cents(amount_cents),
            vat_rate: rate,
        }
    }

    /// Table de cas TVA exhaustive : un cas par taux légal français, montants issus de
    /// factures réelles construites à la main (voir maquette Lumen Bank : 6 175,00 € HT à 20 %
    /// -> 1 235,00 € de TVA).
    #[test]
    #[allow(clippy::cast_precision_loss)] // montants de test petits et connus, aucune perte réelle.
    fn vat_rate_table_matches_expected_amounts() {
        let cases: [(VatRate, i64, i64); 5] = [
            (VatRate::Standard, 617_500, 123_500), // 20 %   : 6 175,00 € -> 1 235,00 €
            (VatRate::Intermediate, 100_000, 10_000), // 10 %   : 1 000,00 € ->   100,00 €
            (VatRate::Reduced, 100_000, 5_500),    // 5,5 %  : 1 000,00 € ->    55,00 €
            (VatRate::SuperReduced, 100_000, 2_100), // 2,1 %  : 1 000,00 € ->    21,00 €
            (VatRate::Zero, 100_000, 0),           // 0 %    : autoliquidation / export
        ];
        for (rate, ht_cents, expected_vat_cents) in cases {
            let totals = compute_totals(&[line(1, ht_cents as f64, rate)]);
            assert_eq!(
                totals.total_vat,
                Money::from_cents(expected_vat_cents),
                "taux {rate:?}"
            );
            assert_eq!(
                totals.total_ttc,
                Money::from_cents(ht_cents + expected_vat_cents),
                "taux {rate:?}"
            );
        }
    }

    #[test]
    fn rounding_happens_once_per_rate_not_once_per_line() {
        // 3 lignes de 0,01 € HT à 20 % (quantité entière : aucun arrondi au niveau de la
        // ligne elle-même, pour isoler l'arrondi de TVA). Ligne par ligne, chaque 0,01 € × 20 %
        // = 0,002 €, arrondi à 0,00 € : un cumul de 3 lignes arrondies individuellement donnerait
        // 0,00 € de TVA. Sur le total HT (0,03 €), 20 % = 0,006 €, qui arrondit à 0,01 € — c'est
        // ce second résultat que la fonction doit produire.
        let lines = vec![
            line(1, 1.0, VatRate::Standard),
            line(1, 1.0, VatRate::Standard),
            line(1, 1.0, VatRate::Standard),
        ];
        let totals = compute_totals(&lines);
        assert_eq!(totals.subtotal_ht, Money::from_cents(3));
        assert_eq!(
            totals.total_vat,
            Money::from_cents(1),
            "arrondi sur le total du taux, pas somme d'arrondis ligne par ligne"
        );
    }

    #[test]
    fn multiple_rates_are_broken_down_independently() {
        // Deux lignes de 1 000,00 € HT chacune, à deux taux différents.
        let lines = vec![
            line(100_000, 1.0, VatRate::Standard),
            line(100_000, 1.0, VatRate::Reduced),
        ];
        let totals = compute_totals(&lines);
        assert_eq!(totals.vat_breakdown.len(), 2);
        assert_eq!(totals.subtotal_ht, Money::from_cents(200_000)); // 2 000,00 €
        // 20 % de 1 000,00 € = 200,00 € ; 5,5 % de 1 000,00 € = 55,00 €.
        assert_eq!(
            totals.total_vat,
            Money::from_cents(20_000) + Money::from_cents(5_500)
        );
    }

    proptest! {
        #[test]
        fn total_ttc_always_equals_ht_plus_vat(
            amounts in proptest::collection::vec(1i64..1_000_000, 1..10),
            rate_indices in proptest::collection::vec(0usize..5, 1..10),
        ) {
            let n = amounts.len().min(rate_indices.len());
            let lines: Vec<InvoiceLine> = (0..n)
                .map(|i| line(amounts[i], 1.0, VatRate::ALL[rate_indices[i]]))
                .collect();
            let totals = compute_totals(&lines);
            prop_assert_eq!(totals.total_ttc, totals.subtotal_ht + totals.total_vat);
        }
    }
}
