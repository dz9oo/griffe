//! Montants monétaires en centimes entiers — jamais de virgule flottante dans un calcul qui
//! doit rester exact au centime (TVA, échéanciers, répartitions).

use std::fmt;
use std::iter::Sum;
use std::ops::{Add, AddAssign, Neg, Sub, SubAssign};

use serde::{Deserialize, Serialize};

/// Un montant en centimes d'euro.
///
/// `FreeFlow` ne manipule que l'euro : le statut fiscal visé (SASU/EURL française) ne justifie
/// pas une gestion multi-devise avant qu'un besoin réel existe.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
pub struct Money(i64);

impl Money {
    /// Le montant nul.
    pub const ZERO: Self = Self(0);

    /// Borne d'un montant **saisi** (lot 36) : cent milliards d'euros, soit 10^13 centimes.
    /// Aucune facture, dépense ou ligne de bilan d'un indépendant n'approche ce chiffre ; en
    /// revanche une valeur proche de `i64::MAX` acceptée par [`Self::parse_decimal`] faisait
    /// déborder la première somme du grand livre. Bornée à l'entrée, l'arithmétique interne
    /// (sommes, `split_equally`, prorata) reste hors d'atteinte du débordement pour tout volume
    /// réaliste — et le grand livre vérifie de son côté que ses totaux tiennent.
    pub const MAX_INPUT: Self = Self(10_000_000_000_000);

    /// Construit un montant à partir d'un nombre entier de centimes.
    #[must_use]
    pub const fn from_cents(cents: i64) -> Self {
        Self(cents)
    }

    /// Nombre de centimes représentés par ce montant.
    #[must_use]
    pub const fn cents(self) -> i64 {
        self.0
    }

    /// Valeur en euros, pour l'affichage uniquement — ne jamais réinjecter ce flottant dans
    /// un calcul.
    #[must_use]
    pub fn euros(self) -> f64 {
        f64_from_cents(self.0)
    }

