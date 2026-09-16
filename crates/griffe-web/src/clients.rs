//! Routes de mutation `clients` : la première tranche complète de gestion des données depuis la
//! fenêtre (lot 15) — jusqu'ici, la GUI n'avait aucune route qui écrive dans le domaine. Le
//! patron introduit ici (liste auto-rafraîchie + panneau `#panel` + `AppState::human_ctx()`)
//! est celui que les lots suivants répéteront pour les autres entités.
//!
//! **Convention de réponse**, commune à toutes les routes de mutation ci-dessous :
//! - succès → `200` avec un corps **vide** et l'en-tête `HX-Trigger: griffe:saved` — un swap
//!   `innerHTML` d'une chaîne vide vide le panneau (il se ferme), et le conteneur de liste
//!   (`views::clients::list_fragment`) écoute `hx-trigger="griffe:saved from:body"` pour se
//!   rafraîchir tout seul. Volontairement **pas** `204` : htmx ne swap jamais un `204` (config
//!   par défaut `{code:"204", swap:false}` dans `assets/htmx.min.js`), ce qui laisserait le
//!   panneau affiché avec son contenu périmé après un succès — un `200` vide est le seul moyen
//!   d'obtenir la fermeture *et* le déclenchement de rafraîchissement dans la même réponse ;
//! - échec de validation ou refus métier → `200` avec le panneau re-rendu (formulaire avec ses
//!   erreurs, ou fiche avec un bandeau) — le panneau reste ouvert, l'utilisateur voit pourquoi.
//!
//! Toute action ici est un acte humain direct (`AppState::human_ctx()`, jamais un acteur agent)
//! — voir `state.rs`.

use axum::Form;
use axum::extract::{Path, Query, State};
use axum::http::HeaderValue;
use axum::response::{Html, IntoResponse, Response};
use griffe_core::app::{AppError, Executor, Outcome};
use griffe_core::clients::{self, ClientFilter};
use griffe_core::domain::{Address, Client, ClientId, ContactId, Siren, VatNumber};
use maud::html;
use serde::Deserialize;

use crate::state::AppState;
use crate::views;
use crate::views::clients::{
    ClientFormErrors, ClientFormValues, ContactFormErrors, ContactFormValues,
};

fn locked_fragment() -> Html<String> {
    Html(
        html! { div class="empty-state" { "coffre verrouillé — rechargez la page" } }.into_string(),
    )
}

fn message_fragment(message: &str) -> Html<String> {
    Html(html! { div class="empty-state" { (message) } }.into_string())
}

/// Réponse d'une mutation réussie — voir la convention de réponse en tête de module.
fn saved() -> Response {
    let mut response = Html(String::new()).into_response();
    response
        .headers_mut()
        .insert("HX-Trigger", HeaderValue::from_static("griffe:saved"));
    response
}

/// Exécute `cmd` comme un acte humain contre le coffre de la fenêtre. `None` signifie que la
/// session a expiré entre l'affichage du panneau et la soumission — même défense que le reste
/// du crate (`state.rs::with_store`).
async fn execute<C: griffe_core::app::Command>(
    state: &AppState,
    cmd: C,
) -> Option<Result<Outcome<C::Output>, AppError>> {
    state
        .with_store_mut(|store| Executor::new(store).execute(&cmd, &AppState::human_ctx()))
        .await
}

#[derive(Debug, Deserialize)]
pub struct ClientForm {
    #[serde(default)]
    revision: Option<String>,
    name: String,
    #[serde(default)]
    siren: String,
    #[serde(default)]
    vat_number: String,
    #[serde(default)]
    street: String,
    #[serde(default)]
    postal_code: String,
    #[serde(default)]
    city: String,
    #[serde(default)]
    country: String,
}

impl From<&ClientForm> for ClientFormValues {
    fn from(f: &ClientForm) -> Self {
        Self {
            name: f.name.clone(),
            siren: f.siren.clone(),
            vat_number: f.vat_number.clone(),
            street: f.street.clone(),
            postal_code: f.postal_code.clone(),
            city: f.city.clone(),
            country: f.country.clone(),
        }
    }
}

