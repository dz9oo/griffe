//! Immobilisations et amortissements (lot 42) — types purs, sans IO.
//!
//! Jusqu'ici, tout équipement passait en charge et un compte 28x repris au bilan d'ouverture
//! restait figé : aucune dotation n'était calculée, le résultat était surestimé et la case 254
//! du 2033-B restait vide (constat 18 de l'audit du 2 septembre 2026). Ce module porte la règle,
//! vérifiée à la source avant d'être codée :
//!
//! - **Amortissement linéaire seul**, à partir de la **mise en service** (BOI-BIC-AMT-20-10
//!   § 120), la première annuité réduite **prorata temporis en jours** (§ 240), la dernière
//!   limitée à ce qui reste — l'usage retient une année de 360 jours faite de douze mois de
//!   30 jours ([`days360`]), ce qui donne les chiffres ronds qu'un cabinet produit (1 200 € sur
//!   36 mois = 400 € par exercice plein).
//! - **Tolérance de 500 € HT** (BOI-BIC-CHG-20-30-10 § 20 à 40) : le petit matériel et
//!   outillage, le matériel et mobilier de bureau, les logiciels d'une valeur unitaire qui
//!   n'excède pas 500 € HT peuvent passer en charge ; au-delà, c'est une immobilisation
//!   ([`SMALL_EQUIPMENT_THRESHOLD`]).
//! - **Reprise d'un bilan de cabinet** : une immobilisation déjà amortie en partie est reprise
//!   avec son cumul d'amortissements ([`FixedAsset::prior_depreciation`]) ; la valeur nette
//!   restante s'amortit sur la durée qui reste à partir de la date du bilan d'ouverture
//!   ([`FixedAsset::depreciated_from`]) — le 28x repris plus les dotations calculées ici
//!   atteignent exactement la base au terme, quelle que soit la convention du cabinet.
//!
//! Le tableau 2033-C (immobilisations et amortissements, cases 400 à 576 — formulaire
//! 2033-C-SD, cadres I et II) range chaque compte 2xx dans une ligne ([`AssetRow`]).

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::{Date, OffsetDateTime};

use super::ids::{ExpenseId, FixedAssetId};
use super::money::Money;
use super::opening::{AccountCode, OpeningBalanceLine, Side};
use super::period::FiscalYear;

/// Valeur unitaire HT au-delà de laquelle un équipement ne peut plus passer en charge
/// (BOI-BIC-CHG-20-30-10 § 20 : 500 € HT, matériel et outillage, mobilier, logiciels).
pub const SMALL_EQUIPMENT_THRESHOLD: Money = Money::from_cents(50_000);

/// Durée d'amortissement proposée par défaut aux façades pour du matériel informatique :
/// trois ans, l'usage le plus courant.
pub const DEFAULT_DURATION_MONTHS: u32 = 36;

/// Nombre de jours entre deux dates sur une année de 360 jours faite de douze mois de
/// 30 jours (convention de l'amortissement linéaire, BOI-BIC-AMT-20-10 § 240) : `to` exclu.
/// Négatif si `to` précède `from`.
#[must_use]
pub fn days360(from: Date, to: Date) -> i64 {
    let years = i64::from(to.year()) - i64::from(from.year());
    let months = i64::from(u8::from(to.month())) - i64::from(u8::from(from.month()));
    let days = i64::from(to.day().min(30)) - i64::from(from.day().min(30));
    years * 360 + months * 30 + days
}

/// `date` avancée de `months` mois, le quantième borné au dernier jour du mois d'arrivée
/// (le 31 janvier + 1 mois = le 28 ou 29 février).
///
/// # Panics
///
/// Jamais : le mois d'arrivée est valide par construction et le quantième est borné à sa
/// longueur.
#[must_use]
pub fn add_months(date: Date, months: u32) -> Date {
    let total =
        i64::from(date.year()) * 12 + i64::from(u8::from(date.month())) - 1 + i64::from(months);
    let year = i32::try_from(total.div_euclid(12)).unwrap_or(i32::MAX);
    let month = u8::try_from(total.rem_euclid(12) + 1).expect("1..=12");
    let month = time::Month::try_from(month).expect("1..=12");
    let last = time::util::days_in_month(month, year);
    Date::from_calendar_date(year, month, date.day().min(last)).expect("date valide")
}

