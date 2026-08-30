//! Écrans `/unlock` et `/setup` : rendus dans [`crate::layout::bare_page`], jamais dans la
//! coque applicative — pas de palette, pas de rail d'audit, pas de `app.js`.

use maud::{Markup, html};

pub fn unlock_form(error: Option<&str>) -> Markup {
    html! {
        div class="auth-card" {
            div class="auth-brand" { span class="dot" {} "freeflow" }
            div class="auth-title" { "Coffre verrouillé" }
            div class="auth-sub" { "Saisissez la passphrase pour le déverrouiller." }
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
                    "rester déverrouillé 12h, même après fermeture de l'application"
                }
                button class="auth-submit" type="submit" { "Déverrouiller" }
            }
            div class="auth-hint" { "Pour un usage en script ou en agent : `freeflow unlock --remember`." }
        }
    }
}

pub fn setup_form(error: Option<&str>) -> Markup {
    html! {
        div class="auth-card" {
            div class="auth-brand" { span class="dot" {} "freeflow" }
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
                    "rester déverrouillé 12h, même après fermeture de l'application"
                }
                button class="auth-submit" type="submit" { "Créer le coffre" }
            }
        }
    }
}
