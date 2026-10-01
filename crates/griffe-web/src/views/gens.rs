//! Les affaires : une liste, un dossier. Les faits viennent de `griffe_core::people` ; cette vue
//! rédige le français.

use griffe_core::app::AppError;
use griffe_core::domain::{
    ExpensePaidBy, FollowUpSubject, InteractionKind, SnoozePreset, format_date, format_date_fr,
    snooze_date,
};
use griffe_core::follow_up::{FollowUpCard, card_for, follow_up_sender};
use griffe_core::people::{
    CurrentSituation, HistoryEvent, HistoryKind, MissionShape, OutgoingCadence, OutgoingChapter,
    OutgoingNote, Paper, PaperKind, PaperStatus, PeopleList, PersonAction, PersonChapter,
    PersonCue, PersonDossier, PersonFigure, PersonKey, PersonRow, people_list, person,
};
use griffe_core::store::Store;
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
            (outgoing_chapter(&list.outgoing))
            (stopped_chapter(&list.stopped))
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
    let n = list.conversations.len() + list.missions.len();
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

pub fn dossier_markup(dossier: &PersonDossier, today: Date, flash: Option<&str>) -> Markup {
    let href = person_href(&dossier.name);
    let (daily, fate): (Vec<_>, Vec<_>) = dossier
        .actions
        .iter()
        .partition(|action| !matches!(action, PersonAction::Stop { .. }));
    html! {
        div class="letter" data-view=(ViewId::Gens.slug()) data-person=(dossier.name) {
            a class="back" href="/affaires" hx-get="/affaires" hx-target="#content" hx-push-url="true" {
                "← Les affaires"
            }
            div class="who" { (dossier.name) }
            p class="co" { (subtitle(dossier, today)) }
            @if let Some(msg) = flash {
                p class="mast-note" role="status" { (msg) }
            }
            @if !daily.is_empty() {
                div class="row-actions" {
                    @for action in &daily {
                        (action_button(action, &href, today))
                    }
                }
            }
            @for action in &fate {
                (action_button(action, &href, today))
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
    pub banner: Option<String>,
}

pub fn fiche_page(dossier: &PersonDossier, values: &FicheValues, errors: &FicheErrors) -> Markup {
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
                (form::text("email", "Courriel", &values.email, None))
                (form::text("phone", "Téléphone", &values.phone, None))
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
                "Les travaux, et ce que chacun vaut. Le total reste dans le coffre. Le devis, tu le rédiges ailleurs."
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
        div class="letter spread" data-view=(ViewId::Gens.slug()) {
            a class="back" href=(href) hx-get=(href) hx-target="#content" hx-push-url="true" {
                "← " (dossier.name)
            }
            h1 { "Une lettre, pas un envoi." }
            p class="lede" {
                "Tu écris ici. « C'est parti » classe le double dans l'historique. Griffe n'envoie pas."
            }
            @if let Some(msg) = flash {
                p class="mast-note" role="status" { (msg) }
            }
            div class="letter-draft" {
                div class="meta" {
                    "De " (from) " · À " (to) " · ne sera pas envoyé par Griffe"
                }
                form {
                    (form::text("subject_line", "Sujet", subject, None))
                    (form::textarea("body", "Lettre", body, 12, None))
                    div class="row-actions" {
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
