//! Chapitre Le courrier. Trois blocs, la largeur de La société.

use griffe_core::app::AppError;
use griffe_core::mail::{MailPreset, MailProfile, profile};
use griffe_core::store::Store;
use maud::{Markup, html};
use time::OffsetDateTime;

use crate::layout::ViewId;

pub struct CourrierForm {
    pub from_name: String,
    pub from_address: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub preset: String,
    pub error: Option<String>,
    pub notice: Option<String>,
}

pub fn page(store: &Store, form: Option<&CourrierForm>) -> Result<Markup, AppError> {
    let account = profile(store.connection())?;
    let pause = griffe_core::mail::hourly_pause(store.connection(), OffsetDateTime::now_utc())?;
    Ok(markup(&account, form, pause))
}

fn markup(account: &MailProfile, form: Option<&CourrierForm>, pause: bool) -> Markup {
    let posted = form.filter(|item| item.error.is_some());
    let from_name = posted.map_or(account.from_name.as_str(), |item| item.from_name.as_str());
    let from_address = posted.map_or(account.from_address.as_str(), |item| {
        item.from_address.as_str()
    });
    let host = posted.map_or(account.host.as_str(), |item| item.host.as_str());
    let username = posted.map_or(account.username.as_str(), |item| item.username.as_str());
    let preset = posted.map_or(account.preset, |item| MailPreset::parse(&item.preset));
    let port = posted.map_or(account.port, |item| item.port);
    let icloud = preset == MailPreset::Icloud;
    html! {
        div class="letter" data-view=(ViewId::Societe.slug()) {
            (back())
            h1 { "Le courrier." }
            p class="lede" { (lede(account)) }
            @if let Some(form) = form {
                @if let Some(error) = &form.error {
                    p class="mast-note" role="status" { (error) }
                }
                @if let Some(notice) = &form.notice {
                    p class="mast-note" role="status" { (notice) }
                }
            }
            @if pause {
                p class="prose" { "Trop de lettres cette heure. Celles qui restent attendent." }
            }
            form hx-post="/societe/courrier" hx-target="#content" hx-push-url="true" {
                div class="block" {
                    h3 { "D'où partent les lettres" }
                    p class="prose" { "L'adresse qui signe. Les brouillons reprennent la même." }
                    div class="field" {
                        label for="from_name" { "Nom affiché" }
                        input id="from_name" name="from_name" type="text" value=(from_name);
                    }
                    div class="field" {
                        label for="from_address" { "L'adresse qui signe" }
                        input id="from_address" name="from_address" type="text" value=(from_address) autocomplete="off";
                    }
                }
                div class="block" {
                    h3 { "Le serveur d'envoi" }
                    fieldset class="word-choice" {
                        legend { "Serveur" }
                        div class="word-choice-row" {
                            label {
                                input type="radio" name="preset" value="icloud" checked[icloud];
                                "iCloud"
                            }
                            label {
                                input type="radio" name="preset" value="custom" checked[!icloud];
                                "un autre"
                            }
                        }
                    }
                    @if icloud {
                        p class="prose" {
                            "smtp.mail.me.com, port 587, liaison chiffrée. L'identifiant est l'adresse complète du compte, en icloud.com, me.com ou mac.com. Le mot de passe est un mot de passe d'application."
                        }
                        p class="prose" {
                            a href="https://account.apple.com" { "Le compte Apple" }
                            " crée ce mot de passe."
                        }
                    }
                    div class="field" {
                        label for="username" { "L'identifiant du serveur" }
                        input id="username" name="username" type="text" value=(username) autocomplete="off";
                    }
                    @if !icloud {
                        div class="field" {
                            label for="host" { "Hôte" }
                            input id="host" name="host" type="text" value=(host) autocomplete="off";
                        }
                        fieldset class="word-choice" {
                            legend { "Port" }
                            div class="word-choice-row" {
                                label {
                                    input type="radio" name="port" value="587" checked[port != 465];
                                    "587"
                                }
                                label {
                                    input type="radio" name="port" value="465" checked[port == 465];
                                    "465"
                                }
                            }
                        }
                    }
                    div class="row-actions" {
                        button class="seal" type="submit" { "Enregistrer" }
                    }
                }
            }
            div class="block" {
                h3 { "Le mot de passe" }
                p class="prose" {
                    @if account.has_secret {
                        "Un mot de passe est déjà dans le coffre. Laisser le champ vide le garde."
                    } @else {
                        "Il reste dans le coffre chiffré. La page ne le réaffiche pas."
                    }
                }
                form hx-post="/societe/courrier/secret" hx-target="#content" {
                    div class="field" {
                        label for="secret" { "Mot de passe du serveur" }
                        input id="secret" name="secret" type="password" autocomplete="new-password" aria-label="Mot de passe du serveur";
                    }
                    div class="row-actions" {
                        button class="seal" type="submit" { "Enregistrer le mot de passe" }
                    }
                }
                @if account.has_secret {
                    form hx-post="/societe/courrier/retirer" hx-target="#content" {
                        div class="row-actions" {
                            button class="quiet" type="submit" { "Retirer le mot de passe" }
                        }
                    }
                }
            }
            div class="block" {
                h3 { "L'envoi du jour" }
                p class="prose" {
                    "Pendant que le coffre est ouvert, Griffe envoie les lettres dont la date prévue est aujourd'hui. Une lettre d'hier reste un geste à la main."
                }
                p class="prose" { (auto_sentence(account.auto_send)) }
                form hx-post="/societe/courrier/automatique" hx-target="#content" {
                    div class="row-actions" {
                        @if account.auto_send {
                            input type="hidden" name="enabled" value="0";
                            button class="quiet" type="submit" { "Couper l'envoi du jour" }
                        } @else {
                            input type="hidden" name="enabled" value="1";
                            button class="seal" type="submit" { "Activer l'envoi du jour" }
                        }
                    }
                }
            }
            @if account.ready {
                form hx-post="/societe/courrier/essai" hx-target="#content" {
                    div class="row-actions" {
                        button class="quiet" type="submit" { "M'envoyer un essai" }
                    }
                }
            }
        }
    }
}

fn lede(account: &MailProfile) -> &'static str {
    if !account.ready {
        "Pas encore branché. Les brouillons continuent de s'ouvrir dans le client mail."
    } else if account.auto_send {
        "Prêt, envoi du jour. Les lettres prévues aujourd'hui partent pendant que le coffre est ouvert."
    } else {
        "Prêt, à la main. Envoyer, sur une lettre, la fait partir."
    }
}

pub fn subtitle(account: &MailProfile) -> &'static str {
    if !account.ready {
        "pas encore branché"
    } else if account.auto_send {
        "prêt, envoi du jour"
    } else {
        "prêt, à la main"
    }
}

fn auto_sentence(enabled: bool) -> &'static str {
    if enabled {
        "L'envoi du jour est actif."
    } else {
        "L'envoi du jour est éteint."
    }
}

fn back() -> Markup {
    html! {
        a class="back" href="/societe"
          hx-get="/societe" hx-target="#content" hx-push-url="true" hx-swap="innerHTML" {
            "← La société"
        }
    }
}