    #[must_use]
    pub const fn is_negative(self) -> bool {
        self.0 < 0
    }

    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }

    /// Multiplie par une quantité de jours (fraction incluse, granularité minimale : le
    /// centième de jour), avec arrondi half-to-even au centime — utilisé pour facturer du
    /// temps passé à un TJM.
    ///
    /// # Panics
    ///
    /// Panique dans le cas extrêmement improbable où le résultat dépasserait les bornes de
    /// `i64`.
    #[must_use]
    pub fn multiply_by_days(self, days: f64) -> Self {
        let hundredths = hundredths_of_day(days);
        let product = i128::from(self.0) * i128::from(hundredths);
        let rounded = round_half_to_even(product, 100);
        Self(
            i64::try_from(rounded)
                .expect("un montant facturé au temps reste dans les bornes de i64"),
        )
    }

    /// Alias sémantique de [`Self::multiply_by_days`] pour les quantités qui ne sont pas des
    /// jours (lignes de facture, etc.) — la mécanique d'arrondi est identique. Fonctionne
    /// aussi pour une quantité négative (ligne d'avoir).
    ///
    /// # Panics
    ///
    /// Mêmes conditions que [`Self::multiply_by_days`].
    #[must_use]
    pub fn multiply_by_quantity(self, quantity: f64) -> Self {
        self.multiply_by_days(quantity)
    }

    /// Divise par une quantité de jours (fraction incluse), avec arrondi half-to-even au
    /// centime — c'est le calcul du TJM effectif (`CA ÷ jours consommés`). Retourne
    /// [`Self::ZERO`] si `days` est nul ou négatif : un TJM effectif n'a de sens que pour une
    /// durée strictement positive, l'appelant est responsable de ce contrôle en amont.
    ///
    /// # Panics
    ///
    /// Panique dans le cas extrêmement improbable où le résultat dépasserait les bornes de
    /// `i64`.
    #[must_use]
    pub fn divide_by_days(self, days: f64) -> Self {
        let hundredths = hundredths_of_day(days);
        if hundredths <= 0 {
            return Self::ZERO;
        }
        let product = i128::from(self.0) * 100;
        let rounded = round_half_to_even(product, i128::from(hundredths));
        Self(i64::try_from(rounded).expect("un TJM effectif reste dans les bornes de i64"))
    }

    /// Multiplie par un taux exprimé en dix-millièmes (`10_000` = 100 %) avec arrondi
    /// half-to-even au centime le plus proche — la règle de calcul de la TVA.
    ///
    /// # Panics
    ///
    /// Panique dans le cas extrêmement improbable où le résultat dépasserait les bornes de
    /// `i64` (au-delà de 92 billions d'euros).
    #[must_use]
    pub fn apply_rate_bps(self, bps: u32) -> Self {
        let product = i128::from(self.0) * i128::from(bps);
        let rounded = round_half_to_even(product, 10_000);
        Self(i64::try_from(rounded).expect("un montant de TVA reste dans les bornes de i64"))
    }

    /// Prorata entier : `self × numerator / denominator`, division tronquée vers zéro.
    /// Dénominateur nul → [`Self::ZERO`]. Numérateur égal au dénominateur → `self`.
    ///
    /// # Panics
    ///
    /// Panique dans le cas extrêmement improbable où le résultat dépasserait les bornes de
    /// `i64`.
    #[must_use]
    pub fn scale(self, numerator: Self, denominator: Self) -> Self {
        if denominator.is_zero() {
            return Self::ZERO;
        }
        if numerator == denominator {
            return self;
        }
        let product = i128::from(self.0) * i128::from(numerator.0);
        let quotient = product / i128::from(denominator.0);
        Self(i64::try_from(quotient).expect("un montant prorata reste dans les bornes de i64"))
    }

    /// Répartit le montant en `parts` parts aussi égales que possible, sans perdre ni créer
    /// un centime : les premiers lots (dans l'ordre) reçoivent un centime de plus si besoin.
    ///
    /// Utilisé pour les échéanciers à parts égales.
    ///
    /// # Panics
    ///
    /// Panique si `parts` vaut zéro.
    #[must_use]
    pub fn split_equally(self, parts: u32) -> Vec<Self> {
        assert!(parts > 0, "split_equally requiert au moins une part");
        let parts_i64 = i64::from(parts);
        let base = self.0 / parts_i64;
        let remainder = self.0 % parts_i64;
        let bonus = remainder.signum();
        let extra_recipients = remainder.unsigned_abs();
        (0..u64::from(parts))
            .map(|i| {
                let extra = if i < extra_recipients { bonus } else { 0 };
                Self(base + extra)
            })
            .collect()
    }

    /// Répartit le montant proportionnellement aux `weights` donnés (méthode du plus fort
    /// reste), sans perdre ni créer un centime : la somme des parts retournées vaut toujours
    /// exactement `self`.
    ///
    /// # Panics
    ///
    /// Panique si `weights` est vide, si la somme des poids vaut zéro, ou dans le cas
    /// extrêmement improbable où une part dépasserait les bornes de `i64`.
    #[must_use]
    pub fn allocate_proportionally(self, weights: &[u32]) -> Vec<Self> {
        assert!(
            !weights.is_empty(),
            "allocate_proportionally requiert au moins un poids"
        );
        let total_weight: u128 = weights.iter().map(|&w| u128::from(w)).sum();
        assert!(total_weight > 0, "la somme des poids doit être positive");

        let negative = self.0 < 0;
        let magnitude = u128::from(self.0.unsigned_abs());

        let mut shares: Vec<(usize, u128, u128)> = weights
            .iter()
            .enumerate()
            .map(|(i, &w)| {
                let numerator = magnitude * u128::from(w);
                (i, numerator / total_weight, numerator % total_weight)
            })
            .collect();

        let allocated: u128 = shares.iter().map(|&(_, base, _)| base).sum();
        let mut remaining = magnitude - allocated;

        // Plus gros reste d'abord : c'est la méthode standard d'allocation sans perte.
        shares.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));

        let mut result = vec![0u128; weights.len()];
        for &(i, base, _) in &shares {
            result[i] = base;
        }
        for &(i, ..) in &shares {
            if remaining == 0 {
                break;
            }
            result[i] += 1;
            remaining -= 1;
        }

        result
            .into_iter()
            .map(|v| {
                let magnitude =
                    i64::try_from(v).expect("une part allouée reste dans les bornes de i64");
                Self(if negative { -magnitude } else { magnitude })
            })
            .collect()
    }

    /// Rend ce montant sous la forme décimale que [`Self::parse_decimal`] relit (`"1234.56"`,
    /// `"-0.50"`) — le format d'échange pour un champ de saisie ou une syntaxe texte, par
    /// opposition à [`fmt::Display`] qui produit un rendu humain (espaces de groupement, `€`)
    /// que `parse_decimal` refuse.
    #[must_use]
    pub fn to_decimal_string(self) -> String {
        let sign = if self.0 < 0 { "-" } else { "" };
        let magnitude = self.0.unsigned_abs();
        format!("{sign}{}.{:02}", magnitude / 100, magnitude % 100)
    }

    /// Analyse un montant décimal saisi par un humain (`.` ou `,` comme séparateur, deux
    /// décimales maximum) en centimes exacts — le format attendu pour toute entrée en euros,
    /// que ce soit en ligne de commande ou dans un import CSV/OFX.
    ///
    /// # Errors
    ///
    /// Retourne une erreur si `s` n'est pas un nombre décimal valide, ou s'il dépasse
    /// [`Self::MAX_INPUT`] en valeur absolue.
    pub fn parse_decimal(s: &str) -> Result<Self, MoneyParseError> {
        let normalized = s.replace(',', ".");
        let (sign, unsigned): (i64, &str) = match normalized.strip_prefix('-') {
            Some(rest) => (-1, rest),
            None => (1, normalized.as_str()),
        };
        let mut parts = unsigned.splitn(2, '.');
        let integer_part = parts.next().ok_or_else(|| MoneyParseError(s.to_string()))?;
        let fractional_part = parts.next().unwrap_or("0");

        if integer_part.is_empty() || !integer_part.bytes().all(|b| b.is_ascii_digit()) {
            return Err(MoneyParseError(s.to_string()));
        }
        if fractional_part.len() > 2 || !fractional_part.bytes().all(|b| b.is_ascii_digit()) {
            return Err(MoneyParseError(s.to_string()));
        }

        let integer: i64 = integer_part
            .parse()
            .map_err(|_| MoneyParseError(s.to_string()))?;
        let fractional_cents: i64 = match fractional_part.len() {
            0 => 0,
            1 => {
                fractional_part
                    .parse::<i64>()
                    .map_err(|_| MoneyParseError(s.to_string()))?
                    * 10
            }
            _ => fractional_part
                .parse()
                .map_err(|_| MoneyParseError(s.to_string()))?,
        };
        // Arithmétique vérifiée : un entier à 17+ chiffres déborde `integer * 100` — sans
        // `checked_*`, ce serait une panique en debug et un enroulement silencieux en release
        // (un montant absurde entrant alors en base depuis un import CSV/OFX hostile).
        let cents = integer
            .checked_mul(100)
            .and_then(|c| c.checked_add(fractional_cents))
            .and_then(|c| c.checked_mul(sign))
            .ok_or_else(|| MoneyParseError(s.to_string()))?;
        if cents.abs() > Self::MAX_INPUT.0 {
            return Err(MoneyParseError(s.to_string()));
        }
        Ok(Self(cents))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("montant décimal invalide : {0}")]
