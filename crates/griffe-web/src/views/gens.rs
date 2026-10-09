//! Les affaires : une liste, un dossier. Les faits viennent de `griffe_core::people` ; cette vue
//! rédige le français.

use std::collections::HashSet;

use griffe_core::app::AppError;
use griffe_core::domain::{
    ClientId, ExpensePaidBy, FollowUpSubject, InteractionKind, display_phone, format_date,
    format_date_fr,
};
use griffe_core::dossier_work::DossierWork;
use griffe_core::follow_up::{
    FollowUpCard, card_for, follow_up_sender, prospect_genre_for, prospect_genres,
};
use griffe_core::mail::{LINK_HINT, OutboundStatus, OutboundView};
use griffe_core::people::{
    CurrentSituation, HistoryEvent, HistoryKind, MissionShape, OutgoingCadence, OutgoingChapter,
    OutgoingNote, Paper, PaperKind, PaperStatus, PeopleList, PersonAction, PersonChapter,
    PersonCue, PersonDossier, PersonFigure, PersonKey, PersonRow, people_list, person,
};
use griffe_core::store::Store;
use griffe_core::work_kinds::{WorkKind, dossiers_of_work_kind, work_kinds};
use maud::{Markup, PreEscaped, html};
use time::Date;

use crate::layout::ViewId;
use crate::markdown::render_work_markdown;
use crate::views::copy::letter_date;
use crate::views::form;

pub fn path_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => out.push(char::from(b)),
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[must_use]
pub fn person_href(name: &str) -> String {
    format!("/affaires/{}", path_encode(name))
}

pub fn render(store: &Store, today: Date) -> Result<Markup, AppError> {
    render_search(store, today, "", "")
}

pub fn render_search(
    store: &Store,
    today: Date,
    query: &str,
    type_id: &str,
) -> Result<Markup, AppError> {
    let list = people_list(store.connection(), today)?;
    let catalog = work_kinds(store.connection())?;
    let type_id = type_id.trim();
    let allowed = if type_id.is_empty() {
        None
    } else if catalog.iter().any(|kind| kind.id == type_id) {
        let ids = dossiers_of_work_kind(store.connection(), type_id)?
            .into_iter()
            .map(|dossier| dossier.client)
            .collect();
        Some(ids)
    } else {
        Some(HashSet::new())
    };
    Ok(list_markup(
        &list,
        today,
        None,
        query.trim(),
        &catalog,
        type_id,
        allowed.as_ref(),
    ))
}

pub fn list_markup(
    list: &PeopleList,
    today: Date,
    flash: Option<&str>,
    query: &str,
    catalog: &[WorkKind],
    type_id: &str,
    allowed: Option<&HashSet<ClientId>>,
) -> Markup {
    let empty = list.is_empty();
    let conversations = rows_kept(&list.conversations, allowed);
    let messages = rows_kept(&list.first_messages, allowed);
    let contacts = rows_kept(&list.first_contacts, allowed);
    let missions = rows_kept(&list.missions, allowed);
    let filtering = allowed.is_some();
    html! {
        div class="letter" data-view=(ViewId::Gens.slug()) {
            div class="date" { "Les affaires · " (letter_date(today)) }
            @if empty {
                h1 { "Les affaires." }
                p class="lede" { "Personne pour l'instant. Une conversation commence par un nom et une phrase." }
            } @else {
                h1 { (list_title(list)) }
                p class="lede" { (list_lede(list)) }
            }
            @if let Some(msg) = flash {
                p class="mast-note" role="status" { (msg) }
            }
            div class="letter-actions" {
                a class="seal" href="/affaires/nouvelle"
                  hx-get="/affaires/nouvelle" hx-target="#content" hx-push-url="true" {
                    "Nouvelle conversation"
                }
                a class="aside" href="/affaires/types"
                  hx-get="/affaires/types" hx-target="#content" hx-push-url="true" {
                    "Les types"
                }
            }
            (kind_row(catalog, query, type_id))
            (conversation_chapter(&conversations, &messages, &contacts, query, type_id, filtering))
            (chapter(
                "En mission",
                &missions,
                if filtering { "Aucune mission de ce type." } else { "Aucune mission en cours." },
            ))
            (outgoing_chapter(&list.outgoing))
            (stopped_chapter(&list.stopped))
        }
    }
}

fn rows_kept<'a>(rows: &'a [PersonRow], allowed: Option<&HashSet<ClientId>>) -> Vec<&'a PersonRow> {
    rows.iter()
        .filter(|row| match allowed {
            None => true,
            Some(ids) => row.client_id.is_some_and(|id| ids.contains(&id)),
        })
        .collect()
}

fn affaires_href(query: &str, type_id: &str) -> String {
    let mut parts = Vec::new();
    if !query.is_empty() {
        parts.push(format!("q={}", path_encode(query)));
    }
    if !type_id.is_empty() {
        parts.push(format!("type={}", path_encode(type_id)));
    }
    if parts.is_empty() {
        "/affaires".to_string()
    } else {
        format!("/affaires?{}", parts.join("&"))
    }
}

fn kind_row(catalog: &[WorkKind], query: &str, active: &str) -> Markup {
    if catalog.is_empty() {
        return html! {};
    }
    html! {
        div class="word-choice" {
            div class="word-choice-row" {
                @for kind in catalog {
                    @let on = kind.id == active;
                    @let href = affaires_href(query, if on { "" } else { kind.id.as_str() });
                    label {
                        input type="checkbox" checked[on] disabled tabindex="-1" aria-hidden="true";
                        a href=(href) hx-get=(href) hx-target="#content" hx-push-url="true" {
                            (kind.name)
                        }
                    }
                }
            }
        }
    }
}

fn outgoing_chapter(rows: &[PersonRow]) -> Markup {
    html! {
        p class="section-label" { "Chez qui ça sort" }
        @if rows.is_empty() {
            p class="empty-state" {
                "Un nom apparaît ici quand une dépense le porte. En rangeant un débit du relevé, ou en notant une sortie."
            }
            div class="row-actions" {
                button class="quiet" type="button"
                    hx-get="/depenses/new" hx-target="#panel" hx-swap="innerHTML" {
                    "Noter une sortie"
                }
            }
        } @else {
            ul class="people" {
                @for row in rows {
                    li { (row_link(row)) }
                }
            }
        }
    }
}

fn stopped_chapter(rows: &[PersonRow]) -> Markup {
    if rows.is_empty() {
        return html! {};
    }
    html! {
        details class="stopped" {
            summary class="section-label" {
                "Arrêtées · " (rows.len())
                span class="stopped-hint" { " · voir" }
            }
            ul class="people" {
                @for row in rows {
                    li { (row_link(row)) }
                }
            }
        }
    }
}

fn list_title(list: &PeopleList) -> String {
    let n = list.open_conversations() + list.missions.len();
    match n {
        0 => "Les affaires.".into(),
        1 => "Un nom.".into(),
        2 => "Deux noms.".into(),
        k => format!("{k} noms."),
    }
}

fn list_lede(list: &PeopleList) -> String {
    match list.open_conversations() {
        0 => "Avec qui j'en suis. Un nom, pas un type de document.".into(),
        1 => {
            let name = sole_open(list).map_or("ce nom", |row| row.name.as_str());
            format!("Derrière {name}, personne. C'est le trou — pas un graphique.")
        }
        _ => "Avec qui j'en suis. Un nom, pas un type de document.".into(),
    }
}

fn sole_open(list: &PeopleList) -> Option<&PersonRow> {
    list.conversations
        .first()
        .or(list.first_messages.first())
        .or(list.first_contacts.first())
}

fn conversation_chapter(
    conversations: &[&PersonRow],
    messages: &[&PersonRow],
    contacts: &[&PersonRow],
    query: &str,
    type_id: &str,
    filtering: bool,
) -> Markup {
    let pile = !messages.is_empty() || !contacts.is_empty();
    let empty = if filtering {
        "Aucune conversation de ce type."
    } else {
        "Aucune conversation ouverte."
    };
    html! {
        p class="section-label" { "En conversation" }
        @if conversations.is_empty() && !pile {
            p class="empty-state" { (empty) }
        } @else if !conversations.is_empty() {
            (people_items(conversations))
        }
        (pile_chapter(messages, query, type_id, "premier message", "premiers messages", "q-messages"))
        (pile_chapter(contacts, query, type_id, "premier contact", "premiers contacts", "q-contacts"))
    }
}

fn people_items(rows: &[&PersonRow]) -> Markup {
    html! {
        ul class="people" {
            @for row in rows {
                li { (row_link(row)) }
            }
        }
    }
}

fn pile_chapter(
    rows: &[&PersonRow],
    query: &str,
    type_id: &str,
    one: &str,
    many: &str,
    field_id: &str,
) -> Markup {
    if rows.is_empty() {
        return html! {};
    }
    let shown: Vec<&PersonRow> = rows
        .iter()
        .copied()
        .filter(|row| name_matches(row, query))
        .collect();
    let title = if rows.len() == 1 {
        format!("1 {one}")
    } else {
        format!("{} {many}", rows.len())
    };
    html! {
        details class="pile" open[!query.is_empty()] {
            summary class="section-label" {
                (title)
                span class="stopped-hint" { " · voir" }
            }
            form class="pile-find" action="/affaires" method="get"
                hx-get="/affaires" hx-target="#content" hx-push-url="true" {
                input id=(field_id) type="search" name="q" value=(query)
                    placeholder="Un nom" aria-label="Un nom";
                @if !type_id.is_empty() {
                    input type="hidden" name="type" value=(type_id);
                }
            }
            @if shown.is_empty() {
                p class="empty-state" { "Aucun nom." }
            } @else {
                (people_items(&shown))
            }
        }
    }
}

fn name_matches(row: &PersonRow, query: &str) -> bool {
    let query = query.trim();
    if query.is_empty() {
        return true;
    }
    let needle = fold_name(query);
    fold_name(&row.name).contains(&needle)
        || fold_name(&row.party).contains(&needle)
        || row
            .contact_name
            .as_deref()
            .is_some_and(|contact| fold_name(contact).contains(&needle))
}

