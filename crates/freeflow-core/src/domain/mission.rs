//! Missions polymorphes (régie / forfait / récurrent). La génération d'échéancier et les
//! calculs de TJM effectif sont portés par la couche applicative (lot 4).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::Date;

use super::ids::{ClientId, MissionId, OpportunityId, QuoteId};
use super::money::Money;
use super::period::{format_date, parse_date};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MissionKind {
    Regie { daily_rate: Money },
    Forfait { budget: Money },
    Recurrent { monthly_amount: Money },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Milestone {
    pub label: String,
    /// Part du budget total en dix-millièmes (`10_000` = 100 %).
    pub share_bps: u32,
    pub due_on: Option<Date>,
}

#[derive(Debug, Error, PartialEq, Eq)]
#[error(
    "jalon invalide : {0} (attendu « libellé:parts_bps[:AAAA-MM-JJ] », \
     ex. « Acompte:3000:2026-03-31 »)"
)]
pub struct MilestoneParseError(pub String);

/// Représentation texte partagée entre la CLI (`--milestone`) et la GUI (un jalon par ligne dans
/// un textarea) — un seul parseur pour les deux façades, comme la thèse « Studio » l'exige :
/// `label:parts_bps[:AAAA-MM-JJ]`. `label` peut contenir des `:`, seuls le premier et
/// (optionnellement) le dernier séparateur sont significatifs si le dernier segment est une date
/// valide.
impl FromStr for Milestone {
    type Err = MilestoneParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || MilestoneParseError(s.to_string());
        let (label, rest) = s.split_once(':').ok_or_else(err)?;
        let label = label.trim();
        if label.is_empty() {
            return Err(err());
        }

        // Le dernier segment est une date si et seulement s'il en parse une — ce qui permet à
        // `label` de contenir lui-même des `:` sans ambiguïté dans le cas courant (pas de date).
        let (share_str, due_on) = match rest.rsplit_once(':') {
            Some((share_str, date_str)) if parse_date(date_str).is_ok() => {
                (share_str, Some(parse_date(date_str).map_err(|_| err())?))
            }
            _ => (rest, None),
        };
        let share_bps: u32 = share_str.trim().parse().map_err(|_| err())?;

        Ok(Self {
            label: label.to_string(),
            share_bps,
            due_on,
        })
    }
}

impl fmt::Display for Milestone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.label, self.share_bps)?;
        if let Some(due_on) = self.due_on {
            write!(f, ":{}", format_date(due_on))?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Mission {
    pub id: MissionId,
    pub client_id: ClientId,
    pub quote_id: Option<QuoteId>,
    /// Lignée de prospection (lot 21) : l'opportunité dont cette mission est issue, posée par
    /// `WinOpportunity` (gain direct) ou héritée du devis par `AcceptQuote`. Comme `quote_id`,
    /// c'est une lignée, pas un lien éditable — `UpdateMission` ne l'expose pas. `None` pour une
    /// mission créée directement, ou antérieure à la migration `0012`.
    pub opportunity_id: Option<OpportunityId>,
    pub name: String,
    pub kind: MissionKind,
    pub milestones: Vec<Milestone>,
    pub started_on: Date,
    pub ended_on: Option<Date>,
    /// Révision optimiste (lot 16) — voir `crate::app::revision`.
    pub revision: i64,
    /// Retirée des listes actives sans prétendre à une date de fin — distinct de `ended_on`
    /// (lot 16) : clore une mission est un fait métier daté (TJM effectif, historique), archiver
    /// est un classement. C'est l'échappatoire pour une mission déjà facturée, donc indélébile.
    pub archived_at: Option<time::OffsetDateTime>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn milestone_display_and_from_str_round_trip_without_a_date() {
        let m = Milestone {
            label: "Acompte".to_string(),
            share_bps: 3_000,
            due_on: None,
        };
        assert_eq!(m.to_string().parse::<Milestone>().unwrap(), m);
    }

    #[test]
    fn milestone_display_and_from_str_round_trip_with_a_date() {
        let m = Milestone {
            label: "Solde".to_string(),
            share_bps: 7_000,
            due_on: Some(Date::from_calendar_date(2026, time::Month::March, 31).unwrap()),
        };
        assert_eq!(m.to_string().parse::<Milestone>().unwrap(), m);
    }

    #[test]
    fn milestone_from_str_rejects_a_missing_separator() {
        assert!("Acompte".parse::<Milestone>().is_err());
    }

    #[test]
    fn milestone_from_str_rejects_a_non_numeric_share() {
        assert!("Acompte:beaucoup".parse::<Milestone>().is_err());
    }

    #[test]
    fn milestone_from_str_rejects_an_empty_label() {
        assert!(":3000".parse::<Milestone>().is_err());
    }
}
