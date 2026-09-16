//! Wordmark et paraphe de la marque Griffe (visage UI uniquement).

use maud::{Markup, PreEscaped, html};

/// Lockup chrome / auth : *Griffe* + paraphe sous le nom.
pub fn wordmark() -> Markup {
    html! {
        span.word { "Griffe" }
        span.paraphe aria-hidden="true" { (PreEscaped(include_str!("../assets/lockup.svg"))) }
    }
}

/// Paraphe seule (icône « G qui s'échappe »). Réservée à l'icône / usages hors lockup.
#[allow(dead_code)] // API de marque ; consommée par le test unit + icône desktop
pub fn mark() -> Markup {
    html! {
        span.paraphe aria-hidden="true" { (PreEscaped(include_str!("../assets/mark.svg"))) }
    }
}

#[cfg(test)]
mod tests {
    use super::mark;

    #[test]
    fn mark_embeds_the_canonical_viewbox() {
        let html = mark().into_string();
        assert!(
            html.contains(r#"viewBox="0 0 88 56""#),
            "mark() doit embarquer mark.svg : {html}"
        );
    }
}
