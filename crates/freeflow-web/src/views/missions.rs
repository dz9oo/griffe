//! Écran `missions` — lot 16 : gestion de bout en bout (créer/modifier/clôturer/rouvrir/archiver/
//! supprimer une mission, saisir/modifier/supprimer du temps), même patron que `views::clients`
//! (lot 15) et `views::prospection` (lot 16).

use freeflow_core::app::AppError;
use freeflow_core::clients::list_clients;
use freeflow_core::domain::{Milestone, Mission, MissionId, MissionKind, TimeEntry, format_date};
use freeflow_core::missions::{
    self, BillingSchedule, MissionFilter, MissionReferences, effective_daily_rate,
    list_missions_with, list_time_entries, mission_by_id, mission_references,
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

const KIND_OPTIONS: &[(&str, &str)] = &[
    ("regie", "Régie"),
    ("forfait", "Forfait"),
    ("recurrent", "Récurrent"),
];

fn kind_value(kind: &MissionKind) -> &'static str {
    match kind {
        MissionKind::Regie { .. } => "regie",
        MissionKind::Forfait { .. } => "forfait",
        MissionKind::Recurrent { .. } => "recurrent",
    }
}

fn milestones_to_text(milestones: &[Milestone]) -> String {
    milestones
        .iter()
        .map(std::string::ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Default, Clone)]
pub struct MissionFormValues {
    pub client: String,
    pub name: String,
    pub kind: String,
    pub amount: String,
    pub started_on: String,
    pub milestones: String,
}

impl MissionFormValues {
    pub fn from_mission(store: &Store, m: &Mission) -> Self {
        // `to_decimal_string`, jamais `to_string` : le `Display` humain de `Money` (espaces de
        // groupement, `€`) n'est pas relu par `Money::parse_decimal` — l'utiliser ici faisait
        // rejeter par le formulaire d'édition le montant qu'il venait lui-même de pré-remplir.
        let amount = match &m.kind {
            MissionKind::Regie { daily_rate } => daily_rate.to_decimal_string(),
            MissionKind::Forfait { budget } => budget.to_decimal_string(),
            MissionKind::Recurrent { monthly_amount } => monthly_amount.to_decimal_string(),
        };
        Self {
            client: client_name(store, m.client_id),
            name: m.name.clone(),
            kind: kind_value(&m.kind).to_string(),
            amount,
            started_on: format_date(m.started_on),
            milestones: milestones_to_text(&m.milestones),
        }
    }
}

#[derive(Default)]
pub struct MissionFormErrors {
    pub client: Option<String>,
    pub name: Option<String>,
    pub amount: Option<String>,
    pub milestones: Option<String>,
    pub banner: Option<String>,
    pub conflict: Option<(String, String)>,
}

fn mission_form(
    action: &str,
    revision: Option<i64>,
    show_client: bool,
    values: &MissionFormValues,
    errors: &MissionFormErrors,
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
                @if show_client {
                    (form::text("client", "Client (nom ou référence)", &values.client, errors.client.as_deref()))
                }
                (form::text("name", "Nom de la mission", &values.name, errors.name.as_deref()))
                (form::select("kind", "Type", KIND_OPTIONS, &values.kind, None))
                (form::text("amount", "Montant (TJM, budget, ou mensuel — selon le type)", &values.amount, errors.amount.as_deref()))
                (form::date("started_on", "Date de début", &values.started_on, None))
                (form::textarea("milestones", "Jalons (un par ligne, forfait uniquement)", &values.milestones, 4, errors.milestones.as_deref()))
                (form::field_help("Syntaxe : libellé:parts_bps[:AAAA-MM-JJ] — ex. Acompte:3000:2026-03-31 (parts_bps en dix-millièmes, 10000 = 100%)"))
                (form::actions(if revision.is_some() { "Enregistrer" } else { "Créer" }))
            }
        }
    }
}

pub fn new_panel(values: &MissionFormValues, errors: &MissionFormErrors) -> Markup {
    panel::sheet(
        "Nouvelle mission",
        mission_form("/missions", None, true, values, errors),
    )
}

pub fn edit_panel(
    id: MissionId,
    revision: i64,
    values: &MissionFormValues,
    errors: &MissionFormErrors,
) -> Markup {
    panel::sheet(
        "Modifier la mission",
        mission_form(
            &format!("/missions/{id}"),
            Some(revision),
            false,
            values,
            errors,
        ),
    )
}

