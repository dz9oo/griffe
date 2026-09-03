//! Écran `console` : la console de la GUI, qui réutilise verbatim le parseur de la CLI (lot 7,
//! `freeflow_cli::run_capturing`) — taper une commande ici ou dans un terminal emprunte
//! exactement le même chemin de code. Voir [`crate::console`] pour le handler `POST`.

use maud::{Markup, html};

use crate::layout::{ViewId, view_head};

/// L'aide de la console : les commandes du parcours de clôture, dans l'ordre où on les tape.
pub fn help(typed: &str) -> Markup {
    let lines = [
        (
            "setup status",
            "où en est la configuration (profil, point de départ, relevé)",
        ),
        (
            "year checklist 2026",
            "le parcours de clôture de l'exercice clos en 2026",
        ),
        (
            "bank list --unmatched",
            "les lignes du relevé qu'il reste à expliquer",
        ),
        (
            "expense record --transaction <id> …",
            "une dépense créée depuis une ligne du relevé",
        ),
        (
            "bank settle <id> --account 401000",
            "le règlement d'une dette reprise au bilan",
        ),
        (
            "year balance 2026",
            "la balance des comptes et le bilan dérivés",
        ),
        (
            "year close --period 2026",
            "clore l'exercice (résultat figé, affectation en projet)",
        ),
        (
            "year approve 2026 --approved-on AAAA-MM-JJ",
            "approuver les comptes (sauvegarde préalable)",
        ),
        ("year glossary", "le lexique des mots de la clôture"),
        (
            "<commande> --help",
            "l'aide détaillée de n'importe quelle commande",
        ),
    ];
    html! {
        @if !typed.is_empty() {
            div class="line" { span class="prompt" { "❯" } " " span class="cmd" { (typed) } }
        }
        div class="out" {
            "Les commandes utiles, dans l'ordre du parcours :"
            @for (cmd, what) in lines {
                "\n  " (cmd) "  —  " (what)
            }
        }
    }
}

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
                input type="text" name="line" placeholder="year checklist 2026 — ou « aide »" autocomplete="off" autofocus;
            }
        }
    }
}
