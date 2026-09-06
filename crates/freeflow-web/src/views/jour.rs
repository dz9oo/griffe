//! Le jour : mât, gestes dépliables, mois. Les faits viennent des queries du cœur (lot 49) ;
//! cette vue ne fait que rédiger le français.

use freeflow_core::app::AppError;
use freeflow_core::day::{
    DayGesture, DayMark, GestureSource, GestureVerb, Mast, MastSignal, MissionTrack, MonthEvent,
    MonthEventKind, MonthTarget, MonthView, day_gestures, day_mast, day_month,
};
use freeflow_core::domain::{
    FollowUpSubject, Money, Month, SnoozePreset, format_date, format_date_fr, snooze_date,
};
use freeflow_core::setup::setup_status;
use freeflow_core::store::Store;
use maud::{Markup, html};
use time::{Date, Weekday};

use crate::layout::ViewId;
use crate::views::copy::{
    deadline_fr, duty_href, event_kind_fr, gestes_title, is_vat, letter_date, month_fr, month_title,
};
use crate::views::gens::{href_for_party, person_href};

pub fn render(store: &Store, today: Date) -> Result<Markup, AppError> {
    let conn = store.connection();
    let setup = setup_status(conn)?;
    let mast = day_mast(conn, today)?;
    let gestes = day_gestures(conn, today)?;
    let month = Month::new(today.year(), u8::from(today.month()))
        .expect("le mois courant est toujours valide");
    let month_view = day_month(conn, month, today)?;

    let title = gestes_title(gestes.len());
    let year_end = month_view
        .events
        .iter()
        .find(|e| e.kind == MonthEventKind::YearEnd)
        .map(|e| e.on);
    let lede = letter_lede(gestes.len(), setup.is_done(), today, year_end);
    let note = mast_note(&mast, setup.is_done());
    let alarming = mast.is_alarming();

    Ok(html! {
        div class="letter" data-view=(ViewId::Jour.slug()) {
            div class="date" { (letter_date(today)) }
            div class={ "mast" @if alarming { " alarm" } } {
                div class="mast-facts" {
                    span {
                        b { (mast.bank.to_string()) }
                        " "
                        span class="dim" { "en banque" }
                    }
                    span {
                        b class={ @if runway_is_bad(&mast) { "bad" } } {
                            (runway_label(mast.runway_months))
                        }
                        " "
                        span class="dim" { "de piste" }
                    }
                    span {
                        b class={ @if receivable_is_bad(&mast) { "bad" } } {
                            (encaisser_label(&mast))
                        }
                        @if let Some(r) = mast.receivables.first() {
                            " "
                            span class="dim" { "chez " (r.party) }
                        } @else {
                            " "
                            span class="dim" { "à encaisser" }
                        }
                    }
                }
                @if let Some(text) = note {
                    p class="mast-note" id="next-step" { (text) }
                } @else if !setup.is_done() {
                    p class="mast-note" id="next-step" { (setup.next_step.text()) }
                }
            }
            h1 { (title) }
            p class="lede" { (lede) }
            @if !gestes.is_empty() {
                ol class="gestes" {
                    @for (i, g) in gestes.iter().enumerate() {
                        (geste_li(i + 1, g, today))
                    }
                }
            }
            (month_section(&month_view, today))
        }
    })
}

fn runway_is_bad(mast: &Mast) -> bool {
    mast.signals
        .iter()
        .any(|s| matches!(s, MastSignal::RunwayAlarm { .. }))
}

fn receivable_is_bad(mast: &Mast) -> bool {
    mast.signals
        .iter()
        .any(|s| matches!(s, MastSignal::ReceivableStale { .. }))
}

fn runway_label(months: Option<u32>) -> String {
    match months {
        Some(n) => n.to_string(),
        None => "—".into(),
    }
}

fn encaisser_label(mast: &Mast) -> String {
    mast.receivables
        .first()
        .map_or_else(|| "rien".into(), |r| r.amount.to_string())
}

fn mast_note(mast: &Mast, setup_done: bool) -> Option<String> {
    if !setup_done {
        return None;
    }
    Some(match mast.signals.first()? {
        MastSignal::RunwayAlarm { months: 0 } => "La piste ne tient plus le mois.".into(),
        MastSignal::RunwayAlarm { .. } => "La piste tient moins de deux mois.".into(),
        MastSignal::RunwayUnbacked { months } => {
            format!("La piste tient {months} mois, et rien n'est ouvert derrière.")
        }
        MastSignal::PipelineThin { party, .. } => {
            format!(
                "Une seule conversation ouverte derrière {party}. Si elle dit non, la piste s'arrête."
            )
        }
        MastSignal::PipelineEmpty => "Aucune conversation ouverte.".into(),
        MastSignal::ReceivableStale { party, days } => {
            format!("{party} : {days} jours à encaisser.")
        }
    })
}

