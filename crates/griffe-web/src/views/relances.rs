//! File d'une carte : le quotidien des relances.

use griffe_core::app::AppError;
use griffe_core::domain::{FollowUpKind, FollowUpSubject, SnoozePreset, format_date, snooze_date};
use griffe_core::follow_up::{
    CardStatus, FollowUpCard, follow_up_board, follow_up_queue, follow_up_sender,
};
use griffe_core::store::Store;
use maud::{Markup, PreEscaped, html};

use crate::layout::{ViewId, view_head};
use crate::views::errors;
use crate::views::form;

pub fn render(
    store: &Store,
    today: time::Date,
    focus: Option<FollowUpSubject>,
    flash: Option<&str>,
) -> Result<Markup, AppError> {
    let queue = follow_up_queue(store.connection(), today)?;
    let board = follow_up_board(store.connection(), today)?;
    let later: Vec<_> = board
        .iter()
        .filter(|c| c.status == CardStatus::Later)
        .take(8)
        .collect();
    let sender = follow_up_sender(store.connection())?;
    let current = pick_card(&queue, &board, focus);
    Ok(html! {
        (view_head(ViewId::Relances, &subtitle(&queue, today)))
        @if let Some(msg) = flash {
            div class="follow-flash" role="status" { (msg) }
        }
        @if sender.sender_email.is_none() {
            (sender_form(None))
        }
        div class="follow-shell" id="relance-workspace" {
            aside class="follow-rail" {
                div class="follow-rail-title" { "Aujourd'hui" }
                @if queue.is_empty() {
                    div class="follow-rail-empty" { "Rien à relancer." }
                } @else {
                    @for card in &queue {
                        @let active = current.as_ref().is_some_and(|c| c.subject == card.subject);
                        a class={ "follow-rail-item" @if active { " active" } }
                          href=(card_href(card.subject))
                          hx-get=(card_href(card.subject))
                          hx-target="#content"
                          hx-push-url="true" {
                            span class={ "follow-dot " (status_class(card.status)) } {}
                            span class="follow-rail-name" { (card.title) }
                            span class="follow-rail-meta" { (card.party) }
                        }
                    }
                }
                @if !later.is_empty() {
                    div class="follow-rail-title" { "Plus tard" }
                    @for card in later {
                        a class="follow-rail-item later"
                          href=(card_href(card.subject))
                          hx-get=(card_href(card.subject))
                          hx-target="#content"
                          hx-push-url="true" {
                            span class="follow-rail-name" { (card.title) }
                            span class="follow-rail-meta" { (or_date(card.due_on)) }
                        }
                    }
                }
            }
            section class="follow-stage" {
                @if let Some(card) = current {
                    (card_markup(card, today))
                } @else {
                    (empty_state())
                }
            }
        }
        (PreEscaped(HINT))
    })
}

const HINT: &str = r#"<p class="follow-hint">↵ brouillon · E envoyé · S demain · K sauter</p>"#;

fn subtitle(queue: &[FollowUpCard], today: time::Date) -> String {
    if queue.is_empty() {
        format!("rien pour le {}", or_date(Some(today)))
    } else {
        format!("{} à traiter", queue.len())
    }
}

fn pick_card<'a>(
    queue: &'a [FollowUpCard],
    board: &'a [FollowUpCard],
    focus: Option<FollowUpSubject>,
) -> Option<&'a FollowUpCard> {
    focus
        .and_then(|subject| {
            queue
                .iter()
                .chain(board.iter())
                .find(|c| c.subject == subject)
        })
        .or_else(|| queue.first())
}

pub(crate) fn card_href(subject: FollowUpSubject) -> String {
    match subject {
        FollowUpSubject::Opportunity(id) => format!("/relances/opportunity/{id}"),
        FollowUpSubject::Invoice(id) => format!("/relances/invoice/{id}"),
    }
}

fn status_class(status: CardStatus) -> &'static str {
    match status {
        CardStatus::Overdue => "overdue",
        CardStatus::Due => "due",
        CardStatus::Drafted => "drafted",
        CardStatus::Blocked => "blocked",
        CardStatus::Later => "later",
        CardStatus::Exhausted => "exhausted",
    }
}

fn kind_label(kind: FollowUpKind) -> &'static str {
    match kind {
        FollowUpKind::Prospect => "Prospect",
        FollowUpKind::Invoice => "Impayé",
    }
}

fn or_date(d: Option<time::Date>) -> String {
    d.map(format_date).unwrap_or_else(|| "—".to_string())
}

fn empty_state() -> Markup {
    html! {
        div class="follow-empty" {
            h2 { "Rien à relancer aujourd'hui." }
            p { "Les opportunités avec une date passée ou due, et les factures échues, arrivent ici. Pose une prochaine action dans Prospection, ou reviens demain." }
        }
    }
}

