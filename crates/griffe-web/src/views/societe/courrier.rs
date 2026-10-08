//! Chapitre Le courrier. Un panneau pour le serveur, puis la formule, l'essai, l'envoi du jour.

use griffe_core::app::AppError;
use griffe_core::mail::{ICLOUD_HOST, MailPreset, MailProfile, OutboundView, TrialLetter, profile};
use griffe_core::store::Store;
use maud::{Markup, html};
use time::OffsetDateTime;

use crate::layout::ViewId;
use crate::views::gens::depart_markup;

pub struct CourrierForm {
    pub from_name: String,
    pub from_address: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub preset: String,
    pub error: Option<String>,
    pub notice: Option<String>,
    /// Texte posté quand l'enregistrement de la formule a échoué.
    pub signature: Option<String>,
    /// Vrai quand les champs du serveur viennent d'un envoi refusé.
    pub redisplay: bool,
}

pub fn page(store: &Store, form: Option<&CourrierForm>) -> Result<Markup, AppError> {
    let account = profile(store.connection())?;
    let pause = griffe_core::mail::hourly_pause(store.connection(), OffsetDateTime::now_utc())?;
    let letter = griffe_core::mail::trial_letter(store.connection())?;
    let depart = griffe_core::mail::outbound_for_anchor(
        store.connection(),
        "essai",
        OffsetDateTime::now_utc(),
    )?;
    Ok(markup(&account, form, pause, &letter, depart.as_ref()))
}

fn markup(
    account: &MailProfile,
    form: Option<&CourrierForm>,
    pause: bool,
    letter: &TrialLetter,
    depart: Option<&OutboundView>,
) -> Markup {
    let posted = form.filter(|item| item.redisplay);
    let from_name = posted.map_or(account.from_name.as_str(), |item| item.from_name.as_str());
    let from_address = posted.map_or(account.from_address.as_str(), |item| {
        item.from_address.as_str()
    });
    let host = posted.map_or(account.host.as_str(), |item| item.host.as_str());
    let username = posted.map_or(account.username.as_str(), |item| item.username.as_str());
    let preset = posted.map_or(account.preset, |item| MailPreset::parse(&item.preset));
    let port = posted.map_or(account.port, |item| item.port);
    let icloud = preset == MailPreset::Icloud;
    let signature = form
        .and_then(|item| item.signature.as_deref())
        .unwrap_or(account.signature.as_str());

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
            div class="block" {
                form class="serveur" action="/societe/courrier" method="post"
                     hx-post="/societe/courrier" hx-target="#content" hx-push-url="true"
                     hx-disabled-elt="find button" {
                    h3 { "Le serveur d'envoi" }
                    (probe_line(account))
                    p class="probe-wait" { "Griffe joint le serveur…" }
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
                    div class="field" {
                        label for="from_name" { "Nom affiché" }
                        input id="from_name" name="from_name" type="text" value=(from_name) autocomplete="off";
                    }
                    div class="field" {
                        label for="from_address" { "L'adresse qui signe" }
                        input id="from_address" name="from_address" type="text" value=(from_address) autocomplete="off";
                    }
                    div class="field" {
                        label for="username" { "L'identifiant du serveur" }
                        input id="username" name="username" type="text" value=(username) autocomplete="off";
                    }
                    div class="serveur-icloud" {
                        p class="prose" {
                            (ICLOUD_HOST) ", port 587, liaison chiffrée. L'identifiant est l'adresse complète du compte, en icloud.com, me.com ou mac.com. Le mot de passe est un mot de passe d'application."
                        }
                        p class="prose" {
                            a href="https://account.apple.com" { "Le compte Apple" }
                            " crée ce mot de passe."
                        }
                    }
                    div class="serveur-custom" {
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
                    div class="field" {
                        label for="secret" { "Mot de passe du serveur" }
                        input id="secret" name="secret" type="password" autocomplete="new-password"
                              aria-label="Mot de passe du serveur";
                    }
                    p class="prose" {
                        @if account.has_secret {
                            "Un mot de passe est dans le coffre. Laisser le champ vide le garde. La page ne le réécrit pas."
                        } @else {
                            "Il reste dans le coffre chiffré. La page ne le réécrit pas."
                        }
                    }
                    div class="row-actions" {
                        button class="seal" type="submit" { "Enregistrer et essayer la liaison" }
                    }
                }
                @if account.has_secret {
                    form hx-post="/societe/courrier/retirer" hx-target="#content" hx-push-url="true" {
                        div class="row-actions" {
                            button class="quiet" type="submit" { "Retirer le mot de passe" }
                        }
                    }
                }
            }
            form hx-post="/societe/courrier/signature" hx-target="#content" hx-push-url="true" {
                div class="block" {
                    h3 { "Ce qui ferme la lettre" }
                    p class="prose" {
                        "Ces lignes remplacent <signature> dans les lettres et dans les phrases. Tant qu'elles sont vides, la lettre se ferme par Bien à vous, le nom, la société."
                    }
                    div class="field" {
                        label for="signature" { "La formule" }
                        textarea id="signature" name="signature" rows="6" { (signature) }
                    }
                    div class="row-actions" {
                        button class="seal" type="submit" { "Enregistrer la formule" }
                    }
                }
            }
            div class="block" {
                h3 { "Une lettre d'essai" }
                p class="prose" {
                    "Le premier message, tel qu'un prospect le lirait. Le prénom est fictif. Rien n'est classé dans une affaire. Vers une adresse réelle, pour la lire dans cette boîte."
                }
                div class="essai-sheet" {
                    p class="phrase-sheet-subject" { (letter.subject) }
                    pre class="phrase-sheet-body" { (letter.body) }
                }
                form {
                    div class="field" {
                        label for="to" { "Vers" }
                        input id="to" name="to" type="text" value=(from_address) autocomplete="email";
                    }
                    (depart_markup("/societe/courrier", depart, None, "Envoyer l'essai"))
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
        }
    }
}

fn probe_line(account: &MailProfile) -> Markup {
    if let Some(probe) = &account.probe {
        let class = if probe.ok {
            "probe-status probe-ok"
        } else {
            "probe-status probe-bad"
        };
        let mark = if probe.ok { "✓" } else { "✗" };
        return html! {
            p class=(class) role="status" { (mark) " " (probe.detail) }
        };
    }
    if !account.has_secret && !account.from_address.is_empty() {
        return html! {
            p class="probe-status probe-bad" role="status" { "✗ Le mot de passe manque." }
        };
    }
    if account.ready {
        return html! {
            p class="probe-status" role="status" { "La liaison n'a pas été essayée." }
        };
    }
    html! {}
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