struct ParsedClientForm {
    name: String,
    siren: Option<Siren>,
    vat_number: Option<VatNumber>,
    address: Option<Address>,
}

/// Validations qui peuvent s'attribuer à un champ précis (adresse, SIREN, TVA) parce que ce
/// module les fait lui-même, avant de construire la commande — au-delà, les erreurs du cœur
/// restent un bandeau général (voir `ClientFormErrors`).
fn parse_client_form(form: &ClientForm) -> Result<ParsedClientForm, Box<ClientFormErrors>> {
    let mut errors = ClientFormErrors::default();

    let siren = match form.siren.trim() {
        "" => None,
        s => match Siren::parse(s) {
            Ok(siren) => Some(siren),
            Err(e) => {
                errors.siren = Some(e.to_string());
                None
            }
        },
    };
    let vat_number = match form.vat_number.trim() {
        "" => None,
        s => match VatNumber::parse(s) {
            Ok(vat) => Some(vat),
            Err(e) => {
                errors.vat_number = Some(e.to_string());
                None
            }
        },
    };

    let fields = [&form.street, &form.postal_code, &form.city, &form.country];
    let filled = fields.iter().filter(|f| !f.trim().is_empty()).count();
    let address = match filled {
        0 => None,
        4 => Some(Address {
            street: form.street.trim().to_string(),
            postal_code: form.postal_code.trim().to_string(),
            city: form.city.trim().to_string(),
            country: form.country.trim().to_string(),
        }),
        _ => {
            errors.address = Some(
                "les 4 champs d'adresse doivent être fournis ensemble, ou tous laissés vides"
                    .to_string(),
            );
            None
        }
    };

    if form.name.trim().is_empty() {
        errors.name = Some("le nom est obligatoire".to_string());
    }

    if errors.name.is_some()
        || errors.siren.is_some()
        || errors.vat_number.is_some()
        || errors.address.is_some()
    {
        Err(Box::new(errors))
    } else {
        Ok(ParsedClientForm {
            name: form.name.trim().to_string(),
            siren,
            vat_number,
            address,
        })
    }
}

/// Traduit une erreur du cœur en bandeau, ou en bandeau de conflit (avec un lien de
/// rechargement) si c'est un `AppError::Conflict`.
fn client_error_banner(e: AppError, reload_hx_get: &str) -> ClientFormErrors {
    match e {
        AppError::Conflict { .. } => ClientFormErrors {
            conflict: Some((e.to_string(), reload_hx_get.to_string())),
            ..Default::default()
        },
        other => ClientFormErrors {
            banner: Some(other.to_string()),
            ..Default::default()
        },
    }
}

fn contact_error_banner(e: AppError, reload_hx_get: &str) -> ContactFormErrors {
    match e {
        AppError::Conflict { .. } => ContactFormErrors {
            conflict: Some((e.to_string(), reload_hx_get.to_string())),
            ..Default::default()
        },
        other => ContactFormErrors {
            banner: Some(other.to_string()),
            ..Default::default()
        },
    }
}

fn parse_id<T: std::str::FromStr>(raw: &str) -> Option<T> {
    raw.parse().ok()
}

// -- Liste ---------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct TableQuery {
    #[serde(default)]
    archived: bool,
}

pub async fn table(State(state): State<AppState>, Query(q): Query<TableQuery>) -> Html<String> {
    let filter = if q.archived {
        ClientFilter::All
    } else {
        ClientFilter::ActiveOnly
    };
    match state
        .with_store(|store| views::clients::list_fragment(store, filter))
        .await
    {
        None => locked_fragment(),
        Some(Ok(markup)) => Html(markup.into_string()),
        Some(Err(e)) => message_fragment(&e.to_string()),
    }
}

// -- Client : créer / afficher / modifier / archiver / supprimer ---------------------------

pub async fn new_panel() -> Html<String> {
    Html(
        views::clients::new_panel(&ClientFormValues::default(), &ClientFormErrors::default())
            .into_string(),
    )
}