fn fold_name(value: &str) -> String {
    value
        .to_lowercase()
        .chars()
        .map(|c| match c {
            'à' | 'â' | 'ä' | 'á' | 'ã' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'î' | 'ï' | 'í' | 'ì' => 'i',
            'ô' | 'ö' | 'ó' | 'ò' | 'õ' => 'o',
            'ù' | 'û' | 'ü' | 'ú' => 'u',
            'ç' => 'c',
            'ñ' => 'n',
            other => other,
        })
        .collect()
}

fn chapter(label: &str, rows: &[&PersonRow], empty: &str) -> Markup {
    html! {
        p class="section-label" { (label) }
        @if rows.is_empty() {
            p class="empty-state" { (empty) }
        } @else {
            (people_items(rows))
        }
    }
}

fn row_link(row: &PersonRow) -> Markup {
    let href = person_href(&row.name);
    html! {
        a href=(href) hx-get=(href) hx-target="#content" hx-push-url="true" {
            div {
                div class="nm" { (row.name) }
                @if !row.work_kinds.is_empty() {
                    p class="kinds" { (row.work_kinds.join(" · ")) }
                }
                div class="st" { (cues_fr(&row.cues)) }
            }
            @if let Some(fig) = &row.figure {
                span class="amt" { (figure_fr(fig)) }
            }
        }
    }
}

fn figure_fr(figure: &PersonFigure) -> String {
    match figure {
        PersonFigure::Money { amount } => amount.to_string(),
        PersonFigure::Around { amount } => format!("autour de {amount}"),
        PersonFigure::Spent { amount } => format!("{amount} versés"),
        PersonFigure::Days { days } => {
            if (days.fract()).abs() < 0.05 {
                format!("{} j", *days as i64)
            } else {
                format!("{days} j")
            }
        }
    }
}

fn cues_fr(cues: &[PersonCue]) -> String {
    if cues.is_empty() {
        return String::new();
    }
    cues.iter().map(cue_fr).collect::<Vec<_>>().join(" · ")
}

fn loss_fr(reason: Option<&griffe_core::domain::LossReason>) -> String {
    use griffe_core::domain::LossReason;
    let why = match reason {
        Some(LossReason::Budget) => "le budget ne suivait pas",
        Some(LossReason::Timing) => "pas le bon moment",
        Some(LossReason::Competitor) => "quelqu'un d'autre a été choisi",
        Some(LossReason::NoResponse) => "pas de réponse",
        Some(LossReason::ScopeMismatch) => "ce n'était pas le bon sujet",
        Some(LossReason::Other(text)) => text.as_str(),
        None => "sans motif",
    };
    format!("arrêtée · {why}")
}

fn cue_fr(cue: &PersonCue) -> String {
    match cue {
        PersonCue::QuoteSent { .. } => "estimation envoyée".into(),
        PersonCue::FollowUpDue { today: true, .. } => "à reprendre aujourd'hui".into(),
        PersonCue::FollowUpDue { on, .. } => format!("prochain pas le {}", format_date_fr(*on)),
        PersonCue::FirstMessage => "premier message à écrire".into(),
        PersonCue::FirstContact => "premier contact".into(),
        PersonCue::InExchange => "en échange".into(),
        PersonCue::EstimateNoted => "estimation posée".into(),
        PersonCue::DraftReady => "brouillon prêt".into(),
        PersonCue::Resumed => "repris".into(),
        PersonCue::Lost { reason } => loss_fr(reason.as_ref()),
        PersonCue::InvoiceOverdue { days } => format!("facture en retard · {days} jours"),
        PersonCue::InvoiceOutstanding => "facture à encaisser".into(),
        PersonCue::NextMilestone { on, label } => {
            format!("{label} · prochain jalon le {}", format_date_fr(*on))
        }
        PersonCue::OpeningDebt => "dette reprise au bilan".into(),
        PersonCue::MatchingDebit => "un débit correspond".into(),
        PersonCue::CadenceMonthly => "tous les mois".into(),
        PersonCue::LastNote { on } => format!("dernière note en {}", month_name(*on)),
        PersonCue::QuietSince { on } => format!("plus rien depuis {}", month_name(*on)),
    }
}

pub fn dossier_page(store: &Store, needle: &str, today: Date) -> Result<Markup, AppError> {
    let dossier = person(store.connection(), needle, today)?;
    Ok(dossier_markup(&dossier, today, None))
}

/// Saisie des types sur le dossier. `naming` affiche « Nomme le type. » sans écrire.
pub struct KindFormState {
    pub selected: Vec<String>,
    pub other_open: bool,
    pub other: String,
    pub naming: bool,
}

pub fn dossier_markup(dossier: &PersonDossier, today: Date, flash: Option<&str>) -> Markup {
    dossier_body(dossier, today, flash, None)
}

pub fn dossier_with_kinds(
    dossier: &PersonDossier,
    today: Date,
    flash: Option<&str>,
    kinds: &KindFormState,
) -> Markup {
    dossier_body(dossier, today, flash, Some(kinds))
}

fn kinds_form(dossier: &PersonDossier, href: &str, posted: Option<&KindFormState>) -> Markup {
    let selected = posted.map_or(dossier.work_kinds.as_slice(), |state| {
        state.selected.as_slice()
    });
    let other_open = posted.is_some_and(|state| state.other_open);
    let other = posted.map_or("", |state| state.other.as_str());
    let naming = posted.is_some_and(|state| state.naming);
    let action = format!("{href}/types");
    html! {
        form hx-post=(action) hx-target="#content" hx-trigger="change, submit" {
            fieldset class="word-choice" {
                legend { "Types" }
                div class="word-choice-row" {
                    @for name in &dossier.work_kind_catalog {
                        label {
                            input type="checkbox" name="kind" value=(name)
                                checked[selected.iter().any(|item| item == name)];
                            (name)
                        }
                    }
                    label {
                        input class="word-other-toggle" type="checkbox" name="kind_other_on" value="1"
                            checked[other_open];
                        "un autre"
                    }
                }
                input class="word-other" name="kind_other" type="text" value=(other)
                    aria-label="Nom du type" aria-invalid[naming];
            }
            @if naming {
                div class="field-error" { "Nomme le type." }
            }
        }
    }
}

fn dossier_body(
    dossier: &PersonDossier,
    today: Date,
    flash: Option<&str>,
    posted: Option<&KindFormState>,
) -> Markup {
    let href = person_href(&dossier.name);
    let client_fiche = matches!(dossier.key, PersonKey::Client { .. });
    let (daily, fate): (Vec<_>, Vec<_>) = dossier
        .actions
        .iter()
        .partition(|action| !matches!(action, PersonAction::Stop { .. }));
    let has_snooze = daily
        .iter()
        .any(|action| matches!(action, PersonAction::Snooze { .. }));
    html! {
        div class="letter" data-view=(ViewId::Gens.slug()) data-person=(dossier.name) {
            a class="back" href="/affaires" hx-get="/affaires" hx-target="#content" hx-push-url="true" {
                "← Les affaires"
            }
            div class="who" { (dossier.name) }
            p class="co" { (subtitle(dossier, today)) }
            @if client_fiche {
                (kinds_form(dossier, &href, posted))
            }
            @if let Some(msg) = flash {
                p class="mast-note" role="status" { (msg) }
            }
            @if client_fiche || !daily.is_empty() {
                div class="row-actions" {
                    @for action in &daily {
                        @if client_fiche && matches!(action, PersonAction::Snooze { .. }) {
                            (travaux_link(&href))
                        }
                        (action_button(action, &href))
                    }
                    @if client_fiche && !has_snooze {
                        (travaux_link(&href))
                    }
                }
            }
            @for action in &fate {
                (action_button(action, &href))
            }
            @if let Some(body) = current_paragraph(&dossier.current, dossier) {
                div class="block" {
                    h3 { "En cours" }
                    p { (body) }
                    @if !dossier.work.is_empty() {
                        ul class="hist" {
                            @for line in &dossier.work {
                                li {
                                    span class="when" { (line.amount) }
                                    span { (line.label) }
                                }
                            }
                        }
                    }
                }
            }
            @if let Some(outgoing) = &dossier.outgoing {
                div class="block" {
                    h3 { "Les notes" }
                    ul class="hist notes" {
                        @for note in &outgoing.notes {
                            (note_item(note, &href))
                        }
                    }
                }
            }
            @if let Some(project) = &dossier.project {
                div class="block" {
                    h3 { "Le projet" }
                    p { (project_paragraph(project, dossier)) }
                }
            }
            @if show_papers_block(dossier) {
                div class="block" {
                    h3 { "Les papiers" }
                    @if !dossier.papers.is_empty() {
                        ul class="hist" {
                            @for paper in &dossier.papers {
                                li {
                                    span class="when" { (short_date(paper.on)) }
                                    span { (paper_line(paper)) }
                                    (paper_write_off_form(paper, &href, today))
                                }
                            }
                        }
                    }
                    @if can_import_invoice(dossier) {
                        (import_invoice_form(dossier, today))
                    }
                }
            }
            @if !dossier.history.is_empty() {
                div class="block" {
                    h3 { "Histoire" }
                    ul class="hist" {
                        @for event in &dossier.history {
                            (history_item(event))
                        }
                    }
                }
            }
        }
    }
}

fn subtitle(d: &PersonDossier, today: Date) -> String {
    if d.chapter == PersonChapter::Stopped {
        return loss_fr(d.loss_reason.as_ref());
    }
    if let Some(outgoing) = &d.outgoing {
        return outgoing_subtitle(outgoing, today);
    }
    let mut parts = Vec::new();
    if let Some(contact) = d
        .contact_name
        .as_ref()
        .filter(|contact| fold_name(contact) != fold_name(&d.party))
    {
        parts.push(contact.clone());
    }
    if let Some(on) = d.since {
        parts.push(format!("depuis {}", month_year(on)));
    }
    if d.not_yet_client {
        parts.push("pas encore cliente".into());
    } else if let Some(project) = &d.project {
        parts.push(shape_fr(project.shape).into());
        if project.days_this_month > 0.0 {
            parts.push(format!(
                "{} jours ce mois-ci",
                as_days(project.days_this_month)
            ));
        }
    }
    parts.join(" · ")
}

