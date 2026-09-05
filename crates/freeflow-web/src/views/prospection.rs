//! Écran `prospection` — lot 16 : gestion de bout en bout (créer/modifier/avancer/gagner/perdre/
//! archiver/supprimer une opportunité, journaliser/modifier/supprimer une interaction), même
//! patron que `views::clients` (lot 15).

use freeflow_core::app::AppError;
use freeflow_core::clients::list_clients;
use freeflow_core::domain::{
    Interaction, InteractionKind, LossReason, Opportunity, OpportunityId, OpportunityStage,
    format_date,
};
use freeflow_core::prospection::{
    OpportunityFilter, OpportunityReferences, list_interactions, list_opportunities_with,
    opportunity_by_id, opportunity_references,
};
use freeflow_core::store::Store;
use maud::{Markup, html};

use crate::layout::{ViewId, view_head};
use crate::views::{form, panel};

fn client_name(store: &Store, client_id: freeflow_core::domain::ClientId) -> String {
    list_clients(store.connection())
        .ok()
        .and_then(|clients| clients.into_iter().find(|c| c.id == client_id))
        .map_or_else(|| "?".to_string(), |c| c.name)
}

fn first_contact(
    store: &Store,
    client_id: freeflow_core::domain::ClientId,
) -> Option<freeflow_core::domain::Contact> {
    freeflow_core::clients::list_contacts(store.connection(), client_id)
        .ok()
        .and_then(|contacts| contacts.into_iter().next())
}

fn party_address(
    store: &Store,
    client_id: freeflow_core::domain::ClientId,
) -> Option<freeflow_core::domain::Address> {
    list_clients(store.connection())
        .ok()
        .and_then(|clients| clients.into_iter().find(|c| c.id == client_id))
        .and_then(|c| c.address)
}

/// Pré-remplit les champs prospect d'un formulaire d'édition.
pub fn party_form_values(
    store: &Store,
    client_id: freeflow_core::domain::ClientId,
) -> OpportunityFormValues {
    let party = list_clients(store.connection())
        .ok()
        .and_then(|clients| clients.into_iter().find(|c| c.id == client_id));
    let contact = first_contact(store, client_id);
    let mut values = OpportunityFormValues::default();
    if let Some(party) = party {
        values.prospect = party.name;
        values.client_revision = party.revision.to_string();
        if let Some(address) = party.address {
            values.street = address.street;
            values.postal_code = address.postal_code;
            values.city = address.city;
            values.country = address.country;
        }
    }
    if let Some(contact) = contact {
        values.representative = contact.name;
        values.email = contact.email.unwrap_or_default();
        values.phone = contact.phone.unwrap_or_default();
    }
    values
}

#[derive(Default, Clone)]
pub struct OpportunityFormValues {
    pub prospect: String,
    pub representative: String,
    pub email: String,
    pub phone: String,
    pub street: String,
    pub postal_code: String,
    pub city: String,
    pub country: String,
    pub client_revision: String,
    pub name: String,
    pub amount: String,
    pub probability: String,
    pub next_action: String,
    pub source: String,
}

#[derive(Default)]
pub struct OpportunityFormErrors {
    pub prospect: Option<String>,
    pub address: Option<String>,
    pub name: Option<String>,
    pub amount: Option<String>,
    pub probability: Option<String>,
    pub banner: Option<String>,
    pub conflict: Option<(String, String)>,
}

