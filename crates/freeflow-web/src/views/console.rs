//! Écran `console` : la console de la GUI, qui réutilise verbatim le parseur de la CLI (lot 7,
//! `freeflow_cli::run_capturing`) — taper une commande ici ou dans un terminal emprunte
//! exactement le même chemin de code. Voir [`crate::console`] pour le handler `POST`.

use maud::{Markup, html};

use crate::layout::{ViewId, view_head};

pub fn render() -> Markup {
    html! {
        (view_head(ViewId::Console, "même API que la CLI et le serveur MCP — aucune logique dupliquée"))

        div class="console" {
            div class="console-log" id="console-log" {}
            form class="input-row" hx-post="/console/run" hx-target="#console-log" hx-swap="beforeend" hx-on--after-request="this.reset(); this.querySelector('input').focus();" {
                span class="prompt" { "❯" }
                input type="text" name="line" placeholder="prospect pipeline --json" autocomplete="off" autofocus;
            }
        }
    }
}
