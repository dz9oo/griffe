//! Écran `clients` — la première entité de bout en bout de la gestion des données (lot 15) :
//! lister, créer, modifier, archiver, supprimer, et gérer les contacts d'un client, depuis la
//! fenêtre. Les entités suivantes (lots 16-18) répéteront ce même patron : liste
//! auto-rafraîchie + panneau latéral, plutôt qu'un formulaire par écran.

use griffe_core::app::AppError;
use griffe_core::clients::{ClientFilter, client_references, list_clients_with, list_contacts};
use griffe_core::domain::{Client, ClientId, Contact};
use griffe_core::store::Store;
use maud::{Markup, html};

use crate::layout::{ViewId, view_head};
use crate::views::{form, panel};

/// Valeurs d'un formulaire client — celles déjà en base pour une édition, celles resoumises par
/// l'utilisateur pour un nouveau rendu après erreur ; jamais recalculées par ce module.
#[derive(Default, Clone)]
pub struct ClientFormValues {
    pub name: String,
    pub siren: String,
    pub vat_number: String,
    pub street: String,
    pub postal_code: String,
    pub city: String,
    pub country: String,
}

impl From<&Client> for ClientFormValues {
    fn from(c: &Client) -> Self {
        Self {
            name: c.name.clone(),
            siren: c.siren.map(|s| s.to_string()).unwrap_or_default(),
            vat_number: c
                .vat_number
                .as_ref()
                .map(std::string::ToString::to_string)
                .unwrap_or_default(),
            street: c
                .address
                .as_ref()
                .map_or_else(String::new, |a| a.street.clone()),
            postal_code: c
                .address
                .as_ref()
                .map_or_else(String::new, |a| a.postal_code.clone()),
            city: c
                .address
                .as_ref()
                .map_or_else(String::new, |a| a.city.clone()),
            country: c
                .address
                .as_ref()
                .map_or_else(String::new, |a| a.country.clone()),
        }
    }
}

/// Erreurs attribuées à un champ précis quand c'est possible (adresse, SIREN, TVA — validés
/// avant même de construire la commande), ou à un bandeau général sinon (règle métier refusée
/// par le cœur, conflit de révision). Le cœur ne renvoie qu'un message non structuré
/// (`AppError::Domain(String)`) : l'attribution par champ s'arrête là où cette structure
/// s'arrête, plutôt que de deviner le champ concerné par correspondance de texte.
#[derive(Default)]
pub struct ClientFormErrors {
    pub name: Option<String>,
    pub siren: Option<String>,
    pub vat_number: Option<String>,
    pub address: Option<String>,
    pub banner: Option<String>,
    pub conflict: Option<(String, String)>,
}

fn client_form(
    action: &str,
    revision: Option<i64>,
    values: &ClientFormValues,
    errors: &ClientFormErrors,
) -> Markup {
    html! {
        form hx-post=(action) hx-target="#panel" hx-swap="innerHTML" {
            @if let Some((message, reload)) = &errors.conflict {
                (form::conflict_banner(message, reload))
            } @else {
                @if let Some(msg) = &errors.banner {
                    (form::error_banner(msg))
                }
                @if let Some(revision) = revision {
                    (form::hidden("revision", &revision.to_string()))
                }
                (form::text("name", "Nom", &values.name, errors.name.as_deref()))
                (form::text("siren", "SIREN (optionnel)", &values.siren, errors.siren.as_deref()))
                (form::text("vat_number", "N° TVA intracommunautaire (optionnel)", &values.vat_number, errors.vat_number.as_deref()))
                div class="field-group" {
                    @if let Some(msg) = &errors.address {
                        div class="field-error" { (msg) }
                    }
                    (form::text("street", "Adresse — rue", &values.street, None))
                    (form::text("postal_code", "Code postal", &values.postal_code, None))
                    (form::text("city", "Ville", &values.city, None))
                    (form::text("country", "Pays (ISO, ex. FR)", &values.country, None))
                }
                (form::actions(if revision.is_some() { "Enregistrer" } else { "Créer" }))
            }
        }
    }
}

pub fn new_panel(values: &ClientFormValues, errors: &ClientFormErrors) -> Markup {
    panel::sheet(
        "Nouveau client",
        client_form("/clients", None, values, errors),
    )
}

pub fn edit_panel(
    id: ClientId,
    revision: i64,
    values: &ClientFormValues,
    errors: &ClientFormErrors,
) -> Markup {
    panel::sheet(
        "Modifier le client",
        client_form(&format!("/clients/{id}"), Some(revision), values, errors),
    )
}

fn status_badge(client: &Client) -> Markup {
    html! {
        @if client.archived_at.is_some() {
            span class="badge warn" { "archivé" }
        } @else {
            span class="badge ok" { "actif" }
        }
    }
}

