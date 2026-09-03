//! Le lexique de la clôture (lot 39) : les dix-huit mots du cœur (`closing::GLOSSARY`), dans
//! un panneau accessible depuis l'en-tête (`?`) et la palette — jusqu'ici replié sous le
//! parcours de clôture seulement.

use freeflow_core::closing::GLOSSARY;
use maud::{Markup, html};

use crate::views::panel;

pub fn panel() -> Markup {
    let body = html! {
        div class="detail-note" { "Les mots de la comptabilité, expliqués dans l'ordre où vous les rencontrez." }
        dl class="detail-list" {
            @for entry in GLOSSARY {
                dt { (entry.term) }
                dd { (entry.meaning) }
            }
        }
    };
    panel::sheet("Lexique — les mots de la clôture", body)
}
