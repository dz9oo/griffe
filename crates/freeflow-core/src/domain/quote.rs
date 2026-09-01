//! Devis. La conversion en mission (lot 6) dérive l'échéancier de facturation de ces lignes.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::{Date, OffsetDateTime};

use super::ids::{ClientId, OpportunityId, QuoteId};
use super::money::Money;
use super::vat::VatRate;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QuoteStatus {
    Draft,
    Sent,
    Accepted,
    Declined,
    Expired,
}

#[derive(Debug, Error, PartialEq, Eq)]
#[error("statut de devis inconnu : {0}")]
pub struct UnknownQuoteStatus(pub String);

impl QuoteStatus {
    #[must_use]
    pub const fn is_closed(self) -> bool {
        matches!(self, Self::Accepted | Self::Declined | Self::Expired)
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Sent => "sent",
            Self::Accepted => "accepted",
            Self::Declined => "declined",
            Self::Expired => "expired",
        }
    }
}

impl std::str::FromStr for QuoteStatus {
    type Err = UnknownQuoteStatus;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "draft" => Ok(Self::Draft),
            "sent" => Ok(Self::Sent),
            "accepted" => Ok(Self::Accepted),
            "declined" => Ok(Self::Declined),
            "expired" => Ok(Self::Expired),
            other => Err(UnknownQuoteStatus(other.to_string())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LineKind {
    Regie { daily_rate: Money, days: f64 },
    Forfait { amount: Money },
    Recurrent { monthly_amount: Money, months: u32 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuoteLine {
    pub description: String,
    pub kind: LineKind,
    pub vat_rate: VatRate,
}

#[derive(Debug, Error, PartialEq, Eq)]
#[error(
    "ligne de devis invalide : {0} (attendu « description:type:montant[:taux] » — \
     « Développement:forfait:1350.00 », « Conseil:regie:650.00x10 » (TJM×jours), \
     « TMA:recurrent:2000.00x12 » (mensuel×mois) ; taux optionnel : standard, intermediate, \
     reduced, super_reduced, zero — standard si omis)"
)]
pub struct QuoteLineParseError(pub String);

/// Représentation texte partagée entre la CLI (`--line`, répétable) et la GUI (une ligne de
/// devis par ligne d'un textarea) — un seul parseur pour les deux façades, comme la thèse
/// « Studio » l'exige, sur le modèle de [`super::Milestone`] :
/// `description:type:montant[:taux]`. `description` peut contenir des `:` sans ambiguïté :
/// l'analyse part de la droite, et `type` est un mot-clé exact (`forfait`/`regie`/`recurrent`).
impl FromStr for QuoteLine {
    type Err = QuoteLineParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || QuoteLineParseError(s.to_string());
        // Le taux est optionnel : le dernier segment n'en est un que s'il en parse un — même
        // astuce que la date optionnelle des jalons (`Milestone::from_str`).
        let (rest, vat_rate) = match s.rsplit_once(':') {
            Some((rest, last)) => match last.trim().parse::<VatRate>() {
                Ok(rate) => (rest, rate),
                Err(_) => (s, VatRate::Standard),
            },
            None => (s, VatRate::Standard),
        };
        let (rest, spec) = rest.rsplit_once(':').ok_or_else(err)?;
        let (description, kind_word) = rest.rsplit_once(':').ok_or_else(err)?;
        let description = description.trim();
        if description.is_empty() {
            return Err(err());
        }
        let spec = spec.trim();
        let kind = match kind_word.trim() {
            "forfait" => LineKind::Forfait {
                amount: Money::parse_decimal(spec).map_err(|_| err())?,
            },
            "regie" => {
                let (rate, days) = spec.split_once('x').ok_or_else(err)?;
                let daily_rate = Money::parse_decimal(rate.trim()).map_err(|_| err())?;
                let days: f64 = days.trim().parse().map_err(|_| err())?;
                if !days.is_finite() || days <= 0.0 {
                    return Err(err());
                }
                LineKind::Regie { daily_rate, days }
            }
            "recurrent" => {
                let (amount, months) = spec.split_once('x').ok_or_else(err)?;
                let monthly_amount = Money::parse_decimal(amount.trim()).map_err(|_| err())?;
                let months: u32 = months.trim().parse().map_err(|_| err())?;
                if months == 0 {
                    return Err(err());
                }
                LineKind::Recurrent {
                    monthly_amount,
                    months,
                }
            }
            _ => return Err(err()),
        };
        Ok(Self {
            description: description.to_string(),
            kind,
            vat_rate,
        })
    }
}

impl fmt::Display for QuoteLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:", self.description)?;
        match &self.kind {
            LineKind::Forfait { amount } => write!(f, "forfait:{}", amount.to_decimal_string())?,
            LineKind::Regie { daily_rate, days } => {
                write!(f, "regie:{}x{days}", daily_rate.to_decimal_string())?;
            }
            LineKind::Recurrent {
                monthly_amount,
                months,
            } => write!(
                f,
                "recurrent:{}x{months}",
                monthly_amount.to_decimal_string()
            )?,
        }
        // Le taux est toujours émis, même `standard` : la représentation sert aussi à pré-remplir
        // un formulaire d'édition, où l'implicite serait une information perdue pour le lecteur.
        write!(f, ":{}", self.vat_rate.as_str())
    }
}