pub async fn create(State(state): State<AppState>, Form(form): Form<ClientForm>) -> Response {
    let parsed = match parse_client_form(&form) {
        Ok(p) => p,
        Err(errors) => {
            return Html(views::clients::new_panel(&(&form).into(), &errors).into_string())
                .into_response();
        }
    };
    let cmd = clients::CreateClient {
        name: parsed.name,
        siren: parsed.siren,
        vat_number: parsed.vat_number,
        address: parsed.address,
    };
    match execute(&state, cmd).await {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => saved(),
        Some(Err(e)) => {
            let errors = client_error_banner(e, "/clients/new");
            Html(views::clients::new_panel(&(&form).into(), &errors).into_string()).into_response()
        }
    }
}

pub async fn show_panel(State(state): State<AppState>, Path(id): Path<String>) -> Html<String> {
    let Some(id) = parse_id::<ClientId>(&id) else {
        return message_fragment("identifiant de client invalide");
    };
    match state
        .with_store(|store| views::clients::load_detail(store, id))
        .await
    {
        None => locked_fragment(),
        Some(Err(e)) => message_fragment(&e.to_string()),
        Some(Ok(None)) => {
            message_fragment("client introuvable — il a peut-être été supprimé entre-temps")
        }
        Some(Ok(Some((client, contacts, refs)))) => {
            Html(views::clients::detail_panel(&client, &contacts, refs, None).into_string())
        }
    }
}

pub async fn edit_panel(State(state): State<AppState>, Path(id): Path<String>) -> Html<String> {
    let Some(id) = parse_id::<ClientId>(&id) else {
        return message_fragment("identifiant de client invalide");
    };
    match state
        .with_store(|store| clients::client_by_id(store.connection(), id))
        .await
    {
        None => locked_fragment(),
        Some(Err(e)) => message_fragment(&e.to_string()),
        Some(Ok(None)) => {
            message_fragment("client introuvable — il a peut-être été supprimé entre-temps")
        }
        Some(Ok(Some(client))) => Html(
            views::clients::edit_panel(
                id,
                client.revision,
                &(&client).into(),
                &ClientFormErrors::default(),
            )
            .into_string(),
        ),
    }
}

pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(form): Form<ClientForm>,
) -> Response {
    let Some(id) = parse_id::<ClientId>(&id) else {
        return message_fragment("identifiant de client invalide").into_response();
    };
    let revision: i64 = form
        .revision
        .as_deref()
        .and_then(|s| s.parse().ok())
        .unwrap_or_default();

    let parsed = match parse_client_form(&form) {
        Ok(p) => p,
        Err(errors) => {
            return Html(
                views::clients::edit_panel(id, revision, &(&form).into(), &errors).into_string(),
            )
            .into_response();
        }
    };
    let cmd = clients::UpdateClient {
        id,
        revision,
        name: parsed.name,
        siren: parsed.siren,
        vat_number: parsed.vat_number,
        address: parsed.address,
    };
    match execute(&state, cmd).await {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => saved(),
        Some(Err(e)) => {
            let errors = client_error_banner(e, &format!("/clients/{id}/edit"));
            Html(views::clients::edit_panel(id, revision, &(&form).into(), &errors).into_string())
                .into_response()
        }
    }
}

async fn current_client(
    state: &AppState,
    id: ClientId,
) -> Option<Result<Option<Client>, AppError>> {
    state
        .with_store(|store| clients::client_by_id(store.connection(), id))
        .await
}

pub async fn archive(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    set_archived(state, id, true).await
}

pub async fn unarchive(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    set_archived(state, id, false).await
}

async fn set_archived(state: AppState, id: String, archived: bool) -> Response {
    let Some(id) = parse_id::<ClientId>(&id) else {
        return message_fragment("identifiant de client invalide").into_response();
    };
    let revision = match current_client(&state, id).await {
        None => return locked_fragment().into_response(),
        Some(Err(e)) => return message_fragment(&e.to_string()).into_response(),
        Some(Ok(None)) => return message_fragment("client introuvable").into_response(),
        Some(Ok(Some(client))) => client.revision,
    };
    let outcome = if archived {
        execute(&state, clients::ArchiveClient { id, revision }).await
    } else {
        execute(&state, clients::UnarchiveClient { id, revision }).await
    };
    match outcome {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => saved(),
        Some(Err(e)) => match state
            .with_store(|store| views::clients::load_detail(store, id))
            .await
        {
            Some(Ok(Some((client, contacts, refs)))) => {
                let markup =
                    views::clients::detail_panel(&client, &contacts, refs, Some(&e.to_string()));
                Html(markup.into_string()).into_response()
            }
            _ => message_fragment(&e.to_string()).into_response(),
        },
    }
}

