//! Chapitre Le courrier : enregistrer le compte, le secret, l'essai. Pas de SMTP ici.

use axum::Form;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue};
use axum::response::{Html, IntoResponse, Response};
use griffe_core::app::{AppError, Executor};
use griffe_core::mail::{
    ArmOutbound, ClearMailSecret, MailSecret, SaveMailAccount, SaveMailSecret, SetAutomaticSend,
    UNDO_SECS,
};
use maud::Markup;
use serde::Deserialize;

use crate::layout::ViewId;
use crate::state::AppState;
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
}

pub async fn save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<AccountForm>,
) -> Response {
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
                    error: Some("Le port est 587 ou 465.".into()),
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
                    error: Some(french(&error)),
                    ..posted_from_command(&command)
                }),
            )
            .await,
        ),
        Some(Ok(_)) => saved(
            &headers,
            courrier_markup(
                &state,
                Some(&CourrierForm {
                    notice: Some("Le serveur est enregistré.".into()),
                    ..CourrierForm::blank()
                }),
            )
            .await,
        ),
    }
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
        Some(Ok(_)) => saved(
            &headers,
            courrier_markup(
                &state,
                Some(&CourrierForm {
                    notice: Some("Le mot de passe est dans le coffre.".into()),
                    ..CourrierForm::blank()
                }),
            )
            .await,
        ),
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

pub async fn trial(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let token = state.mail_session().to_string();
    let result = state
        .with_store_mut(|store| {
            let account = griffe_core::mail::profile(store.connection())?;
            if !account.ready {
                return Err(AppError::Domain(
                    "Le courrier n'est pas encore branché.".into(),
                ));
            }
            Executor::new(store)
                .execute(
                    &ArmOutbound {
                        kind: "trial".into(),
                        anchor: Some("essai".into()),
                        to_address: account.from_address,
                        subject: "Essai".into(),
                        body: "Ceci est un essai envoyé depuis Griffe.".into(),
                        delay_secs: UNDO_SECS,
                        session_token: Some(token),
                        follow_subject: None,
                        follow_subject_id: None,
                        follow_cycle: None,
                        follow_step: None,
                    },
                    &AppState::human_ctx(),
                )
                .map(|_| ())
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
                    notice: Some(
                        "L'essai part. Cinq secondes pour fermer la fenêtre et le garder.".into(),
                    ),
                    ..CourrierForm::blank()
                }),
            )
            .await,
        ),
    }
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
            error: None,
            notice: None,
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
        port,
        error: None,
        notice: None,
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
        error: None,
        notice: None,
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
