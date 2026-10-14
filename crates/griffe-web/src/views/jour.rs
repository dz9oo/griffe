//! Le jour : mât, gestes dépliables, mois. Les faits viennent des queries du cœur (lot 49) ;
//! cette vue ne fait que rédiger le français.

use griffe_core::app::AppError;
use griffe_core::company::company_profile;
use griffe_core::day::{
    DayGesture, DayMark, GestureSource, GestureVerb, Mast, MastSignal, MissionTrack, MonthEvent,
    MonthEventKind, MonthTarget, MonthView, day_gestures, day_mast, day_month,
};
use griffe_core::domain::{
    FiscalYearEnd, FollowUpSubject, Money, Month, SnoozePreset, format_date, format_date_fr,
    parse_date, snooze_date,
};
use griffe_core::people::people_list;
use griffe_core::setup::setup_status;
use griffe_core::store::Store;
use maud::{Markup, html};
use time::{Date, OffsetDateTime, Weekday};

use griffe_core::fiscal::VatFilingScheme;
use griffe_core::society::{VatRefundStatus, duty_briefing};

use crate::layout::ViewId;
use crate::views::copy::{
    deadline_fr, duty_href, duty_occurrence_fr, event_kind_fr, gestes_title, is_vat, month_fr,
    month_title,
};
use crate::views::gens::{href_for_party, person_href};

pub fn render(store: &Store, today: Date) -> Result<Markup, AppError> {
    let conn = store.connection();
    let setup = setup_status(conn)?;
    let mast = day_mast(conn, today)?;
    let people = people_list(conn, today)?;
    let gestes = day_gestures(conn, today)?;
    let month = Month::new(today.year(), u8::from(today.month()))
        .expect("le mois courant est toujours valide");
    let month_view = day_month(conn, month, today)?;
    let vat_scheme = {
        let profile = company_profile(conn).ok().flatten();
        let regime = profile.as_ref().and_then(|p| p.vat_regime);
        let fye = profile
            .as_ref()
            .and_then(|p| p.fiscal_year_end)
            .unwrap_or(FiscalYearEnd::CALENDAR);
        VatFilingScheme::for_exercise(regime, fye.current(today))
    };

    let title = gestes_title(gestes.len());
    let year_end = month_view
        .events
        .iter()
        .find(|e| e.kind == MonthEventKind::YearEnd)
        .map(|e| e.on);
    let lede = letter_lede(gestes.len(), setup.is_done(), today, year_end);
    let note = mast_note(&mast, setup.is_done());
    let alarming = mast.is_alarming();
    let show_lede = gestes.is_empty() || !setup.is_done() || lede.contains("clôture");

    let pile = people.first_messages.len() + people.first_contacts.len();
    let bodies = people.conversations.len();
    let courrier = griffe_core::mail::day_notes(conn, today).unwrap_or_default();
    let pause = griffe_core::mail::hourly_pause(conn, OffsetDateTime::now_utc()).unwrap_or(false);

    Ok(html! {
        div class="pupitre letter spread" data-view=(ViewId::Jour.slug()) {
            div class={ "mast" @if alarming { " alarm" } } {
                p class="pupitre-sentence" { (morning_sentence(gestes.len(), title, &mast)) }
                @if let Some(text) = note {
                    p class="mast-note" id="next-step" { (text) }
                } @else if !setup.is_done() {
                    p class="mast-note" id="next-step" { (setup.next_step.text()) }
                }
            }
            @if pause || !courrier.is_empty() {
                div class="courrier-notes" {
                    @if pause {
                        p { "Trop de lettres cette heure. Celles qui restent attendent." }
                    }
                    @for line in &courrier {
                        p { (line) }
                    }
                }
            }
            @if show_lede {
                p class="lede" { (lede) }
            }
            @if !gestes.is_empty() {
                div class="pupitre-head" {
                    span { "Geste" }
                    span { "Qui" }
                    span { "Prochain pas" }
                    span class="amt" { "Montant" }
                }
                ol class="gestes" {
                    @for g in &gestes {
                        (geste_li(g, store, today, vat_scheme))
                    }
                }
            }
            @if pile > 0 {
                a class="day-others" href="/affaires"
                  hx-get="/affaires" hx-target="#content" hx-push-url="true" {
                    (others_fr(bodies, pile))
                }
            }
            (month_section(&month_view, today, vat_scheme))
        }
    })
}

fn others_fr(bodies: usize, pile: usize) -> String {
    if bodies == 0 {
        return if pile == 1 {
            "une autre conversation".into()
        } else {
            format!("{pile} autres conversations")
        };
    }
    if pile == 1 {
        "et une autre".into()
    } else {
        format!("et {pile} autres")
    }
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
        Some(n) => format!("{n} mois"),
        None => "—".into(),
    }
}

