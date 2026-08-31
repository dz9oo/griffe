//! Composants de formulaire réutilisables — jusqu'au lot 15, chaque écran de saisie
//! (`/unlock`, `/setup`) répétait son propre balisage de champ. Ce module factorise le patron
//! `label + champ + erreur` pour les panneaux de saisie métier (`views::clients`, et les
//! suivants au fil des lots), en réutilisant les variables CSS déjà posées par `app.css`
//! (`.auth-field`, généralisé ici en `.field`).
//!
//! Volontairement limité aux composants qu'un vrai formulaire de ce lot utilise (`text`,
//! `hidden`, `actions`, les deux bandeaux) — pas de `select`/`date`/`checkbox` posés par
//! anticipation sans appelant réel : les prochains lots (missions, devis…) en auront besoin
//! pour de vrais champs énumérés/datés, à ajouter alors en suivant exactement ce patron.

use maud::{Markup, html};

/// Champ texte simple. `value` est déjà la valeur à afficher (celle re-soumise en cas d'erreur,
/// ou celle de l'entité en édition) — jamais recalculée par ce composant.
pub fn text(name: &str, label: &str, value: &str, error: Option<&str>) -> Markup {
    html! {
        div class="field" {
            label for=(name) { (label) }
            input id=(name) name=(name) type="text" value=(value) aria-invalid[error.is_some()];
            @if let Some(e) = error {
                div class="field-error" { (e) }
            }
        }
    }
}

/// Champ caché — porte la révision optimiste d'une entité en édition (voir `CLAUDE.md`,
/// « garde-fou d'écriture concurrente ») : invisible, mais bien soumis avec le reste du
/// formulaire.
pub fn hidden(name: &str, value: &str) -> Markup {
    html! {
        input type="hidden" name=(name) value=(value);
    }
}

/// Ligne d'actions en pied de formulaire : un unique bouton de soumission — fermer le panneau
/// sans enregistrer se fait par `Échap`, un clic hors panneau, ou le bouton `×` de son en-tête
/// (voir `app.js`), jamais par un second bouton qui devrait lui aussi être stylé et testé.
pub fn actions(submit_label: &str) -> Markup {
    html! {
        div class="form-actions" {
            button class="btn primary" type="submit" { (submit_label) }
        }
    }
}

/// Bandeau d'erreur générique (échec de validation ou règle métier refusée par le cœur).
pub fn error_banner(message: &str) -> Markup {
    html! {
        div class="form-error" { (message) }
    }
}

/// Bandeau de conflit d'écriture (`AppError::Conflict`) : `reload_hx_get` recharge le panneau
/// depuis l'état actuel plutôt que de laisser l'utilisateur réessayer à l'aveugle sur une
/// révision déjà périmée une seconde fois.
pub fn conflict_banner(message: &str, reload_hx_get: &str) -> Markup {
    html! {
        div class="form-conflict" {
            div { (message) }
            button class="btn" type="button" hx-get=(reload_hx_get) hx-target="#panel" hx-swap="innerHTML" {
                "recharger la fiche actuelle"
            }
        }
    }
}
