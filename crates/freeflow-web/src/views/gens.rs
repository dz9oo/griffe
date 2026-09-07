//! Les affaires : une liste, un dossier. Les faits viennent de `freeflow_core::people` ; cette vue
//! rédige le français.

use freeflow_core::app::AppError;
use freeflow_core::domain::{
    FollowUpSubject, InteractionKind, SnoozePreset, format_date, format_date_fr, snooze_date,
};
use freeflow_core::follow_up::{FollowUpCard, card_for, follow_up_sender};
use freeflow_core::people::{
    CurrentSituation, HistoryEvent, HistoryKind, MissionShape, Paper, PaperKind, PaperStatus,
    PeopleList, PersonAction, PersonChapter, PersonCue, PersonDossier, PersonFigure, PersonRow,
    people_list, person,
};
use freeflow_core::store::Store;
use maud::{Markup, html};
use time::Date;

use crate::layout::ViewId;
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
    let list = people_list(store.connection(), today)?;
    Ok(list_markup(&list, today, None))
}

pub fn list_markup(list: &PeopleList, today: Date, flash: Option<&str>) -> Markup {
    let empty = list.is_empty();
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
            }
            (chapter("En conversation", &list.conversations, "Aucune conversation ouverte."))
            (chapter("En mission", &list.missions, "Aucune mission en cours."))
            (chapter("Fournisseurs", &list.suppliers, "Les bénéficiaires d'une dépense apparaîtront ici."))
        }
    }
}

fn list_title(list: &PeopleList) -> String {
    let n = list.conversations.len() + list.missions.len() + list.suppliers.len();
    match n {
        0 => "Les affaires.".into(),
        1 => "Un nom.".into(),
        2 => "Deux noms.".into(),
        k => format!("{k} noms."),
    }
}

fn list_lede(list: &PeopleList) -> String {
    match list.conversations.len() {
        0 => "Avec qui j'en suis. Un nom, pas un type de document.".into(),
        1 => {
            let name = &list.conversations[0].name;
            format!("Derrière {name}, personne. C'est le trou — pas un graphique.")
        }
        _ => "Avec qui j'en suis. Un nom, pas un type de document.".into(),
    }
}

fn chapter(label: &str, rows: &[PersonRow], empty: &str) -> Markup {
    html! {
        p class="section-label" { (label) }
        @if rows.is_empty() {
            p class="empty-state" { (empty) }
        } @else {
            ul class="people" {
                @for row in rows {
                    li { (row_link(row)) }
                }
            }
        }
    }
}

