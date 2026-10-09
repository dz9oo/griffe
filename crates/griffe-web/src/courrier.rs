//! Chapitre Le courrier : enregistrer le compte, le secret, l'essai.
//! La liaison SMTP est tentée ici, hors du coffre, par la sonde de la fenêtre.

use axum::Form;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue};
use axum::response::{Html, IntoResponse, Response};
use griffe_core::app::{AppError, Executor};
use griffe_core::mail::{
    ArmOutbound, CancelOutbound, ClearMailSecret, MailSecret, RecordMailProbe, ResolveUncertain,
    RetryOutbound, SaveLetterface, SaveMailAccount, SaveMailSecret, SaveMailSignature,
    SetAutomaticSend, UNDO_SECS, probe_material,
};
use maud::Markup;
use serde::Deserialize;

use crate::layout::ViewId;
use crate::state::AppState;
use crate::views::gens::depart_poll_fragment;
use crate::views::societe::{CourrierForm, courrier_page};

pub async fn show(State(state): State<AppState>, headers: HeaderMap) -> Html<String> {
    render(&state, &headers, None).await
}

#[derive(Debug, Deserialize)]
pub struct AccountForm {
    #[serde(default)]
    from_name: String,
    #[serde(default)]
    from_address: String,
    #[serde(default)]
    host: String,
    #[serde(default)]
    port: String,
    #[serde(default)]
    username: String,
    #[serde(default)]
    preset: String,
    #[serde(default)]
    imap_host: String,
    #[serde(default)]
    imap_port: String,
    #[serde(default)]
    secret: String,
}

pub async fn save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<AccountForm>,
) -> Response {
    let typed_secret = !form.secret.trim().is_empty();
    let posted = posted_from(&form);
    let preset = if form.preset == "icloud" {
        "icloud"
    } else {
        "custom"
    };
    let (port, tls) = if preset == "icloud" {
        (587, "starttls")
    } else if form.port.trim() == "465" {
        (465, "implicit")
    } else if form.port.trim() == "587" {
        (587, "starttls")
    } else {
        return saved(
            &headers,
            courrier_markup(
                &state,
                Some(&CourrierForm {
                    error: Some(with_secret_note(
                        "Le port est 587 ou 465.".into(),
                        typed_secret,
                    )),
                    ..posted
                }),
            )
            .await,
        );
    };
    let command = SaveMailAccount {
        from_name: Some(form.from_name),
        from_address: form.from_address,
        host: form.host,
        port,
        tls: tls.to_string(),
        username: form.username,
        preset: preset.to_string(),
        imap_host: form.imap_host.clone(),
        imap_port: form.imap_port.trim().parse().unwrap_or(0),
    };
    let result = state
        .with_store_mut(|store| Executor::new(store).execute(&command, &AppState::human_ctx()))
        .await;
    match result {
        None => locked(&headers),
        Some(Err(error)) => saved(
            &headers,
            courrier_markup(
                &state,
                Some(&CourrierForm {
                    error: Some(with_secret_note(french(&error), typed_secret)),
                    ..posted_from_command(&command)
                }),
            )
            .await,
        ),
        Some(Ok(_)) => {
            if typed_secret {
                match keep_secret(&state, form.secret).await {
                    SecretSave::Locked => return locked(&headers),
                    SecretSave::Refused(message) => {
                        return saved(
                            &headers,
                            courrier_markup(
                                &state,
                                Some(&CourrierForm {
                                    error: Some(with_secret_note(message, true)),
                                    ..CourrierForm::blank()
                                }),
                            )
                            .await,
                        );
                    }
                    SecretSave::Kept => {}
                }
            }
            remember_probe(&state).await;
            saved(
                &headers,
                courrier_markup(
                    &state,
                    Some(&CourrierForm {
                        notice: Some("Le serveur est enregistré.".into()),
                        ..CourrierForm::blank()
                    }),
                )
                .await,
            )
        }
    }
}

enum SecretSave {
    Kept,
    Locked,
    Refused(String),
}

