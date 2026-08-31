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
            // Le reset du champ et le refocus après soumission se font dans `app.js`, via un
            // écouteur `htmx:afterRequest` délégué sur `#console-form` : `hx-on--after-request`
            // (l'attribut htmx idiomatique pour ce cas) s'appuie sur `new Function`, que la CSP
            // de la fenêtre packagée bloque (`script-src 'self'`, sans `'unsafe-eval'`) — voir
            // `CLAUDE.md`.
            form class="input-row" id="console-form" hx-post="/console/run" hx-target="#console-log" hx-swap="beforeend" {
                span class="prompt" { "❯" }
                input type="text" name="line" placeholder="prospect pipeline --json" autocomplete="off" autofocus;
            }
        }
    }
}