fn row_link(row: &PersonRow) -> Markup {
    let href = person_href(&row.name);
    html! {
        a href=(href) hx-get=(href) hx-target="#content" hx-push-url="true" {
            div {
                div class="nm" { (row.name) }
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

fn cue_fr(cue: &PersonCue) -> String {
    match cue {
        PersonCue::QuoteSent { .. } => "devis envoyé".into(),
        PersonCue::FollowUpDue { today: true, .. } => "à relancer aujourd'hui".into(),
        PersonCue::FollowUpDue { on, .. } => format!("à relancer le {}", format_date_fr(*on)),
        PersonCue::FirstExchange { on } => format!("premier échange le {}", format_date_fr(*on)),
        PersonCue::NothingScheduled => "rien de posé".into(),
        PersonCue::InvoiceOverdue { days } => format!("facture en retard · {days} jours"),
        PersonCue::InvoiceOutstanding => "facture à encaisser".into(),
        PersonCue::NextMilestone { on, label } => {
            format!("{label} · prochain jalon le {}", format_date_fr(*on))
        }
        PersonCue::OpeningDebt => "dette reprise au bilan".into(),
        PersonCue::MatchingDebit => "un débit correspond".into(),
    }
}

pub fn dossier_page(store: &Store, needle: &str, today: Date) -> Result<Markup, AppError> {
    let dossier = person(store.connection(), needle, today)?;
    Ok(dossier_markup(&dossier, today, None))
}

pub fn dossier_markup(dossier: &PersonDossier, today: Date, flash: Option<&str>) -> Markup {
    let href = person_href(&dossier.name);
    html! {
        div class="letter" data-view=(ViewId::Gens.slug()) data-person=(dossier.name) {
            a class="back" href="/affaires" hx-get="/affaires" hx-target="#content" hx-push-url="true" {
                "← Les affaires"
            }
            div class="who" { (dossier.name) }
            p class="co" { (subtitle(dossier)) }
            @if let Some(msg) = flash {
                p class="mast-note" role="status" { (msg) }
            }
            @if !dossier.actions.is_empty() {
                div class="row-actions" {
                    @for action in &dossier.actions {
                        (action_button(action, &href, today))
                    }
                }
            }
            @if let Some(body) = current_paragraph(&dossier.current, dossier) {
                div class="block" {
                    h3 { "En cours" }
                    p { (body) }
                }
            }
            @if let Some(project) = &dossier.project {
                div class="block" {
                    h3 { "Le projet" }
                    p { (project_paragraph(project, dossier)) }
                }
            }
            @if !dossier.papers.is_empty() {
                div class="block" {
                    h3 { "Les papiers" }
                    ul class="hist" {
                        @for paper in &dossier.papers {
                            li {
                                span class="when" { (short_date(paper.on)) }
                                span { (paper_line(paper)) }
                            }
                        }
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

fn subtitle(d: &PersonDossier) -> String {
    let mut parts = Vec::new();
    if d.contact_name.as_ref().is_some_and(|c| c != &d.party) {
        parts.push(d.party.clone());
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
    } else if d.chapter == PersonChapter::Supplier {
        parts.push("fournisseur".into());
    }
    parts.join(" · ")
}

fn month_year(on: Date) -> String {
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
    let m = MONTHS
        .get(usize::from(u8::from(on.month()).saturating_sub(1)))
        .copied()
        .unwrap_or("");
    format!("{m} {}", on.year())
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
    if dossier.chapter == PersonChapter::Supplier {
        let mut parts = Vec::new();
        if let Some(amount) = current.amount {
            if current.opening_debt {
                parts.push(format!("{amount} repris en dette."));
            } else {
                parts.push(format!("{amount}."));
            }
        }
        if current.matching_debit {
            parts.push("Un débit du relevé correspond au centime. Le ranger comme règlement — pas comme une nouvelle charge.".into());
        }
        if parts.is_empty() {
            return None;
        }
        return Some(parts.join(" "));
    }
    let mut sentences = Vec::new();
    if let Some(quote) = &current.quote {
        let name = current
            .opportunity_name
            .as_deref()
            .unwrap_or("accompagnement");
        sentences.push(format!(
            "Devis {name}, {} HT, envoyé le {}.",
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
        sentences.push("À relancer aujourd'hui.".into());
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

fn project_paragraph(
    project: &freeflow_core::people::ProjectChapter,
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

fn paper_line(paper: &Paper) -> String {
    let kind = match paper.kind {
        PaperKind::Quote => "Devis",
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
            @if let HistoryKind::Letter { subject, body } = &event.kind {
                details class="letter-fold" {
                    summary { (subject) }
                    pre { (body) }
                }
            } @else {
                span { (history_fr(event)) }
            }
        }
    }
}

fn history_fr(event: &HistoryEvent) -> String {
    match &event.kind {
        HistoryKind::Interaction { interaction } => {
            let kind = match interaction {
                InteractionKind::Call => "Appel",
                InteractionKind::Email => "E-mail",
                InteractionKind::Meeting => "Rencontre",
                InteractionKind::Note => "Note",
            };
            match &event.note {
                Some(n) => format!("{kind}. {n}"),
                None => format!("{kind}."),
            }
        }
        HistoryKind::Letter { subject, .. } => format!("Lettre. {subject}"),
        HistoryKind::QuoteSent { .. } => "Devis envoyé.".into(),
        HistoryKind::QuoteAccepted => "Devis accepté.".into(),
        HistoryKind::InvoiceIssued { number } => format!("Facture {number}."),
    }
}

fn action_button(action: &PersonAction, dossier_href: &str, today: Date) -> Markup {
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
        PersonAction::Quote { .. } => html! {
            a class="quiet" href=(format!("{dossier_href}/devis"))
              hx-get=(format!("{dossier_href}/devis")) hx-target="#panel" hx-swap="innerHTML" {
                "Le devis"
            }
        },
        PersonAction::LogMeeting { .. } => html! {
            a class="quiet" href=(format!("{dossier_href}/rencontre"))
              hx-get=(format!("{dossier_href}/rencontre")) hx-target="#content" hx-push-url="true" {
                "Noter une rencontre"
            }
        },
        PersonAction::Snooze { .. } => {
            let until =
                format_date(snooze_date(today, SnoozePreset::Tomorrow) + time::Duration::days(2));
            html! {
                form hx-post=(format!("{dossier_href}/reporter")) hx-target="#content" {
                    input type="hidden" name="until" value=(until);
                    button class="quiet" type="submit" { "Reporter de trois jours" }
                }
            }
        }
        PersonAction::FileStatement => html! {
            a class="seal" href=(ViewId::Depenses.path())
              hx-get=(ViewId::Depenses.path()) hx-target="#content" hx-push-url="true" {
                "Ranger le mouvement"
            }
        },
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

pub fn meeting_form(
    dossier: &PersonDossier,
    today: Date,
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
                (form::date("when", "Quand", &format_date(today), None))
                (form::textarea("note", "Ce qu'on s'est dit", note, 6, None))
                div class="row-actions" {
                    button class="seal" type="submit" { "Poser la note" }
                }
            }
        }
    }
}

pub fn letter_page(
    store: &Store,
    dossier: &PersonDossier,
    card: Option<&FollowUpCard>,
    today: Date,
    flash: Option<&str>,
) -> Result<Markup, AppError> {
    let href = person_href(&dossier.name);
    let sender = follow_up_sender(store.connection())?;
    let from = sender.sender_email.as_deref().unwrap_or("—");
    let to = card.and_then(|c| c.contact_email.as_deref()).unwrap_or("—");
    let subject = card
        .and_then(|c| c.preview_subject.as_deref())
        .unwrap_or("");
    let body = card.and_then(|c| c.preview_body.as_deref()).unwrap_or("");
    let previous: Vec<&HistoryEvent> = dossier
        .history
        .iter()
        .filter(|e| matches!(e.kind, HistoryKind::Letter { .. }))
        .collect();
    Ok(html! {
        div class="letter" data-view=(ViewId::Gens.slug()) {
            a class="back" href=(href) hx-get=(href) hx-target="#content" hx-push-url="true" {
                "← " (dossier.name)
            }
            h1 { "Une lettre, pas un envoi." }
            p class="lede" {
                "Tu écris ici. Le client mail n'est que la poste. Le double classé est celui de cette page — un mot changé dans le client ne s'y met que si tu le rectifies en classant."
            }
            @if let Some(msg) = flash {
                p class="mast-note" role="status" { (msg) }
            }
            div class="letter-draft" {
                div class="meta" {
                    "De " (from) " · À " (to) " · ne sera pas envoyé par FreeFlow"
                }
                form {
                    (form::text("subject_line", "Sujet", subject, None))
                    (form::textarea("body", "Lettre", body, 12, None))
                    div class="row-actions" {
                        button class="seal" type="submit"
                               formaction=(format!("{href}/ecrire"))
                               formmethod="post"
                               hx-post=(format!("{href}/ecrire"))
                               hx-target="#content" {
                            "Poster"
                        }
                        button class="quiet" type="submit"
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