async fn keep_secret(state: &AppState, raw: String) -> SecretSave {
    let secret = match MailSecret::new(raw) {
        Ok(secret) => secret,
        Err(error) => return SecretSave::Refused(error.to_string()),
    };
    let result = state
        .with_store_mut(|store| {
            Executor::new(store).execute(&SaveMailSecret { secret }, &AppState::human_ctx())
        })
        .await;
    match result {
        None => SecretSave::Locked,
        Some(Err(error)) => SecretSave::Refused(french(&error)),
        Some(Ok(_)) => SecretSave::Kept,
    }
}

fn with_secret_note(message: String, typed: bool) -> String {
    if typed {
        format!("{message} Le mot de passe n'est pas réaffiché. Saisissez-le de nouveau.")
    } else {
        message
    }
}

/// Hors du verrou du coffre. Sans sonde installée, la page le dit et rien ne sort.
async fn remember_probe(state: &AppState) {
    let prepared = state
        .with_store(|store| -> Result<_, AppError> { probe_material(store.connection()) })
        .await;
    let Some(Ok(Some(material))) = prepared else {
        return;
    };
    let Some(probe) = state.mail_probe() else {
        return;
    };
    let joined = tokio::task::spawn_blocking(move || {
        probe(material.endpoint, material.copy, material.secret)
    })
    .await;
    let (ok, detail, imap_username) = match joined {
        Ok(verdict) => (verdict.ok, verdict.detail, verdict.imap_username),
        Err(_) => (false, "Le serveur n'a pas répondu.".to_string(), None),
    };
    let _ = state
        .with_store_mut(|store| {
            if let Some(username) = imap_username {
                Executor::new(store).execute(
                    &griffe_core::mail::RememberImapUsername { username },
                    &AppState::human_ctx(),
                )?;
            }
            Executor::new(store).execute(&RecordMailProbe { ok, detail }, &AppState::human_ctx())
        })
        .await;
}

#[derive(Debug, Deserialize)]
pub struct SecretForm {
    #[serde(default)]
    secret: String,
}

pub async fn save_secret(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<SecretForm>,
) -> Response {
    if form.secret.trim().is_empty() {
        return saved(
            &headers,
            courrier_markup(
                &state,
                Some(&CourrierForm {
                    notice: Some("Le mot de passe déjà là est gardé.".into()),
                    ..CourrierForm::blank()
                }),
            )
            .await,
        );
    }
    let secret = match MailSecret::new(form.secret) {
        Ok(secret) => secret,
        Err(error) => {
            return saved(
                &headers,
                courrier_markup(
                    &state,
                    Some(&CourrierForm {
                        error: Some(error.to_string()),
                        ..CourrierForm::blank()
                    }),
                )
                .await,
            );
        }
    };
    let result = state
        .with_store_mut(|store| {
            Executor::new(store).execute(&SaveMailSecret { secret }, &AppState::human_ctx())
        })
        .await;
    match result {
        None => locked(&headers),
        Some(Err(error)) => saved(
            &headers,
            courrier_markup(
                &state,
                Some(&CourrierForm {
                    error: Some(french(&error)),
                    ..CourrierForm::blank()
                }),
            )
            .await,
        ),
        Some(Ok(_)) => {
            remember_probe(&state).await;
            saved(
                &headers,
                courrier_markup(
                    &state,
                    Some(&CourrierForm {
                        notice: Some("Le mot de passe est dans le coffre.".into()),
                        ..CourrierForm::blank()
                    }),
                )
                .await,
            )
        }
    }
}

pub async fn clear_secret(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let result = state
        .with_store_mut(|store| {
            Executor::new(store).execute(&ClearMailSecret, &AppState::human_ctx())
        })
        .await;
    match result {
        None => locked(&headers),
        Some(Err(error)) => saved(
            &headers,
            courrier_markup(
                &state,
                Some(&CourrierForm {
                    error: Some(french(&error)),
                    ..CourrierForm::blank()
                }),
            )
            .await,
        ),
        Some(Ok(_)) => saved(
            &headers,
            courrier_markup(
                &state,
                Some(&CourrierForm {
                    notice: Some("Le mot de passe est retiré.".into()),
                    ..CourrierForm::blank()
                }),
            )
            .await,
        ),
    }
}