/// Une remise globale sur un devis, allouée proportionnellement sur les lignes (méthode du
/// plus fort reste via [`super::Money::allocate_proportionally`]) — jamais appliquée ligne par
/// ligne, pour ne jamais perdre ni créer un centime.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Discount {
    /// En dix-millièmes (`10_000` = 100 %).
    Percentage(u32),
    FixedAmount(Money),
}

/// Une version de devis. Le contenu (lignes, remise, validité, conditions) est immuable dès la
/// création — pour réviser un devis, on crée une nouvelle version portant le même `root_id`,
/// jamais une modification en place. Seul `status` évolue, via des transitions contrôlées par
/// la couche applicative (lot 6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Quote {
    pub id: QuoteId,
    /// Id de la toute première version de ce devis — vaut `id` pour la version 1 elle-même.
    /// Permet de retrouver toutes les versions d'un même devis par une simple égalité.
    pub root_id: QuoteId,
    pub client_id: ClientId,
    pub opportunity_id: Option<OpportunityId>,
    pub version: u32,
    pub status: QuoteStatus,
    pub lines: Vec<QuoteLine>,
    pub discount: Option<Discount>,
    pub terms: Option<String>,
    pub valid_until: Date,
    pub created_at: OffsetDateTime,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(description: &str, kind: LineKind, vat_rate: VatRate) -> QuoteLine {
        QuoteLine {
            description: description.to_string(),
            kind,
            vat_rate,
        }
    }

    #[test]
    fn quote_line_display_and_from_str_round_trip_for_every_kind() {
        let lines = [
            line(
                "Développement",
                LineKind::Forfait {
                    amount: Money::from_cents(1_350_000),
                },
                VatRate::Standard,
            ),
            line(
                "Conseil",
                LineKind::Regie {
                    daily_rate: Money::from_cents(65_000),
                    days: 10.5,
                },
                VatRate::Reduced,
            ),
            line(
                "TMA",
                LineKind::Recurrent {
                    monthly_amount: Money::from_cents(200_000),
                    months: 12,
                },
                VatRate::Zero,
            ),
        ];
        for l in lines {
            assert_eq!(l.to_string().parse::<QuoteLine>(), Ok(l));
        }
    }

    #[test]
    fn quote_line_description_may_contain_colons() {
        let l = line(
            "Phase 1 : cadrage",
            LineKind::Forfait {
                amount: Money::from_cents(100_000),
            },
            VatRate::Standard,
        );
        assert_eq!(l.to_string(), "Phase 1 : cadrage:forfait:1000.00:standard");
        assert_eq!(l.to_string().parse::<QuoteLine>(), Ok(l));
    }

    #[test]
    fn quote_line_vat_rate_defaults_to_standard_when_omitted() {
        assert_eq!(
            "Audit:forfait:500".parse::<QuoteLine>(),
            Ok(line(
                "Audit",
                LineKind::Forfait {
                    amount: Money::from_cents(50_000),
                },
                VatRate::Standard,
            ))
        );
    }

    #[test]
    fn quote_line_accepts_a_comma_as_decimal_separator_and_surrounding_spaces() {
        assert_eq!(
            "Conseil : regie : 650,50 x 3 : reduced".parse::<QuoteLine>(),
            Ok(line(
                "Conseil",
                LineKind::Regie {
                    daily_rate: Money::from_cents(65_050),
                    days: 3.0,
                },
                VatRate::Reduced,
            ))
        );
    }

    #[test]
    fn quote_line_rejects_malformed_input() {
        for bad in [
            "",
            "sans-separateur",
            "desc:forfait",              // pas de montant
            "desc:inconnu:100",          // type inexistant
            ":forfait:100",              // description vide
            "desc:forfait:100x2",        // quantité sur un forfait
            "desc:regie:650",            // régie sans jours
            "desc:regie:650x0",          // zéro jour
            "desc:regie:650x-1",         // jours négatifs
            "desc:regie:650xNaN",        // jours non numériques
            "desc:recurrent:2000",       // récurrent sans mois
            "desc:recurrent:2000x0",     // zéro mois
            "desc:recurrent:2000x1.5",   // mois fractionnaires
            "desc:forfait:100:exotique", // ni un taux ni un montant en dernier segment
        ] {
            assert!(
                bad.parse::<QuoteLine>().is_err(),
                "aurait dû échouer : {bad}"
            );
        }
    }
}