pub fn sender_form(error: Option<&str>) -> Markup {
    html! {
        form class="follow-sender" hx-post="/relances/sender" hx-target="#content" hx-swap="innerHTML" {
            p { "Avec quelle adresse écrivez-vous ? Le brouillon s'ouvrira dans votre client mail, jamais envoyé d'ici." }
            (form::text("email", "Email", "", error))
            (form::text("name", "Nom signataire", "", None))
            button class="btn primary" type="submit" { "Enregistrer" }
        }
    }
}

fn card_markup(card: &FollowUpCard, today: time::Date) -> Markup {
    let href = card_href(card.subject);
    html! {
        article class="follow-card" data-subject=(href) {
            header class="follow-card-head" {
                span class={ "badge " (status_class(card.status)) } { (kind_label(card.kind)) }
                @if card.status == CardStatus::Overdue {
                    span class="badge danger" { "en retard" }
                }
                @if card.status == CardStatus::Drafted {
                    span class="badge warn" { "brouillon ouvert" }
                }
                span class="follow-amount" { (card.amount) }
            }
            h2 class="follow-title" { (card.title) }
            p class="follow-party" {
                (card.party)
                @if let Some(email) = &card.contact_email {
                    " · " (email)
                }
            }
            (track(card))
            @if let Some(reason) = &card.block_reason {
                div class="follow-block" role="alert" { (reason) }
            }
            @if let Some(subject) = &card.preview_subject {
                div class="follow-preview" {
                    div class="follow-preview-subject" { (subject) }
                    @if let Some(body) = &card.preview_body {
                        pre { (body) }
                    }
                }
            }
            (actions(card, today, &href))
            @if !card.history.is_empty() {
                details class="follow-history" {
                    summary { "Historique" }
                    ol {
                        @for item in &card.history {
                            li { (item.label) " · " (format_date(item.on)) }
                        }
                    }
                }
            }
        }
    }
}

fn track(card: &FollowUpCard) -> Markup {
    html! {
        ol class="follow-track" {
            @for i in 0..card.step_count {
                @let done = i < card.step_index;
                @let current = i == card.step_index;
                li class={
                    @if done { "done" }
                    @else if current { "current" }
                    @else { "todo" }
                } {
                    span class="follow-track-dot" {}
                    @if current {
                        @if let Some(label) = &card.step_label {
                            span class="follow-track-label" { (label) }
                        }
                    }
                }
            }
        }
    }
}

fn actions(card: &FollowUpCard, today: time::Date, href: &str) -> Markup {
    let tomorrow = format_date(snooze_date(today, SnoozePreset::Tomorrow));
    let monday = format_date(snooze_date(today, SnoozePreset::NextMonday));
    let week = format_date(snooze_date(today, SnoozePreset::NextWeek));
    html! {
        div class="follow-actions" {
            @if card.status != CardStatus::Blocked && card.status != CardStatus::Exhausted {
                form hx-post=(format!("{href}/draft")) hx-target="#content" {
                    button class="btn primary" type="submit" data-follow-action="draft" {
                        @if card.status == CardStatus::Drafted { "Rouvrir le brouillon" }
                        @else { "Ouvrir le brouillon" }
                    }
                }
            }
            @if card.status == CardStatus::Drafted || card.status == CardStatus::Due || card.status == CardStatus::Overdue {
                form hx-post=(format!("{href}/sent")) hx-target="#content" {
                    button class="btn" type="submit" data-follow-action="sent" { "C'est envoyé" }
                }
            }
            @if card.status != CardStatus::Exhausted && card.status != CardStatus::Blocked {
                form hx-post=(format!("{href}/skip")) hx-target="#content" {
                    button class="btn ghost" type="submit" data-follow-action="skip" { "Sauter" }
                }
            }
            @if !card.history.is_empty() {
                form hx-post=(format!("{href}/retract")) hx-target="#content" {
                    button class="btn ghost" type="submit" { "Annuler" }
                }
            }
        }
        @if card.status != CardStatus::Exhausted {
            div class="follow-snooze" {
                span { "Reporter" }
                form hx-post=(format!("{href}/snooze")) hx-target="#content" {
                    input type="hidden" name="until" value=(tomorrow);
                    button class="chip" type="submit" data-follow-action="snooze-tomorrow" { "Demain" }
                }
                form hx-post=(format!("{href}/snooze")) hx-target="#content" {
                    input type="hidden" name="until" value=(monday);
                    button class="chip" type="submit" { "Lundi" }
                }
                form hx-post=(format!("{href}/snooze")) hx-target="#content" {
                    input type="hidden" name="until" value=(week);
                    button class="chip" type="submit" { "+7 jours" }
                }
                form class="follow-date" hx-post=(format!("{href}/schedule")) hx-target="#content" {
                    input type="date" name="on" value=(or_date(card.due_on));
                    button class="chip" type="submit" { "Date" }
                }
            }
        }
    }
}

pub fn error_banner(err: &AppError) -> Markup {
    html! {
        div class="follow-block" role="alert" { (errors::message(err)) }
    }
}