#[derive(Debug, Deserialize)]
pub struct AutoForm {
    #[serde(default)]
    enabled: String,
}

pub async fn automatic(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<AutoForm>,
) -> Response {
    let enabled = form.enabled == "1";
    let result = state
        .with_store_mut(|store| {
            Executor::new(store).execute(&SetAutomaticSend { enabled }, &AppState::human_ctx())
        })
        .await;
    let notice = if enabled {
        "L'envoi du jour est actif."
    } else {
        "L'envoi du jour est éteint."
    };
    match result {
        None => locked(&headers),
        Some(Err(error)) => saved(
            &headers,
            courrier_markup(
                &state,
                Some(&CourrierForm {
                    error: Some(french(&error)),
                    ..CourrierForm::blank()
                }),
            )
            .await,
        ),
        Some(Ok(_)) => saved(
            &headers,
            courrier_markup(
                &state,
                Some(&CourrierForm {
                    notice: Some(notice.into()),
                    ..CourrierForm::blank()
                }),
            )
            .await,
        ),
    }
}

#[derive(Debug, Deserialize)]
pub struct SignatureForm {
    #[serde(default)]
    signature: String,
}

#[derive(Debug, Deserialize)]
pub struct LetterfaceForm {
    #[serde(default)]
    ink: String,
    #[serde(default)]
    metier: String,
    #[serde(default)]
    site: String,
}

pub async fn save_apparence(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<LetterfaceForm>,
) -> Response {
    let result = state
        .with_store_mut(|store| {
            Executor::new(store).execute(
                &SaveLetterface {
                    ink: form.ink.clone(),
                    metier: form.metier.clone(),
                    site: form.site.clone(),
                },
                &AppState::human_ctx(),
            )
        })
        .await;
    match result {
        None => locked(&headers),
        Some(Err(error)) => saved(
            &headers,
            courrier_markup(
                &state,
                Some(&CourrierForm {
                    error: Some(french(&error)),
                    ink: Some(form.ink),
                    metier: Some(form.metier),
                    site: Some(form.site),
                    ..CourrierForm::blank()
                }),
            )
            .await,
        ),
        Some(Ok(_)) => saved(
            &headers,
            courrier_markup(
                &state,
                Some(&CourrierForm {
                    notice: Some("L'allure est enregistrée.".into()),
                    ..CourrierForm::blank()
                }),
            )
            .await,
        ),
    }
}

pub async fn save_signature(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<SignatureForm>,
) -> Response {
    let empty = form.signature.replace('\r', "");
    let empty = empty.trim().is_empty();
    let result = state
        .with_store_mut(|store| {
            Executor::new(store).execute(
                &SaveMailSignature {
                    signature: form.signature.clone(),
                },
                &AppState::human_ctx(),
            )
        })
        .await;
    let notice = if empty {
        "La lettre se ferme par Bien à vous, le nom, la société."
    } else {
        "La formule est enregistrée."
    };
    match result {
        None => locked(&headers),
        Some(Err(error)) => saved(
            &headers,
            courrier_markup(
                &state,
                Some(&CourrierForm {
                    error: Some(french(&error)),
                    signature: Some(form.signature),
                    ..CourrierForm::blank()
                }),
            )
            .await,
        ),
        Some(Ok(_)) => saved(
            &headers,
            courrier_markup(
                &state,
                Some(&CourrierForm {
                    notice: Some(notice.into()),
                    ..CourrierForm::blank()
                }),
            )
            .await,
        ),
    }
}

#[derive(Debug, Deserialize)]
pub struct TrialForm {
    #[serde(default)]
    to: String,
}

pub async fn send_trial(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<TrialForm>,
) -> Response {
    let token = state.mail_session().to_string();
    let to = form.to;
    let result = state
        .with_store_mut(|store| arm_trial(store, &to, &token))
        .await;
    match result {
        None => locked(&headers),
        Some(Err(error)) => fragment(
            &headers,
            depart_for(&state, Some(&french(&error))).await,
            false,
        ),
        Some(Ok(())) => fragment(&headers, depart_for(&state, None).await, true),
    }
}