pub struct MoneyParseError(pub String);

/// Arrondi half-to-even (bancaire) de `product / divisor`, `divisor` strictement positif.
/// Reste en arithmétique signée de bout en bout : `%` en Rust suit le signe du dividende, donc
/// `quotient % 2 != 0` détecte correctement la parité quel que soit le signe de `product`.
fn round_half_to_even(product: i128, divisor: i128) -> i128 {
    debug_assert!(divisor > 0);
    let quotient = product / divisor;
    let remainder = product % divisor;
    let twice_abs = remainder.abs() * 2;
    if twice_abs > divisor || (twice_abs == divisor && quotient % 2 != 0) {
        quotient + remainder.signum()
    } else {
        quotient
    }
}

#[allow(clippy::cast_precision_loss)] // affichage uniquement, jamais réinjecté dans un calcul.
fn f64_from_cents(cents: i64) -> f64 {
    cents as f64 / 100.0
}

/// Convertit une quantité de jours (fraction) en centièmes de jour entiers, seule opération de
/// ce fichier qui reparte du flottant vers l'entier — isolée ici pour que le reste du module
/// reste en arithmétique exacte.
#[allow(clippy::cast_possible_truncation)]
fn hundredths_of_day(days: f64) -> i64 {
    (days * 100.0).round() as i64
}

impl fmt::Display for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.0 < 0 { "-" } else { "" };
        let magnitude = self.0.unsigned_abs();
        let euros = magnitude / 100;
        let cents = magnitude % 100;

        let mut grouped = String::new();
        for (i, ch) in euros.to_string().chars().rev().enumerate() {
            if i > 0 && i % 3 == 0 {
                grouped.push('\u{202f}'); // espace fine insécable
            }
            grouped.push(ch);
        }
        let grouped: String = grouped.chars().rev().collect();

        write!(f, "{sign}{grouped},{cents:02}\u{a0}€")
    }
}

