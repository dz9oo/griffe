//! Chapitre Le courrier. Un panneau pour le serveur, puis la formule, l'essai, l'envoi du jour.

use griffe_core::app::AppError;
use griffe_core::mail::{
    LetterChrome, MailPreset, MailProfile, OutboundView, TrialLetter, profile,
};
use griffe_core::store::Store;
use maud::{Markup, html};
use time::OffsetDateTime;

use crate::layout::ViewId;
use crate::views::gens::{depart_markup, letter_stage};

pub struct CourrierForm {
    pub from_name: String,
    pub from_address: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub preset: String,
    pub imap_host: String,
    pub imap_port: String,
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
    let letter = match form.and_then(|item| item.signature.as_deref()) {
        Some(signature) => griffe_core::mail::trial_with_closing(store.connection(), signature)?,
        None => griffe_core::mail::trial_letter(store.connection())?,
    };
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
    let imap_host = posted.map_or(account.imap_host.as_str(), |item| item.imap_host.as_str());
    let imap_port = posted.map_or_else(
        || {
            if account.imap_host.is_empty() {
                "993".to_string()
            } else {
                account.imap_port.to_string()
            }
        },
        |item| item.imap_port.clone(),
    );
    let icloud = preset == MailPreset::Icloud;
    let signature = form
        .and_then(|item| item.signature.as_deref())
        .unwrap_or(account.signature.as_str());
    let mut chrome = LetterChrome::from_profile(account);
    chrome.signature = signature.to_string();
    let known = !from_address.trim().is_empty();
    let sheet_open = !known || form.is_some_and(|item| item.redisplay);
    let other_id = !username.trim().is_empty() && username.trim() != from_address.trim();
    let copies = !imap_host.trim().is_empty();