pub fn detail_panel(
    client: &Client,
    contacts: &[Contact],
    refs: griffe_core::clients::ClientReferences,
    error: Option<&str>,
) -> Markup {
    let body = html! {
        @if let Some(msg) = error {
            (form::error_banner(msg))
        }
        div class="detail-head" {
            div class="detail-title" { (client.name) }
            (status_badge(client))
        }
        dl class="detail-fields" {
            @if let Some(siren) = client.siren {
                dt { "SIREN" } dd class="mono" { (siren) }
            }
            @if let Some(vat) = &client.vat_number {
                dt { "TVA intra" } dd class="mono" { (vat) }
            }
            @if let Some(a) = &client.address {
                dt { "Adresse" } dd { (a.street) ", " (a.postal_code) " " (a.city) " (" (a.country) ")" }
            }
        }

        div class="detail-actions" {
            button class="btn" hx-get=(format!("/clients/{}/edit", client.id)) hx-target="#panel" hx-swap="innerHTML" { "modifier" }
            @if client.archived_at.is_some() {
                button class="btn" hx-post=(format!("/clients/{}/unarchive", client.id)) hx-target="#panel" hx-swap="innerHTML" { "réintégrer" }
            } @else {
                button class="btn" hx-post=(format!("/clients/{}/archive", client.id)) hx-target="#panel" hx-swap="innerHTML" { "archiver" }
            }
            button class="btn danger" hx-get=(format!("/clients/{}/delete", client.id)) hx-target="#panel" hx-swap="innerHTML" { "supprimer" }
        }

        @if !refs.is_empty() {
            div class="detail-note" {
                "Référencé par " (refs.opportunities) " opportunité(s), " (refs.quotes) " devis, "
                (refs.missions) " mission(s), " (refs.invoices) " facture(s) — la suppression sera refusée tant que ce total n'est pas nul."
            }
        }

        div class="detail-section" {
            div class="detail-section-head" {
                span { "Contacts" }
                button class="btn small" hx-get=(format!("/clients/{}/contacts/new", client.id)) hx-target="#panel" hx-swap="innerHTML" { "+ contact" }
            }
            @if contacts.is_empty() {
                div class="empty-state" { "aucun contact" }
            } @else {
                @for contact in contacts {
                    div class="contact-row" {
                        div {
                            div class="contact-name" { (contact.name) }
                            div class="contact-meta" {
                                @if let Some(role) = &contact.role { (role) " · " }
                                @if let Some(email) = &contact.email { (email) }
                                @if contact.email.is_none() && contact.role.is_none() { "—" }
                            }
                        }
                        div class="contact-actions" {
                            button class="btn small" hx-get=(format!("/contacts/{}/edit", contact.id)) hx-target="#panel" hx-swap="innerHTML" { "modifier" }
                            button class="btn small danger" hx-post=(format!("/contacts/{}/delete", contact.id)) hx-target="#panel" hx-swap="innerHTML" { "supprimer" }
                        }
                    }
                }
            }
        }
    };
    panel::sheet(&client.name, body)
}

pub fn delete_confirm_panel(client: &Client) -> Markup {
    let body = html! {
        div class="detail-note" {
            "Supprimer définitivement « " (client.name) " » ? Cette action est irréversible — "
            "si ce client est référencé quelque part, la suppression sera refusée."
        }
        div class="form-actions" {
            // Révision relue côté serveur au moment du clic (pas portée ici) : entre l'ouverture
            // de ce panneau et la confirmation, un autre process a pu modifier la fiche — mieux
            // vaut agir sur l'état le plus frais que sur celui lu à l'ouverture du panneau.
            button class="btn danger" hx-post=(format!("/clients/{}/delete", client.id)) hx-target="#panel" hx-swap="innerHTML" {
                "confirmer la suppression"
            }
        }
    };
    panel::sheet("Confirmer la suppression", body)
}

#[derive(Default)]
pub struct ContactFormValues {
    pub name: String,
    pub email: String,
    pub phone: String,
    pub role: String,
}

impl From<&Contact> for ContactFormValues {
    fn from(c: &Contact) -> Self {
        Self {
            name: c.name.clone(),
            email: c.email.clone().unwrap_or_default(),
            phone: c.phone.clone().unwrap_or_default(),
            role: c.role.clone().unwrap_or_default(),
        }
    }
}

#[derive(Default)]
pub struct ContactFormErrors {
    pub name: Option<String>,
    pub banner: Option<String>,
    pub conflict: Option<(String, String)>,
}