impl Money {
    /// Addition vérifiée : `None` en cas de débordement, là où `+` paniquerait (les
    /// `overflow-checks` sont actifs en release depuis le lot 17). Réservée aux sommes de
    /// volume non borné par construction (grand livre) ; l'arithmétique ordinaire du domaine
    /// travaille sur des montants bornés par [`Self::MAX_INPUT`].
    #[must_use]
    pub const fn checked_add(self, rhs: Self) -> Option<Self> {
        match self.0.checked_add(rhs.0) {
            Some(cents) => Some(Self(cents)),
            None => None,
        }
    }
}

impl Add for Money {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self(self.0 + rhs.0)
    }
}

impl AddAssign for Money {
    fn add_assign(&mut self, rhs: Self) {
        self.0 += rhs.0;
    }
}

impl Sub for Money {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self(self.0 - rhs.0)
    }
}

impl SubAssign for Money {
    fn sub_assign(&mut self, rhs: Self) {
        self.0 -= rhs.0;
    }
}

impl Neg for Money {
    type Output = Self;
    fn neg(self) -> Self {
        Self(-self.0)
    }
}

impl Sum for Money {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(Self::ZERO, Add::add)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn scale_matches_spec_prorata() {
        let ht = Money::from_cents(350_667);
        let remaining = Money::from_cents(320_800);
        let original = Money::from_cents(420_800);
        assert_eq!(ht.scale(remaining, original), Money::from_cents(267_333));
    }

    #[test]
    fn display_formats_thousands_and_cents() {
        assert_eq!(
            Money::from_cents(123_456).to_string(),
            "1\u{202f}234,56\u{a0}€"
        );
        assert_eq!(Money::from_cents(-500).to_string(), "-5,00\u{a0}€");
        assert_eq!(Money::from_cents(0).to_string(), "0,00\u{a0}€");
    }

    #[test]
    fn vat_20_percent_on_reference_amount() {
        // 6 175,00 € HT à 20 % -> 1 235,00 € de TVA (cas Lumen Bank de la maquette).
        let ht = Money::from_cents(617_500);
        assert_eq!(ht.apply_rate_bps(2_000), Money::from_cents(123_500));
    }

    #[test]
    fn multiply_by_days_reference_case() {
        // 650,00 €/j × 9,5 j = 6 175,00 €.
        let daily_rate = Money::from_cents(65_000);
        assert_eq!(daily_rate.multiply_by_days(9.5), Money::from_cents(617_500));
    }

    #[test]
    fn multiply_by_quantity_handles_a_negative_quantity_symmetrically() {
        // Ligne d'avoir : quantité négatée, même prix unitaire.
        let unit_price = Money::from_cents(65_000);
        assert_eq!(
            unit_price.multiply_by_quantity(-9.5),
            -unit_price.multiply_by_quantity(9.5)
        );
    }

    #[test]
    fn parse_decimal_accepts_dot_and_comma_separators() {
        assert_eq!(
            Money::parse_decimal("7800.00"),
            Ok(Money::from_cents(780_000))
        );
        assert_eq!(Money::parse_decimal("59,99"), Ok(Money::from_cents(5_999)));
        assert_eq!(
            Money::parse_decimal("-59.99"),
            Ok(Money::from_cents(-5_999))
        );
        assert_eq!(Money::parse_decimal("100"), Ok(Money::from_cents(10_000)));
    }

    #[test]
    fn parse_decimal_rejects_malformed_input() {
        assert!(Money::parse_decimal("abc").is_err());
        assert!(
            Money::parse_decimal("12.345").is_err(),
            "plus de deux décimales"
        );
        assert!(Money::parse_decimal("").is_err());
    }

    #[test]
    fn parse_decimal_rejects_amounts_beyond_the_input_bound() {
        // 100 000 000 000,00 € == `Money::MAX_INPUT` pile : la dernière valeur acceptable.
        assert_eq!(
            Money::parse_decimal("100000000000.00"),
            Ok(Money::MAX_INPUT)
        );
        assert_eq!(Money::parse_decimal("-100000000000"), Ok(-Money::MAX_INPUT));
        // Un centime de plus est refusé : ni panique, ni enroulement, ni débordement plus loin
        // dans le grand livre (lot 36).
        assert!(
            Money::parse_decimal("100000000000.01").is_err(),
            "un montant au-delà de MAX_INPUT doit être rejeté"
        );
        // Et ce qui déborde i64 lui-même l'est a fortiori.
        assert!(Money::parse_decimal("92233720368547758.08").is_err());
        assert!(
            Money::parse_decimal("99999999999999999999.99").is_err(),
            "très grand nombre de chiffres"
        );
    }