fn outgoing_subtitle(outgoing: &OutgoingChapter, today: Date) -> String {
    let mut parts = Vec::new();
    if outgoing.opening_debt {
        parts.push("depuis la reprise".into());
        parts.push("une dette encore ouverte".into());
        return parts.join(" · ");
    }
    parts.push(format!("depuis {}", month_year(outgoing.since)));
    match outgoing.cadence {
        OutgoingCadence::Monthly => parts.push("tous les mois".into()),
        OutgoingCadence::Once | OutgoingCadence::Occasional => {
            if (today - outgoing.last_on).whole_days() > 90 {
                parts.push(format!("plus rien depuis {}", month_name(outgoing.last_on)));
            }
        }
    }
    parts.join(" · ")
}

fn month_name(on: Date) -> &'static str {
    const MONTHS: [&str; 12] = [
        "janvier",
        "février",
        "mars",
        "avril",
        "mai",
        "juin",
        "juillet",
        "août",
        "septembre",
        "octobre",
        "novembre",
        "décembre",
    ];
    MONTHS
        .get(usize::from(u8::from(on.month()).saturating_sub(1)))
        .copied()
        .unwrap_or("")
}

fn month_year(on: Date) -> String {
    format!("{} {}", month_name(on), on.year())
}

fn as_days(days: f64) -> String {
    if (days.fract()).abs() < 0.05 {
        format!("{}", days as i64)
    } else {
        format!("{days:.1}")
    }
}

fn shape_fr(shape: MissionShape) -> &'static str {
    match shape {
        MissionShape::Regie => "régie",
        MissionShape::Forfait => "forfait",
        MissionShape::Recurrent => "récurrent",
    }
}

fn current_paragraph(current: &CurrentSituation, dossier: &PersonDossier) -> Option<String> {
    if let Some(outgoing) = &dossier.outgoing {
        return Some(outgoing_paragraph(outgoing, current));
    }
    let mut sentences = Vec::new();
    if let Some(quote) = &current.quote {
        let name = current
            .opportunity_name
            .as_deref()
            .unwrap_or("accompagnement");
        sentences.push(format!(
            "Estimation {name}, {} HT, posée le {}.",
            quote.amount,
            format_date_fr(quote.on)
        ));
    } else if let Some(invoice) = &current.invoice {
        let days = match invoice.status {
            PaperStatus::InvoiceOutstanding { days_overdue } if days_overdue > 0 => {
                format!(" {days_overdue} jours.")
            }
            _ => String::new(),
        };
        sentences.push(format!(
            "Une facture de {}, échue le {}.{}",
            invoice.amount,
            format_date_fr(invoice.on),
            days
        ));
    } else if let Some(name) = &current.opportunity_name {
        let amount = current
            .amount
            .filter(|a| a.cents() != 0)
            .map(|a| format!(", autour de {a}"))
            .unwrap_or_default();
        sentences.push(format!("{name}{amount}."));
    }
    if let Some(PersonCue::FollowUpDue { today: true, .. }) = &current.follow_up {
        sentences.push("À reprendre aujourd'hui.".into());
    }
    if current.nothing_scheduled && sentences.is_empty() {
        sentences.push("Rien n'est encore écrit.".into());
    }
    if sentences.is_empty() {
        None
    } else {
        Some(sentences.join(" "))
    }
}

fn outgoing_paragraph(outgoing: &OutgoingChapter, current: &CurrentSituation) -> String {
    let mut parts = Vec::new();
    if let Some(owed) = outgoing.owed {
        if current.opening_debt {
            parts.push(format!("{owed} repris en dette."));
        } else {
            parts.push(format!("{owed} encore à ranger."));
        }
    } else {
        let mut sentence = format!(
            "{} versés depuis {}.",
            outgoing.total_paid,
            month_year(outgoing.since)
        );
        if matches!(outgoing.cadence, OutgoingCadence::Monthly) {
            sentence = format!(
                "{} versés depuis {}, tous les mois.",
                outgoing.total_paid,
                month_year(outgoing.since)
            );
        }
        parts.push(sentence);
        parts.push("Rien à ranger.".into());
        parts.push(format!(
            "Dernière note le {}.",
            format_date_fr(outgoing.last_on)
        ));
    }
    if current.matching_debit {
        parts.push("Un débit du relevé correspond au centime. Le ranger comme règlement — pas comme une nouvelle charge.".into());
    }
    parts.join(" ")
}

fn note_item(note: &OutgoingNote, dossier_href: &str) -> Markup {
    let paid = match note.paid_by {
        ExpensePaidBy::Associate => " · avancé par toi",
        ExpensePaidBy::Company => "",
    };
    let phrase = format!("{} · {}{paid}", note.label, note.amount);
    let receipt_href = format!("/depenses/{}/receipt", note.expense_id);
    let join_href = format!("{dossier_href}/notes/{}/receipt", note.expense_id);
    html! {
        li {
            span class="when" { (short_date(note.on)) }
            div {
                span { (phrase) }
                @if note.receipt_filename.is_some() {
                    details class="letter-fold" {
                        summary { "Ouvrir" }
                        @if is_image_receipt(note.receipt_filename.as_deref()) {
                            img class="note-receipt" src=(receipt_href) alt=(note.label);
                        } @else {
                            object class="note-receipt" data=(receipt_href) type="application/pdf" {
                                a href=(receipt_href) target="_blank" { "Ouvrir le papier" }
                            }
                        }
                    }
                } @else {
                    p class="mast-note" { "pas de justificatif." }
                    form class="note-join" hx-encoding="multipart/form-data"
                         hx-post=(join_href) hx-target="#content" {
                        input type="hidden" name="revision" value=(note.revision);
                        (form::file("receipt", "Joindre", ".pdf,.png,.jpg,.jpeg,.webp"));
                        button class="quiet" type="submit" { "Joindre" }
                    }
                }
            }
        }
    }
}

fn is_image_receipt(filename: Option<&str>) -> bool {
    filename
        .and_then(|n| n.rsplit('.').next())
        .is_some_and(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "webp"
            )
        })
}

fn project_paragraph(
    project: &griffe_core::people::ProjectChapter,
    _dossier: &PersonDossier,
) -> String {
    let mut s = format!(
        "Mission {}, ouverte le {}.",
        shape_fr(project.shape),
        format_date_fr(project.started_on)
    );
    if project.days_this_month > 0.0 {
        s.push_str(&format!(
            " {} jours ce mois-ci.",
            as_days(project.days_this_month)
        ));
    }
    if let Some(ms) = &project.next_milestone
        && let Some(on) = ms.due_on
    {
        s.push_str(&format!(
            " Prochain jalon le {}, {}.",
            format_date_fr(on),
            ms.label
        ));
    }
    if let Some(end) = project.ended_on {
        s.push_str(&format!(" Fin le {}.", format_date_fr(end)));
    }
    s
}

fn short_date(on: Date) -> String {
    format!(
        "{} {}",
        on.day(),
        crate::views::copy::month_fr(u8::from(on.month()))
    )
}

fn show_papers_block(dossier: &PersonDossier) -> bool {
    !dossier.papers.is_empty() || can_import_invoice(dossier)
}

fn can_import_invoice(dossier: &PersonDossier) -> bool {
    !dossier.not_yet_client && matches!(dossier.key, PersonKey::Client { .. })
}

const VAT_RATE_OPTIONS: [(&str, &str); 5] = [
    ("standard", "20 %"),
    ("intermediate", "10 %"),
    ("reduced", "5,5 %"),
    ("super_reduced", "2,1 %"),
    ("zero", "0 %"),
];

fn import_invoice_form(dossier: &PersonDossier, today: Date) -> Markup {
    let href = format!("{}/facture", person_href(&dossier.name));
    let issued = format_date(today);
    let mission_options: Vec<(String, String)> = dossier
        .project
        .as_ref()
        .map(|project| (project.mission_id.to_string(), project.name.clone()))
        .into_iter()
        .collect();
    let credit_options: Vec<(String, String)> = dossier
        .papers
        .iter()
        .filter(|paper| paper.kind == PaperKind::Invoice)
        .filter_map(|paper| {
            let id = paper.invoice_id?;
            let number = paper.number.as_deref().filter(|n| !n.is_empty())?;
            Some((id.to_string(), number.to_string()))
        })
        .collect();
    html! {
        div class="import-invoice" {
            p class="section-label" { "Poser une facture née ailleurs" }
            p class="lede" {
                "Le numéro est celui de Tiime, Indy ou de votre plateforme. Griffe n'en invente pas un second."
            }
            p class="lede" {
                "Un avoir doit porter une quantité négative."
            }
            form class="note-join" hx-encoding="multipart/form-data"
                 hx-post=(href) hx-target="#content" {
                (form::file("file", "PDF", ".pdf,application/pdf"))
                (form::text("number", "Numéro", "", None))
                (form::date("issued_on", "Date", &issued, None))
                (form::number("payment_terms_days", "Délai (jours)", "30", "1", None))
                (form::text("description", "Description", "", None))
                (form::number("quantity", "Quantité", "1", "0.01", None))
                (form::text("unit_price", "Prix HT", "", None))
                (form::select("vat_rate", "Taux", &VAT_RATE_OPTIONS, "standard", None))
                div class="field" {
                    label for="mission" { "Mission" }
                    select id="mission" name="mission" {
                        option value="" { "aucune" }
                        @for (value, label) in &mission_options {
                            option value=(value) { (label) }
                        }
                    }
                }
                div class="field" {
                    label for="credits" { "Avoir pour" }
                    select id="credits" name="credits" {
                        option value="" { "une vente" }
                        @for (value, label) in &credit_options {
                            option value=(value) { (label) }
                        }
                    }
                }
                div class="row-actions" {
                    button class="seal" type="submit" { "Coller au dossier" }
                }
            }
        }
    }
}

