//! Les réglages, Le fichier : choisir, lire le compte-rendu, appliquer.

use maud::{Markup, html};

use crate::layout::ViewId;

/// Ce que la lecture ou l'application a donné. Le texte d'un compte-rendu est celui du cœur.
pub enum Notice {
    Compte(String),
    Faute(String),
}

/// Le fragment que le formulaire remplace. Le pli et le champ fichier restent.
pub fn body(notice: &Notice) -> Markup {
    match notice {
        Notice::Compte(text) => html! { pre class="compte-rendu" { (text) } },
        Notice::Faute(text) => html! { p class="compte-rendu-faute" { (text) } },
    }
}

/// La lettre, dans le pli des réglages.
pub fn letter(notice: Option<&Notice>) -> Markup {
    html! {
        div class="letter reglages-desk" id="fichier-letter" data-view=(ViewId::Societe.slug()) {
            a class="back" href="/societe"
              hx-get="/societe" hx-target="#content" hx-push-url="true" hx-swap="innerHTML" {
                "← La société"
            }
            p class="lede" { "Vous choisissez le fichier. Vous lisez le compte-rendu. Vous appliquez." }
            article class="reglages-card" {
                h3 { "Le fichier" }
                p { "Griffe n'ouvre pas Notion. Le fichier est déjà là, à côté du coffre." }
                form class="fichier-geste" method="post" action="/societe/fichier"
                     enctype="multipart/form-data"
                     hx-post="/societe/fichier" hx-encoding="multipart/form-data"
                     hx-target="#compte-rendu" hx-swap="innerHTML" hx-push-url="false" {
                    label class="pick-file" {
                        input class="sr-only" id="fichier-echange" name="fichier" type="file"
                              accept=".json,application/json" required;
                        span class="pick-name" data-empty="Choisir le fichier" { "Choisir le fichier" }
                    }
                    button class="quiet" type="submit" name="geste" value="lire" { "Lire" }
                    button class="seal" type="submit" name="geste" value="appliquer" { "Appliquer" }
                }
            }
            article class="reglages-card" {
                h3 { "Le compte-rendu" }
                div id="compte-rendu" {
                    @if let Some(notice) = notice {
                        (body(notice))
                    } @else {
                        p class="compte-rendu-vide" { "Le compte-rendu s'écrit ici." }
                    }
                }
            }
        }
    }
}