pub async fn trial_status(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let (markup, polling) = depart_status(&state, None).await;
    status_fragment(&headers, markup, polling)
}

#[derive(Debug, Deserialize)]
pub struct TrialPost {
    #[serde(default)]
    id: String,
    #[serde(default)]
    sent: String,
}

pub async fn trial_cancel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<TrialPost>,
) -> Response {
    trial_gesture(&state, &headers, &form.id, |store, id| {
        Executor::new(store)
            .execute(
                &CancelOutbound { id: id.to_string() },
                &AppState::human_ctx(),
            )
            .map(|_| ())
    })
    .await
}

pub async fn trial_retry(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<TrialPost>,
) -> Response {
    let token = state.mail_session().to_string();
    trial_gesture(&state, &headers, &form.id, move |store, id| {
        Executor::new(store)
            .execute(
                &RetryOutbound {
                    id: id.to_string(),
                    session_token: Some(token),
                },
                &AppState::human_ctx(),
            )
            .map(|_| ())
    })
    .await
}

pub async fn trial_decision(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<TrialPost>,
) -> Response {
    let sent = form.sent == "1";
    let today = state.today();
    trial_gesture(&state, &headers, &form.id, move |store, id| {
        Executor::new(store)
            .execute(
                &ResolveUncertain {
                    id: id.to_string(),
                    sent,
                    today,
                },
                &AppState::human_ctx(),
            )
            .map(|_| ())
    })
    .await
}

fn arm_trial(store: &mut griffe_core::store::Store, to: &str, token: &str) -> Result<(), AppError> {
    let account = griffe_core::mail::profile(store.connection())?;
    if !account.ready {
        return Err(AppError::Domain(
            "Le courrier n'est pas encore branché.".into(),
        ));
    }
    let letter = griffe_core::mail::trial_letter(store.connection())?;
    Executor::new(store)
        .execute(
            &ArmOutbound {
                kind: "trial".into(),
                anchor: Some("essai".into()),
                to_address: to.to_string(),
                subject: letter.subject,
                body: letter.body,
                delay_secs: UNDO_SECS,
                session_token: Some(token.to_string()),
                follow_subject: None,
                follow_subject_id: None,
                follow_cycle: None,
                follow_step: None,
                client_id: None,
            },
            &AppState::human_ctx(),
        )
        .map(|_| ())
}

async fn trial_gesture(
    state: &AppState,
    headers: &HeaderMap,
    id: &str,
    apply: impl FnOnce(&mut griffe_core::store::Store, &str) -> Result<(), AppError>,
) -> Response {
    if id.trim().is_empty() {
        let markup = depart_for(state, Some("Cette lettre est introuvable.")).await;
        return fragment(headers, markup, false);
    }
    let result = state.with_store_mut(|store| apply(store, id)).await;
    match result {
        None => locked(headers),
        Some(Err(error)) => fragment(
            headers,
            depart_for(state, Some(&french(&error))).await,
            false,
        ),
        Some(Ok(())) => fragment(headers, depart_for(state, None).await, true),
    }
}

async fn depart_for(state: &AppState, note: Option<&str>) -> Markup {
    state
        .with_store(|store| {
            let view = griffe_core::mail::outbound_for_anchor(
                store.connection(),
                "essai",
                time::OffsetDateTime::now_utc(),
            )
            .ok()
            .flatten();
            let destination = view.as_ref().map(|item| item.to_address.clone());
            crate::views::gens::depart_markup(
                "/societe/courrier",
                view.as_ref(),
                note,
                "Envoyer l'essai",
                destination.as_deref(),
            )
        })
        .await
        .unwrap_or_else(|| maud::html! { div id="depart" { "coffre verrouillé" } })
}

async fn depart_status(state: &AppState, note: Option<&str>) -> (Markup, bool) {
    state
        .with_store(|store| {
            let view = griffe_core::mail::outbound_for_anchor(
                store.connection(),
                "essai",
                time::OffsetDateTime::now_utc(),
            )
            .ok()
            .flatten();
            let destination = view.as_ref().map(|item| item.to_address.clone());
            depart_poll_fragment(
                "/societe/courrier",
                view.as_ref(),
                note,
                "Envoyer l'essai",
                destination.as_deref(),
            )
        })
        .await
        .unwrap_or_else(|| {
            (
                maud::html! { div id="depart" { "coffre verrouillé" } },
                false,
            )
        })
}