fn paper_write_off_form(paper: &Paper, dossier_href: &str, today: Date) -> Markup {
    let Some(invoice_id) = paper.invoice_id else {
        return html! {};
    };
    let on = format_date(today);
    match &paper.status {
        PaperStatus::InvoiceOutstanding { .. } => {
            let action = format!("{dossier_href}/facture/{invoice_id}/ne-plus-attendre");
            html! {
                form class="note-join" hx-post=(action) hx-target="#content" {
                    (form::date("on", "Date", &on, None))
                    div class="row-actions" {
                        button class="quiet" type="submit" { "Je n'attends plus cet argent" }
                    }
                }
            }
        }
        PaperStatus::InvoiceWrittenOff => {
            let Some(write_off_id) = paper.write_off_id else {
                return html! {};
            };
            let action = format!("{dossier_href}/facture/{invoice_id}/j-attends-encore");
            html! {
                form class="note-join" hx-post=(action) hx-target="#content" {
                    input type="hidden" name="write_off_id" value=(write_off_id.to_string());
                    (form::date("on", "Date", &on, None))
                    div class="row-actions" {
                        button class="quiet" type="submit" { "Finalement j'attends encore" }
                    }
                }
            }
        }
        _ => html! {},
    }
}

fn paper_line(paper: &Paper) -> String {
    let kind = match paper.kind {
        PaperKind::Quote => "Estimation",
        PaperKind::Invoice => "Facture",
    };
    let number = paper.number.as_deref().unwrap_or("");
    let status = match &paper.status {
        PaperStatus::QuoteDraft => "brouillon",
        PaperStatus::QuoteSent => "en attente",
        PaperStatus::QuoteAccepted => "accepté",
        PaperStatus::QuoteDeclined => "décliné",
        PaperStatus::QuoteExpired => "expiré",
        PaperStatus::InvoiceOutstanding { days_overdue } if *days_overdue > 0 => {
            "échue · à relancer"
        }
        PaperStatus::InvoiceOutstanding { .. } => "à encaisser",
        PaperStatus::InvoicePaid => "encaissée",
        PaperStatus::InvoiceCredited => "annulée par avoir",
        PaperStatus::InvoiceWrittenOff => "on ne l'attend plus",
    };
    if number.is_empty() {
        format!("{kind} · {} · {status}", paper.amount)
    } else {
        format!("{kind} {number} · {} · {status}", paper.amount)
    }
}

fn history_item(event: &HistoryEvent) -> Markup {
    html! {
        li {
            time { (format_date_fr(event.on)) }
            (history_body(event))
        }
    }
}

fn nature_fr(kind: InteractionKind) -> &'static str {
    match kind {
        InteractionKind::Call => "Téléphone",
        InteractionKind::Email => "E-mail",
        InteractionKind::Meeting => "Rencontre physique",
        InteractionKind::Visio => "Visioconférence",
        InteractionKind::Note => "Note",
    }
}

/// Au-delà, l'historique ne montre qu'un aperçu : le texte entier s'ouvre au clic.
fn note_is_long(note: &str) -> bool {
    note.contains('\n') || note.chars().count() > 80
}

fn note_preview(note: &str) -> String {
    let line = note.lines().next().unwrap_or("").trim();
    let mut preview: String = line.chars().take(80).collect();
    if line.chars().count() > 80 || note.lines().nth(1).is_some() {
        preview.push('…');
    }
    preview
}

fn fold(summary: Markup, body: &str) -> Markup {
    html! {
        details class="letter-fold" {
            summary {
                (summary)
                span class="fold-mark" aria-hidden="true" {}
            }
            pre { (body) }
        }
    }
}

fn history_body(event: &HistoryEvent) -> Markup {
    match &event.kind {
        HistoryKind::Letter { subject, body } => {
            let title = if subject.is_empty() {
                "Lettre".to_string()
            } else {
                subject.clone()
            };
            fold(html! { span class="preview" { (title) } }, body)
        }
        HistoryKind::Interaction { interaction } => {
            let nature = nature_fr(*interaction);
            match event.note.as_deref().filter(|n| !n.is_empty()) {
                Some(note) if note_is_long(note) => fold(
                    html! {
                        span class="nature" { (nature) }
                        span class="preview" { (note_preview(note)) }
                    },
                    note,
                ),
                Some(note) => html! {
                    span {
                        span class="nature" { (nature) }
                        " "
                        (note)
                    }
                },
                None => html! { span class="nature" { (nature) } },
            }
        }
        HistoryKind::QuoteSent { .. } => html! { span { "Estimation envoyée." } },
        HistoryKind::QuoteAccepted => html! { span { "Estimation acceptée." } },
        HistoryKind::InvoiceIssued { number } => html! { span { "Facture " (number) "." } },
    }
}

fn snooze_preset(action: &str, days: u16, label: &str) -> Markup {
    html! {
        form method="post" action=(action) hx-post=(action) hx-target="#content" {
            input type="hidden" name="days" value=(days);
            button class="quiet" type="submit" { (label) }
        }
    }
}

fn snooze_control(dossier_href: &str) -> Markup {
    let action = format!("{dossier_href}/reporter");
    html! {
        details class="snooze" {
            summary class="quiet" { "Reporter" }
            div class="snooze-line" {
                (snooze_preset(&action, 3, "3 jours"))
                (snooze_preset(&action, 10, "10 jours"))
                form class="snooze-count" method="post" action=(action)
                  hx-post=(action) hx-target="#content" {
                    input name="days" type="text" inputmode="numeric" autocomplete="off"
                      aria-label="Nombre de jours";
                    button type="submit" { "jours" }
                }
            }
        }
    }
}

fn travaux_link(dossier_href: &str) -> Markup {
    let href = format!("{dossier_href}/travaux");
    html! {
        a class="quiet" href=(href)
          hx-get=(href) hx-target="#content" hx-push-url="true" {
            "Les travaux"
        }
    }
}

fn action_button(action: &PersonAction, dossier_href: &str) -> Markup {
    match action {
        PersonAction::Write { subject } => {
            let (label, class) = match subject {
                FollowUpSubject::Invoice(_) => ("Relancer", "seal"),
                FollowUpSubject::Opportunity(_) => ("Écrire", "seal"),
            };
            let href = format!("{dossier_href}/ecrire");
            html! {
                a class=(class) href=(href)
                  hx-get=(href) hx-target="#content" hx-push-url="true" {
                    (label)
                }
            }
        }
        PersonAction::Fiche { .. } => {
            let href = format!("{dossier_href}/fiche");
            html! {
                a class="quiet" href=(href)
                  hx-get=(href) hx-target="#content" hx-push-url="true" {
                    "La fiche"
                }
            }
        }
        PersonAction::Quote { .. } => {
            let href = format!("{dossier_href}/estimation");
            html! {
                a class="quiet" href=(href)
                  hx-get=(href) hx-target="#content" hx-push-url="true" {
                    "L'estimation"
                }
            }
        }
        PersonAction::LogMeeting { .. } => html! {
            a class="quiet" href=(format!("{dossier_href}/rencontre"))
              hx-get=(format!("{dossier_href}/rencontre")) hx-target="#content" hx-push-url="true" {
                "Noter une rencontre"
            }
        },
        PersonAction::Snooze { .. } => snooze_control(dossier_href),
        PersonAction::FileStatement => html! {
            a class="seal" href=(ViewId::Depenses.path())
              hx-get=(ViewId::Depenses.path()) hx-target="#content" hx-push-url="true" {
                "Ranger le mouvement"
            }
        },
        PersonAction::Stop { .. } => {
            let href = format!("{dossier_href}/arreter");
            html! {
                a class="fate" href=(href)
                  hx-get=(href) hx-target="#content" hx-push-url="true" {
                    "Cette conversation s'arrête"
                }
            }
        }
        PersonAction::Win { .. } => {
            let href = format!("{dossier_href}/client");
            html! {
                a class="quiet" href=(href)
                  hx-get=(href) hx-target="#content" hx-push-url="true" {
                    "C'est un client"
                }
            }
        }
        PersonAction::Reopen { .. } => {
            let href = format!("{dossier_href}/reprise");
            html! {
                a class="seal" href=(href)
                  hx-get=(href) hx-target="#content" hx-push-url="true" {
                    "Ils reviennent"
                }
            }
        }
    }
}

const LOSS_REASONS: &[(&str, &str)] = &[
    ("", "Choisir"),
    ("budget", "Le budget ne suit pas"),
    ("timing", "Pas le bon moment"),
    ("competitor", "Quelqu'un d'autre a été choisi"),
    ("no-response", "Pas de réponse"),
    ("scope-mismatch", "Ce n'est pas le bon sujet"),
    ("other", "Autre motif"),
];

pub fn stop_page(
    dossier: &PersonDossier,
    reason: &str,
    detail: &str,
    error: Option<&str>,
) -> Markup {
    let href = person_href(&dossier.name);
    html! {
        div class="letter" data-view=(ViewId::Gens.slug()) {
            a class="back" href=(href) hx-get=(href) hx-target="#content" hx-push-url="true" {
                "← " (dossier.name)
            }
            h1 { "Cette conversation s'arrête." }
            p class="lede" { "Le dossier quitte la liste du jour. Les lettres, les rencontres et l'estimation restent. On pourra le rouvrir." }
            @if let Some(msg) = error {
                p class="mast-note" role="alert" { (msg) }
            }
            form hx-post=(format!("{href}/arreter")) hx-target="#content" hx-push-url="true" {
                (form::select("reason", "Pourquoi", LOSS_REASONS, reason, None))
                (form::text("detail", "Préciser, si besoin", detail, None))
                div class="row-actions" {
                    button class="fate" type="submit" { "Arrêter la conversation" }
                }
            }
        }
    }
}

