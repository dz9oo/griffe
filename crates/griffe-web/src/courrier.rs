//! Chapitre Le courrier : enregistrer le compte, le secret, l'essai.
//! La liaison SMTP est tentée ici, hors du coffre, par la sonde de la fenêtre.

use axum::Form;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue};
use axum::response::{Html, IntoResponse, Response};
use griffe_core::app::{AppError, Executor};
use griffe_core::mail::{
    ArmOutbound, CancelOutbound, ClearMailSecret, KEPT_ADDRESS_CAP, MailSecret, RecordMailProbe,
    ResolveUncertain, RetryOutbound, SaveLetterface, SaveMailAccount, SaveMailSecret,
    SaveMailSignature, SetAutomaticSend, SetKeptAddresses, SetReleveInterval, UNDO_SECS,
    probe_material, profile,
};
use maud::{Markup, html};
use serde::Deserialize;
use serde::de::{self, Deserializer, MapAccess};

use crate::layout::ViewId;
use crate::releve::kick_releve;
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
            &state,
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
    let username = if form.username.trim().is_empty() {
        form.from_address.clone()
    } else {
        form.username.clone()
    };
    let command = SaveMailAccount {
        from_name: Some(form.from_name),
        from_address: form.from_address,
        host: form.host,
        port,
        tls: tls.to_string(),
        username,
        preset: preset.to_string(),
        imap_host: form.imap_host.clone(),
        imap_port: form.imap_port.trim().parse().unwrap_or(0),
    };
    let result = state
        .with_store_mut(|store| Executor::new(store).execute(&command, &AppState::human_ctx()))
        .await;
    match result {
        None => locked(&state, &headers),
        Some(Err(error)) => saved(
            &state,
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
                    SecretSave::Locked => return locked(&state, &headers),
                    SecretSave::Refused(message) => {
                        return saved(
                            &state,
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
                &state,
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
            &state,
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
                &state,
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
        None => locked(&state, &headers),
        Some(Err(error)) => saved(
            &state,
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
                &state,
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
        None => locked(&state, &headers),
        Some(Err(error)) => saved(
            &state,
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
            &state,
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
        None => locked(&state, &headers),
        Some(Err(error)) => saved(
            &state,
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
            &state,
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
pub struct ReleveForm {
    #[serde(default)]
    minutes: String,
}

pub async fn releve_interval(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<ReleveForm>,
) -> Response {
    let minutes: u64 = form.minutes.trim().parse().unwrap_or(0);
    let seconds = minutes.saturating_mul(60);
    let result = state
        .with_store_mut(|store| {
            Executor::new(store).execute(&SetReleveInterval { seconds }, &AppState::human_ctx())
        })
        .await;
    let notice = griffe_core::mail::releve_cadence(seconds).to_string();
    match result {
        None => locked(&state, &headers),
        Some(Err(error)) => saved(
            &state,
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
            &state,
            &headers,
            courrier_markup(
                &state,
                Some(&CourrierForm {
                    notice: Some(format!("La relève : {notice}")),
                    ..CourrierForm::blank()
                }),
            )
            .await,
        ),
    }
}

#[derive(Debug)]
pub struct AddressForm {
    address: Vec<String>,
    intent: String,
    scope: String,
}

/// Le formulaire répète `address`. `serde_urlencoded` livre chaque exemplaire
/// à part : un `Vec` ne les réunit pas.
impl<'de> Deserialize<'de> for AddressForm {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct Visit;

        impl<'de> de::Visitor<'de> for Visit {
            type Value = AddressForm;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("les adresses, le choix et l'intention")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut address = Vec::new();
                let mut intent = String::new();
                let mut scope = String::new();
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "address" => address.push(map.next_value()?),
                        "intent" => intent = map.next_value()?,
                        "scope" => scope = map.next_value()?,
                        _ => {
                            map.next_value::<de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(AddressForm {
                    address,
                    intent,
                    scope,
                })
            }
        }

        deserializer.deserialize_map(Visit)
    }
}

pub async fn save_addresses(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<AddressForm>,
) -> Response {
    let intent = form.intent.trim();
    let scope = if form.scope == "all" { "all" } else { "listed" };
    if intent == "ajouter" {
        return redisplay(&state, &headers, None, add_line(form.address), scope).await;
    }
    if let Some(index) = intent.strip_prefix("retirer:") {
        let mut lines = form.address;
        if let Ok(index) = index.parse::<usize>()
            && index < lines.len()
        {
            lines.remove(index);
        }
        return redisplay(&state, &headers, None, with_blank(lines), scope).await;
    }
    if !intent.is_empty() && intent != "garder" {
        return redisplay(&state, &headers, None, with_blank(form.address), scope).await;
    }
    let read_all = scope == "all";
    if !read_all && form.address.iter().all(|line| line.trim().is_empty()) {
        return redisplay(
            &state,
            &headers,
            Some("Écrivez une adresse.".to_string()),
            with_blank(form.address),
            scope,
        )
        .await;
    }
    let posted = form.address.clone();
    let result = state
        .with_store_mut(|store| {
            Executor::new(store).execute(
                &SetKeptAddresses {
                    addresses: if read_all { Vec::new() } else { form.address },
                    all: read_all,
                },
                &AppState::human_ctx(),
            )
        })
        .await;
    match result {
        None => locked(&state, &headers),
        Some(Err(error)) => {
            redisplay(
                &state,
                &headers,
                Some(french(&error)),
                with_blank(posted),
                scope,
            )
            .await
        }
        Some(Ok(_)) => {
            state.request_releve();
            kick_releve(&state).await;
            let notice = if read_all {
                "Tout le compte est lu."
            } else {
                "Ces adresses sont gardées. Les autres lettres quittent le coffre à cette relève."
            };
            saved(
                &state,
                &headers,
                courrier_markup(
                    &state,
                    Some(&CourrierForm {
                        notice: Some(notice.into()),
                        ..CourrierForm::blank()
                    }),
                )
                .await,
            )
        }
    }
}

fn with_blank(mut lines: Vec<String>) -> Vec<String> {
    if lines.len() < KEPT_ADDRESS_CAP && lines.last().is_none_or(|line| !line.trim().is_empty()) {
        lines.push(String::new());
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn add_line(mut lines: Vec<String>) -> Vec<String> {
    if lines.len() < KEPT_ADDRESS_CAP {
        lines.push(String::new());
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

async fn redisplay(
    state: &AppState,
    headers: &HeaderMap,
    error: Option<String>,
    lines: Vec<String>,
    scope: &str,
) -> Response {
    respond(
        state,
        headers,
        courrier_markup(
            state,
            Some(&CourrierForm {
                error,
                kept_lines: Some(lines),
                address_scope: Some(scope.to_string()),
                ..CourrierForm::blank()
            }),
        )
        .await,
    )
    .into_response()
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
}

pub async fn save_apparence(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<LetterfaceForm>,
) -> Response {
    let result = state
        .with_store_mut(|store| {
            let account = profile(store.connection())?;
            Executor::new(store).execute(
                &SaveLetterface {
                    ink: form.ink.clone(),
                    metier: account.metier,
                    site: account.site,
                },
                &AppState::human_ctx(),
            )
        })
        .await;
    match result {
        None => locked(&state, &headers),
        Some(Err(error)) => saved(
            &state,
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
            &state,
            &headers,
            courrier_markup(
                &state,
                Some(&CourrierForm {
                    notice: Some("La feuille est enregistrée.".into()),
                    ..CourrierForm::blank()
                }),
            )
            .await,
        ),
    }
}

/// Aperçu de la carte. N'écrit rien et ne signale pas une sauvegarde.
pub async fn letter_preview(
    State(state): State<AppState>,
    Form(form): Form<SignatureForm>,
) -> Html<String> {
    let content = state
        .with_store(|store| crate::views::societe::preview_card(store, &form.signature))
        .await
        .and_then(Result::ok)
        .unwrap_or_else(|| html! { p class="phrase-caption" { "coffre verrouillé" } });
    Html(content.into_string())
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
        None => locked(&state, &headers),
        Some(Err(error)) => saved(
            &state,
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
            &state,
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
pub struct ReadForm {
    #[serde(default)]
    ink: String,
    #[serde(default)]
    metier: String,
    #[serde(default)]
    site: String,
    #[serde(default)]
    signature: String,
}

/// Couleur, ligne sous le nom, bas de lettre, puis formule. La formule n'est
/// enregistrée que si l'allure a été acceptée.
pub async fn save_lire(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<ReadForm>,
) -> Response {
    let face = state
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
    let posted = CourrierForm {
        signature: Some(form.signature.clone()),
        ..CourrierForm::blank()
    };
    match face {
        None => return locked(&state, &headers),
        Some(Err(error)) => {
            return saved(
                &state,
                &headers,
                courrier_markup(
                    &state,
                    Some(&CourrierForm {
                        error: Some(french(&error)),
                        ..posted
                    }),
                )
                .await,
            );
        }
        Some(Ok(_)) => {}
    }
    let empty = form.signature.replace('\r', "");
    let empty = empty.trim().is_empty();
    let signature = state
        .with_store_mut(|store| {
            Executor::new(store).execute(
                &SaveMailSignature {
                    signature: form.signature.clone(),
                },
                &AppState::human_ctx(),
            )
        })
        .await;
    match signature {
        None => locked(&state, &headers),
        Some(Err(error)) => saved(
            &state,
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
        Some(Ok(_)) => {
            let notice = if empty {
                "La lettre se ferme par Bien à vous, le nom, la société."
            } else {
                "La lettre est enregistrée."
            };
            saved(
                &state,
                &headers,
                courrier_markup(
                    &state,
                    Some(&CourrierForm {
                        notice: Some(notice.into()),
                        ..CourrierForm::blank()
                    }),
                )
                .await,
            )
        }
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
        None => locked(&state, &headers),
        Some(Err(error)) => fragment(
            &state,
            &headers,
            depart_for(&state, Some(&french(&error))).await,
            false,
        ),
        Some(Ok(())) => fragment(&state, &headers, depart_for(&state, None).await, true),
    }
}

pub async fn trial_status(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let (markup, polling) = depart_status(&state, None).await;
    status_fragment(&state, &headers, markup, polling)
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
                in_reply_to: None,
                references: None,
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
        return fragment(state, headers, markup, false);
    }
    let result = state.with_store_mut(|store| apply(store, id)).await;
    match result {
        None => locked(state, headers),
        Some(Err(error)) => fragment(
            state,
            headers,
            depart_for(state, Some(&french(&error))).await,
            false,
        ),
        Some(Ok(())) => fragment(state, headers, depart_for(state, None).await, true),
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

fn fragment(state: &AppState, headers: &HeaderMap, content: Markup, trigger: bool) -> Response {
    let mut response = respond(state, headers, content).into_response();
    if trigger {
        response
            .headers_mut()
            .insert("HX-Trigger", HeaderValue::from_static("griffe:saved"));
    }
    response
}

/// Le sondage ne doit pas être rejoué depuis le cache de WebKit. Quand le minuteur
/// s'arrête, le bloc entier remplace celui qui interrogeait.
fn status_fragment(
    state: &AppState,
    headers: &HeaderMap,
    content: Markup,
    polling: bool,
) -> Response {
    let mut response = fragment(state, headers, content, false);
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
            error: None,
            notice: None,
            signature: None,
            redisplay: false,
            kept_lines: None,
            address_scope: None,
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
        port,
        error: None,
        notice: None,
        signature: None,
        redisplay: true,
        kept_lines: None,
        address_scope: None,
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
        error: None,
        notice: None,
        signature: None,
        redisplay: true,
        kept_lines: None,
        address_scope: None,
    }
}

fn french(error: &AppError) -> String {
    let text = error.to_string();
    text.strip_prefix("règle métier violée : ")
        .unwrap_or(&text)
        .to_string()
}

async fn courrier_markup(state: &AppState, form: Option<&CourrierForm>) -> Markup {
    let today = state.today();
    state
        .with_store(|store| {
            let inner = courrier_page(store, form).unwrap_or_else(|error| {
                maud::html! { div class="empty-state" { (error.to_string()) } }
            });
            crate::views::societe::bureau(store, today, "/societe/courrier", inner).unwrap_or_else(
                |error| maud::html! { div class="empty-state" { (error.to_string()) } },
            )
        })
        .await
        .unwrap_or_else(|| maud::html! { div class="empty-state" { "coffre verrouillé" } })
}

async fn render(
    state: &AppState,
    headers: &HeaderMap,
    form: Option<&CourrierForm>,
) -> Html<String> {
    respond(state, headers, courrier_markup(state, form).await)
}

fn respond(state: &AppState, headers: &HeaderMap, content: Markup) -> Html<String> {
    if headers.get("HX-Request").is_some() {
        Html(content.into_string())
    } else {
        Html(
            crate::layout::page(ViewId::Societe, "déverrouillé", state.today(), content)
                .into_string(),
        )
    }
}

fn saved(state: &AppState, headers: &HeaderMap, content: Markup) -> Response {
    let mut response = respond(state, headers, content).into_response();
    response
        .headers_mut()
        .insert("HX-Trigger", HeaderValue::from_static("griffe:saved"));
    response
}

fn locked(state: &AppState, headers: &HeaderMap) -> Response {
    respond(
        state,
        headers,
        maud::html! { div class="empty-state" { "coffre verrouillé — rechargez la page" } },
    )
    .into_response()
}
