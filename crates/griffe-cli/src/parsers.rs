//! Analyseurs de valeurs CLI partagés entre sous-commandes. Les types du domaine qui
//! implémentent déjà `FromStr` (identifiants typés, énumérations simples) se branchent
//! directement sur `clap::value_parser!` sans code supplémentaire ; seuls les types
//! nécessitant un format spécifique à la CLI (dates, montants, acteur) ont un analyseur dédié
//! ici.

use std::time::Duration;

use griffe_core::app::Actor;
use griffe_core::domain::{self, LossReason, Money, OpeningBalanceLine, Siren, VatNumber};
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

/// Mois civil `AAAA-MM`.
///
/// # Errors
pub fn parse_month(s: &str) -> Result<domain::Month, String> {
    let (year_str, month_str) = s
        .split_once('-')
        .ok_or_else(|| format!("mois invalide : {s} (attendu AAAA-MM)"))?;
    let year: i32 = year_str
        .parse()
        .map_err(|_| format!("mois invalide : {s} (attendu AAAA-MM)"))?;
    let month: u8 = month_str
        .parse()
        .map_err(|_| format!("mois invalide : {s} (attendu AAAA-MM)"))?;
    domain::Month::new(year, month).map_err(|e| e.to_string())
}

/// # Errors
pub fn parse_money(s: &str) -> Result<Money, String> {
    Money::parse_decimal(s).map_err(|e| e.to_string())
}

/// Une ligne de bilan d'ouverture, `compte:libellé:D|C:montant` — le parseur du domaine
/// (`OpeningBalanceLine: FromStr`), partagé avec la fenêtre et le serveur MCP.
///
/// # Errors
pub fn parse_opening_line(s: &str) -> Result<OpeningBalanceLine, String> {
    s.parse::<OpeningBalanceLine>().map_err(|e| e.to_string())
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

/// Durée de session : un entier suivi de `m` (minutes), `h` (heures) ou `d` (jours) — `12h`,
/// `30m`, `7d`. Volontairement minimal : le dépôt parse déjà ses dates à la main plutôt que
/// d'ajouter une dépendance dédiée pour un format aussi simple.
///
/// # Errors
pub fn parse_ttl(s: &str) -> Result<Duration, String> {
    let invalid =
        || format!("durée invalide : {s} (attendu un entier suivi de m, h ou d — ex. 12h)");
    let (digits, unit) = s.split_at(s.len().saturating_sub(1));
    let amount: u64 = digits.parse().map_err(|_| invalid())?;
    let seconds = match unit {
        "m" => amount.checked_mul(60),
        "h" => amount.checked_mul(3_600),
        "d" => amount.checked_mul(86_400),
        _ => None,
    }
    .ok_or_else(invalid)?;
    Ok(Duration::from_secs(seconds))
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