pub fn win_page(dossier: &PersonDossier, when: &str, error: Option<&str>) -> Markup {
    let href = person_href(&dossier.name);
    let amount = dossier
        .current
        .amount
        .map(|m| m.to_string())
        .unwrap_or_else(|| "—".into());
    html! {
        div class="letter" data-view=(ViewId::Gens.slug()) {
            a class="back" href=(href) hx-get=(href) hx-target="#content" hx-push-url="true" {
                "← " (dossier.name)
            }
            h1 { "C'est un client." }
            p class="lede" { (format!("{amount} HT deviennent le forfait de la mission. Griffe ne rédige pas le devis.")) }
            @if let Some(msg) = error {
                p class="mast-note" role="alert" { (msg) }
            }
            form hx-post=(format!("{href}/client")) hx-target="#content" hx-push-url="true" {
                (form::date("started_on", "À partir du", when, None))
                div class="row-actions" {
                    button class="seal" type="submit" { "Ouvrir la mission" }
                }
            }
        }
    }
}

pub fn reopen_page(dossier: &PersonDossier, when: &str, error: Option<&str>) -> Markup {
    let href = person_href(&dossier.name);
    html! {
        div class="letter" data-view=(ViewId::Gens.slug()) {
            a class="back" href=(href) hx-get=(href) hx-target="#content" hx-push-url="true" {
                "← " (dossier.name)
            }
            h1 { "Ils reviennent." }
            p class="lede" { "Même dossier : les lettres, les rencontres et l'estimation sont toujours là. La relance repart du premier message." }
            @if let Some(msg) = error {
                p class="mast-note" role="alert" { (msg) }
            }
            form hx-post=(format!("{href}/reprise")) hx-target="#content" hx-push-url="true" {
                (form::date("when", "Prochain pas", when, None))
                div class="row-actions" {
                    button class="seal" type="submit" { "Rouvrir la conversation" }
                }
            }
        }
    }
}

pub fn new_conversation(
    today: Date,
    who: &str,
    phrase: &str,
    who_error: Option<&str>,
    phrase_error: Option<&str>,
    banner: Option<&str>,
) -> Markup {
    html! {
        div class="letter" data-view=(ViewId::Gens.slug()) {
            a class="back" href="/affaires" hx-get="/affaires" hx-target="#content" hx-push-url="true" {
                "← Les affaires"
            }
            h1 { "Une conversation." }
            p class="lede" { "Un nom, une phrase. Le reste viendra — devis, projet, facture — quand ce sera vrai." }
            @if let Some(msg) = banner {
                p class="mast-note" role="alert" { (msg) }
            }
            form hx-post="/affaires/nouvelle" hx-target="#content" hx-push-url="true" {
                (form::text("who", "Qui", who, who_error))
                (form::text("phrase", "Ce dont il s'agit", phrase, phrase_error))
                input type="hidden" name="today" value=(format_date(today));
                div class="row-actions" {
                    button class="seal" type="submit" { "Ouvrir la conversation" }
                    a class="quiet" href="/affaires" hx-get="/affaires" hx-target="#content" hx-push-url="true" {
                        "Annuler"
                    }
                }
            }
        }
    }
}

const MEETING_NATURES: &[(&str, &str)] = &[
    ("", "Choisir"),
    ("call", "Téléphone"),
    ("visio", "Visioconférence"),
    ("email", "E-mail"),
    ("meeting", "Rencontre physique"),
    ("note", "Note"),
];

pub fn meeting_form(
    dossier: &PersonDossier,
    today: Date,
    kind: &str,
    note: &str,
    error: Option<&str>,
) -> Markup {
    let href = person_href(&dossier.name);
    html! {
        div class="letter" data-view=(ViewId::Gens.slug()) {
            a class="back" href=(href) hx-get=(href) hx-target="#content" hx-push-url="true" {
                "← " (dossier.name)
            }
            h1 { "Une rencontre." }
            p class="lede" { "Pas un CRM. Ce que tu veux encore savoir dans six mois." }
            @if let Some(msg) = error {
                p class="mast-note" role="alert" { (msg) }
            }
            form hx-post=(format!("{href}/rencontre")) hx-target="#content" hx-push-url="true" {
                (form::select("kind", "Nature", MEETING_NATURES, kind, None))
                (form::date("when", "Quand", &format_date(today), None))
                (form::textarea("note", "Ce qu'on s'est dit", note, 6, None))
                div class="row-actions" {
                    button class="seal" type="submit" { "Poser la note" }
                }
            }
        }
    }
}

pub struct FicheValues {
    pub who: String,
    pub street: String,
    pub postal_code: String,
    pub city: String,
    pub representative: String,
    pub email: String,
    pub phone: String,
    pub client_revision: i64,
    pub contact_id: String,
    pub contact_revision: String,
}

pub struct FicheErrors {
    pub who: Option<String>,
    pub address: Option<String>,
    pub email: Option<String>,
    pub banner: Option<String>,
}

pub fn fiche_page(
    dossier: &PersonDossier,
    values: &FicheValues,
    errors: &FicheErrors,
    genre: &GenreField,
) -> Markup {
    let href = person_href(&dossier.name);
    html! {
        div class="letter" data-view=(ViewId::Gens.slug()) {
            a class="back" href=(href) hx-get=(href) hx-target="#content" hx-push-url="true" {
                "← " (dossier.name)
            }
            h1 { "La fiche." }
            p class="lede" { "Qui c'est, pour leur écrire plus tard. Pas besoin d'une estimation pour ça." }
            @if let Some(msg) = &errors.banner {
                p class="mast-note" role="alert" { (msg) }
            }
            form hx-post=(format!("{href}/fiche")) hx-target="#content" hx-push-url="true" {
                (form::hidden("client_revision", &values.client_revision.to_string()))
                (form::hidden("contact_id", &values.contact_id))
                (form::hidden("contact_revision", &values.contact_revision))
                (form::text("who", "Qui", &values.who, errors.who.as_deref()))
                (form::text("street", "Rue", &values.street, errors.address.as_deref()))
                (form::text("postal_code", "Code postal", &values.postal_code, None))
                (form::text("city", "Ville", &values.city, None))
                (form::text("representative", "Qui répond", &values.representative, None))
                (form::text("email", "Courriel", &values.email, errors.email.as_deref()))
                (form::text("phone", "Téléphone", &display_phone(&values.phone), None))
                @if genre.show {
                    (genre_line(genre))
                }
                div class="row-actions" {
                    button class="seal" type="submit" { "Enregistrer la fiche" }
                }
            }
        }
    }
}

pub struct EstimateValues {
    pub phrase: String,
    pub lines: Vec<(String, String)>,
    pub revision: String,
}

pub struct EstimateErrors {
    pub phrase: Option<String>,
    pub lines: Option<String>,
    pub banner: Option<String>,
}

pub fn estimate_page(
    dossier: &PersonDossier,
    values: &EstimateValues,
    errors: &EstimateErrors,
) -> Markup {
    let href = person_href(&dossier.name);
    html! {
        div class="letter spread" data-view=(ViewId::Gens.slug()) {
            a class="back" href=(href) hx-get=(href) hx-target="#content" hx-push-url="true" {
                "← " (dossier.name)
            }
            h1 { "L'estimation." }
            p class="lede" {
                "Ce que chaque ligne vaut. Le devis, tu le rédiges ailleurs."
            }
            @if let Some(msg) = &errors.banner {
                p class="mast-note" role="alert" { (msg) }
            }
            @if let Some(msg) = &errors.lines {
                p class="mast-note" role="alert" { (msg) }
            }
            form hx-post=(format!("{href}/estimation")) hx-target="#content" hx-push-url="true" {
                (form::hidden("revision", &values.revision))
                (form::text("phrase", "Ce dont il s'agit", &values.phrase, errors.phrase.as_deref()))
                @for (label, amount) in &values.lines {
                    div class="field-inline" {
                        (form::text("label", "Travail", label, None))
                        (form::text("euros", "€ HT", amount, None))
                    }
                }
                div class="row-actions" {
                    button class="quiet" type="submit" name="add" value="1" { "Ajouter une ligne" }
                    button class="seal" type="submit" { "Noter l'estimation" }
                }
            }
        }
    }
}

/// Fragment rendu du récit, cible de l'aperçu. Une page vide dit « Rien d'écrit. »
pub fn travaux_fragment(body: &str) -> Markup {
    if body.is_empty() {
        html! { p class="work-empty" { "Rien d'écrit." } }
    } else {
        html! { (PreEscaped(render_work_markdown(body))) }
    }
}

pub fn travaux_page(dossier: &PersonDossier, note: &DossierWork, error: Option<&str>) -> Markup {
    let href = person_href(&dossier.name);
    let action = format!("{href}/travaux");
    let preview = format!("{href}/travaux/apercu");
    html! {
        div class="letter spread" data-view=(ViewId::Gens.slug()) {
            a class="back" href=(href) hx-get=(href) hx-target="#content" hx-push-url="true" {
                "← " (dossier.name)
            }
            h1 { "Les travaux." }
            p class="lede" {
                "Où on en est. L'estimation, à côté, dit ce que chaque ligne vaut."
            }
            @if let Some(msg) = error {
                p class="mast-note" role="alert" { (msg) }
            }
            div id="travaux-rendu" class="work-prose" {
                (travaux_fragment(&note.body))
            }
            form hx-post=(action) hx-target="#content" hx-push-url="true" {
                input type="hidden" name="revision" value=(note.revision);
                div class="field" {
                    label for="body" { "Le récit" }
                    textarea id="body" name="body" rows="14"
                      hx-post=(preview)
                      hx-trigger="keyup changed delay:400ms"
                      hx-target="#travaux-rendu"
                      hx-swap="innerHTML" {
                        (note.body)
                    }
                }
                div class="row-actions" {
                    button class="seal" type="submit" { "Garder" }
                }
            }
        }
    }
}

