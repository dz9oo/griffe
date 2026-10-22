//! Wordmark et paraphe de la marque Griffe (visage UI uniquement).

use maud::{Markup, PreEscaped, html};

/// Lockup chrome / auth : *Griffe* + paraphe sous le nom.
pub fn wordmark() -> Markup {
    html! {
        span.word { "Griffe" }
        span.paraphe aria-hidden="true" { (PreEscaped(include_str!("../assets/lockup.svg"))) }
    }
}

/// Grand paraphe de la lecture. Les chemins sont ceux de `lockup.svg`.
/// Le trait n'est pas dans le bloc sondé : un sondage ne le relance pas.
#[must_use]
pub fn reading_glyph(stall: bool) -> Markup {
    let class = if stall { "glyph stall" } else { "glyph draw" };
    html! {
        div id="boite-glyph" class=(class) {
            span class="word" { "Griffe" }
            svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 128 22" fill="none" aria-hidden="true" {
                path class="bowl" pathLength="100" d="M8 16c-4 0-6-3-6-6 0-5 4-8 9-8 3 0 6 1.5 7 5" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" {}
                path class="tail" pathLength="100" d="M12 12h8c0 4-3 7-8 7 8 1 22-4 48-10 18-4 40-4 58 2" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" {}
                @if !stall {
                    circle class="nib" r="1.15" fill="currentColor" {
                        animateMotion dur="1.35s" begin="0s" fill="freeze"
                            path="M12 12h8c0 4-3 7-8 7 8 1 22-4 48-10 18-4 40-4 58 2" {}
                    }
                }
            }
        }
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