fn letter_lede(n: usize, setup_done: bool, today: Date, year_end: Option<Date>) -> String {
    if n == 0 {
        return "Rien n'est dû ce matin.".into();
    }
    if !setup_done {
        return "On commence par dire qui vous êtes. Le reste attend.".into();
    }
    if let Some(end) = year_end {
        let days = (end - today).whole_days();
        if (1..=45).contains(&days) {
            return format!(
                "Rien d'autre n'est urgent. La clôture est dans {days} jours — pas aujourd'hui."
            );
        }
    }
    "Rien d'autre n'est urgent.".into()
}

fn geste_li(n: usize, g: &DayGesture, today: Date) -> Markup {
    let (title, body) = geste_copy(g);
    html! {
        li {
            button class="geste" type="button" {
                span class="num" { (n) }
                div {
                    h2 { (title) }
                    p { (body) }
                }
            }
            div class="unfold" {
                div class="row-actions" { (geste_actions(g, today)) }
            }
        }
    }
}

fn geste_copy(g: &DayGesture) -> (String, String) {
    match &g.source {
        GestureSource::Setup { step } => ("Configurer ma société".into(), step.text().to_string()),
        GestureSource::FollowUp {
            contact_name,
            party,
            title,
            drafted,
            block_reason,
            due_on,
            follow_kind,
            ..
        } => {
            let who = contact_name.as_deref().unwrap_or(party.as_str());
            let head = match g.verb {
                GestureVerb::Remind => format!("Relancer {who}"),
                _ => format!("Écrire à {who}"),
            };
            let mut body = title.clone();
            if let Some(on) = due_on {
                body.push_str(" — prévue le ");
                body.push_str(&format_date_fr(*on));
            }
            if *drafted {
                body.push_str(". Un brouillon est prêt — il s'ouvre dans ton client mail.");
            } else if let Some(reason) = block_reason {
                body.push_str(". ");
                body.push_str(reason);
            } else if *follow_kind == freeflow_core::domain::FollowUpKind::Invoice {
                body.push_str(". Le montant est dans le coffre.");
            }
            (head, body)
        }
        GestureSource::BankStatement { unmatched } => {
            let body = if *unmatched == 1 {
                "Un mouvement n'a pas encore de lecture.".into()
            } else {
                format!("{unmatched} mouvements n'ont pas encore de lecture.")
            };
            ("Ranger le relevé".into(), body)
        }
        GestureSource::StateDuty {
            deadline,
            due_on,
            amount,
        } => {
            let title = if is_vat(*deadline) {
                "Savoir pour la TVA".into()
            } else {
                format!("Savoir pour {}", deadline_fr(*deadline))
            };
            let amount = amount
                .filter(|m| *m != Money::ZERO)
                .map(|m| format!("{m} · "))
                .unwrap_or_default();
            let body = format!(
                "{amount}à déposer avant le {}. La lettre dit le chemin — le dépôt se fait sur le site des impôts, pas ici.",
                format_date_fr(*due_on)
            );
            (title, body)
        }
    }
}

fn geste_actions(g: &DayGesture, today: Date) -> Markup {
    match &g.source {
        GestureSource::Setup { .. } => html! {
            a class="seal" href="/premiers-pas"
              hx-get="/premiers-pas" hx-target="#content" hx-push-url="true" {
                "Ouvrir l'assistant"
            }
        },
        GestureSource::FollowUp {
            subject,
            drafted,
            contact_name,
            party,
            ..
        } => {
            let who = contact_name
                .as_deref()
                .filter(|n| !n.is_empty())
                .unwrap_or(party.as_str());
            let href = person_href(who);
            let write = format!("{href}/ecrire");
            let tomorrow = format_date(snooze_date(today, SnoozePreset::Tomorrow));
            let _ = subject;
            html! {
                a class="seal" href=(href)
                  hx-get=(href) hx-target="#content" hx-push-url="true" {
                    "Ouvrir son dossier"
                }
                form hx-post=(write) hx-target="#content" hx-push-url="true" {
                    button class="quiet" type="submit" {
                        @if *drafted { "Lire le brouillon" } @else { "Préparer le brouillon" }
                    }
                }
                form hx-post=(format!("{href}/reporter")) hx-target="#content" {
                    input type="hidden" name="until" value=(tomorrow);
                    button class="quiet" type="submit" { "Demain" }
                }
            }
        }
        GestureSource::BankStatement { .. } => html! {
            a class="seal" href="/societe/releve"
              hx-get="/societe/releve" hx-target="#content" hx-push-url="true" {
                "Lire les mouvements"
            }
        },
        GestureSource::StateDuty { deadline, .. } => {
            let href = duty_href(*deadline);
            html! {
                a class="quiet" href=(href)
                  hx-get=(href) hx-target="#content" hx-push-url="true" {
                    "Lire la lettre avant de partir"
                }
            }
        }
    }
}

