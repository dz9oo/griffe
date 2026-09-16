//! La date du jour, pour les **adaptateurs** (lot 36).
//!
//! Le cœur ne lit jamais l'horloge lui-même : `today` est une donnée d'appel de chaque requête
//! ou commande qui en dépend (règle posée au lot 27), si bien qu'un test rejoue n'importe quelle
//! date et que « l'exercice est-il écoulé ? » n'est jamais un effet de bord caché. Ce module
//! n'existe que pour que CLI, serveur MCP et fenêtre obtiennent cette date **de la même façon**
//! — en heure **locale**, pas UTC : un indépendant qui clôt le 1er octobre à 0 h 30 à Paris est
//! bien le 1er octobre, quoi qu'en dise Greenwich.

use time::{Date, OffsetDateTime};

/// La date d'aujourd'hui dans le fuseau local de la machine, avec repli sur UTC si l'offset
/// local n'est pas déterminable (contexte multi-thread où `time` refuse de le lire, machine sans
/// zone configurée) — le repli reste une date plausible, jamais une erreur.
#[must_use]
pub fn today_local() -> Date {
    OffsetDateTime::now_local()
        .unwrap_or_else(|_| OffsetDateTime::now_utc())
        .date()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn today_is_within_a_day_of_utc() {
        let utc = OffsetDateTime::now_utc().date();
        let local = today_local();
        let gap = (local - utc).whole_days().abs();
        assert!(gap <= 1, "local {local} vs utc {utc}");
    }
}