pub async fn delete_confirm_panel(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Html<String> {
    let Some(id) = parse_id::<ClientId>(&id) else {
        return message_fragment("identifiant de client invalide");
    };
    match current_client(&state, id).await {
        None => locked_fragment(),
        Some(Err(e)) => message_fragment(&e.to_string()),
        Some(Ok(None)) => message_fragment("client introuvable"),
        Some(Ok(Some(client))) => Html(views::clients::delete_confirm_panel(&client).into_string()),
    }
}

pub async fn delete(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let Some(id) = parse_id::<ClientId>(&id) else {
        return message_fragment("identifiant de client invalide").into_response();
    };
    let revision = match current_client(&state, id).await {
        None => return locked_fragment().into_response(),
        Some(Err(e)) => return message_fragment(&e.to_string()).into_response(),
        Some(Ok(None)) => {
            return message_fragment("client introuvable — déjà supprimé").into_response();
        }
        Some(Ok(Some(client))) => client.revision,
    };
    match execute(&state, clients::DeleteClient { id, revision }).await {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => saved(),
        Some(Err(e)) => match current_client(&state, id).await {
            Some(Ok(Some(client))) => {
                let body = html! {
                    div class="form-error" { (e.to_string()) }
                    (views::clients::delete_confirm_panel(&client))
                };
                Html(body.into_string()).into_response()
            }
            _ => message_fragment(&e.to_string()).into_response(),
        },
    }
}

// -- Contacts ------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ContactForm {
    #[serde(default)]
    revision: Option<String>,
    name: String,
    #[serde(default)]
    email: String,
    #[serde(default)]
    phone: String,
    #[serde(default)]
    role: String,
}

impl From<&ContactForm> for ContactFormValues {
    fn from(f: &ContactForm) -> Self {
        Self {
            name: f.name.clone(),
            email: f.email.clone(),
            phone: f.phone.clone(),
            role: f.role.clone(),
        }
    }
}