    #[test]
    fn checked_add_reports_overflow_instead_of_panicking() {
        let max = Money::from_cents(i64::MAX);
        assert_eq!(max.checked_add(Money::from_cents(1)), None);
        assert_eq!(
            Money::from_cents(2).checked_add(Money::from_cents(3)),
            Some(Money::from_cents(5))
        );
    }

    #[test]
    fn divide_by_days_is_the_inverse_of_multiply_by_days_on_exact_cases() {
        let daily_rate = Money::from_cents(65_000);
        let revenue = daily_rate.multiply_by_days(9.5);
        assert_eq!(revenue.divide_by_days(9.5), daily_rate);
    }

    #[test]
    fn divide_by_days_is_zero_for_non_positive_days() {
        let revenue = Money::from_cents(4_500_000);
        assert_eq!(revenue.divide_by_days(0.0), Money::ZERO);
        assert_eq!(revenue.divide_by_days(-3.0), Money::ZERO);
    }

    #[test]
    fn half_to_even_rounds_ties_to_the_nearest_even_cent() {
        // 5 c × 50 % = 2,5 c pile : la parité départage vers le pair le plus proche.
        assert_eq!(
            Money::from_cents(5).apply_rate_bps(5_000),
            Money::from_cents(2)
        );
        // 7 c × 50 % = 3,5 c pile : 3 est impair, on monte à 4.
        assert_eq!(
            Money::from_cents(7).apply_rate_bps(5_000),
            Money::from_cents(4)
        );
    }

    proptest! {
        #[test]
        fn split_equally_never_loses_or_creates_a_cent(cents in -1_000_000_000i64..1_000_000_000, parts in 1u32..500) {
            let parts_out = Money::from_cents(cents).split_equally(parts);
            let total: Money = parts_out.into_iter().sum();
            prop_assert_eq!(total, Money::from_cents(cents));
        }

        #[test]
        fn split_equally_parts_differ_by_at_most_one_cent(cents in -1_000_000_000i64..1_000_000_000, parts in 1u32..500) {
            let parts_out = Money::from_cents(cents).split_equally(parts);
            let min = parts_out.iter().map(|m| m.cents()).min().unwrap();
            let max = parts_out.iter().map(|m| m.cents()).max().unwrap();
            prop_assert!(max - min <= 1);
        }

        #[test]
        fn allocate_proportionally_never_loses_or_creates_a_cent(
            cents in -1_000_000_000i64..1_000_000_000,
            weights in proptest::collection::vec(1u32..1000, 1..20),
        ) {
            let allocated = Money::from_cents(cents).allocate_proportionally(&weights);
            let total: Money = allocated.into_iter().sum();
            prop_assert_eq!(total, Money::from_cents(cents));
        }

        #[test]
        fn addition_is_associative(a in -1_000_000_000i64..1_000_000_000, b in -1_000_000_000i64..1_000_000_000, c in -1_000_000_000i64..1_000_000_000) {
            let (a, b, c) = (Money::from_cents(a), Money::from_cents(b), Money::from_cents(c));
            prop_assert_eq!((a + b) + c, a + (b + c));
        }

        #[test]
        fn apply_rate_bps_of_zero_is_zero(cents in -1_000_000_000i64..1_000_000_000) {
            prop_assert_eq!(Money::from_cents(cents).apply_rate_bps(0), Money::ZERO);
        }

        #[test]
        fn apply_rate_bps_of_full_rate_is_identity(cents in -1_000_000_000i64..1_000_000_000) {
            prop_assert_eq!(Money::from_cents(cents).apply_rate_bps(10_000), Money::from_cents(cents));
        }

        /// Robustesse : `parse_decimal` ne panique jamais, quelle que soit l'entrée — y compris
        /// des chaînes de chiffres arbitrairement longues qui débordent `i64` (elles renvoient
        /// une erreur, jamais un enroulement ni un abort).
        #[test]
        fn parse_decimal_never_panics_on_arbitrary_digit_strings(
            s in r"-?[0-9]{1,30}([.,][0-9]{0,3})?"
        ) {
            let _ = Money::parse_decimal(&s);
        }

        /// `to_decimal_string` est l'inverse exact de `parse_decimal` — le contrat sur lequel
        /// reposent les champs de formulaire et la syntaxe texte des lignes de devis (lot 23).
        #[test]
        fn to_decimal_string_round_trips_through_parse_decimal(
            cents in -1_000_000_000i64..1_000_000_000
        ) {
            let m = Money::from_cents(cents);
            prop_assert_eq!(Money::parse_decimal(&m.to_decimal_string()), Ok(m));
        }
    }
}