/// Inverse de [`add_months`] : `date` reculée de `months` mois, le quantième borné au dernier
/// jour du mois d'arrivée.
///
/// # Panics
///
/// Jamais : mêmes garanties que [`add_months`].
#[must_use]
pub fn sub_months(date: Date, months: u32) -> Date {
    let total =
        i64::from(date.year()) * 12 + i64::from(u8::from(date.month())) - 1 - i64::from(months);
    let year = i32::try_from(total.div_euclid(12)).unwrap_or(i32::MIN);
    let month = u8::try_from(total.rem_euclid(12) + 1).expect("1..=12");
    let month = time::Month::try_from(month).expect("1..=12");
    let last = time::util::days_in_month(month, year);
    Date::from_calendar_date(year, month, date.day().min(last)).expect("date valide")
}

/// Mise en service estimée d'une immobilisation reprise au bilan d'ouverture : le cumul
/// repris est réputé la fraction déjà écoulée de la durée, au prorata. Sans cumul, la mise en
/// service est le jour du bilan (toute la durée reste à courir).
#[must_use]
pub fn inferred_acquired_on(
    opens_on: Date,
    base: Money,
    prior: Money,
    duration_months: u32,
) -> Date {
    if prior.cents() <= 0 || base.cents() <= 0 {
        return opens_on;
    }
    let elapsed =
        i128::from(duration_months) * i128::from(prior.cents()) / i128::from(base.cents());
    let elapsed = u32::try_from(elapsed)
        .unwrap_or(duration_months)
        .min(duration_months);
    sub_months(opens_on, elapsed)
}

/// Le compte d'amortissement (28x) d'un compte d'immobilisation (2xx) : un `8` inséré après le
/// premier chiffre, le zéro final retiré pour garder la longueur usuelle — `218300` → `281830`,
/// `205000` → `280500`, `2183` → `28183`.
///
/// # Panics
///
/// Jamais : le résultat a entre 4 et 13 chiffres, tronqué à [`AccountCode::MAX_LEN`].
#[must_use]
pub fn depreciation_account(account: &AccountCode) -> AccountCode {
    let n = account.as_str();
    let mut out = format!("{}8{}", &n[..1], &n[1..]);
    if out.len() > 6 && out.ends_with('0') {
        out.pop();
    }
    out.truncate(AccountCode::MAX_LEN);
    AccountCode::parse(&out).expect("chiffres seulement, longueur bornée")
}

/// Ligne du tableau 2033-C-SD (cadres I immobilisations et II amortissements) d'un compte 2xx.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetRow {
    /// Fonds commercial (207) — cases 400 à 406, sans ligne d'amortissement propre.
    Goodwill,
    /// Autres immobilisations incorporelles (20x hors 207 : logiciels 205, frais 201…).
    OtherIntangible,
    /// Terrains (211, 212).
    Land,
    /// Constructions (213, 214).
    Buildings,
    /// Installations techniques, matériel et outillage industriels (215).
    TechnicalInstallations,
    /// Installations générales, agencements, aménagements divers (2181).
    GeneralFittings,
    /// Matériel de transport (2182).
    Transport,
    /// Autres immobilisations corporelles (2183 bureau et informatique, 2184 mobilier, 2185,
    /// 2188, immobilisations en cours 23x…).
    OtherTangible,
    /// Immobilisations financières (26, 27) — non amortissables.
    Financial,
}

impl AssetRow {
    /// Toutes les lignes, dans l'ordre du formulaire.
    pub const ALL: [Self; 9] = [
        Self::Goodwill,
        Self::OtherIntangible,
        Self::Land,
        Self::Buildings,
        Self::TechnicalInstallations,
        Self::GeneralFittings,
        Self::Transport,
        Self::OtherTangible,
        Self::Financial,
    ];