fn fragment(headers: &HeaderMap, content: Markup, trigger: bool) -> Response {
    let mut response = respond(headers, content).into_response();
    if trigger {
        response
            .headers_mut()
            .insert("HX-Trigger", HeaderValue::from_static("griffe:saved"));
    }
    response
}

/// Le sondage ne doit pas être rejoué depuis le cache de WebKit. Quand le minuteur
/// s'arrête, le bloc entier remplace celui qui interrogeait.
fn status_fragment(headers: &HeaderMap, content: Markup, polling: bool) -> Response {
    let mut response = fragment(headers, content, false);
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, no-cache, must-revalidate"),
    );
    if !polling {
        response
            .headers_mut()
            .insert("HX-Reswap", HeaderValue::from_static("outerHTML"));
    }
    response
}

impl CourrierForm {
    fn blank() -> Self {
        Self {
            from_name: String::new(),
            from_address: String::new(),
            host: String::new(),
            port: 587,
            username: String::new(),
            preset: String::new(),
            imap_host: String::new(),
            imap_port: "993".to_string(),
            error: None,
            notice: None,
            signature: None,
            ink: None,
            metier: None,
            site: None,
            redisplay: false,
        }
    }
}

fn posted_from(form: &AccountForm) -> CourrierForm {
    let port = if form.port.trim() == "465" { 465 } else { 587 };
    CourrierForm {
        from_name: form.from_name.clone(),
        from_address: form.from_address.clone(),
        host: form.host.clone(),
        username: form.username.clone(),
        preset: form.preset.clone(),
        imap_host: form.imap_host.clone(),
        imap_port: if form.imap_port.trim().is_empty() {
            "993".to_string()
        } else {
            form.imap_port.clone()
        },
        port,
        error: None,
        notice: None,
        signature: None,
        ink: None,
        metier: None,
        site: None,
        redisplay: true,
    }
}

fn posted_from_command(command: &SaveMailAccount) -> CourrierForm {
    CourrierForm {
        from_name: command.from_name.clone().unwrap_or_default(),
        from_address: command.from_address.clone(),
        host: command.host.clone(),
        port: command.port,
        username: command.username.clone(),
        preset: command.preset.clone(),
        imap_host: command.imap_host.clone(),
        imap_port: command.imap_port.to_string(),
        error: None,
        notice: None,
        signature: None,
        ink: None,
        metier: None,
        site: None,
        redisplay: true,
    }
}

fn french(error: &AppError) -> String {
    let text = error.to_string();
    text.strip_prefix("règle métier violée : ")
        .unwrap_or(&text)
        .to_string()
}

async fn courrier_markup(state: &AppState, form: Option<&CourrierForm>) -> Markup {
    state
        .with_store(|store| {
            courrier_page(store, form).unwrap_or_else(|error| {
                maud::html! { div class="empty-state" { (error.to_string()) } }
            })
        })
        .await
        .unwrap_or_else(|| maud::html! { div class="empty-state" { "coffre verrouillé" } })
}

async fn render(
    state: &AppState,
    headers: &HeaderMap,
    form: Option<&CourrierForm>,
) -> Html<String> {
    respond(headers, courrier_markup(state, form).await)
}

fn respond(headers: &HeaderMap, content: Markup) -> Html<String> {
    if headers.get("HX-Request").is_some() {
        Html(content.into_string())
    } else {
        Html(crate::layout::page(ViewId::Societe, "déverrouillé", content).into_string())
    }
}

fn saved(headers: &HeaderMap, content: Markup) -> Response {
    let mut response = respond(headers, content).into_response();
    response
        .headers_mut()
        .insert("HX-Trigger", HeaderValue::from_static("griffe:saved"));
    response
}

fn locked(headers: &HeaderMap) -> Response {
    respond(
        headers,
        maud::html! { div class="empty-state" { "coffre verrouillé — rechargez la page" } },
    )
    .into_response()
}