/// Le formulaire principal porte l'état *descriptif* de l'opportunité, et — à la création ou
/// tant que la fiche est encore un prospect — les coordonnées de la partie. L'étape
/// (`advance`/`win`/`lose`) a ses propres panneaux.
fn opportunity_form(
    action: &str,
    revision: Option<i64>,
    show_prospect: bool,
    values: &OpportunityFormValues,
    errors: &OpportunityFormErrors,
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
                @if show_prospect {
                    @if !values.client_revision.is_empty() {
                        (form::hidden("client_revision", &values.client_revision))
                    }
                    (form::text("prospect", "Nom du prospect", &values.prospect, errors.prospect.as_deref()))
                    (form::text("representative", "Représentant (optionnel)", &values.representative, None))
                    (form::text("email", "Email (optionnel)", &values.email, None))
                    (form::text("phone", "Téléphone (optionnel)", &values.phone, None))
                    div class="field-group" {
                        @if let Some(msg) = &errors.address {
                            div class="field-error" { (msg) }
                        }
                        (form::text("street", "Adresse — rue", &values.street, None))
                        (form::text("postal_code", "Code postal", &values.postal_code, None))
                        (form::text("city", "Ville", &values.city, None))
                        (form::text("country", "Pays (ISO, ex. FR)", &values.country, None))
                    }
                    (form::field_help("Le prospect n'apparaît dans Clients qu'au premier devis ou à la première facture."))
                }
                (form::text("name", "Nom de l'opportunité (optionnel)", &values.name, errors.name.as_deref()))
                (form::text("amount", "Montant estimé (€)", &values.amount, errors.amount.as_deref()))
                (form::number("probability", "Probabilité de gain (%)", &values.probability, "1", errors.probability.as_deref()))
                (form::date("next_action", "Prochaine action", &values.next_action, None))
                (form::text("source", "Source (optionnel)", &values.source, None))
                (form::actions(if revision.is_some() { "Enregistrer" } else { "Créer" }))
            }
        }
    }
}

pub fn new_panel(values: &OpportunityFormValues, errors: &OpportunityFormErrors) -> Markup {
    panel::sheet(
        "Nouveau prospect",
        opportunity_form("/prospection", None, true, values, errors),
    )
}

pub fn edit_panel(
    id: OpportunityId,
    revision: i64,
    show_prospect: bool,
    values: &OpportunityFormValues,
    errors: &OpportunityFormErrors,
) -> Markup {
    panel::sheet(
        "Modifier l'opportunité",
        opportunity_form(
            &format!("/prospection/{id}"),
            Some(revision),
            show_prospect,
            values,
            errors,
        ),
    )
}

fn stage_badge(stage: OpportunityStage) -> Markup {
    html! {
        @match stage {
            OpportunityStage::Won => span class="badge ok" { "gagnée" },
            OpportunityStage::Lost => span class="badge danger" { "perdue" },
            other => span class="badge info" { (other.as_str()) },
        }
    }
}

fn status_badge(o: &Opportunity) -> Markup {
    html! {
        @if o.archived_at.is_some() {
            span class="badge warn" { "archivée" }
        } @else {
            (stage_badge(o.stage))
        }
    }
}