    /// La ligne d'un compte d'immobilisation (2xx hors 28/29) — les comptes d'amortissement
    /// 28x se rangent sur la ligne de leur immobilisation (`281830` → celle de `218300`).
    #[must_use]
    pub fn from_account(number: &str) -> Self {
        let n = if number.starts_with("28") || number.starts_with("29") {
            // 28x → le compte amorti : on retire le 8 en deuxième position.
            format!("2{}", &number[2..])
        } else {
            number.to_string()
        };
        let p = |prefix: &str| n.starts_with(prefix);
        if p("207") {
            Self::Goodwill
        } else if p("20") {
            Self::OtherIntangible
        } else if p("211") || p("212") {
            Self::Land
        } else if p("213") || p("214") {
            Self::Buildings
        } else if p("215") {
            Self::TechnicalInstallations
        } else if p("2181") {
            Self::GeneralFittings
        } else if p("2182") {
            Self::Transport
        } else if p("26") || p("27") {
            Self::Financial
        } else {
            Self::OtherTangible
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Goodwill => "goodwill",
            Self::OtherIntangible => "other_intangible",
            Self::Land => "land",
            Self::Buildings => "buildings",
            Self::TechnicalInstallations => "technical_installations",
            Self::GeneralFittings => "general_fittings",
            Self::Transport => "transport",
            Self::OtherTangible => "other_tangible",
            Self::Financial => "financial",
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Goodwill => "Fonds commercial",
            Self::OtherIntangible => "Autres immobilisations incorporelles",
            Self::Land => "Terrains",
            Self::Buildings => "Constructions",
            Self::TechnicalInstallations => {
                "Installations techniques, matériel et outillage industriels"
            }
            Self::GeneralFittings => "Installations générales, agencements, aménagements divers",
            Self::Transport => "Matériel de transport",
            Self::OtherTangible => "Autres immobilisations corporelles",
            Self::Financial => "Immobilisations financières",
        }
    }

    /// Cases du cadre I (valeur brute) : début d'exercice, augmentations, diminutions, fin.
    #[must_use]
    pub const fn gross_cases(self) -> [&'static str; 4] {
        match self {
            Self::Goodwill => ["400", "402", "404", "406"],
            Self::OtherIntangible => ["410", "412", "414", "416"],
            Self::Land => ["420", "422", "424", "426"],
            Self::Buildings => ["430", "432", "434", "436"],
            Self::TechnicalInstallations => ["440", "442", "444", "446"],
            Self::GeneralFittings => ["450", "452", "454", "456"],
            Self::Transport => ["460", "462", "464", "466"],
            Self::OtherTangible => ["470", "472", "474", "476"],
            Self::Financial => ["480", "482", "484", "486"],
        }
    }

    /// Cases du cadre II (amortissements) : début, dotations, diminutions, fin — `None` pour
    /// le fonds commercial et les immobilisations financières, sans ligne d'amortissement
    /// (les incorporelles n'en ont qu'une, 500 à 506).
    #[must_use]
    pub const fn depreciation_cases(self) -> Option<[&'static str; 4]> {
        match self {
            Self::Goodwill | Self::Financial => None,
            Self::OtherIntangible => Some(["500", "502", "504", "506"]),
            Self::Land => Some(["510", "512", "514", "516"]),
            Self::Buildings => Some(["520", "522", "524", "526"]),
            Self::TechnicalInstallations => Some(["530", "532", "534", "536"]),
            Self::GeneralFittings => Some(["540", "542", "544", "546"]),
            Self::Transport => Some(["550", "552", "554", "556"]),
            Self::OtherTangible => Some(["560", "562", "564", "566"]),
        }
    }