fn opt(s: &str) -> Option<String> {
    let trimmed = s.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

pub async fn new_contact_panel(
    State(state): State<AppState>,
    Path(client_id): Path<String>,
) -> Html<String> {
    let Some(client_id) = parse_id::<ClientId>(&client_id) else {
        return message_fragment("identifiant de client invalide");
    };
    match state
        .with_store(|store| clients::client_by_id(store.connection(), client_id))
        .await
    {
        None => locked_fragment(),
        Some(Err(e)) => message_fragment(&e.to_string()),
        Some(Ok(None)) => message_fragment("client introuvable"),
        Some(Ok(Some(client))) => Html(
            views::clients::new_contact_panel(
                &client,
                &ContactFormValues::default(),
                &ContactFormErrors::default(),
            )
            .into_string(),
        ),
    }
}

pub async fn create_contact(
    State(state): State<AppState>,
    Path(client_id): Path<String>,
    Form(form): Form<ContactForm>,
) -> Response {
    let Some(client_id) = parse_id::<ClientId>(&client_id) else {
        return message_fragment("identifiant de client invalide").into_response();
    };
    if form.name.trim().is_empty() {
        let client = match current_client(&state, client_id).await {
            Some(Ok(Some(c))) => c,
            _ => return message_fragment("client introuvable").into_response(),
        };
        let errors = ContactFormErrors {
            name: Some("le nom est obligatoire".to_string()),
            ..Default::default()
        };
        return Html(
            views::clients::new_contact_panel(&client, &(&form).into(), &errors).into_string(),
        )
        .into_response();
    }
    let cmd = clients::CreateContact {
        client_id,
        name: form.name.trim().to_string(),
        email: opt(&form.email),
        phone: opt(&form.phone),
        role: opt(&form.role),
    };
    match execute(&state, cmd).await {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => reload_detail_or_saved(&state, client_id).await,
        Some(Err(e)) => {
            let client = match current_client(&state, client_id).await {
                Some(Ok(Some(c))) => c,
                _ => return message_fragment(&e.to_string()).into_response(),
            };
            let errors = contact_error_banner(e, &format!("/clients/{client_id}/contacts/new"));
            Html(views::clients::new_contact_panel(&client, &(&form).into(), &errors).into_string())
                .into_response()
        }
    }
}

pub async fn edit_contact_panel(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Html<String> {
    let Some(id) = parse_id::<ContactId>(&id) else {
        return message_fragment("identifiant de contact invalide");
    };
    match state
        .with_store(|store| clients::contact_by_id(store.connection(), id))
        .await
    {
        None => locked_fragment(),
        Some(Err(e)) => message_fragment(&e.to_string()),
        Some(Ok(None)) => message_fragment("contact introuvable"),
        Some(Ok(Some(contact))) => Html(
            views::clients::edit_contact_panel(
                &contact,
                &(&contact).into(),
                &ContactFormErrors::default(),
            )
            .into_string(),
        ),
    }
}

pub async fn update_contact(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(form): Form<ContactForm>,
) -> Response {
    let Some(id) = parse_id::<ContactId>(&id) else {
        return message_fragment("identifiant de contact invalide").into_response();
    };
    let current = match state
        .with_store(|store| clients::contact_by_id(store.connection(), id))
        .await
    {
        None => return locked_fragment().into_response(),
        Some(Err(e)) => return message_fragment(&e.to_string()).into_response(),
        Some(Ok(None)) => return message_fragment("contact introuvable").into_response(),
        Some(Ok(Some(c))) => c,
    };
    if form.name.trim().is_empty() {
        let errors = ContactFormErrors {
            name: Some("le nom est obligatoire".to_string()),
            ..Default::default()
        };
        return Html(
            views::clients::edit_contact_panel(&current, &(&form).into(), &errors).into_string(),
        )
        .into_response();
    }
    let revision: i64 = form
        .revision
        .as_deref()
        .and_then(|s| s.parse().ok())
        .unwrap_or(current.revision);
    let cmd = clients::UpdateContact {
        id,
        revision,
        name: form.name.trim().to_string(),
        email: opt(&form.email),
        phone: opt(&form.phone),
        role: opt(&form.role),
    };
    let client_id = current.client_id;
    match execute(&state, cmd).await {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => reload_detail_or_saved(&state, client_id).await,
        Some(Err(e)) => {
            let errors = contact_error_banner(e, &format!("/contacts/{id}/edit"));
            Html(
                views::clients::edit_contact_panel(&current, &(&form).into(), &errors)
                    .into_string(),
            )
            .into_response()
        }
    }
}

pub async fn delete_contact(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let Some(id) = parse_id::<ContactId>(&id) else {
        return message_fragment("identifiant de contact invalide").into_response();
    };
    let current = match state
        .with_store(|store| clients::contact_by_id(store.connection(), id))
        .await
    {
        None => return locked_fragment().into_response(),
        Some(Err(e)) => return message_fragment(&e.to_string()).into_response(),
        Some(Ok(None)) => {
            return message_fragment("contact introuvable — déjà supprimé").into_response();
        }
        Some(Ok(Some(c))) => c,
    };
    let client_id = current.client_id;
    match execute(
        &state,
        clients::DeleteContact {
            id,
            revision: current.revision,
        },
    )
    .await
    {
        None => locked_fragment().into_response(),
        Some(Ok(_)) => reload_detail_or_saved(&state, client_id).await,
        Some(Err(e)) => message_fragment(&e.to_string()).into_response(),
    }
}

/// Après une mutation de contact réussie, l'utilisateur regarde presque toujours encore la
/// fiche du client concerné : on la recharge à jour plutôt que de fermer le panneau comme pour
/// les mutations de client elles-mêmes (voir la convention de réponse en tête de module).
async fn reload_detail_or_saved(state: &AppState, client_id: ClientId) -> Response {
    match state
        .with_store(|store| views::clients::load_detail(store, client_id))
        .await
    {
        Some(Ok(Some((client, contacts, refs)))) => {
            Html(views::clients::detail_panel(&client, &contacts, refs, None).into_string())
                .into_response()
        }
        _ => saved(),
    }
}