pub struct PhraseMoment {
    pub key: String,
    pub label: String,
    pub subject: String,
    pub body: String,
    pub revision: i64,
    pub gap: String,
    pub first: bool,
    pub last: bool,
    pub alone: bool,
    pub open: bool,
    pub reads_subject: String,
    pub reads: String,
}

pub struct PhraseLink {
    pub label: String,
    pub href: String,
    pub current: bool,
}

pub struct PhrasesView {
    pub back_href: String,
    pub back_label: String,
    pub action: String,
    pub title: String,
    pub lede: String,
    pub links: Vec<PhraseLink>,
    pub other_href: String,
    pub drop: bool,
    pub moments: Vec<PhraseMoment>,
    pub chronicle: String,
    pub read_caption: String,
    pub prenom: String,
    pub sujet: String,
    pub montant: String,
    pub moi: String,
    pub societe: String,
    /// Formule déjà résolue, pour l'aperçu à côté de la frise.
    pub signature: String,
    pub banner: Option<String>,
    pub status: Option<String>,
}

/// Valeur du bouton « un autre ». Ce n'est pas un nom de genre.
pub const GENRE_OTHER: &str = "autre";

/// Ce que la fiche a posté pour le genre, avant de décider du nom à enregistrer.
#[derive(Debug, Clone, Default)]
pub struct GenrePost {
    pub checked: String,
    pub other: String,
}