fn status_badge(m: &Mission) -> Markup {
    html! {
        @if m.archived_at.is_some() {
            span class="badge warn" { "archivée" }
        } @else if m.ended_on.is_some() {
            span class="badge info" { "clôturée" }
        } @else {
            span class="badge ok" { "active" }
        }
    }
}

fn kind_badge(kind: &MissionKind) -> Markup {
    html! {
        @match kind {
            MissionKind::Regie { .. } => span class="badge info" { "régie" },
            MissionKind::Forfait { .. } => span class="badge warn" { "forfait" },
            MissionKind::Recurrent { .. } => span class="badge ok" { "récurrent" },
        }
    }
}

fn schedule_markup(schedule: &BillingSchedule) -> Markup {
    html! {
        @match schedule {
            BillingSchedule::Regie { daily_rate } => div { "Régie — " (daily_rate) " / jour, facturé au fil de l'eau" },
            BillingSchedule::Recurrent { monthly_amount } => div { "Récurrent — " (monthly_amount) " / mois" },
            BillingSchedule::Forfait { installments } => {
                @for (milestone, amount) in installments {
                    div { (milestone.label) " — " (amount) @if let Some(d) = milestone.due_on { " (échéance " (format_date(d)) ")" } }
                }
            }
        }
    }
}

pub fn detail_panel(
    store: &Store,
    mission: &Mission,
    time_entries: &[TimeEntry],
    refs: MissionReferences,
    error: Option<&str>,
) -> Result<Markup, AppError> {
    let effective_rate = effective_daily_rate(store.connection(), mission.id)?;
    let closed = mission.ended_on.is_some();
    let total_days: f64 = time_entries.iter().map(|e| e.days).sum();
    let body = html! {
        @if let Some(msg) = error {
            (form::error_banner(msg))
        }
        div class="detail-head" {
            div class="detail-title" { (mission.name) }
            (status_badge(mission))
        }
        dl class="detail-fields" {
            dt { "client" } dd { (client_name(store, mission.client_id)) }
            dt { "type" } dd { (kind_badge(&mission.kind)) }
            dt { "démarrée" } dd class="mono" { (format_date(mission.started_on)) }
            @if let Some(d) = mission.ended_on {
                dt { "clôturée" } dd class="mono" { (format_date(d)) }
            }
            @if let Some(rate) = effective_rate {
                dt { "tjm effectif" } dd class="mono" { (rate) }
            }
        }

        div class="detail-section" {
            div class="detail-section-head" { span { "Échéancier de facturation" } }
            (schedule_markup(&missions::billing_schedule(mission)))
        }

        div class="detail-actions" {
            button class="btn" hx-get=(format!("/missions/{}/edit", mission.id)) hx-target="#panel" hx-swap="innerHTML" { "modifier" }
            @if closed {
                button class="btn" hx-post=(format!("/missions/{}/reopen", mission.id)) hx-target="#panel" hx-swap="innerHTML" { "rouvrir" }
            } @else {
                button class="btn" hx-get=(format!("/missions/{}/close", mission.id)) hx-target="#panel" hx-swap="innerHTML" { "clôturer" }
            }
            @if mission.archived_at.is_some() {
                button class="btn" hx-post=(format!("/missions/{}/unarchive", mission.id)) hx-target="#panel" hx-swap="innerHTML" { "réintégrer" }
            } @else {
                button class="btn" hx-post=(format!("/missions/{}/archive", mission.id)) hx-target="#panel" hx-swap="innerHTML" { "archiver" }
            }
            button class="btn danger" hx-get=(format!("/missions/{}/delete", mission.id)) hx-target="#panel" hx-swap="innerHTML" { "supprimer" }
        }

        @if !refs.is_empty() {
            div class="detail-note" {
                "Référencée par " (refs.invoices) " facture(s) et " (refs.time_entries) " saisie(s) de temps — "
                "la suppression sera refusée tant que ce total n'est pas nul."
            }
        }

        div class="detail-section" {
            div class="detail-section-head" {
                span { "Temps saisi (" (total_days) " j)" }
                button class="btn small" hx-get=(format!("/missions/{}/time/new", mission.id)) hx-target="#panel" hx-swap="innerHTML" { "+ temps" }
            }
            @if time_entries.is_empty() {
                div class="empty-state" { "aucune saisie de temps" }
            } @else {
                @for entry in time_entries {
                    div class="contact-row" {
                        div {
                            div class="contact-name" { (format_date(entry.worked_on)) " — " (entry.days) " j — " (entry.category.as_str()) }
                            div class="contact-meta" { (entry.note.clone().unwrap_or_else(|| "—".to_string())) }
                        }
                        div class="contact-actions" {
                            button class="btn small" hx-get=(format!("/time-entries/{}/edit", entry.id)) hx-target="#panel" hx-swap="innerHTML" { "modifier" }
                            button class="btn small danger" hx-post=(format!("/time-entries/{}/delete", entry.id)) hx-target="#panel" hx-swap="innerHTML" { "supprimer" }
                        }
                    }
                }
            }
        }
    };
    Ok(panel::sheet(&mission.name, body))
}

