//! Écrans `/unlock` et `/setup` : rendus dans [`crate::layout::bare_page`], jamais dans la
//! coque applicative — pas de palette, pas de rail d'audit, pas de `app.js`.

use maud::{Markup, html};

/// `pending` : un changement de passphrase a été commencé et interrompu (voir
/// `StoreError::PassphraseChangeInterrupted`) — affiché avant même une première tentative,
/// pour ne pas laisser l'utilisateur deviner pourquoi son ancienne passphrase ne suffit plus.
pub fn unlock_form(error: Option<&str>, pending: bool) -> Markup {
    html! {
        div class="auth-card" {
            div class="auth-brand" { (crate::brand::wordmark()) }
            div class="auth-title" { "Coffre verrouillé" }
            div class="auth-sub" { "Saisissez la passphrase pour le déverrouiller." }
            @if pending {
                div class="auth-warning" {
                    "Un changement de passphrase a été interrompu. Réessayez avec la NOUVELLE "
                    "passphrase — la bascule se terminera d'elle-même. À défaut, restaurez la "
                    "sauvegarde `pre-passphrase-change-*` écrite juste avant le changement."
                }
            }
            @if let Some(err) = error {
                div class="auth-error" { (err) }
            }
            form method="post" action="/unlock" {
                div class="auth-field" {
                    label for="passphrase" { "Passphrase" }
                    input id="passphrase" type="password" name="passphrase" autocomplete="current-password" autofocus;
                }
                label class="auth-remember" {
                    input type="checkbox" name="remember" value="on";
                    "Au prochain lancement, ne pas redemander la passphrase pendant 12 h."
                }
                button class="auth-submit" type="submit" { "Déverrouiller" }
            }
            div class="auth-hint" { "En script, la même passphrase s'obtient hors de la fenêtre." }
        }
    }
}

pub fn setup_form(error: Option<&str>) -> Markup {
    html! {
        div class="auth-card" {
            div class="auth-brand" { (crate::brand::wordmark()) }
            div class="auth-title" { "Créer le coffre" }
            div class="auth-sub" { "Premier lancement : choisissez une passphrase." }
            div class="auth-warning" {
                "Cette passphrase est la seule clé de vos données. Il n'existe aucune "
                "récupération : si vous la perdez, le coffre est définitivement illisible."
            }
            @if let Some(err) = error {
                div class="auth-error" { (err) }
            }
            form method="post" action="/setup" {
                div class="auth-field" {
                    label for="passphrase" { "Nouvelle passphrase" }
                    input id="passphrase" type="password" name="passphrase" autocomplete="new-password" autofocus;
                }
                div class="auth-field" {
                    label for="confirm" { "Confirmez la passphrase" }
                    input id="confirm" type="password" name="confirm" autocomplete="new-password";
                }
                label class="auth-remember" {
                    input type="checkbox" name="remember" value="on";
                    "Au prochain lancement, ne pas redemander la passphrase pendant 12 h."
                }
                button class="auth-submit" type="submit" { "Créer le coffre" }
            }
        }
    }
}