fn morning_sentence(n: usize, title: &str, mast: &Mast) -> Markup {
    let head = in_sentence(title);
    html! {
        @if n > 0 {
            "Ce matin, " (head) " — "
        }
        b { (mast.bank.to_string()) }
        " en banque · "
        b class={ @if runway_is_bad(mast) { "bad" } } { (runway_label(mast.runway_months)) }
        " de piste · "
        b class={ @if receivable_is_bad(mast) { "bad" } } { (encaisser_phrase(mast)) }
    }
}

/// « Quatre gestes. » devient « quatre gestes » dans « Ce matin, … ».
fn in_sentence(title: &str) -> String {
    let head = title.trim_end_matches('.');
    let mut chars = head.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let mut out = first.to_lowercase().collect::<String>();
    out.push_str(chars.as_str());
    out
}

fn encaisser_phrase(mast: &Mast) -> String {
    mast.receivables.first().map_or_else(
        || "rien à encaisser".into(),
        |r| format!("{} chez {}", r.amount, r.party),
    )
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

struct GesteCells {
    verb: String,
    who: String,
    cue: String,
    amount: String,
    alarm: bool,
}

fn amount_cell(amount: Money) -> String {
    if amount == Money::ZERO {
        String::new()
    } else {
        amount.to_string()
    }
}

fn geste_li(g: &DayGesture, store: &Store, today: Date, vat_scheme: VatFilingScheme) -> Markup {
    let cells = geste_cells(g, store, today, vat_scheme);
    html! {
        li {
            button class="geste" type="button" {
                span class="verb" { (cells.verb) }
                span class="qui" { (cells.who) }
                span class="cue" { (cells.cue) }
                span class={ "amt" @if cells.alarm { " alarm" } } { (cells.amount) }
            }
            div class="unfold" {
                div class="row-actions" { (geste_actions(g, today)) }
            }
        }
    }
}

fn geste_cells(
    g: &DayGesture,
    store: &Store,
    today: Date,
    vat_scheme: VatFilingScheme,
) -> GesteCells {
    match &g.source {
        GestureSource::Setup { step } => GesteCells {
            verb: "Configurer ma société".into(),
            who: String::new(),
            cue: step.text().to_string(),
            amount: String::new(),
            alarm: false,
        },
        GestureSource::FollowUp {
            party,
            title,
            drafted,
            block_reason,
            due_on,
            follow_kind,
            amount,
            ..
        } => {
            let verb = match g.verb {
                GestureVerb::Remind => "Relancer",
                _ => "Écrire",
            };
            let mut cue = title.clone();
            if let Some(on) = due_on {
                cue.push_str(" — prévue le ");
                cue.push_str(&format_date_fr(*on));
            }
            if *drafted {
                cue.push_str(". Un brouillon est prêt — il s'ouvre dans ton client mail.");
            } else if let Some(reason) = block_reason {
                cue.push_str(". ");
                cue.push_str(reason);
            } else if *follow_kind == griffe_core::domain::FollowUpKind::Invoice {
                cue.push_str(". Le montant est dans le coffre.");
            }
            GesteCells {
                verb: verb.into(),
                who: party.clone(),
                cue,
                amount: amount_cell(*amount),
                alarm: false,
            }
        }
        GestureSource::BankStatement {
            unmatched,
            deposited,
        } => {
            if !deposited {
                GesteCells {
                    verb: "Déposer le relevé".into(),
                    who: "la banque".into(),
                    cue: "Aucun mouvement n'est encore dans le coffre. L'export de la banque, tel quel.".into(),
                    amount: String::new(),
                    alarm: false,
                }
            } else {
                let cue = if *unmatched == 1 {
                    "Un mouvement n'est pas encore traité.".into()
                } else {
                    format!("{unmatched} mouvements ne sont pas encore traités.")
                };
                GesteCells {
                    verb: "Ranger le relevé".into(),
                    who: "la banque".into(),
                    cue,
                    amount: String::new(),
                    alarm: false,
                }
            }
        }
        GestureSource::StateDuty {
            deadline,
            due_on,
            amount,
            period_key,
        } => {
            let verb = if is_vat(*deadline) {
                "Savoir pour la TVA".into()
            } else {
                format!("Savoir pour {}", deadline_fr(*deadline, vat_scheme))
            };
            let recover = if is_vat(*deadline)
                && let Ok(briefing) =
                    duty_briefing(store.connection(), *deadline, today, Some(period_key))
                && let Some(VatRefundStatus::Offered { credit, .. }) = briefing.vat_refund
            {
                format!("{credit} à récupérer · ")
            } else {
                String::new()
            };
            let shown = amount.filter(|m| *m != Money::ZERO);
            let inline = shown.map(|m| format!("{m} · ")).unwrap_or_default();
            let cue = format!(
                "{recover}{inline}à déposer avant le {}. La lettre dit le chemin — le dépôt se fait sur le site des impôts, pas ici.",
                format_date_fr(*due_on)
            );
            GesteCells {
                verb,
                who: "l'État".into(),
                cue,
                amount: shown.map(amount_cell).unwrap_or_default(),
                alarm: false,
            }
        }
        GestureSource::ReleaseReceivable { party, amount, .. } => GesteCells {
            verb: "Ne plus attendre".into(),
            who: party.clone(),
            cue: "La confirmation se fait sur le dossier.".into(),
            amount: amount.to_string(),
            alarm: true,
        },
        GestureSource::Outgoing {
            party,
            amount,
            matching_debit,
        } => GesteCells {
            verb: "Régler".into(),
            who: party.clone(),
            cue: if *matching_debit {
                "un débit correspond".into()
            } else {
                "une dette d'ouverture".into()
            },
            amount: amount.to_string(),
            alarm: false,
        },
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
            party,
            ..
        } => {
            let href = person_href(party);
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
        GestureSource::StateDuty {
            deadline,
            period_key,
            ..
        } => {
            let href = duty_href(*deadline, Some(period_key.as_str()));
            html! {
                a class="quiet" href=(href)
                  hx-get=(href) hx-target="#content" hx-push-url="true" {
                    "Lire la lettre avant de partir"
                }
            }
        }
        GestureSource::ReleaseReceivable { party, .. } | GestureSource::Outgoing { party, .. } => {
            let href = person_href(party);
            html! {
                a class="seal" href=(href)
                  hx-get=(href) hx-target="#content" hx-push-url="true" {
                    "Ouvrir son dossier"
                }
            }
        }
    }
}

fn month_section(view: &MonthView, today: Date, vat_scheme: VatFilingScheme) -> Markup {
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
                        li { (agenda_row(event, today, vat_scheme)) }
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

fn agenda_row(event: &MonthEvent, today: Date, vat_scheme: VatFilingScheme) -> Markup {
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
    let ttl = agenda_title(event, vat_scheme);
    let href = event_href(
        &event.target,
        event.party.as_deref(),
        event.deadline,
        event.period_key.as_deref(),
    );
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

fn preview_note(note: &str) -> String {
    let line = note.lines().next().unwrap_or("").trim();
    let mut preview: String = line.chars().take(80).collect();
    if line.chars().count() > 80 || note.lines().nth(1).is_some() {
        preview.push('…');
    }
    preview
}

fn agenda_title(event: &MonthEvent, vat_scheme: VatFilingScheme) -> String {
    match event.kind {
        MonthEventKind::StateDuty => {
            let amount = event
                .amount
                .filter(|m| *m != Money::ZERO)
                .map(|m| format!(" · {m}"))
                .unwrap_or_default();
            let label = match event.deadline {
                Some(k) => duty_occurrence_fr(
                    k,
                    vat_scheme,
                    event.period_key.as_deref().unwrap_or(""),
                    event.on,
                ),
                None => event.title.clone(),
            };
            format!("{label}{amount}")
        }
        MonthEventKind::YearEnd => "Dernier jour — on clôt le lendemain".into(),
        MonthEventKind::CloseExercise => {
            let end = event
                .period_key
                .as_deref()
                .and_then(|key| parse_date(key).ok())
                .unwrap_or(event.on);
            format!("Clore l'exercice clos le {}", format_date_fr(end))
        }
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
            let note = preview_note(&event.title);
            if note.is_empty() {
                who.to_string()
            } else if who.is_empty() {
                note
            } else {
                format!("{who} · {note}")
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
    deadline: Option<griffe_core::fiscal::FiscalDeadlineKind>,
    period_key: Option<&str>,
) -> Option<String> {
    match target {
        MonthTarget::FollowUp { subject } => Some(party.map_or_else(
            || match subject {
                FollowUpSubject::Opportunity(id) => format!("/affaires/{id}"),
                FollowUpSubject::Invoice(id) => format!("/affaires/{id}"),
            },
            href_for_party,
        )),
        MonthTarget::Mission { id } => {
            Some(party.map_or_else(|| format!("/affaires/{id}"), href_for_party))
        }
        MonthTarget::Invoice { id } => {
            Some(party.map_or_else(|| format!("/affaires/{id}"), href_for_party))
        }
        MonthTarget::Taxes => Some(deadline.map_or_else(
            || "/societe/impots".to_string(),
            |k| {
                if k == griffe_core::fiscal::FiscalDeadlineKind::ApprovalMeeting {
                    "/societe/cloture".to_string()
                } else {
                    duty_href(k, period_key)
                }
            },
        )),
        MonthTarget::Closing => Some("/societe/cloture".to_string()),
    }
}