pub fn delete_confirm_panel(mission: &Mission) -> Markup {
    let body = html! {
        div class="detail-note" {
            "Supprimer définitivement « " (mission.name) " » ? Cette action est irréversible — "
            "si une facture ou une saisie de temps la référence, la suppression sera refusée."
        }
        div class="form-actions" {
            button class="btn danger" hx-post=(format!("/missions/{}/delete", mission.id)) hx-target="#panel" hx-swap="innerHTML" {
                "confirmer la suppression"
            }
        }
    };
    panel::sheet("Confirmer la suppression", body)
}

#[derive(Default)]
pub struct TransitionErrors {
    pub banner: Option<String>,
}

pub fn close_panel(mission: &Mission, errors: &TransitionErrors) -> Markup {
    let body = html! {
        form hx-post=(format!("/missions/{}/close", mission.id)) hx-target="#panel" hx-swap="innerHTML" {
            @if let Some(msg) = &errors.banner { (form::error_banner(msg)) }
            (form::date("ended_on", "Date de fin", "", None))
            (form::actions("Clôturer"))
        }
    };
    panel::sheet(&format!("Clôturer — {}", mission.name), body)
}

// -- Temps -----------------------------------------------------------------------------------

#[derive(Default, Clone)]
pub struct TimeEntryFormValues {
    pub worked_on: String,
    pub days: String,
    pub category: String,
    pub note: String,
}

impl From<&TimeEntry> for TimeEntryFormValues {
    fn from(e: &TimeEntry) -> Self {
        Self {
            worked_on: format_date(e.worked_on),
            days: e.days.to_string(),
            category: e.category.as_str().to_string(),
            note: e.note.clone().unwrap_or_default(),
        }
    }
}

#[derive(Default)]
pub struct TimeEntryFormErrors {
    pub days: Option<String>,
    pub banner: Option<String>,
    pub conflict: Option<(String, String)>,
}

const CATEGORY_OPTIONS: &[(&str, &str)] = &[
    ("billable", "Facturable"),
    ("pre_sales", "Avant-vente"),
    ("admin", "Administratif"),
    ("training", "Formation"),
];

fn time_entry_form(
    action: &str,
    revision: Option<i64>,
    values: &TimeEntryFormValues,
    errors: &TimeEntryFormErrors,
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
                (form::date("worked_on", "Date", &values.worked_on, None))
                (form::number("days", "Jours", &values.days, "0.25", errors.days.as_deref()))
                (form::select("category", "Catégorie", CATEGORY_OPTIONS, &values.category, None))
                (form::text("note", "Note (optionnel)", &values.note, None))
                (form::actions(if revision.is_some() { "Enregistrer" } else { "Ajouter" }))
            }
        }
    }
}

pub fn new_time_entry_panel(
    mission: &Mission,
    values: &TimeEntryFormValues,
    errors: &TimeEntryFormErrors,
) -> Markup {
    panel::sheet(
        &format!("Nouvelle saisie de temps — {}", mission.name),
        time_entry_form(
            &format!("/missions/{}/time", mission.id),
            None,
            values,
            errors,
        ),
    )
}