fn month_section(view: &MonthView, today: Date) -> Markup {
    let title = month_title(view.month.month());
    let lede = month_lede(view);
    let days = view.month.last_day().day();
    let first_weekday = view.month.first_day().weekday();
    let empty_before = match first_weekday {
        Weekday::Monday => 0,
        Weekday::Tuesday => 1,
        Weekday::Wednesday => 2,
        Weekday::Thursday => 3,
        Weekday::Friday => 4,
        Weekday::Saturday => 5,
        Weekday::Sunday => 6,
    };
    let used = empty_before + days;
    let pad = (7 - (used % 7)) % 7;
    let agenda: Vec<&MonthEvent> = view.events.iter().filter(|e| e.on >= today).collect();
    let next_empty_name = view.next_month_empty.then(|| {
        let name = month_title(view.month.succ().month());
        name.trim_end_matches('.').to_string()
    });

    html! {
        section class="month" aria-label=(format!("{} {}", month_fr(view.month.month()), view.month.year())) {
            h2 { (title) }
            p class="lede" { (lede) }
            div class="cal" {
                @for d in ["Lu", "Ma", "Me", "Je", "Ve", "Sa", "Di"] {
                    div class="dow" { (d) }
                }
                @for _ in 0..empty_before {
                    div class="d empty" {}
                }
                @for day in 1..=days {
                    (day_cell(day, view, today))
                }
                @for _ in 0..pad {
                    div class="d pad" {}
                }
            }
            @if !view.tracks.is_empty() {
                div class="tracks" {
                    @for track in &view.tracks {
                        (track_row(track, view.month, today))
                    }
                }
            }
            @if !agenda.is_empty() {
                ul class="agenda" {
                    @for event in &agenda {
                        li { (agenda_row(event, today)) }
                    }
                }
            }
            @if let Some(on) = view.empty_after {
                p class="month-note" {
                    "Après le " (on.day()) ", plus aucune mission sur le papier."
                    @if let Some(name) = next_empty_name.as_deref() {
                        " " (name) " est vide."
                    }
                }
            }
        }
    }
}

fn month_lede(view: &MonthView) -> String {
    let n = view.tracks.len();
    let mut parts = Vec::new();
    parts.push(match n {
        0 => "Aucune mission ce mois-ci.".to_string(),
        1 => "Une mission en cours.".to_string(),
        2 => "Deux missions en cours.".to_string(),
        k => format!("{k} missions en cours."),
    });
    if let Some(e) = view
        .events
        .iter()
        .find(|e| e.kind == MonthEventKind::StateDuty && e.deadline.is_some_and(is_vat))
    {
        parts.push(format!("La TVA le {}.", e.on.day()));
    }
    if let Some(e) = view
        .events
        .iter()
        .find(|e| e.kind == MonthEventKind::MissionEnd)
    {
        let who = e.party.as_deref().unwrap_or(e.title.as_str());
        parts.push(format!("{who} s'arrête le {}.", e.on.day()));
    }
    parts.join(" ")
}

fn day_cell(day: u8, view: &MonthView, today: Date) -> Markup {
    let cell = view.month.first_day() + time::Duration::days(i64::from(day) - 1);
    let marks: Vec<DayMark> = view
        .events
        .iter()
        .filter(|e| e.on == cell)
        .map(|e| e.kind.mark())
        .collect();
    let mark = if marks.contains(&DayMark::Legal) {
        Some("legal")
    } else if marks.contains(&DayMark::Meet) {
        Some("meet")
    } else if !marks.is_empty() {
        Some("")
    } else {
        None
    };
    let is_today = cell == today;
    let has = mark.is_some();
    html! {
        button class={
            "d"
            @if has { " has" }
            @if is_today { " today on" }
            @if let Some(kind) = mark {
                @if !kind.is_empty() { " " (kind) }
            }
        }
        type="button"
        data-day=(day.to_string()) {
            (day)
            @if has { i class="mark" {} }
        }
    }
}