    /// Cases des totaux du cadre I (490 à 496).
    pub const GROSS_TOTAL_CASES: [&'static str; 4] = ["490", "492", "494", "496"];
    /// Cases des totaux du cadre II (570 à 576).
    pub const DEPRECIATION_TOTAL_CASES: [&'static str; 4] = ["570", "572", "574", "576"];
}

/// Libellé usuel d'un compte d'immobilisation du plan comptable général — les sous-comptes
/// courants d'un indépendant, sinon le libellé de sa ligne 2033-C.
#[must_use]
pub fn asset_account_label(number: &str) -> &'static str {
    let p = |prefix: &str| number.starts_with(prefix);
    if p("201") {
        "Frais d'établissement"
    } else if p("205") {
        "Concessions, brevets, licences, logiciels"
    } else if p("2183") {
        "Matériel de bureau et matériel informatique"
    } else if p("2184") {
        "Mobilier"
    } else if p("2154") {
        "Matériel industriel"
    } else if p("2155") {
        "Outillage industriel"
    } else if p("2135") {
        "Installations générales, agencements des constructions"
    } else {
        AssetRow::from_account(number).label()
    }
}

/// Une immobilisation amortissable, telle qu'enregistrée.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixedAsset {
    pub id: FixedAssetId,
    pub label: String,
    /// Compte d'immobilisation (2xx hors 26/27/28/29).
    pub account: AccountCode,
    /// Date de mise en service — le point de départ de l'amortissement (BOI-BIC-AMT-20-10
    /// § 120).
    #[serde(with = "crate::domain::serde_date::date")]
    pub acquired_on: Date,
    /// Base amortissable : le coût d'acquisition HT (TVA non déductible comprise).
    pub base: Money,
    /// Durée d'usage en mois (36 pour du matériel informatique, 60 pour du mobilier…).
    pub duration_months: u32,
    /// Date à partir de laquelle *cette application* calcule les dotations : la mise en
    /// service, ou la date du bilan d'ouverture pour une immobilisation reprise d'un bilan de
    /// cabinet (les dotations antérieures sont dans `prior_depreciation`).
    #[serde(with = "crate::domain::serde_date::date")]
    pub depreciated_from: Date,
    /// Amortissements cumulés à `depreciated_from`, repris au bilan d'ouverture (28x) — zéro
    /// pour une immobilisation acquise ici.
    pub prior_depreciation: Money,
    /// La dépense dont cette immobilisation est issue (« immobiliser » une dépense `equipment`
    /// au-delà de 500 € HT) : sa charge est alors remplacée par l'entrée à l'actif.
    pub expense_id: Option<ExpenseId>,
    pub revision: i64,
    #[serde(with = "crate::domain::serde_date::datetime")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Error, PartialEq, Eq, Clone)]
pub enum FixedAssetError {
    #[error("la base amortissable doit être strictement positive ({0})")]
    NonPositiveBase(Money),

    #[error("la durée d'amortissement doit être d'au moins un mois")]
    ZeroDuration,

    #[error("la durée d'amortissement ({0} mois) dépasse cinquante ans")]
    DurationTooLong(u32),

    #[error(
        "le compte {0} n'est pas un compte d'immobilisation amortissable (attendu 20x à 23x : \
         205 logiciels, 2183 matériel informatique, 2184 mobilier, 2182 véhicule…)"
    )]
    NotDepreciable(AccountCode),

    #[error(
        "les amortissements repris ({prior}) dépassent la base amortissable ({base}) : une \
         immobilisation ne s'amortit pas au-delà de son coût"
    )]
    PriorExceedsBase { prior: Money, base: Money },

    #[error(
        "les amortissements repris ({0}) supposent une immobilisation acquise avant le bilan \
         d'ouverture : ici la mise en service est postérieure, aucune dotation n'a pu être \
         pratiquée ailleurs"
    )]
    PriorWithoutReprise(Money),

    #[error("le libellé de l'immobilisation est vide")]
    EmptyLabel,
}

impl FixedAsset {
    /// Vérifie les invariants qui ne dépendent d'aucune base : base positive, durée bornée,
    /// compte amortissable, cumul repris cohérent.
    ///
    /// # Errors
    ///
    /// Voir [`FixedAssetError`].
    pub fn validate(&self) -> Result<(), FixedAssetError> {
        if self.label.trim().is_empty() {
            return Err(FixedAssetError::EmptyLabel);
        }
        if self.base.cents() <= 0 {
            return Err(FixedAssetError::NonPositiveBase(self.base));
        }
        if self.duration_months == 0 {
            return Err(FixedAssetError::ZeroDuration);
        }
        if self.duration_months > 600 {
            return Err(FixedAssetError::DurationTooLong(self.duration_months));
        }
        if !is_depreciable_account(&self.account) {
            return Err(FixedAssetError::NotDepreciable(self.account.clone()));
        }
        if self.prior_depreciation.is_negative() || self.prior_depreciation > self.base {
            return Err(FixedAssetError::PriorExceedsBase {
                prior: self.prior_depreciation,
                base: self.base,
            });
        }
        if !self.prior_depreciation.is_zero() && self.depreciated_from <= self.acquired_on {
            return Err(FixedAssetError::PriorWithoutReprise(
                self.prior_depreciation,
            ));
        }
        Ok(())
    }

    /// Le compte d'amortissement (28x) de cette immobilisation.
    #[must_use]
    pub fn depreciation_account(&self) -> AccountCode {
        depreciation_account(&self.account)
    }

    /// Le terme de l'amortissement : la mise en service plus la durée d'usage (exclu).
    #[must_use]
    pub fn depreciation_end(&self) -> Date {
        add_months(self.acquired_on, self.duration_months)
    }