    html! {
        div class="letter courrier-desk" data-view=(ViewId::Societe.slug()) {
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
            div class="courrier-read" {
                div class="courrier-letter-col" {
                    p class="courrier-kicker" { "Une lettre d'essai. Le prénom est fictif." }
                    div id="letter-card" {
                        (card_column(from_name, &letter.subject, &letter.body, &chrome))
                    }
                    form class="courrier-send" {
                        div class="field" {
                            label for="to" { "Vers" }
                            input id="to" name="to" type="text" value=(from_address) autocomplete="email";
                        }
                        (depart_markup(
                            "/societe/courrier",
                            depart,
                            None,
                            "Envoyer l'essai",
                            depart.map(|item| item.to_address.as_str()),
                        ))
                    }
                }
                form class="courrier-formula" hx-post="/societe/courrier/signature" hx-target="#content" hx-push-url="true" {
                    h3 class="panel-title" { "Ce qui ferme la lettre" }
                    p class="field-help" {
                        "Ces lignes ferment chaque lettre. On les écrit ici une fois. Par exemple : Ingénieur logiciel indépendant, sur sa ligne. Un mail, un numéro ou une adresse, seuls sur leur ligne, restent cliquables. L'adresse se lit sans ?utm_…."
                    }
                    div class="field" {
                        label for="signature" { "La formule" }
                        textarea id="signature" name="signature" rows="5"
                                 hx-post="/societe/courrier/apercu"
                                 hx-trigger="input changed delay:600ms"
                                 hx-target="#letter-card"
                                 hx-swap="innerHTML"
                                 hx-push-url="false" { (signature) }
                    }
                    div class="row-actions" {
                        button class="seal" type="submit" { "Enregistrer" }
                    }
                }
            }
            section class="courrier-account" {
                h3 class="panel-title" { "D'où elles partent" }
                @if known {
                    (account_line(from_name, from_address, preset, host, account))
                }
                details class="courrier-sheet" open[sheet_open] {
                    summary class="quiet" { @if known { "Modifier" } @else { "Le serveur" } }
                    (probe_line(account))
                    p class="probe-wait" { "Griffe joint le serveur…" }
                    form class="serveur" action="/societe/courrier" method="post"
                         hx-post="/societe/courrier" hx-target="#content" hx-push-url="true"
                         hx-disabled-elt="find button" {
                        div class="courrier-grid" {
                            div class="field" {
                                label for="from_name" { "Nom affiché" }
                                input id="from_name" name="from_name" type="text" value=(from_name) autocomplete="name";
                            }
                            div class="field" {
                                label for="from_address" { "L'adresse qui signe" }
                                input id="from_address" name="from_address" type="text" value=(from_address) autocomplete="email";
                            }
                        }
                        details class="courrier-fold" open[other_id] {
                            summary { "Autre identifiant" }
                            div class="field" {
                                label for="username" { "L'identifiant du serveur" }
                                input id="username" name="username" type="text" value=(username) autocomplete="off";
                            }
                        }
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
                        div class="serveur-icloud" {
                            p class="prose" {
                                "iCloud, port 587. L'identifiant est cette adresse. Le mot de passe est un mot de passe d'application. "
                                a href="https://account.apple.com" { "Le compte Apple" }
                                " le crée. La copie part dans Envoyés."
                            }
                        }
                        div class="serveur-custom" {
                            div class="courrier-grid" {
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
                            details class="courrier-fold" open[copies] {
                                summary { "Les copies dans Envoyés" }
                                div class="courrier-grid" {
                                    div class="field" {
                                        label for="imap_host" { "Hôte des copies" }
                                        input id="imap_host" name="imap_host" type="text" value=(imap_host) autocomplete="off";
                                    }
                                    div class="field" {
                                        label for="imap_port" { "Port des copies" }
                                        input id="imap_port" name="imap_port" type="text" value=(imap_port) autocomplete="off";
                                    }
                                }
                                p class="prose" {
                                    "Vide, la lettre part et la copie n'ira pas dans Envoyés. Le port est 993."
                                }
                            }
                        }
                        div class="courrier-secret" {
                            div class="field" {
                                label for="secret" { "Mot de passe du serveur" }
                                input id="secret" name="secret" type="password" autocomplete="new-password"
                                      aria-label="Mot de passe du serveur";
                            }
                            button class="seal" type="submit" { "Enregistrer et essayer la liaison" }
                        }
                        p class="prose" {
                            @if account.has_secret {
                                "Un mot de passe est dans le coffre. Laisser le champ vide le garde. La page ne le réécrit pas."
                            } @else {
                                "Il reste dans le coffre chiffré. La page ne le réécrit pas."
                            }
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
            }
            div class="courrier-jour" {
                h3 class="panel-title" { "L'envoi du jour" }
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

/// La carte et son sujet. L'aperçu remplace ce bloc sans enregistrer.
pub fn preview_card(store: &Store, signature: &str) -> Result<Markup, AppError> {
    let account = profile(store.connection())?;
    let letter = griffe_core::mail::trial_with_closing(store.connection(), signature)?;
    let mut chrome = LetterChrome::from_profile(&account);
    chrome.signature = signature.to_string();
    Ok(card_column(
        &account.from_name,
        &letter.subject,
        &letter.body,
        &chrome,
    ))
}

fn card_column(from_name: &str, subject: &str, body: &str, chrome: &LetterChrome) -> Markup {
    html! {
        p class="phrase-sheet-subject" { (subject) }
        (letter_stage(from_name, subject, body, chrome, true, false))
    }
}

fn account_line(
    name: &str,
    address: &str,
    preset: MailPreset,
    host: &str,
    account: &MailProfile,
) -> Markup {
    let server = if preset == MailPreset::Icloud {
        "iCloud".to_string()
    } else if host.trim().is_empty() {
        "un autre".to_string()
    } else {
        host.to_string()
    };
    html! {
        p class="courrier-account-line" {
            @if !name.trim().is_empty() {
                span { (name) }
                " · "
            }
            span { (address) }
            " · "
            span { (server) }
            " · "
            (liaison(account))
        }
    }
}

fn liaison(account: &MailProfile) -> Markup {
    if let Some(probe) = &account.probe {
        let class = if probe.ok { "probe-ok" } else { "probe-bad" };
        return html! { span class=(class) { (probe.detail) } };
    }
    if !account.has_secret && !account.from_address.is_empty() {
        return html! { span class="probe-bad" { "Le mot de passe manque." } };
    }
    html! { span { "La liaison n'a pas été essayée." } }
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