fn contact_form(
    action: &str,
    revision: Option<i64>,
    values: &ContactFormValues,
    errors: &ContactFormErrors,
) -> Markup {
    html! {
        form hx-post=(action) hx-target="#panel" hx-swap="innerHTML" {
            @if let Some((message, reload)) = &errors.conflict {
                (form::conflict_banner(message, reload))
            } @else {
                @if let Some(msg) = &errors.banner {
                    (form::error_banner(msg))
                }
                @if let Some(revision) = revision {
                    (form::hidden("revision", &revision.to_string()))
                }
                (form::text("name", "Nom", &values.name, errors.name.as_deref()))
                (form::text("role", "Rôle (optionnel)", &values.role, None))
                (form::text("email", "Email (optionnel)", &values.email, None))
                (form::text("phone", "Téléphone (optionnel)", &values.phone, None))
                (form::actions(if revision.is_some() { "Enregistrer" } else { "Ajouter" }))
            }
        }
    }
}

pub fn new_contact_panel(
    client: &Client,
    values: &ContactFormValues,
    errors: &ContactFormErrors,
) -> Markup {
    panel::sheet(
        &format!("Nouveau contact — {}", client.name),
        contact_form(
            &format!("/clients/{}/contacts", client.id),
            None,
            values,
            errors,
        ),
    )
}

pub fn edit_contact_panel(
    contact: &Contact,
    values: &ContactFormValues,
    errors: &ContactFormErrors,
) -> Markup {
    panel::sheet(
        "Modifier le contact",
        contact_form(
            &format!("/contacts/{}", contact.id),
            Some(contact.revision),
            values,
            errors,
        ),
    )
}

/// Table des clients, avec sa bascule actifs/archivés, dans un conteneur qui se rafraîchit
/// lui-même après toute mutation (`griffe:saved`, déclenché par `HX-Trigger` sur chaque
/// réponse de mutation réussie — voir `crate::clients`) — et qui se remplace intégralement,
/// bascule comprise, quand elle est actionnée : le libellé et la cible de la bascule doivent
/// donc vivre ici plutôt que dans [`render`], sans quoi ils resteraient périmés après un
/// rafraîchissement qui ne touche que ce conteneur.
pub fn list_fragment(store: &Store, filter: ClientFilter) -> Result<Markup, AppError> {
    let clients = list_clients_with(store.connection(), filter)?;
    let (archived_query, toggle_label, toggle_target) = match filter {
        ClientFilter::ActiveOnly => (false, "voir les archivés", "/clients/table?archived=true"),
        ClientFilter::All => (
            true,
            "voir les actifs seulement",
            "/clients/table?archived=false",
        ),
    };

    Ok(html! {
        div id="clients-list"
            hx-get=(format!("/clients/table?archived={archived_query}"))
            hx-trigger="griffe:saved from:body"
            hx-target="this"
            hx-swap="outerHTML" {
            div class="pipe-toolbar" {
                button class="btn primary" hx-get="/clients/new" hx-target="#panel" hx-swap="innerHTML" { "+ nouveau client" }
                a class="filter-chip" href="#" hx-get=(toggle_target) hx-target="#clients-list" hx-swap="outerHTML" { (toggle_label) }
            }
            @if clients.is_empty() {
                div class="empty-state" { "aucun client — cliquez sur « nouveau client »" }
            } @else {
                div class="panel bordered" style="padding:0" {
                    table {
                        tr {
                            th style="padding-left:18px" { "nom" }
                            th { "siren" }
                            th { "ville" }
                            th style="padding-right:18px" { "statut" }
                        }
                        @for client in &clients {
                            tr class="row-clickable" hx-get=(format!("/clients/{}", client.id)) hx-target="#panel" hx-swap="innerHTML" {
                                td style="padding-left:18px" { (client.name) }
                                td class="mono" { (client.siren.map(|s| s.to_string()).unwrap_or_else(|| "—".to_string())) }
                                td { (client.address.as_ref().map_or_else(|| "—".to_string(), |a| a.city.clone())) }
                                td style="padding-right:18px" { (status_badge(client)) }
                            }
                        }
                    }
                }
            }
        }
    })
}

#[allow(dead_code)] // écran historique hors nav ; les tableaux et panneaux restent.
pub fn render(store: &Store, filter: ClientFilter) -> Result<Markup, AppError> {
    let count = list_clients_with(store.connection(), filter)?.len();
    Ok(html! {
        (view_head(ViewId::Clients, &format!("{count} client(s)")))
        (list_fragment(store, filter)?)
    })
}

/// Fiche + contacts + références, assemblés pour [`detail_panel`] — factorisé parce que la
/// route de détail et celle de confirmation de suppression en ont toutes les deux besoin.
pub fn load_detail(
    store: &Store,
    id: ClientId,
) -> Result<Option<(Client, Vec<Contact>, griffe_core::clients::ClientReferences)>, AppError> {
    let Some(client) = griffe_core::clients::client_by_id(store.connection(), id)? else {
        return Ok(None);
    };
    let contacts = list_contacts(store.connection(), id)?;
    let refs = client_references(store.connection(), id)?;
    Ok(Some((client, contacts, refs)))
}
