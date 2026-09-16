//! Panneau latéral générique (création/édition/détail/confirmation) : un unique conteneur
//! `#panel`, vide par défaut, gonflé par une requête htmx ciblée dessus. Voir
//! `layout::page` pour l'emplacement du conteneur et `assets/app.js` pour sa fermeture
//! (`Échap`, clic hors panneau, bouton `×`) — toutes purement côté client, sans aller-retour
//! serveur : seul son *contenu* passe par le réseau.

use maud::{Markup, html};

/// Enveloppe `content` dans l'en-tête standard (titre + bouton fermer) du panneau.
pub fn sheet(title: &str, content: Markup) -> Markup {
    html! {
        div class="panel-sheet" {
            div class="panel-head" {
                span { (title) }
                button class="panel-close" type="button" aria-label="fermer" { "×" }
            }
            div class="panel-body" { (content) }
        }
    }
}