pub fn edit_time_entry_panel(
    entry: &TimeEntry,
    values: &TimeEntryFormValues,
    errors: &TimeEntryFormErrors,
) -> Markup {
    panel::sheet(
        "Modifier la saisie de temps",
        time_entry_form(
            &format!("/time-entries/{}", entry.id),
            Some(entry.revision),
            values,
            errors,
        ),
    )
}

// -- Liste -----------------------------------------------------------------------------------

fn toggle_url(ended: bool, archived: bool) -> String {
    format!("/missions/table?ended={ended}&archived={archived}")
}

pub fn list_fragment(store: &Store, filter: MissionFilter) -> Result<Markup, AppError> {
    let missions = list_missions_with(store.connection(), filter)?;

    Ok(html! {
        div id="missions-list"
            hx-get=(toggle_url(filter.include_ended, filter.include_archived))
            hx-trigger="freeflow:saved from:body"
            hx-target="this"
            hx-swap="outerHTML" {
            div class="pipe-toolbar" {
                button class="btn primary" hx-get="/missions/new" hx-target="#panel" hx-swap="innerHTML" { "+ nouvelle mission" }
                a class="filter-chip" href="#" hx-get=(toggle_url(!filter.include_ended, filter.include_archived)) hx-target="#missions-list" hx-swap="outerHTML" {
                    (if filter.include_ended { "masquer les clôturées" } else { "voir les clôturées" })
                }
                a class="filter-chip" href="#" hx-get=(toggle_url(filter.include_ended, !filter.include_archived)) hx-target="#missions-list" hx-swap="outerHTML" {
                    (if filter.include_archived { "masquer les archivées" } else { "voir les archivées" })
                }
            }
            @if missions.is_empty() {
                div class="empty-state" { "aucune mission — cliquez sur « nouvelle mission »" }
            } @else {
                @for mission in &missions {
                    @let name = client_name(store, mission.client_id);
                    @let effective = effective_daily_rate(store.connection(), mission.id).ok().flatten();
                    div class="mission-row row-clickable" hx-get=(format!("/missions/{}", mission.id)) hx-target="#panel" hx-swap="innerHTML" {
                        div class="mission-top" {
                            div {
                                div class="mission-id" { (mission.id.to_string()) }
                                div class="mission-client" { (name) }
                                div class="mission-name" { (mission.name) }
                            }
                            div { (kind_badge(&mission.kind)) (status_badge(mission)) }
                        }
                        div class="mission-metrics" {
                            @match &mission.kind {
                                MissionKind::Regie { daily_rate } => div class="mm" { div class="v" { (daily_rate) } div class="l" { "tjm" } },
                                MissionKind::Forfait { budget } => div class="mm" { div class="v" { (budget) } div class="l" { "budget" } },
                                MissionKind::Recurrent { monthly_amount } => div class="mm" { div class="v" { (monthly_amount) } div class="l" { "mensuel" } },
                            }
                            @if let Some(rate) = effective {
                                div class="mm" { div class="v" { (rate) } div class="l" { "tjm_eff" } }
                            } @else {
                                div class="mm" { div class="v" { "—" } div class="l" { "tjm_eff" } }
                            }
                        }
                    }
                }
            }
        }
    })
}

#[allow(dead_code)] // écran historique hors nav ; les tableaux et panneaux restent.
pub fn render(store: &Store, filter: MissionFilter) -> Result<Markup, AppError> {
    let count = list_missions_with(store.connection(), filter)?.len();
    Ok(html! {
        (view_head(ViewId::Missions, &format!("{count} mission(s)")))
        (list_fragment(store, filter)?)
    })
}

/// Mission + saisies de temps + références, pour [`detail_panel`]/[`delete_confirm_panel`].
pub fn load_detail(
    store: &Store,
    id: MissionId,
) -> Result<Option<(Mission, Vec<TimeEntry>, MissionReferences)>, AppError> {
    let Some(mission) = mission_by_id(store.connection(), id)? else {
        return Ok(None);
    };
    let time_entries = list_time_entries(store.connection(), id)?;
    let refs = mission_references(store.connection(), id)?;
    Ok(Some((mission, time_entries, refs)))
}

/// Parse la collection de jalons du textarea — un par ligne, lignes vides ignorées.
pub fn parse_milestones(text: &str) -> Result<Vec<Milestone>, String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| l.parse::<Milestone>().map_err(|e| e.to_string()))
        .collect()
}
