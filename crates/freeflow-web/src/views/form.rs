//! Composants de formulaire réutilisables — jusqu'au lot 15, chaque écran de saisie
//! (`/unlock`, `/setup`) répétait son propre balisage de champ. Ce module factorise le patron
//! `label + champ + erreur` pour les panneaux de saisie métier (`views::clients`, et les
//! suivants au fil des lots), en réutilisant les variables CSS déjà posées par `app.css`
//! (`.auth-field`, généralisé ici en `.field`).
//!
//! Volontairement limité aux composants qui ont un appelant réel — pas de composant posé par
//! anticipation sans utilisateur. Lot 16 (prospection, missions) a ajouté `date`/`select`/
//! `number`/`textarea`/`field_help`, en suivant ce même patron ; les prochains lots (devis…) en
//! ajouteront d'autres de la même façon.

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

/// Champ date (`<input type="date">`), valeur au format `AAAA-MM-JJ` — le même format que
/// `freeflow_core::domain::format_date`/`parse_date`, jamais reformaté par ce composant.
pub fn date(name: &str, label: &str, value: &str, error: Option<&str>) -> Markup {
    html! {
        div class="field" {
            label for=(name) { (label) }
            input id=(name) name=(name) type="date" value=(value) aria-invalid[error.is_some()];
            @if let Some(e) = error {
                div class="field-error" { (e) }
            }
        }
    }
}

/// Champ numérique (`<input type="number">`) — `step` porte la granularité attendue (`"1"` pour
/// un pourcentage, `"0.25"` pour un nombre de jours).
pub fn number(name: &str, label: &str, value: &str, step: &str, error: Option<&str>) -> Markup {
    html! {
        div class="field" {
            label for=(name) { (label) }
            input id=(name) name=(name) type="number" step=(step) value=(value) aria-invalid[error.is_some()];
            @if let Some(e) = error {
                div class="field-error" { (e) }
            }
        }
    }
}

/// Liste déroulante. `options` est `(valeur, libellé)` ; `selected` compare sur la valeur.
pub fn select(
    name: &str,
    label: &str,
    options: &[(&str, &str)],
    selected: &str,
    error: Option<&str>,
) -> Markup {
    html! {
        div class="field" {
            label for=(name) { (label) }
            select id=(name) name=(name) aria-invalid[error.is_some()] {
                @for (value, text) in options {
                    option value=(value) selected[*value == selected] { (text) }
                }
            }
            @if let Some(e) = error {
                div class="field-error" { (e) }
            }
        }
    }
}

/// Zone de texte multi-lignes — sert notamment à la collection de jalons d'une mission, un par
/// ligne : `<textarea>` est le seul moyen simple d'accepter une collection de taille variable
/// dans un formulaire `application/x-www-form-urlencoded` (une liste de champs répétés du même
/// nom n'y survit pas, `serde_urlencoded` ne garde que la dernière valeur — voir le commentaire
/// de `crate::missions` sur ce point).
pub fn textarea(name: &str, label: &str, value: &str, rows: u8, error: Option<&str>) -> Markup {
    html! {
        div class="field" {
            label for=(name) { (label) }
            textarea id=(name) name=(name) rows=(rows.to_string()) aria-invalid[error.is_some()] { (value) }
            @if let Some(e) = error {
                div class="field-error" { (e) }
            }
        }
    }
}

/// Rappel de syntaxe sous un champ (ex. le format `label:parts_bps[:AAAA-MM-JJ]` des jalons) —
/// jamais une erreur, juste de l'aide.
pub fn field_help(text: &str) -> Markup {
    html! {
        div class="field-help" { (text) }
    }
}

/// Champ fichier (`<input type="file">`) — n'a de sens que dans un formulaire en
/// `multipart/form-data` (`hx-encoding`), sinon le navigateur n'envoie que le nom. `accept`
/// filtre le sélecteur natif, sans rien valider côté serveur. Aucune valeur pré-remplie : un
/// navigateur n'en accepte pas pour ce type de champ.
pub fn file(name: &str, label: &str, accept: &str) -> Markup {
    html! {
        div class="field" {
            label for=(name) { (label) }
            input id=(name) name=(name) type="file" accept=(accept);
        }
    }
}

/// Case à cocher — soumise comme `name=on` quand cochée, absente sinon (sémantique HTML
/// standard, que le lecteur du formulaire interprète en booléen).
pub fn checkbox(name: &str, label: &str) -> Markup {
    html! {
        label class="field-checkbox" {
            input name=(name) type="checkbox" value="on";
            span { (label) }
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