pub fn detail_panel(
    store: &Store,
    opportunity: &Opportunity,
    interactions: &[Interaction],
    refs: OpportunityReferences,
    error: Option<&str>,
) -> Markup {
    let weighted = opportunity.probability.weighted(opportunity.amount);
    let closed = opportunity.stage.is_closed();
    let body = html! {
        @if let Some(msg) = error {
            (form::error_banner(msg))
        }
        div class="detail-head" {
            div class="detail-title" { (opportunity.name) }
            (status_badge(opportunity))
        }
        dl class="detail-fields" {
            dt { "prospect" } dd { (client_name(store, opportunity.client_id)) }
            @if let Some(contact) = first_contact(store, opportunity.client_id) {
                @if !contact.name.is_empty() {
                    dt { "représentant" } dd { (contact.name) }
                }
                @if let Some(email) = &contact.email {
                    dt { "email" } dd { (email) }
                }
                @if let Some(phone) = &contact.phone {
                    dt { "téléphone" } dd { (phone) }
                }
            }
            @if let Some(address) = party_address(store, opportunity.client_id) {
                dt { "adresse" } dd {
                    (address.street) ", " (address.postal_code) " " (address.city) ", " (address.country)
                }
            }
            dt { "montant" } dd class="mono" { (opportunity.amount) }
            dt { "probabilité" } dd { (opportunity.probability.percent()) "%" }
            dt { "pondéré" } dd class="mono" { (weighted) }
            @if let Some(d) = opportunity.next_action_at {
                dt { "prochaine action" } dd class="mono" { (format_date(d)) }
            }
            @if let Some(source) = &opportunity.source {
                dt { "source" } dd { (source) }
            }
            @if let Some(reason) = &opportunity.loss_reason {
                dt { "motif de perte" } dd { (format!("{reason:?}")) }
            }
        }

        div class="detail-actions" {
            @if !closed {
                button class="btn" hx-get=(format!("/prospection/{}/edit", opportunity.id)) hx-target="#panel" hx-swap="innerHTML" { "modifier" }
                button class="btn" hx-get=(format!("/prospection/{}/advance", opportunity.id)) hx-target="#panel" hx-swap="innerHTML" { "avancer" }
                button class="btn" hx-get=(format!("/prospection/{}/win", opportunity.id)) hx-target="#panel" hx-swap="innerHTML" { "gagner" }
                button class="btn danger" hx-get=(format!("/prospection/{}/lose", opportunity.id)) hx-target="#panel" hx-swap="innerHTML" { "perdre" }
            }
            @if opportunity.archived_at.is_some() {
                button class="btn" hx-post=(format!("/prospection/{}/unarchive", opportunity.id)) hx-target="#panel" hx-swap="innerHTML" { "réintégrer" }
            } @else {
                button class="btn" hx-post=(format!("/prospection/{}/archive", opportunity.id)) hx-target="#panel" hx-swap="innerHTML" { "archiver" }
            }
            button class="btn danger" hx-get=(format!("/prospection/{}/delete", opportunity.id)) hx-target="#panel" hx-swap="innerHTML" { "supprimer" }
        }

        @if !refs.is_empty() {
            div class="detail-note" {
                "Référencée par " (refs.quotes) " devis — la suppression sera refusée tant que ce total n'est pas nul."
            }
        }

        div class="detail-section" {
            div class="detail-section-head" {
                span { "Interactions" }
                button class="btn small" hx-get=(format!("/prospection/{}/interactions/new", opportunity.id)) hx-target="#panel" hx-swap="innerHTML" { "+ interaction" }
            }
            @if interactions.is_empty() {
                div class="empty-state" { "aucune interaction" }
            } @else {
                @for interaction in interactions {
                    div class="contact-row" {
                        div {
                            div class="contact-name" { (interaction.kind.as_str()) " — " (interaction.occurred_at.date()) }
                            div class="contact-meta" { (interaction.note) }
                        }
                        div class="contact-actions" {
                            button class="btn small" hx-get=(format!("/interactions/{}/edit", interaction.id)) hx-target="#panel" hx-swap="innerHTML" { "modifier" }
                            button class="btn small danger" hx-post=(format!("/interactions/{}/delete", interaction.id)) hx-target="#panel" hx-swap="innerHTML" { "supprimer" }
                        }
                    }
                }
            }
        }
    };
    panel::sheet(&opportunity.name, body)
}

pub fn delete_confirm_panel(opportunity: &Opportunity) -> Markup {
    let body = html! {
        div class="detail-note" {
            "Supprimer définitivement « " (opportunity.name) " » ? Cette action est irréversible — "
            "si un devis la référence, la suppression sera refusée."
        }
        div class="form-actions" {
            button class="btn danger" hx-post=(format!("/prospection/{}/delete", opportunity.id)) hx-target="#panel" hx-swap="innerHTML" {
                "confirmer la suppression"
            }
        }
    };
    panel::sheet("Confirmer la suppression", body)
}

// -- Panneaux de transition : avancer / gagner / perdre --------------------------------------

#[derive(Default)]
pub struct TransitionErrors {
    pub banner: Option<String>,
}

pub fn advance_panel(opportunity: &Opportunity, errors: &TransitionErrors) -> Markup {
    let stages: &[(&str, &str)] = &[
        ("qualification", "Qualification"),
        ("discovery", "Discovery"),
        ("proposal", "Proposal"),
        ("negotiation", "Negotiation"),
    ];
    let body = html! {
        form hx-post=(format!("/prospection/{}/advance", opportunity.id)) hx-target="#panel" hx-swap="innerHTML" {
            @if let Some(msg) = &errors.banner { (form::error_banner(msg)) }
            (form::select("to", "Étape cible", stages, opportunity.stage.as_str(), None))
            (form::date("next_action", "Prochaine action", "", None))
            (form::actions("Avancer"))
        }
    };
    panel::sheet(&format!("Avancer — {}", opportunity.name), body)
}