fn track_row(track: &MissionTrack, month: Month, today: Date) -> Markup {
    let days = f64::from(month.last_day().day());
    let start = if track.started_on < month.first_day() {
        1
    } else {
        track.started_on.day()
    };
    let end = match track.ended_on {
        Some(d) if d <= month.last_day() && d >= month.first_day() => d.day(),
        _ => month.last_day().day(),
    };
    let left = f64::from(start.saturating_sub(1)) / days * 100.0;
    let width = f64::from(end.saturating_sub(start).saturating_add(1)) / days * 100.0;
    let now = if today >= month.first_day() && today <= month.last_day() {
        Some(f64::from(today.day().saturating_sub(1)) / days * 100.0)
    } else {
        None
    };
    let end_label = if track.whole_month {
        "tout le mois".to_string()
    } else if let Some(d) = track.ended_on {
        let jalon = track
            .milestones
            .first()
            .map(|m| format!("jalon le {} · ", m.due_on.day()));
        format!("{}fin le {}", jalon.unwrap_or_default(), d.day())
    } else {
        String::new()
    };
    html! {
        div class="track" {
            span class="lab" { (track.client_name) }
            div class="rail" aria-hidden="true" {
                i class="bar" style=(format!("left:{left:.1}%;width:{width:.1}%")) {}
                @if let Some(p) = now {
                    i class="now" style=(format!("left:{p:.1}%")) {}
                }
                @for tick in &track.milestones {
                    @let p = f64::from(tick.due_on.day().saturating_sub(1)) / days * 100.0;
                    i class="tick" style=(format!("left:{p:.1}%")) {}
                }
            }
            span class="end" { (end_label) }
        }
    }
}

fn agenda_row(event: &MonthEvent, today: Date) -> Markup {
    let kind = if event.on == today {
        "aujourd'hui"
    } else if event.kind == MonthEventKind::StateDuty {
        if event.deadline.is_some_and(is_vat) {
            "TVA"
        } else {
            "État"
        }
    } else {
        event_kind_fr(event.kind)
    };
    let ttl = agenda_title(event);
    let href = event_href(&event.target, event.party.as_deref(), event.deadline);
    let on_today = event.on == today;
    html! {
        @if let Some(href) = href {
            a href=(href) hx-get=(href) hx-target="#content" hx-push-url="true"
              class={ @if on_today { "on" } }
              data-day=(event.on.day().to_string()) {
                span class="dt" { (event.on.day()) }
                span class="kind" { (kind) }
                span class="ttl" { (ttl) }
            }
        } @else {
            button type="button"
              class={ @if on_today { "on" } }
              data-day=(event.on.day().to_string()) {
                span class="dt" { (event.on.day()) }
                span class="kind" { (kind) }
                span class="ttl" { (ttl) }
            }
        }
    }
}

fn agenda_title(event: &MonthEvent) -> String {
    match event.kind {
        MonthEventKind::StateDuty => {
            let amount = event
                .amount
                .filter(|m| *m != Money::ZERO)
                .map(|m| format!(" · {m}"))
                .unwrap_or_default();
            let label = event
                .deadline
                .map(deadline_fr)
                .unwrap_or(event.title.as_str());
            format!("{label}{amount}")
        }
        MonthEventKind::YearEnd => "Dernier jour — on clôt le lendemain".into(),
        MonthEventKind::MissionEnd => {
            let who = event.party.as_deref().unwrap_or("la mission");
            format!("{who} · le forfait s'arrête")
        }
        MonthEventKind::Milestone => {
            let who = event.party.as_deref().unwrap_or("");
            if who.is_empty() {
                event.title.clone()
            } else {
                format!("{who} · {}", event.title)
            }
        }
        MonthEventKind::Meeting => {
            let who = event.party.as_deref().unwrap_or("");
            if event.title.is_empty() {
                who.to_string()
            } else if who.is_empty() {
                event.title.clone()
            } else {
                format!("{who} · {}", event.title)
            }
        }
        _ => {
            let who = event.party.as_deref().unwrap_or("");
            if who.is_empty() {
                event.title.clone()
            } else {
                format!("{who} · {}", event.title)
            }
        }
    }
}

fn event_href(
    target: &MonthTarget,
    party: Option<&str>,
    deadline: Option<freeflow_core::fiscal::FiscalDeadlineKind>,
) -> Option<String> {
    match target {
        MonthTarget::FollowUp { subject } => Some(party.map_or_else(
            || match subject {
                FollowUpSubject::Opportunity(id) => format!("/gens/{id}"),
                FollowUpSubject::Invoice(id) => format!("/gens/{id}"),
            },
            href_for_party,
        )),
        MonthTarget::Mission { id } => {
            Some(party.map_or_else(|| format!("/gens/{id}"), href_for_party))
        }
        MonthTarget::Invoice { id } => {
            Some(party.map_or_else(|| format!("/gens/{id}"), href_for_party))
        }
        MonthTarget::Taxes => Some(deadline.map_or_else(
            || "/societe/impots".to_string(),
            |k| {
                if k == freeflow_core::fiscal::FiscalDeadlineKind::ApprovalMeeting {
                    "/societe/cloture".to_string()
                } else {
                    duty_href(k)
                }
            },
        )),
        MonthTarget::Closing => Some("/societe/cloture".to_string()),
    }
}