    /// Ce qui reste à amortir à `depreciated_from`.
    #[must_use]
    pub fn net_at_start(&self) -> Money {
        self.base - self.prior_depreciation
    }

    /// Dotations cumulées calculées ici entre `depreciated_from` et `date` (exclue), linéaires
    /// en jours 360 sur la durée restante, bornées à la valeur nette de départ. Une durée
    /// restante nulle ou négative (immobilisation reprise après son terme, cumul repris
    /// incomplet) amortit tout le reste dès le premier jour.
    #[must_use]
    pub fn accumulated_since_start(&self, date: Date) -> Money {
        let net = self.net_at_start();
        if date <= self.depreciated_from || net.cents() <= 0 {
            return Money::ZERO;
        }
        let end = self.depreciation_end();
        let total = days360(self.depreciated_from, end);
        if total <= 0 || date >= end {
            return net;
        }
        let elapsed = days360(self.depreciated_from, date).clamp(0, total);
        // Arrondi au centime le plus proche, en arithmétique large : `net × elapsed / total`.
        let numerator = i128::from(net.cents()) * i128::from(elapsed);
        let cents = (numerator * 2 + i128::from(total)) / (2 * i128::from(total));
        Money::from_cents(i64::try_from(cents).unwrap_or(i64::MAX))
    }

    /// La dotation de l'exercice `period` : la différence des cumuls entre son premier jour et
    /// le lendemain de son dernier — prorata temporis le premier et le dernier exercice par
    /// construction, et la somme des dotations de tous les exercices vaut exactement la valeur
    /// nette de départ (chaque cumul est recalculé depuis la base, jamais additionné).
    #[must_use]
    pub fn depreciation_for(&self, period: FiscalYear) -> Money {
        let after = period.end().next_day().unwrap_or(period.end());
        self.accumulated_since_start(after) - self.accumulated_since_start(period.start())
    }

    /// Amortissements cumulés (repris + calculés) à `date` exclue.
    #[must_use]
    pub fn accumulated_at(&self, date: Date) -> Money {
        self.prior_depreciation + self.accumulated_since_start(date)
    }

    /// Amortissements cumulés au dernier jour de `period`.
    #[must_use]
    pub fn accumulated_at_end(&self, period: FiscalYear) -> Money {
        let after = period.end().next_day().unwrap_or(period.end());
        self.accumulated_at(after)
    }

    /// Valeur nette comptable au dernier jour de `period`.
    #[must_use]
    pub fn net_book_value_at_end(&self, period: FiscalYear) -> Money {
        self.base - self.accumulated_at_end(period)
    }

    /// `true` si l'immobilisation figure à l'actif pendant `period` (mise en service au plus
    /// tard le dernier jour, et pas encore sortie — le domaine ne modélise pas de cession).
    #[must_use]
    pub fn is_held_during(&self, period: FiscalYear) -> bool {
        self.acquired_on <= period.end()
    }

    /// Ligne du tableau 2033-C.
    #[must_use]
    pub fn row(&self) -> AssetRow {
        AssetRow::from_account(self.account.as_str())
    }
}

/// Un compte d'immobilisation qui s'amortit : classe 2, hors financières (26, 27) et hors
/// comptes d'amortissement et de dépréciation (28, 29). Les terrains (211) ne s'amortissent
/// pas non plus en principe, mais le domaine ne l'interdit pas (agencements de terrains 212).
#[must_use]
pub fn is_depreciable_account(account: &AccountCode) -> bool {
    account.class() == 2
        && !account.starts_with("26")
        && !account.starts_with("27")
        && !account.starts_with("28")
        && !account.starts_with("29")
}

/// Un couple compte d'immobilisation / compte d'amortissement lu dans un bilan d'ouverture
/// (lot 40 : « un 28x est repris, la dotation n'est pas calculée ») : ce qu'il faut déclarer
/// comme [`FixedAsset`] pour que la dotation le soit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetCandidate {
    pub account: AccountCode,
    pub label: String,
    /// Valeur brute reprise (solde débiteur du 2xx).
    pub gross: Money,
    /// Amortissements cumulés repris (solde créditeur du 28x associé), zéro s'il n'y en a pas.
    pub depreciation: Money,
}

impl AssetCandidate {
    #[must_use]
    pub fn net(&self) -> Money {
        self.gross - self.depreciation
    }
}