pub fn win_panel(opportunity: &Opportunity, errors: &TransitionErrors) -> Markup {
    let body = html! {
        form hx-post=(format!("/prospection/{}/win", opportunity.id)) hx-target="#panel" hx-swap="innerHTML" {
            @if let Some(msg) = &errors.banner { (form::error_banner(msg)) }
            (form::date("started_on", "Date de début de la mission", "", None))
            (form::actions("Gagner"))
        }
    };
    panel::sheet(&format!("Gagner — {}", opportunity.name), body)
}

pub fn lose_panel(opportunity: &Opportunity, errors: &TransitionErrors) -> Markup {
    let reasons: &[(&str, &str)] = &[
        ("budget", "Budget"),
        ("timing", "Timing"),
        ("competitor", "Concurrent"),
        ("no-response", "Sans réponse"),
        ("scope-mismatch", "Périmètre inadapté"),
        ("other", "Autre (préciser ci-dessous)"),
    ];
    let body = html! {
        form hx-post=(format!("/prospection/{}/lose", opportunity.id)) hx-target="#panel" hx-swap="innerHTML" {
            @if let Some(msg) = &errors.banner { (form::error_banner(msg)) }
            (form::select("reason", "Motif de perte", reasons, "budget", None))
            (form::text("detail", "Détail (si « Autre »)", "", None))
            (form::actions("Perdre"))
        }
    };
    panel::sheet(&format!("Perdre — {}", opportunity.name), body)
}

// -- Interactions --------------------------------------------------------------------------

#[derive(Default, Clone)]
pub struct InteractionFormValues {
    pub kind: String,
    pub note: String,
    pub occurred_on: String,
}

impl From<&Interaction> for InteractionFormValues {
    fn from(i: &Interaction) -> Self {
        Self {
            kind: i.kind.as_str().to_string(),
            note: i.note.clone(),
            occurred_on: i.occurred_at.date().to_string(),
        }
    }
}

#[derive(Default)]
pub struct InteractionFormErrors {
    pub banner: Option<String>,
    pub conflict: Option<(String, String)>,
}

const INTERACTION_KINDS: &[(&str, &str)] = &[
    ("call", "Appel"),
    ("email", "Email"),
    ("meeting", "Réunion"),
    ("note", "Note"),
];

fn interaction_form(
    action: &str,
    revision: Option<i64>,
    values: &InteractionFormValues,
    errors: &InteractionFormErrors,
) -> Markup {
    html! {
        form hx-post=(action) hx-target="#panel" hx-swap="innerHTML" {
            @if let Some((message, reload)) = &errors.conflict {
                (form::conflict_banner(message, reload))
            } @else {
                @if let Some(msg) = &errors.banner { (form::error_banner(msg)) }
                @if let Some(revision) = revision {
                    (form::hidden("revision", &revision.to_string()))
                }
                (form::select("kind", "Type", INTERACTION_KINDS, &values.kind, None))
                (form::text("note", "Note", &values.note, None))
                (form::date("occurred_on", "Date (par défaut : aujourd'hui)", &values.occurred_on, None))
                (form::actions(if revision.is_some() { "Enregistrer" } else { "Ajouter" }))
            }
        }
    }
}

pub fn new_interaction_panel(
    opportunity: &Opportunity,
    values: &InteractionFormValues,
    errors: &InteractionFormErrors,
) -> Markup {
    panel::sheet(
        &format!("Nouvelle interaction — {}", opportunity.name),
        interaction_form(
            &format!("/prospection/{}/interactions", opportunity.id),
            None,
            values,
            errors,
        ),
    )
}

pub fn edit_interaction_panel(
    interaction: &Interaction,
    values: &InteractionFormValues,
    errors: &InteractionFormErrors,
) -> Markup {
    panel::sheet(
        "Modifier l'interaction",
        interaction_form(
            &format!("/interactions/{}", interaction.id),
            Some(interaction.revision),
            values,
            errors,
        ),
    )
}

// -- Liste -----------------------------------------------------------------------------------

fn toggle_url(closed: bool, archived: bool) -> String {
    format!("/prospection/table?closed={closed}&archived={archived}")
}