#[derive(Debug, Clone, Default)]
pub struct GenreField {
    pub show: bool,
    /// Nom attaché au dossier, pour le lien vers les phrases.
    pub name: String,
    pub known: Vec<String>,
    pub phrases: String,
    pub created: bool,
    /// Le dossier a déjà un genre : le mot « aucun » est proposé.
    pub attached: bool,
    /// Mot coché quand la ligne « un autre » est fermée. Vide : « aucun », ou rien.
    pub picked: String,
    pub other_open: bool,
    pub other: String,
    pub naming: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct KeepUi {
    pub ask: bool,
    pub confirm: bool,
    pub proposed_name: String,
    /// Texte encore dans la lettre, quand le geste n'est pas terminé.
    pub subject: Option<String>,
    pub body: Option<String>,
}

pub fn phrases_page(view: &PhrasesView) -> Markup {
    let order = view
        .moments
        .iter()
        .map(|moment| moment.key.as_str())
        .collect::<Vec<_>>()
        .join(",");
    let open = view.moments.iter().find(|moment| moment.open);
    let sheet_subject = open
        .map(|moment| moment.reads_subject.as_str())
        .unwrap_or("");
    let sheet_body = open
        .map(|moment| griffe_core::mail::readable_links(&moment.reads))
        .unwrap_or_default();
    html! {
        div class="phrase-desk" data-view=(ViewId::Gens.slug()) {
            a class="back" href=(view.back_href) hx-get=(view.back_href) hx-target="#content" hx-push-url="true" {
                "← " (view.back_label)
            }
            h1 { (view.title) }
            p class="phrase-line" {
                @for link in &view.links {
                    @if link.current {
                        span class="current" { (link.label) }
                    } @else {
                        a href=(link.href) hx-get=(link.href) hx-target="#content" hx-push-url="true" {
                            (link.label)
                        }
                    }
                }
                a href=(view.other_href) hx-get=(view.other_href) hx-target="#content" hx-push-url="true" {
                    "Un autre genre"
                }
            }
            p class="lede" { (view.lede) }
            p class="phrase-chronicle" id="phrase-chronicle" { (view.chronicle) }
            p class="phrase-when" { "Les conversations déjà engagées finissent leur série." }
            @if let Some(msg) = &view.banner {
                p class="mast-note" role="alert" { (msg) }
            }
            @if let Some(msg) = &view.status {
                p class="mast-note" role="status" { (msg) }
            }
            form hx-post=(view.action) hx-target="#content" hx-push-url="true" {
                input type="hidden" name="order" value=(order);
                div class="phrase-stage" {
                    div class="phrase-frise" role="radiogroup" aria-label="Les moments" {
                        span class="phrase-playhead" aria-hidden="true" {}
                        @for moment in &view.moments {
                            @if !moment.first {
                                div class="phrase-link" {
                                    span class="phrase-rule" {}
                                    label class="phrase-gap" {
                                        input id=(format!("ecart_{}", moment.key))
                                              name=(format!("ecart_{}", moment.key))
                                              type="text"
                                              inputmode="numeric"
                                              value=(moment.gap)
                                              aria-label=(format!("Jours avant {}", moment.label));
                                        span { "j" }
                                    }
                                }
                            }
                            div class="phrase-unit" {
                                input class="phrase-pick"
                                      type="radio"
                                      name="ouvert"
                                      id=(format!("pick-{}", moment.key))
                                      value=(moment.key)
                                      checked[moment.open];
                                label class="phrase-node" for=(format!("pick-{}", moment.key)) {
                                    span class="dot" aria-hidden="true" {}
                                    span class="phrase-node-name" { (moment.label) }
                                    @if moment.first {
                                        span class="phrase-node-when" { "le jour" }
                                    }
                                }
                                div class="phrase-panel" {
                                    (phrase_line(&format!("label_{}", moment.key), "Ce moment", &moment.label, false))
                                    (phrase_line(&format!("subject_{}", moment.key), "Sujet", &moment.subject, false))
                                    (phrase_line(&format!("body_{}", moment.key), "Lettre", &moment.body, true))
                                    input type="hidden" name=(format!("revision_{}", moment.key)) value=(moment.revision);
                                    div class="token-row" {
                                        @for (name, insert) in PHRASE_TOKENS {
                                            button type="button" data-insert=(insert) { (name) }
                                        }
                                    }
                                    @if !moment.alone {
                                        div class="phrase-moves" {
                                            @if !moment.first {
                                                button type="submit" name="move" value=(format!("{}:earlier", moment.key)) { "Monter" }
                                            }
                                            @if !moment.last {
                                                button type="submit" name="move" value=(format!("{}:later", moment.key)) { "Descendre" }
                                            }
                                            button type="submit" name="drop" value=(moment.key) { "Retirer" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    aside class="phrase-sheet"
                          data-prenom=(view.prenom)
                          data-sujet=(view.sujet)
                          data-montant=(view.montant)
                          data-moi=(view.moi)
                          data-societe=(view.societe) {
                        template class="signature-source" { (view.signature) }
                        p class="phrase-caption" { (view.read_caption) }
                        p class="phrase-sheet-subject" { (sheet_subject) }
                        pre class="phrase-sheet-body" { (sheet_body) }
                    }
                }
                div class="row-actions" {
                    button class="quiet" type="submit" name="add" value="1" { "Ajouter un moment" }
                    button class="seal" type="submit" { "Enregistrer les phrases" }
                    @if view.drop {
                        button class="quiet" type="submit" name="retirer_genre" value="1" { "Retirer ce genre" }
                    }
                }
            }
        }
    }
}

const PHRASE_TOKENS: &[(&str, &str)] = &[
    ("<prénom>", "<prénom>"),
    ("<sujet>", "<sujet>"),
    ("<montant>", "<montant>"),
    ("<moi>", "<moi>"),
    ("<société>", "<société>"),
    ("<signature>", "<signature>"),
];

fn phrase_line(id: &str, label: &str, value: &str, letter: bool) -> Markup {
    html! {
        div class="field" {
            label for=(id) { (label) }
            @if letter {
                textarea id=(id) name=(id) rows="8" { (value) }
                p class="field-help" { (LINK_HINT) }
            } @else {
                input id=(id) name=(id) type="text" value=(value);
            }
        }
    }
}

pub fn phrases_href(depuis: &str, pour: &str, genre: &str) -> String {
    query_href("/affaires/phrases", depuis, pour, genre, "")
}

pub fn new_genre_href(depuis: &str, pour: &str, source: &str) -> String {
    query_href("/affaires/phrases/nouveau", depuis, pour, "", source)
}

fn query_href(path: &str, depuis: &str, pour: &str, genre: &str, source: &str) -> String {
    let mut parts = Vec::new();
    if !depuis.is_empty() {
        parts.push(format!("depuis={}", path_encode(depuis)));
    }
    if !pour.is_empty() {
        parts.push(format!("pour={}", path_encode(pour)));
    }
    if !genre.is_empty() {
        parts.push(format!("genre={}", path_encode(genre)));
    }
    if !source.is_empty() {
        parts.push(format!("source={}", path_encode(source)));
    }
    if parts.is_empty() {
        path.to_string()
    } else {
        format!("{path}?{}", parts.join("&"))
    }
}

/// Le créneau d'envoi. Tant que la lettre est retenue ou en route, le bloc reste en place
/// et son contenu se renouvelle chaque seconde (`every 1s`, `innerHTML`). Le bouton qui
/// l'a armé le remplace en entier. Une fois partie, annulée ou en échec, la réponse du
/// sondage porte `HX-Reswap: outerHTML` et ce bloc-ci, sans minuteur.
#[must_use]
pub fn depart_markup(
    href: &str,
    view: Option<&OutboundView>,
    note: Option<&str>,
    send_label: &str,
    destination: Option<&str>,
) -> Markup {
    let poll = format!("{href}/envoi");
    let polling = view.is_some_and(depart_polls);
    let poll_url = polling.then_some(poll.as_str());
    let trigger = polling.then_some("every 1s");
    let swap = polling.then_some("innerHTML");
    html! {
        div id="depart" class="depart"
            hx-get=[poll_url]
            hx-trigger=[trigger]
            hx-swap=[swap] {
            (depart_body(href, view, note, send_label, destination))
        }
    }
}

/// Corps du sondage. `true` tant que le minuteur doit continuer.
#[must_use]
pub fn depart_poll_fragment(
    href: &str,
    view: Option<&OutboundView>,
    note: Option<&str>,
    send_label: &str,
    destination: Option<&str>,
) -> (Markup, bool) {
    let polling = view.is_some_and(depart_polls);
    let markup = if polling {
        depart_body(href, view, note, send_label, destination)
    } else {
        depart_markup(href, view, note, send_label, destination)
    };
    (markup, polling)
}

fn depart_body(
    href: &str,
    view: Option<&OutboundView>,
    note: Option<&str>,
    send_label: &str,
    destination: Option<&str>,
) -> Markup {
    let send = format!("{href}/envoyer");
    let left = view.map(|item| item.seconds_left).unwrap_or(0);
    html! {
        @if let Some(note) = note {
            p class="depart-line" { (note) }
        }
        @match view.map(|item| item.status) {
            Some(OutboundStatus::Armed | OutboundStatus::Held) => {
                p class="depart-line" {
                    @if let Some(address) = destination.filter(|value| !value.is_empty()) {
                        @if left > 0 {
                            "Elle part vers "
                            (address)
                            " dans "
                            span class="depart-count" { (left) }
                            "."
                        } @else {
                            "Elle part vers "
                            (address)
                            "."
                        }
                    } @else {
                        "Il manque l'adresse. Elle ne partira pas."
                    }
                }
                span class="depart-track" data-left=(left) { i {} }
                (post_button(&format!("{href}/envoi/annuler"), "quiet", "id", view.map(|item| item.id.as_str()), "Annuler"))
            }
            Some(OutboundStatus::Sending) => {
                p class="depart-line" { "Elle est en route." }
            }
            Some(OutboundStatus::Sent) => {
                p class="depart-line" { "Courrier envoyé." }
                (envoyer_button(&send, send_label))
            }
            Some(OutboundStatus::Failed) => {
                p class="depart-line" {
                    (view.and_then(|item| item.error.as_deref()).unwrap_or("Elle n'est pas partie."))
                }
                (post_button(&format!("{href}/envoi/reessayer"), "quiet", "id", view.map(|item| item.id.as_str()), "Réessayer"))
            }
            Some(OutboundStatus::Uncertain) => {
                p class="depart-line" { "Griffe ne sait pas si elle est partie." }
                input type="hidden" name="id" value=(view.map(|item| item.id.as_str()).unwrap_or(""));
                (post_button(&format!("{href}/envoi/decision"), "quiet", "sent", Some("1"), "Elle est partie"))
                (post_button(&format!("{href}/envoi/decision"), "quiet", "sent", Some("0"), "Elle n'est pas partie"))
                (post_button(&format!("{href}/envoi/reessayer"), "quiet", "retry", Some("1"), "Réessayer"))
            }
            Some(OutboundStatus::Cancelled) | None => {
                (envoyer_button(&send, send_label))
            }
        }
    }
}

fn depart_polls(view: &OutboundView) -> bool {
    matches!(
        view.status,
        OutboundStatus::Armed | OutboundStatus::Held | OutboundStatus::Sending
    )
}

fn envoyer_button(action: &str, label: &str) -> Markup {
    html! {
        button class="seal" type="submit"
               formaction=(action)
               formmethod="post"
               hx-post=(action)
               hx-target="#depart"
               hx-swap="outerHTML" {
            (label)
        }
    }
}

fn post_button(action: &str, class: &str, name: &str, value: Option<&str>, label: &str) -> Markup {
    html! {
        button class=(class) type="submit"
               formaction=(action)
               formmethod="post"
               hx-post=(action)
               hx-target="#depart"
               hx-swap="outerHTML"
               name=(name)
               value=(value.unwrap_or("")) {
            (label)
        }
    }
}

pub fn letter_page(
    store: &Store,
    dossier: &PersonDossier,
    card: Option<&FollowUpCard>,
    today: Date,
    flash: Option<&str>,
    keep: &KeepUi,
) -> Result<Markup, AppError> {
    let href = person_href(&dossier.name);
    let sender = follow_up_sender(store.connection())?;
    let from = sender.sender_email.as_deref().unwrap_or("—");
    let recipient = card.and_then(|c| c.contact_email.as_deref()).unwrap_or("");
    let to = if recipient.is_empty() {
        "—"
    } else {
        recipient
    };
    let mail = griffe_core::mail::profile(store.connection()).unwrap_or_default();
    let can_send = mail.ready && recipient.contains('@');
    let depart = if can_send {
        griffe_core::mail::outbound_for_anchor(
            store.connection(),
            &dossier.name,
            time::OffsetDateTime::now_utc(),
        )
        .ok()
        .flatten()
    } else {
        None
    };
    let preview_subject = card
        .and_then(|c| c.preview_subject.as_deref())
        .unwrap_or("");
    let preview_body = card.and_then(|c| c.preview_body.as_deref()).unwrap_or("");
    let subject = keep.subject.as_deref().unwrap_or(preview_subject);
    let body = keep.body.as_deref().unwrap_or(preview_body);
    let opportunity = match dossier.follow_up_subject {
        Some(FollowUpSubject::Opportunity(id)) => Some(id),
        _ => None,
    };
    let genre = opportunity
        .map(|id| prospect_genre_for(store.connection(), id))
        .transpose()?
        .flatten();
    let moment = card.and_then(|c| c.step_label.clone());
    let phrases = phrases_href(
        "lettre",
        &dossier.name,
        genre.as_ref().map(|item| item.id.as_str()).unwrap_or(""),
    );
    let known = if keep.ask {
        prospect_genres(store.connection())?
            .into_iter()
            .map(|item| item.name)
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let genre_name = genre
        .as_ref()
        .map(|item| item.name.clone())
        .filter(|name| !name.is_empty())
        .or_else(|| {
            let proposed = keep.proposed_name.trim();
            if keep.confirm && !proposed.is_empty() {
                Some(proposed.to_string())
            } else {
                None
            }
        });
    let keep_sentence = match (&moment, &genre_name) {
        (Some(moment), Some(name)) if keep.confirm => Some(format!(
            "Ces mots remplaceront le {} des {}. Les lettres déjà classées ne bougent pas.",
            lower_first(moment),
            lower_first(name)
        )),
        _ => None,
    };
    let keep_label = if keep.confirm {
        moment
            .as_deref()
            .map(|moment| format!("Oui, remplacer le {}", lower_first(moment)))
            .unwrap_or_else(|| "Garder ces mots".to_string())
    } else if genre.is_some() {
        "Garder ces mots pour ce genre".to_string()
    } else {
        "Garder ces mots".to_string()
    };
    let show_keep = opportunity.is_some() && moment.is_some();
    let mots = format!("{href}/mots");
    let previous: Vec<&HistoryEvent> = dossier
        .history
        .iter()
        .filter(|e| matches!(e.kind, HistoryKind::Letter { .. }))
        .collect();
    Ok(html! {
        div class="letter spread" data-view=(ViewId::Gens.slug()) {
            a class="back" href=(href) hx-get=(href) hx-target="#content" hx-push-url="true" {
                "← " (dossier.name)
            }
            @if mail.ready {
                h1 { "Une lettre." }
                p class="lede" {
                    "Tu écris ici. « Envoyer » la fait partir, avec cinq secondes pour la retenir. « C'est parti » classe le double dans l'historique."
                }
            } @else {
                h1 { "Une lettre, pas un envoi." }
                p class="lede" {
                    "Tu écris ici. « C'est parti » classe le double dans l'historique. Griffe n'envoie pas."
                }
                p class="phrase-nav" {
                    a href="/societe/courrier" hx-get="/societe/courrier" hx-target="#content" hx-push-url="true" {
                        "Le courrier"
                    }
                }
            }
            @if let Some(msg) = flash {
                p class="mast-note" role="status" { (msg) }
            }
            @if show_keep {
                @if let Some(label) = &moment {
                    p class="phrase-line" {
                        span { (genre_line_name(genre.as_ref().map(|item| item.name.as_str()))) " · " (label) }
                        a href=(phrases) hx-get=(phrases) hx-target="#content" hx-push-url="true" {
                            "Les phrases"
                        }
                    }
                }
            } @else if let Some(label) = &moment {
                p class="phrase-nav" {
                    (label)
                    " · "
                    a href=(phrases) hx-get=(phrases) hx-target="#content" hx-push-url="true" {
                        "Les phrases"
                    }
                }
            }
            div class="letter-compose" {
            div class="letter-draft" {
                div class="meta" {
                    "De " (from) " · À " (to)
                    @if !mail.ready { " · ne sera pas envoyé par Griffe" }
                }
                form {
                    (form::text("subject_line", "Sujet", subject, None))
                    (form::textarea("body", "Lettre", body, 12, None))
                    p class="field-help" { (LINK_HINT) }
                    @if keep.ask {
                        p class="phrase-caption" { "Pour qui ?" }
                        div class="field" {
                            label for="genre_name" { "Genre" }
                            input id="genre_name" name="genre_name" type="text" value=(keep.proposed_name);
                        }
                        p class="phrase-line" {
                            @for name in &known {
                                span { (name) }
                            }
                        }
                    }
                    @if let Some(sentence) = &keep_sentence {
                        p class="phrase-caption" { (sentence) }
                    }
                    @if keep.confirm && genre.is_none() {
                        input type="hidden" name="genre_name" value=(keep.proposed_name);
                    }
                    div class="row-actions" {
                        @if can_send {
                            (depart_markup(&href, depart.as_ref(), None, "Envoyer", (!recipient.is_empty()).then_some(recipient)))
                        } @else if mail.ready {
                            p class="depart-line" { "Il manque l'adresse de la personne." }
                        }
                        @if show_keep {
                            button class="quiet" type="submit"
                                   formaction=(mots)
                                   formmethod="post"
                                   hx-post=(mots)
                                   hx-target="#content"
                                   hx-push-url="true"
                                   name=(if keep.confirm { "confirmer" } else { "garder" })
                                   value="1" {
                                (keep_label)
                            }
                        }
                        button class="seal" type="submit"
                               formaction=(format!("{href}/envoye"))
                               formmethod="post"
                               hx-post=(format!("{href}/envoye"))
                               hx-target="#content"
                               hx-push-url="true" {
                            "C'est parti"
                        }
                    }
                }
            }
            (letter_face(
                &mail.from_name,
                &mail.metier,
                body,
                &mail.site,
                mail.link_ink.as_str(),
            ))
            }
            @if !previous.is_empty() {
                div class="block" {
                    h3 { "Déjà classées" }
                    ul class="hist" {
                        @for event in previous {
                            (history_item(event))
                        }
                    }
                }
            }
            p class="date" { (letter_date(today)) }
        }
    })
}

pub fn letter_face(name: &str, metier: &str, body: &str, site: &str, ink: &str) -> Markup {
    let name = name.trim();
    let metier = metier.trim();
    let site = site.trim();
    let ink = match ink {
        "encre" | "sceau" => ink,
        _ => "vert",
    };
    let body = griffe_core::mail::readable_links(body);
    let site_label = griffe_core::mail::readable_links(site);
    html! {
        aside class=(format!("letter-face letter-ink-{ink}")) {
            @if !name.is_empty() {
                p class="letter-face-name" { (name) }
            }
            @if !metier.is_empty() {
                p class="letter-face-metier" { (metier) }
            }
            @if !name.is_empty() || !metier.is_empty() {
                hr class="letter-face-rule";
            }
            pre class="letter-face-body" { (body) }
            @if !site.is_empty() {
                p class="letter-face-site" { (site_label) }
            }
        }
    }
}

fn lower_first(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => {
            let mut out = first.to_lowercase().collect::<String>();
            out.push_str(chars.as_str());
            out
        }
    }
}

fn genre_line_name(name: Option<&str>) -> String {
    name.unwrap_or("Pas encore de genre").to_string()
}

fn genre_line(genre: &GenreField) -> Markup {
    let naming = genre.naming.is_some();
    html! {
        @if genre.known.is_empty() {
            div class="field" {
                label for="genre_other" { "Genre" }
                input id="genre_other" name="genre_other" type="text" value=(genre.other) aria-invalid[naming];
                p class="field-help" { "Le premier nom servira aux phrases." }
                @if let Some(msg) = &genre.naming {
                    div class="field-error" { (msg) }
                }
            }
        } @else {
            fieldset class="word-choice" {
                legend { "Genre" }
                div class="word-choice-row" {
                    @if genre.attached {
                        label {
                            input type="radio" name="genre" value="" checked[genre.picked.is_empty() && !genre.other_open];
                            "aucun"
                        }
                    }
                    @for name in &genre.known {
                        label {
                            input type="radio" name="genre" value=(name) checked[!genre.other_open && genre.picked == name.as_str()];
                            (name)
                        }
                    }
                    label {
                        input class="word-other-toggle" type="radio" name="genre" value=(GENRE_OTHER) checked[genre.other_open];
                        "un autre"
                    }
                }
                input class="word-other" name="genre_other" type="text" value=(genre.other) aria-label="Nom du genre" aria-invalid[naming];
            }
            @if let Some(msg) = &genre.naming {
                div class="field-error" { (msg) }
            }
        }
        @if genre.created {
            p class="mast-note" { "Ce genre n'existe pas encore. La fiche le crée." }
        }
        @if !genre.phrases.is_empty() {
            p {
                a href=(genre.phrases) hx-get=(genre.phrases) hx-target="#content" hx-push-url="true" {
                    @if genre.created { "Écrire les phrases" } @else { (genre.name) }
                }
            }
        }
    }
}

pub fn genre_field(
    store: &Store,
    dossier: &PersonDossier,
    posted: Option<&GenrePost>,
    created: bool,
) -> Result<GenreField, AppError> {
    let known = prospect_genres(store.connection())?
        .into_iter()
        .map(|genre| genre.name)
        .collect();
    let Some(id) = dossier.current.opportunity_id else {
        return Ok(GenreField {
            known,
            ..GenreField::default()
        });
    };
    let current = prospect_genre_for(store.connection(), id)?;
    let stored = current
        .as_ref()
        .map(|genre| genre.name.clone())
        .unwrap_or_default();
    let phrases = current
        .as_ref()
        .map(|genre| phrases_href("fiche", &dossier.name, &genre.id))
        .unwrap_or_default();
    let (picked, other_open, other, naming) = match posted {
        None => (stored.clone(), false, String::new(), None),
        Some(post) => {
            let other = post.other.trim().to_string();
            if !other.is_empty() {
                (String::new(), true, other, None)
            } else if post.checked.trim() == GENRE_OTHER {
                (
                    String::new(),
                    true,
                    String::new(),
                    Some("Nomme le genre.".to_string()),
                )
            } else {
                (post.checked.trim().to_string(), false, String::new(), None)
            }
        }
    };
    Ok(GenreField {
        show: true,
        name: stored,
        known,
        phrases,
        created,
        attached: current.is_some(),
        picked,
        other_open,
        other,
        naming,
    })
}

pub fn new_genre_page(
    back_href: &str,
    back_label: &str,
    action: &str,
    name: &str,
    banner: Option<&str>,
) -> Markup {
    html! {
        div class="letter" data-view=(ViewId::Gens.slug()) {
            a class="back" href=(back_href) hx-get=(back_href) hx-target="#content" hx-push-url="true" {
                "← " (back_label)
            }
            h1 { "Un autre genre." }
            p class="lede" {
                "Un nom suffit. Les phrases de la page d'où tu viens sont copiées, pour avoir quelque chose à corriger. Griffe n'envoie pas."
            }
            @if let Some(msg) = banner {
                p class="mast-note" role="alert" { (msg) }
            }
            form hx-post=(action) hx-target="#content" hx-push-url="true" {
                (form::text("name", "Nom", name, None))
                div class="row-actions" {
                    button class="seal" type="submit" { "Enregistrer" }
                }
            }
        }
    }
}

fn choice_label(label: &str, key: &PersonKey, choices: &[(PersonKey, String)]) -> String {
    let same = choices.iter().filter(|(_, other)| other == label).count();
    if same > 1 {
        let short: String = key.as_ref_str().chars().take(8).collect();
        format!("{label} · {short}")
    } else {
        label.to_string()
    }
}

pub fn several(needle: &str, choices: &[(PersonKey, String)]) -> Markup {
    html! {
        div class="letter" data-view=(ViewId::Gens.slug()) {
            a class="back" href="/affaires" hx-get="/affaires" hx-target="#content" hx-push-url="true" {
                "← Les affaires"
            }
            div class="who" { (needle) }
            p class="lede" { "Plusieurs fiches. Choisis laquelle." }
            ul class="people" {
                @for (key, label) in choices {
                    @let href = person_href(&key.as_ref_str());
                    @let shown = choice_label(label, key, choices);
                    li {
                        a href=(href) hx-get=(href) hx-target="#content" hx-push-url="true" {
                            div class="nm" { (shown) }
                        }
                    }
                }
            }
        }
    }
}

pub fn not_found(needle: &str, today: Date) -> Markup {
    html! {
        div class="letter" data-view=(ViewId::Gens.slug()) {
            a class="back" href="/affaires" hx-get="/affaires" hx-target="#content" hx-push-url="true" {
                "← Les affaires"
            }
            h1 { "Personne." }
            p class="lede" { "Aucune fiche ne correspond à « " (needle) " »." }
            p class="date" { (letter_date(today)) }
        }
    }
}

#[must_use]
pub fn href_for_party(party: &str) -> String {
    person_href(party)
}

pub(crate) fn person_line(row: &PersonRow) -> Markup {
    row_link(row)
}

pub fn load_card(
    store: &Store,
    subject: FollowUpSubject,
    today: Date,
) -> Result<Option<FollowUpCard>, AppError> {
    match card_for(store.connection(), subject, today) {
        Ok(card) => Ok(Some(card)),
        Err(_) => Ok(None),
    }
}

pub fn types_page(
    kinds: &[WorkKind],
    draft: &str,
    banner: Option<&str>,
    confirming: Option<&str>,
) -> Markup {
    html! {
        div class="letter" data-view=(ViewId::Gens.slug()) {
            a class="back" href="/affaires" hx-get="/affaires" hx-target="#content" hx-push-url="true" {
                "← Les affaires"
            }
            h1 { "Les types." }
            p class="lede" { "Un type dit le métier du dossier. Plusieurs peuvent tenir sur la même fiche." }
            @if let Some(msg) = banner {
                p class="mast-note" role="alert" { (msg) }
            }
            @for kind in kinds {
                div class="kind-line" {
                    @if confirming == Some(kind.id.as_str()) {
                        form hx-post=(format!("/affaires/types/{}", kind.id)) hx-target="#content" {
                            p class="kind-word" { (kind.name) }
                            div class="row-actions" {
                                button class="btn danger" type="submit" name="geste" value="confirmer" {
                                    "Oui, le retirer"
                                }
                                a class="quiet" href="/affaires/types"
                                  hx-get="/affaires/types" hx-target="#content" hx-push-url="true" {
                                    "Garder"
                                }
                            }
                        }
                    } @else {
                        form hx-post=(format!("/affaires/types/{}", kind.id)) hx-target="#content" {
                            p class="kind-word" { (kind.name) }
                            div class="field" {
                                label for=(format!("rename-{}", kind.id)) { "Nouveau nom" }
                                input id=(format!("rename-{}", kind.id)) name="name" type="text" value=(kind.name);
                            }
                            div class="row-actions" {
                                button class="quiet" type="submit" name="geste" value="renommer" { "Renommer" }
                                button class="quiet" type="submit" name="geste" value="retirer" { "Retirer" }
                            }
                        }
                    }
                }
            }
            form hx-post="/affaires/types" hx-target="#content" {
                (form::text("name", "Un type", draft, None))
                div class="row-actions" {
                    button class="seal" type="submit" { "Enregistrer" }
                }
            }
        }
    }
}
