//! Analyseurs de valeurs CLI partagés entre sous-commandes. Les types du domaine qui
//! implémentent déjà `FromStr` (identifiants typés, énumérations simples) se branchent
//! directement sur `clap::value_parser!` sans code supplémentaire ; seuls les types
//! nécessitant un format spécifique à la CLI (dates, montants, acteur) ont un analyseur dédié
//! ici.

use freeflow_core::app::Actor;
use freeflow_core::domain::{self, LossReason, Money, Siren, VatNumber};
use time::Date;

/// # Errors
pub fn parse_siren(s: &str) -> Result<Siren, String> {
    Siren::parse(s).map_err(|e| e.to_string())
}

/// # Errors
pub fn parse_vat_number(s: &str) -> Result<VatNumber, String> {
    VatNumber::parse(s).map_err(|e| e.to_string())
}

/// # Errors
pub fn parse_date(s: &str) -> Result<Date, String> {
    domain::parse_date(s).map_err(|e| e.to_string())
}

/// # Errors
pub fn parse_money(s: &str) -> Result<Money, String> {
    Money::parse_decimal(s).map_err(|e| e.to_string())
}

/// # Errors
pub fn parse_probability(s: &str) -> Result<domain::Probability, String> {
    let percent: u8 = s
        .parse()
        .map_err(|_| format!("probabilité invalide : {s} (attendu un entier 0-100)"))?;
    domain::Probability::new(percent).map_err(|e| e.to_string())
}

/// Format : `human` (par défaut), `system`, ou `agent:<session>`.
///
/// # Errors
pub fn parse_actor(s: &str) -> Result<Actor, String> {
    match s {
        "human" => Ok(Actor::Human),
        "system" => Ok(Actor::System),
        other => other
            .strip_prefix("agent:")
            .filter(|session| !session.is_empty())
            .map(|session| Actor::Agent {
                session: session.to_string(),
            })
            .ok_or_else(|| {
                format!("acteur invalide : {other} (attendu human, system, ou agent:<session>)")
            }),
    }
}

/// Format : `budget`, `timing`, `competitor`, `no-response`, `scope-mismatch`, ou
/// `other:<détail>`.
///
/// # Errors
pub fn parse_loss_reason(s: &str) -> Result<LossReason, String> {
    match s {
        "budget" => Ok(LossReason::Budget),
        "timing" => Ok(LossReason::Timing),
        "competitor" => Ok(LossReason::Competitor),
        "no-response" => Ok(LossReason::NoResponse),
        "scope-mismatch" => Ok(LossReason::ScopeMismatch),
        other => other
            .strip_prefix("other:")
            .map(|detail| LossReason::Other(detail.to_string()))
            .ok_or_else(|| format!("motif de perte invalide : {other}")),
    }
}