pub fn list_fragment(store: &Store, filter: OpportunityFilter) -> Result<Markup, AppError> {
    let opportunities = list_opportunities_with(store.connection(), filter)?;

    Ok(html! {
        div id="prospection-list"
            hx-get=(toggle_url(filter.include_closed, filter.include_archived))
            hx-trigger="freeflow:saved from:body"
            hx-target="this"
            hx-swap="outerHTML" {
            div class="pipe-toolbar" {
                button class="btn primary" hx-get="/prospection/new" hx-target="#panel" hx-swap="innerHTML" { "+ nouveau prospect" }
                a class="filter-chip" href="#" hx-get=(toggle_url(!filter.include_closed, filter.include_archived)) hx-target="#prospection-list" hx-swap="outerHTML" {
                    (if filter.include_closed { "masquer les closes" } else { "voir les closes" })
                }
                a class="filter-chip" href="#" hx-get=(toggle_url(filter.include_closed, !filter.include_archived)) hx-target="#prospection-list" hx-swap="outerHTML" {
                    (if filter.include_archived { "masquer les archivées" } else { "voir les archivées" })
                }
            }
            @if opportunities.is_empty() {
                div class="empty-state" { "aucun prospect — cliquez sur « nouveau prospect »" }
            } @else {
                div class="panel bordered" style="padding:0" {
                    table {
                        tr {
                            th style="padding-left:18px" { "prospect" }
                            th { "étape" }
                            th { "montant" }
                            th { "proba" }
                            th { "prochaine action" }
                            th style="padding-right:18px" { "statut" }
                        }
                        @for o in &opportunities {
                            @let name = client_name(store, o.client_id);
                            @let today = freeflow_core::clock::today_local();
                            @let is_late = o.next_action_at.is_some_and(|d| d < today);
                            tr class="row-clickable" hx-get=(format!("/prospection/{}", o.id)) hx-target="#panel" hx-swap="innerHTML" {
                                td style="padding-left:18px" { (name) " — " (o.name) }
                                td class="mono" { (o.stage.as_str()) }
                                td class="num" { (o.amount) }
                                td { (o.probability.percent()) "%" }
                                @match o.next_action_at {
                                    Some(d) => td class="mono" style=(if is_late { "color:var(--red)" } else { "" }) { (format_date(d)) },
                                    None => td class="mono" { "—" },
                                }
                                td style="padding-right:18px" { (status_badge(o)) }
                            }
                        }
                    }
                }
            }
        }
    })
}

pub fn render(store: &Store, filter: OpportunityFilter) -> Result<Markup, AppError> {
    let count = list_opportunities_with(store.connection(), filter)?.len();
    Ok(html! {
        (view_head(ViewId::Prospection, &format!("{count} opportunité(s)")))
        (list_fragment(store, filter)?)
    })
}

/// Opportunité + interactions + références, pour [`detail_panel`]/[`delete_confirm_panel`].
pub fn load_detail(
    store: &Store,
    id: OpportunityId,
) -> Result<Option<(Opportunity, Vec<Interaction>, OpportunityReferences)>, AppError> {
    let Some(opportunity) = opportunity_by_id(store.connection(), id)? else {
        return Ok(None);
    };
    let interactions = list_interactions(store.connection(), id)?;
    let refs = opportunity_references(store.connection(), id)?;
    Ok(Some((opportunity, interactions, refs)))
}

/// Résout `LossReason` depuis les champs `reason`/`detail` du panneau `lose_panel`.
pub fn parse_loss_reason(reason: &str, detail: &str) -> Result<LossReason, String> {
    match reason {
        "budget" => Ok(LossReason::Budget),
        "timing" => Ok(LossReason::Timing),
        "competitor" => Ok(LossReason::Competitor),
        "no-response" => Ok(LossReason::NoResponse),
        "scope-mismatch" => Ok(LossReason::ScopeMismatch),
        "other" => Ok(LossReason::Other(detail.trim().to_string())),
        other => Err(format!("motif de perte invalide : {other}")),
    }
}

pub fn parse_interaction_kind(s: &str) -> Result<InteractionKind, String> {
    s.parse()
        .map_err(|e: freeflow_core::domain::UnknownInteractionKind| e.to_string())
}