impl fmt::Display for AssetCandidate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {} : brut {}, amortissements repris {}, net {}",
            self.account,
            self.label,
            self.gross,
            self.depreciation,
            self.net()
        )
    }
}

/// Les immobilisations amortissables qu'un bilan d'ouverture reprend : chaque compte 2xx
/// amortissable au débit, apparié au 28x qui lui correspond (même numéro avec un `8` inséré,
/// ou le 28x dont le numéro se déduit — `281830` pour `218300`, `2818` pour `218`), dans
/// l'ordre des comptes.
#[must_use]
pub fn fixed_asset_candidates(lines: &[OpeningBalanceLine]) -> Vec<AssetCandidate> {
    let mut candidates: Vec<AssetCandidate> = lines
        .iter()
        .filter(|l| l.side == Side::Debit && is_depreciable_account(&l.account))
        .map(|l| {
            let expected = depreciation_account(&l.account);
            let depreciation = lines
                .iter()
                .filter(|d| {
                    d.side == Side::Credit
                        && d.account.starts_with("28")
                        && (d.account == expected
                            || d.account.as_str().starts_with(expected.as_str())
                            || expected.as_str().starts_with(d.account.as_str()))
                })
                .map(|d| d.amount)
                .sum();
            AssetCandidate {
                account: l.account.clone(),
                label: l.label.clone(),
                gross: l.amount,
                depreciation,
            }
        })
        .collect();
    candidates.sort_by(|a, b| a.account.cmp(&b.account));
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use time::Month as M;

    fn d(y: i32, m: u8, day: u8) -> Date {
        Date::from_calendar_date(y, M::try_from(m).unwrap(), day).unwrap()
    }

    fn account(n: &str) -> AccountCode {
        AccountCode::parse(n).unwrap()
    }

    fn asset(acquired: Date, base: i64, months: u32) -> FixedAsset {
        FixedAsset {
            id: FixedAssetId::new(),
            label: "Ordinateur portable".to_string(),
            account: account("218300"),
            acquired_on: acquired,
            base: Money::from_cents(base),
            duration_months: months,
            depreciated_from: acquired,
            prior_depreciation: Money::ZERO,
            expense_id: None,
            revision: 1,
            created_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn the_depreciation_account_inserts_an_eight_after_the_class() {
        assert_eq!(depreciation_account(&account("218300")).as_str(), "281830");
        assert_eq!(depreciation_account(&account("205000")).as_str(), "280500");
        assert_eq!(depreciation_account(&account("2183")).as_str(), "28183");
        assert_eq!(depreciation_account(&account("215400")).as_str(), "281540");
        assert_eq!(depreciation_account(&account("218")).as_str(), "2818");
    }

    #[test]
    fn days360_counts_thirty_day_months() {
        assert_eq!(days360(d(2026, 1, 1), d(2027, 1, 1)), 360);
        assert_eq!(days360(d(2026, 3, 15), d(2026, 10, 1)), 196);
        assert_eq!(days360(d(2026, 1, 31), d(2026, 2, 28)), 28);
        assert_eq!(days360(d(2026, 10, 1), d(2026, 9, 30)), -1);
    }

    #[test]
    fn add_months_clamps_to_the_end_of_the_target_month() {
        assert_eq!(add_months(d(2026, 1, 31), 1), d(2026, 2, 28));
        assert_eq!(add_months(d(2024, 1, 31), 1), d(2024, 2, 29));
        assert_eq!(add_months(d(2026, 3, 15), 36), d(2029, 3, 15));
        assert_eq!(add_months(d(2026, 11, 30), 14), d(2028, 1, 30));
        assert_eq!(sub_months(d(2026, 2, 28), 1), d(2026, 1, 28));
        assert_eq!(sub_months(d(2025, 10, 1), 18), d(2024, 4, 1));
        // 1 500 €, 900 € déjà amortis, 36 mois : 21 mois écoulés au prorata (900/1500 × 36).
        assert_eq!(
            inferred_acquired_on(
                d(2025, 10, 1),
                Money::from_cents(150_000),
                Money::from_cents(90_000),
                36
            ),
            d(2024, 1, 1)
        );
        assert_eq!(
            inferred_acquired_on(d(2025, 10, 1), Money::from_cents(150_000), Money::ZERO, 36),
            d(2025, 10, 1)
        );
    }

    /// 1 200 € sur 36 mois mis en service le 15 mars 2026, clôture au 30 septembre : première
    /// annuité 1 200 × 196 / 1 080 = 217,78 € (prorata temporis en jours 360), puis 400 € par
    /// exercice plein, et le solde le dernier — la somme vaut exactement la base.
    #[test]
    fn a_linear_schedule_is_prorated_on_the_first_and_last_exercises() {
        let a = asset(d(2026, 3, 15), 120_000, 36);
        let ex = |y: i32| FiscalYear::new(d(y - 1, 10, 1), d(y, 9, 30));
        assert_eq!(a.depreciation_for(ex(2026)), Money::from_cents(21_778));
        assert_eq!(a.depreciation_for(ex(2027)), Money::from_cents(40_000));
        assert_eq!(a.depreciation_for(ex(2028)), Money::from_cents(40_000));
        assert_eq!(a.depreciation_for(ex(2029)), Money::from_cents(18_222));
        assert_eq!(a.depreciation_for(ex(2030)), Money::ZERO);
        assert_eq!(a.depreciation_for(ex(2025)), Money::ZERO);
        assert_eq!(a.net_book_value_at_end(ex(2027)), Money::from_cents(58_222));
        assert_eq!(a.accumulated_at_end(ex(2029)), Money::from_cents(120_000));
        assert!(a.validate().is_ok());
    }

    /// Reprise d'un bilan de cabinet : ordinateur de 1 500 € acquis le 1er avril 2024 sur
    /// 36 mois, 900 € déjà amortis au 1er octobre 2025 (convention du cabinet). Il reste
    /// 600 € à amortir jusqu'au 1er avril 2027, soit 540 jours : 400 € sur l'exercice clos le
    /// 30 septembre 2026, 200 € sur le suivant, puis plus rien — le cumul atteint la base.
    #[test]
    fn a_reprise_depreciates_the_remaining_net_value_over_the_remaining_duration() {
        let mut a = asset(d(2024, 4, 1), 150_000, 36);
        a.depreciated_from = d(2025, 10, 1);
        a.prior_depreciation = Money::from_cents(90_000);
        assert!(a.validate().is_ok());
        let ex = |y: i32| FiscalYear::new(d(y - 1, 10, 1), d(y, 9, 30));
        assert_eq!(a.depreciation_for(ex(2026)), Money::from_cents(40_000));
        assert_eq!(a.depreciation_for(ex(2027)), Money::from_cents(20_000));
        assert_eq!(a.depreciation_for(ex(2028)), Money::ZERO);
        assert_eq!(a.accumulated_at_end(ex(2026)), Money::from_cents(130_000));
        assert_eq!(a.net_book_value_at_end(ex(2027)), Money::ZERO);
    }

    #[test]
    fn a_reprise_past_its_term_is_fully_depreciated_in_the_first_exercise() {
        let mut a = asset(d(2020, 1, 1), 100_000, 36);
        a.depreciated_from = d(2025, 10, 1);
        a.prior_depreciation = Money::from_cents(95_000);
        let ex = FiscalYear::new(d(2025, 10, 1), d(2026, 9, 30));
        assert_eq!(a.depreciation_for(ex), Money::from_cents(5_000));
        assert_eq!(a.net_book_value_at_end(ex), Money::ZERO);
    }

    #[test]
    fn validation_refuses_what_cannot_be_depreciated() {
        let mut a = asset(d(2026, 1, 1), 100_000, 36);
        a.account = account("261000");
        assert_eq!(
            a.validate(),
            Err(FixedAssetError::NotDepreciable(account("261000")))
        );
        a.account = account("281830");
        assert!(matches!(
            a.validate(),
            Err(FixedAssetError::NotDepreciable(_))
        ));
        let mut a = asset(d(2026, 1, 1), 0, 36);
        assert!(matches!(
            a.validate(),
            Err(FixedAssetError::NonPositiveBase(_))
        ));
        a.base = Money::from_cents(100);
        a.duration_months = 0;
        assert_eq!(a.validate(), Err(FixedAssetError::ZeroDuration));
        a.duration_months = 36;
        a.prior_depreciation = Money::from_cents(200);
        assert!(matches!(
            a.validate(),
            Err(FixedAssetError::PriorExceedsBase { .. })
        ));
        a.prior_depreciation = Money::from_cents(50);
        assert!(matches!(
            a.validate(),
            Err(FixedAssetError::PriorWithoutReprise(_))
        ));
        a.label = "  ".to_string();
        assert_eq!(a.validate(), Err(FixedAssetError::EmptyLabel));
    }

    #[test]
    fn accounts_map_to_the_2033c_rows() {
        assert_eq!(AssetRow::from_account("207000"), AssetRow::Goodwill);
        assert_eq!(AssetRow::from_account("205000"), AssetRow::OtherIntangible);
        assert_eq!(AssetRow::from_account("280500"), AssetRow::OtherIntangible);
        assert_eq!(AssetRow::from_account("211000"), AssetRow::Land);
        assert_eq!(AssetRow::from_account("213500"), AssetRow::Buildings);
        assert_eq!(
            AssetRow::from_account("215400"),
            AssetRow::TechnicalInstallations
        );
        assert_eq!(AssetRow::from_account("218100"), AssetRow::GeneralFittings);
        assert_eq!(AssetRow::from_account("218200"), AssetRow::Transport);
        assert_eq!(AssetRow::from_account("218300"), AssetRow::OtherTangible);
        assert_eq!(AssetRow::from_account("281830"), AssetRow::OtherTangible);
        assert_eq!(AssetRow::from_account("271000"), AssetRow::Financial);
        assert_eq!(
            AssetRow::OtherTangible.gross_cases(),
            ["470", "472", "474", "476"]
        );
        assert_eq!(
            AssetRow::OtherTangible.depreciation_cases(),
            Some(["560", "562", "564", "566"])
        );
        assert_eq!(AssetRow::Goodwill.depreciation_cases(), None);
        assert_eq!(
            asset_account_label("218300"),
            "Matériel de bureau et matériel informatique"
        );
        assert_eq!(
            asset_account_label("218800"),
            "Autres immobilisations corporelles"
        );
    }

    #[test]
    fn candidates_pair_each_gross_account_with_its_depreciation() {
        let lines: Vec<OpeningBalanceLine> = [
            "218300:Ordinateur:D:1500.00",
            "281830:Amort. ordinateur:C:900.00",
            "205000:Logiciel:D:800.00",
            "512000:Banque:D:1000.00",
            "101000:Capital:C:2400.00",
            "261000:Titres:D:100.00",
        ]
        .iter()
        .map(|l| l.parse().unwrap())
        .collect();
        let candidates = fixed_asset_candidates(&lines);
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].account.as_str(), "205000");
        assert_eq!(candidates[0].depreciation, Money::ZERO);
        assert_eq!(candidates[1].account.as_str(), "218300");
        assert_eq!(candidates[1].gross, Money::from_cents(150_000));
        assert_eq!(candidates[1].depreciation, Money::from_cents(90_000));
        assert_eq!(candidates[1].net(), Money::from_cents(60_000));
    }

    proptest! {
        /// Quelle que soit la découpe en exercices, la somme des dotations vaut exactement la
        /// valeur nette de départ, chaque dotation est positive ou nulle et le cumul ne dépasse
        /// jamais la base.
        #[test]
        fn depreciations_sum_to_the_net_value_and_never_exceed_the_base(
            base in 1i64..=10_000_000,
            prior_bps in 0u32..10_000,
            months in 1u32..=120,
            start_day in 0i64..3_000,
            fye_month in 1u8..=12,
        ) {
            let origin = d(2020, 1, 1);
            let acquired = origin + time::Duration::days(start_day);
            let mut a = asset(acquired, base, months);
            // Une reprise sur deux : un cumul repris et un départ décalé de six mois.
            if prior_bps % 2 == 1 {
                a.prior_depreciation = Money::from_cents(base * i64::from(prior_bps) / 10_000);
                a.depreciated_from = add_months(acquired, 6);
            }
            prop_assert!(a.validate().is_ok());
            let fye = super::super::period::FiscalYearEnd::new(fye_month, 28).unwrap();
            let mut period = fye.containing(a.depreciated_from);
            // Un exercice qui commence avant le départ compte aussi (dotation partielle).
            let mut total = Money::ZERO;
            for _ in 0..(months / 12 + 3) {
                let dotation = a.depreciation_for(period);
                prop_assert!(!dotation.is_negative());
                total += dotation;
                prop_assert!(a.accumulated_at_end(period) <= a.base);
                period = fye.containing(period.end().next_day().unwrap());
            }
            prop_assert_eq!(total, a.net_at_start());
        }
    }
}
