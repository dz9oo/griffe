//! Le courriel. Le texte reçu est montré tel quel. Le HTML reçu passe dans un cadre fermé.

use griffe_core::mail::{
    BoxCard, BoxPage, BoxVue, ClassChoice, LectureFace, LectureShow, LetterPlace,
};
use maud::{Markup, html};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc2822;

use crate::brand;
use crate::views::copy::month_fr;
use crate::views::gens::person_href;

/// Lecture en cours, déjà traduite en phrases.
pub struct LectureView {
    /// Phrases et mode.
    pub face: LectureFace,
    /// Génération du miroir, emportée par le sondage.
    pub generation: u64,
    /// Lignes qui arrivent dans cette réponse.
    pub fresh: Vec<String>,
    /// La liaison est ouverte.
    pub busy: bool,
}

/// Ce que la page doit garder dans ses liens.
pub struct Keep {
    pub vue: BoxVue,
    pub q: String,
    pub lettre: String,
    pub dossier: String,
}

#[must_use]
pub fn render(
    page: &BoxPage,
    keep: &Keep,
    note: Option<&str>,
    lecture: Option<&LectureView>,
) -> Markup {
    let open_id = page.open.as_ref().map(|letter| letter.id.as_str());
    let fresh = lecture.map(|view| view.fresh.as_slice()).unwrap_or(&[]);
    let empty = empty_line(
        page,
        keep,
        lecture
            .map(|view| view.face.empty)
            .unwrap_or("Aucune lettre dans cette vue."),
    );
    html! {
        div class="boite" data-view="boite" {
            article class="plateau-status" {
                (facts(page, keep, lecture))
            }
            @if let Some(note) = note {
                p class="note" role="alert" { (note) }
            }
            div class="board" {
                (vues(page, keep, false))
                (list(page, keep, open_id, empty, fresh, false))
                (reading(page, keep, lecture.map(|view| &view.face), false))
            }
        }
    }
}

/// Bloc sondé, et la liste ou la colonne seulement si elles changent.
#[must_use]
pub fn poll_fragment(
    page: &BoxPage,
    keep: &Keep,
    lecture: &LectureView,
    refresh_list: bool,
    refresh_column: bool,
) -> Markup {
    let open_id = page.open.as_ref().map(|letter| letter.id.as_str());
    let update_measure = is_glyph(&lecture.face) && !refresh_column;
    html! {
        (facts(page, keep, Some(lecture)))
        @if refresh_list {
            (vues(page, keep, true))
            (list(
                page,
                keep,
                open_id,
                empty_line(page, keep, lecture.face.empty),
                &lecture.fresh,
                true,
            ))
        }
        @if refresh_column {
            (reading(page, keep, Some(&lecture.face), true))
        } @else if update_measure {
            (measure_oob(&lecture.face))
        }
    }
}

fn facts(page: &BoxPage, keep: &Keep, lecture: Option<&LectureView>) -> Markup {
    let Some(lecture) = lecture else {
        return html! {
            (status(page.dossier_letters, page.failed_addresses))
            (kept_line(page))
            (releve_bar(page, keep, false))
        };
    };
    let trigger = if lecture.face.poll {
        "every 1s"
    } else {
        "every 2s"
    };
    let url = lecture_href(keep, lecture);
    html! {
        div id="lecture" hx-get=(url) hx-trigger=(trigger) hx-swap="outerHTML" hx-target="this" {
            (shown_sentence(page, &lecture.face))
            (kept_line(page))
            (releve_bar(page, keep, lecture.busy))
        }
    }
}

fn kept_line(page: &BoxPage) -> Markup {
    let line = if page.kept_addresses.is_empty() {
        "Tout le compte".to_string()
    } else {
        page.kept_addresses.join(" · ")
    };
    html! {
        p class="quiet-line kept-line" { (line) }
    }
}

fn releve_bar(page: &BoxPage, keep: &Keep, busy: bool) -> Markup {
    let hour = page
        .releve_at
        .as_deref()
        .and_then(griffe_core::mail::releve_clock);
    html! {
        p class="quiet-line" aria-live="polite" {
            @if busy {
                "Relève du courrier en cours."
            } @else if let Some(error) = page.releve_error.as_deref() {
                "La relève n'a pas abouti. "
                (error)
                @if let Some(hour) = hour.as_deref() {
                    " Dernière relève à " (hour) "."
                }
            } @else if let Some(hour) = hour.as_deref() {
                "Dernière relève à " (hour) "."
            } @else {
                "La relève n'a pas encore eu lieu."
            }
        }
        p class="quiet-line" {
            a href="/societe/courrier#releve"
              hx-get="/societe/courrier"
              hx-target="#content"
              hx-push-url="/societe/courrier#releve"
              hx-swap="innerHTML" {
                (griffe_core::mail::releve_cadence(page.releve_every_secs))
            }
        }
        form hx-post="/boite/relever" hx-target="#content" hx-swap="innerHTML" {
            input type="hidden" name="vue" value=(keep.vue.as_str());
            input type="hidden" name="q" value=(keep.q);
            input type="hidden" name="lettre" value=(keep.lettre);
            input type="hidden" name="dossier" value=(keep.dossier);
            div class="row-actions" {
                button type="submit" disabled[busy] { "Relever le courrier" }
            }
        }
    }
}

fn empty_line<'a>(page: &BoxPage, keep: &Keep, fallback: &'a str) -> &'a str {
    if keep.vue == BoxVue::Lire
        && page.cards.is_empty()
        && page.counts.tout > 0
        && keep.q.is_empty()
        && keep.dossier.is_empty()
    {
        "Les lettres déjà lues sont dans Tout."
    } else {
        fallback
    }
}

fn shown_sentence(page: &BoxPage, face: &LectureFace) -> Markup {
    match &face.show {
        LectureShow::Glyph {
            sentence, stall, ..
        } => html! {
            p class={ "plateau-sentence" @if *stall { " seal" } } aria-live="polite" { (sentence) }
        },
        LectureShow::Replace { sentence, link } => html! {
            p class="plateau-sentence" aria-live="polite" {
                (sentence)
                @if *link {
                    " "
                    a href="/societe/courrier"
                      hx-get="/societe/courrier"
                      hx-target="#content"
                      hx-push-url="true"
                      hx-swap="innerHTML" { "Le courrier" }
                }
            }
        },
        LectureShow::Quiet { line, seal } => html! {
            (status(page.dossier_letters, page.failed_addresses))
            p class={ "quiet-line" @if *seal { " seal" } } { (line) }
        },
        LectureShow::Rest => status(page.dossier_letters, page.failed_addresses),
    }
}

fn measure(count: &str, meta: &str) -> Markup {
    html! {
        p id="live-count" class="live-count" { (count) }
        p id="live-meta" class="live-meta" { (meta) }
    }
}

fn measure_oob(face: &LectureFace) -> Markup {
    let LectureShow::Glyph { count, meta, .. } = &face.show else {
        return html! {};
    };
    html! {
        p id="live-count" class="live-count" hx-swap-oob="outerHTML" { (count) }
        p id="live-meta" class="live-meta" hx-swap-oob="outerHTML" { (meta) }
    }
}

fn is_glyph(face: &LectureFace) -> bool {
    matches!(face.show, LectureShow::Glyph { .. })
}

fn lecture_href(keep: &Keep, lecture: &LectureView) -> String {
    let col = if is_glyph(&lecture.face) {
        "glyph"
    } else {
        "letter"
    };
    let stall = match &lecture.face.show {
        LectureShow::Glyph { stall: true, .. } => "1",
        _ => "0",
    };
    format!(
        "/boite/lecture?vue={}&q={}&lettre={}&dossier={}&gen={}&mode={}&col={col}&stall={stall}",
        enc(keep.vue.as_str()),
        enc(&keep.q),
        enc(&keep.lettre),
        enc(&keep.dossier),
        lecture.generation,
        lecture.face.mode.as_str(),
    )
}

fn status(letters: u32, failed: u32) -> Markup {
    html! {
        p class="plateau-sentence" {
            @if letters == 0 && failed == 0 {
                "Rien ne demande un regard."
            } @else {
                @if letters == 1 {
                    "Une lettre concerne les dossiers."
                } @else if letters > 1 {
                    (letters) " lettres concernent les dossiers."
                }
                @if letters > 0 && failed > 0 { " " }
                @if failed == 1 {
                    span class="bad" { "Une adresse ne reçoit pas." }
                } @else if failed > 1 {
                    span class="bad" { (failed) " adresses ne reçoivent pas." }
                }
            }
        }
    }
}

fn vues(page: &BoxPage, keep: &Keep, oob: bool) -> Markup {
    let folder_on = !keep.dossier.is_empty();
    html! {
        nav id="boite-vues" class="vues" aria-label="Vues du courriel"
          hx-swap-oob=[oob.then_some("outerHTML")] {
            (vue_link(BoxVue::Lire, "À lire", page.counts.lire, keep, folder_on, false))
            (vue_link(BoxVue::Dossiers, "Avec les dossiers", page.counts.dossiers, keep, folder_on, false))
            (vue_link(BoxVue::Envoyees, "Envoyées", page.counts.envoyees, keep, folder_on, false))
            (vue_link(BoxVue::Tout, "Tout", page.counts.tout, keep, folder_on, false))
            (vue_link(BoxVue::Echec, "En échec", page.counts.echec, keep, folder_on, true))
            details class="server" open[folder_on] {
                summary { "Les dossiers du serveur" }
                ul {
                    @for name in &page.folders {
                        @let current = folder_on && name.eq_ignore_ascii_case(&keep.dossier);
                        li {
                            a href=(href(keep.vue, &keep.q, "", name))
                              hx-get=(href(keep.vue, &keep.q, "", name))
                              hx-target="#content"
                              hx-push-url="true"
                              hx-swap="innerHTML"
                              aria-current=[current.then_some("page")] {
                                (name)
                            }
                        }
                    }
                }
            }
        }
    }
}

fn vue_link(
    vue: BoxVue,
    label: &str,
    count: u32,
    keep: &Keep,
    folder_on: bool,
    echec: bool,
) -> Markup {
    let current = !folder_on && keep.vue == vue;
    let class = if echec { "echec" } else { "" };
    let target = href(vue, &keep.q, "", "");
    html! {
        a class=(class) href=(target)
          hx-get=(target)
          hx-target="#content"
          hx-push-url="true"
          hx-swap="innerHTML"
          aria-current=[current.then_some("page")] {
            (label)
            span class="count" { (count) }
        }
    }
}

fn list(
    page: &BoxPage,
    keep: &Keep,
    open_id: Option<&str>,
    empty: &str,
    fresh: &[String],
    oob: bool,
) -> Markup {
    html! {
        div id="boite-list" class="list" hx-swap-oob=[oob.then_some("outerHTML")] {
            form method="get" action="/boite" {
                input type="hidden" name="vue" value=(keep.vue.as_str());
                input type="hidden" name="lettre" value=(keep.lettre);
                input type="hidden" name="dossier" value=(keep.dossier);
                input class="find" type="search" name="q" value=(keep.q)
                  placeholder="Un nom, un objet"
                  aria-label="Chercher dans la vue"
                  hx-get="/boite"
                  hx-trigger="keyup changed delay:300ms, search"
                  hx-target="#content"
                  hx-push-url="true"
                  hx-include="closest form"
                  hx-swap="innerHTML";
            }
            @if page.cards.is_empty() {
                p class="empty" { (empty) }
            } @else {
                ol class="letters" {
                    @for card in &page.cards {
                        (row(card, keep, open_id, fresh.iter().any(|id| id == &card.id)))
                    }
                }
            }
        }
    }
}

fn row(card: &BoxCard, keep: &Keep, open_id: Option<&str>, arriving: bool) -> Markup {
    let target = href(keep.vue, &keep.q, &card.id, &keep.dossier);
    let current = open_id == Some(card.id.as_str());
    html! {
        li class={
            @if arriving { "arriving" }
            @if card.notice.as_deref() == Some("certain") { " fail" }
            @if card.seen { " read" }
        }
          aria-current=[current.then_some("true")] {
            a class="row" href=(target)
              hx-get=(target)
              hx-target="#content"
              hx-push-url="true"
              hx-swap="innerHTML" {
                span class="who" { (card.who) }
                span class="when" { (when_of(card.sent_on.as_deref())) }
                span class="fact" { (fact(card)) }
                span class="subject" { (card.subject) }
            }
        }
    }
}

fn fact(card: &BoxCard) -> &'static str {
    if card.notice.as_deref() == Some("certain") {
        "Adresse en échec"
    } else if card.role == "sent" {
        "Lettre partie"
    } else if card.on_dossier {
        "Ils ont répondu"
    } else {
        "Pas encore rangée"
    }
}

fn reading(page: &BoxPage, keep: &Keep, face: Option<&LectureFace>, oob: bool) -> Markup {
    if let Some(LectureShow::Glyph {
        count, meta, stall, ..
    }) = face.map(|face| &face.show)
    {
        return html! {
            article id="boite-reading" class="reading live" hx-swap-oob=[oob.then_some("outerHTML")] {
                (brand::reading_glyph(*stall))
                (measure(count, meta))
            }
        };
    }
    let Some(letter) = &page.open else {
        return html! {
            article id="boite-reading" class="reading" hx-swap-oob=[oob.then_some("outerHTML")] {
                p class="empty" { "Rien à ouvrir." }
            }
        };
    };
    let html_frame = letter.html.as_deref().and_then(received_html);
    html! {
        article id="boite-reading" class="reading" hx-swap-oob=[oob.then_some("outerHTML")] {
            (kicker(letter))
            h2 { (letter.subject) }
            p class="meta" { (meta(letter)) }
            @if let Some(address) = letter.dead_address.as_deref() {
                p class="note" {
                    "L'adresse " (address) " ne reçoit pas. L'envoi vers elle est retenu."
                }
            }
            @if let Some(sentence) = crate::releve::body_sentence(&letter.body_state) {
                p class="note" { (sentence) }
            }
            @if letter.body_state == "ready" && !letter.text.trim().is_empty() {
                div class="body" { (letter.text) }
            }
            @if letter.body_state == "ready" {
                @if let Some(document) = html_frame {
                    iframe class="received-frame" sandbox="" title="Le message reçu" srcdoc=(document) {}
                }
            }
            (actions(letter, keep))
        }
    }
}

fn kicker(letter: &griffe_core::mail::BoxLetter) -> Markup {
    match &letter.place {
        LetterPlace::Filed {
            name,
            opportunity_id,
            ..
        } => {
            let href = if opportunity_id.is_some() {
                person_href(name)
            } else {
                format!("{}/fiche", person_href(name))
            };
            html! {
                p class="kicker" {
                    a href=(href) hx-get=(href) hx-target="#content" hx-push-url="true" hx-swap="innerHTML" { (name) }
                }
            }
        }
        LetterPlace::Quiet if letter.role == "sent" => {
            html! { p class="kicker" { "Lettre partie" } }
        }
        LetterPlace::Pick { .. } | LetterPlace::Quiet => {
            html! { p class="kicker" { "Pas de dossier" } }
        }
    }
}

fn meta(letter: &griffe_core::mail::BoxLetter) -> String {
    let who = letter
        .from_name
        .clone()
        .or_else(|| letter.from_address.clone())
        .unwrap_or_else(|| "—".to_string());
    let when = when_of(letter.sent_on.as_deref());
    if letter.role == "sent" {
        format!("De {who} · {when} · copie dans Envoyées")
    } else if let Some(address) = letter
        .from_address
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        format!("De {who} · {address} · {when}")
    } else {
        format!("De {who} · {when}")
    }
}

fn actions(letter: &griffe_core::mail::BoxLetter, keep: &Keep) -> Markup {
    match &letter.place {
        LetterPlace::Quiet if letter.message_id.as_deref().is_none_or(str::is_empty) => {
            html! { p class="soft" { "Ce message n'a pas d'identifiant. Il reste dans le courriel." } }
        }
        LetterPlace::Quiet => html! {},
        LetterPlace::Filed {
            client_id,
            opportunity_id,
            name,
        } => {
            let dossier = person_href(name);
            let fiche = format!("{dossier}/fiche");
            html! {
                div class="actions" {
                    @if letter.dead_address.is_some() {
                        a class="seal" href=(fiche)
                          hx-get=(fiche) hx-target="#content" hx-push-url="true" hx-swap="innerHTML" {
                            "Corriger l'adresse"
                        }
                    }
                    @if opportunity_id.is_some() {
                        a class="quiet" href=(dossier)
                          hx-get=(dossier) hx-target="#content" hx-push-url="true" hx-swap="innerHTML" {
                            "Ouvrir le dossier"
                        }
                    }
                    @if let Some(address) = letter.dead_address.as_deref() {
                        form method="post" action="/boite/adresse"
                          hx-post="/boite/adresse" hx-target="#content" hx-swap="innerHTML" {
                            input type="hidden" name="client_id" value=(client_id);
                            input type="hidden" name="address" value=(address);
                            (hidden_keep(keep, &letter.id))
                            button class="quiet" type="submit" { "Elle est bonne" }
                        }
                    }
                }
            }
        }
        LetterPlace::Pick { choices } => {
            let Some(message_id) = letter.message_id.as_deref() else {
                return html! {
                    p class="soft" { "Ce message n'a pas d'identifiant. Il reste dans le courriel." }
                };
            };
            html! {
                details class="ranger" {
                    summary { "Ranger sur un dossier" }
                    @if choices.is_empty() {
                        p class="soft" { "Aucun dossier ouvert." }
                    } @else {
                        ul {
                            @for choice in choices {
                                li { (class_form(choice, message_id, keep, &letter.id)) }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn class_form(choice: &ClassChoice, message_id: &str, keep: &Keep, letter_id: &str) -> Markup {
    html! {
        form method="post" action="/boite/classer"
          hx-post="/boite/classer" hx-target="#content" hx-swap="innerHTML" {
            input type="hidden" name="message_id" value=(message_id);
            input type="hidden" name="client_id" value=(choice.client_id);
            @if let Some(opportunity) = choice.opportunity_id.as_deref() {
                input type="hidden" name="opportunity_id" value=(opportunity);
            }
            (hidden_keep(keep, letter_id))
            button type="submit" { (choice.label) }
        }
    }
}

fn hidden_keep(keep: &Keep, letter_id: &str) -> Markup {
    html! {
        input type="hidden" name="vue" value=(keep.vue.as_str());
        input type="hidden" name="q" value=(keep.q);
        input type="hidden" name="lettre" value=(letter_id);
        input type="hidden" name="dossier" value=(keep.dossier);
    }
}

fn href(vue: BoxVue, q: &str, lettre: &str, dossier: &str) -> String {
    format!(
        "/boite?vue={}&q={}&lettre={}&dossier={}",
        enc(vue.as_str()),
        enc(q),
        enc(lettre),
        enc(dossier)
    )
}

fn enc(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => {
                out.push(char::from(byte));
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn when_of(raw: Option<&str>) -> String {
    let Some(raw) = raw else {
        return "—".to_string();
    };
    OffsetDateTime::parse(raw, &Rfc2822).map_or_else(
        |_| "—".to_string(),
        |at| format!("{} {}", at.day(), month_fr(u8::from(at.month()))),
    )
}

fn received_html(raw: &str) -> Option<String> {
    let cleaned = strip_received(raw);
    if visible_text(&cleaned).is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

fn visible_text(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        rest = &rest[start + 1..];
        let Some(end) = rest.find('>') else {
            break;
        };
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn strip_received(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut i = 0;
    while i < html.len() {
        if !html.is_char_boundary(i) {
            i += 1;
            continue;
        }
        if !html[i..].starts_with('<') {
            let Some(ch) = html[i..].chars().next() else {
                break;
            };
            out.push(ch);
            i += ch.len_utf8();
            continue;
        }
        if html[i..].starts_with("<!--") {
            i = match html[i..].find("-->") {
                Some(end) => i + end + 3,
                None => html.len(),
            };
            continue;
        }
        let Some(name) = element_name(&html[i..]) else {
            out.push('<');
            i += 1;
            continue;
        };
        if dropped(&name) {
            i += skip_dropped(&html[i..], &name);
            continue;
        }
        if name == "img" && tag_is_remote(&html[i..]) {
            i += tag_len(&html[i..]);
            continue;
        }
        let len = tag_len(&html[i..]);
        out.push_str(&strip_remote_style(&html[i..i + len]));
        i += len;
    }
    out
}

fn dropped(name: &str) -> bool {
    matches!(
        name,
        "script"
            | "style"
            | "iframe"
            | "object"
            | "embed"
            | "link"
            | "meta"
            | "base"
            | "form"
            | "svg"
    )
}

fn void_tag(name: &str) -> bool {
    matches!(
        name,
        "link" | "meta" | "base" | "embed" | "img" | "br" | "hr"
    )
}

fn element_name(html: &str) -> Option<String> {
    let bytes = html.as_bytes();
    if bytes.first() != Some(&b'<') {
        return None;
    }
    let mut i = 1;
    if bytes.get(i) == Some(&b'/') {
        i += 1;
    }
    if bytes.get(i) == Some(&b'!') {
        return None;
    }
    let start = i;
    while bytes.get(i).is_some_and(u8::is_ascii_alphanumeric) {
        i += 1;
    }
    if i == start {
        return None;
    }
    Some(html[start..i].to_ascii_lowercase())
}

fn tag_len(html: &str) -> usize {
    let bytes = html.as_bytes();
    let mut quote: Option<u8> = None;
    for (i, &byte) in bytes.iter().enumerate() {
        if let Some(mark) = quote {
            if byte == mark {
                quote = None;
            }
            continue;
        }
        match byte {
            b'"' | b'\'' => quote = Some(byte),
            b'>' => return i + 1,
            _ => {}
        }
    }
    html.len()
}

fn skip_dropped(html: &str, name: &str) -> usize {
    let open = tag_len(html).max(1);
    let head = &html[..open.min(html.len())];
    if void_tag(name) || head.ends_with("/>") {
        return open;
    }
    let mut depth = 1_u32;
    let mut i = open;
    while i < html.len() {
        if !html[i..].starts_with('<') {
            let Some(ch) = html[i..].chars().next() else {
                break;
            };
            i += ch.len_utf8();
            continue;
        }
        let Some(inner) = element_name(&html[i..]) else {
            i += 1;
            continue;
        };
        let len = tag_len(&html[i..]).max(1);
        let closing = html[i..].starts_with("</");
        if inner == name {
            if closing {
                depth = depth.saturating_sub(1);
                i += len;
                if depth == 0 {
                    return i;
                }
                continue;
            }
            depth = depth.saturating_add(1);
        }
        i += len;
    }
    html.len()
}

fn tag_is_remote(html: &str) -> bool {
    let len = tag_len(html);
    let tag = &html[..len];
    attr_values(tag, "src")
        .chain(attr_values(tag, "srcset"))
        .any(|value| url_is_remote(&value))
}

fn url_is_remote(value: &str) -> bool {
    let lower = value.trim().to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("//")
}

fn attr_values(tag: &str, name: &str) -> impl Iterator<Item = String> {
    let mut found = Vec::new();
    let lower = tag.to_ascii_lowercase();
    let mut search = lower.as_str();
    let mut base = 0;
    while let Some(at) = search.find(name) {
        let abs = base + at;
        let before_ok = abs == 0
            || tag
                .as_bytes()
                .get(abs - 1)
                .is_some_and(|byte| byte.is_ascii_whitespace() || *byte == b'<');
        let after = abs + name.len();
        if before_ok
            && lower
                .as_bytes()
                .get(after)
                .is_some_and(|byte| *byte == b'=')
            && let Some(value) = read_attr(&tag[after + 1..])
        {
            found.push(value);
        }
        let next = at + name.len();
        search = &search[next..];
        base += next;
    }
    found.into_iter()
}

fn read_attr(raw: &str) -> Option<String> {
    let trimmed = raw.trim_start();
    let bytes = trimmed.as_bytes();
    let quote = bytes.first().copied()?;
    if quote == b'"' || quote == b'\'' {
        let rest = &trimmed[1..];
        let end = rest.find(char::from(quote))?;
        return Some(rest[..end].to_string());
    }
    let end = trimmed
        .find(|ch: char| ch.is_ascii_whitespace() || ch == '>')
        .unwrap_or(trimmed.len());
    Some(trimmed[..end].to_string())
}

fn strip_remote_style(tag: &str) -> String {
    let Some((start, end)) = style_span(tag) else {
        return tag.to_string();
    };
    let value = &tag[start..end];
    if style_is_remote(value) {
        let mut out = String::new();
        out.push_str(tag[..start].trim_end());
        out.push_str(&tag[end..]);
        return out;
    }
    tag.to_string()
}

fn style_span(tag: &str) -> Option<(usize, usize)> {
    let lower = tag.to_ascii_lowercase();
    let at = lower.find("style")?;
    let before_ok = at == 0
        || tag
            .as_bytes()
            .get(at - 1)
            .is_some_and(|byte| byte.is_ascii_whitespace());
    if !before_ok {
        return None;
    }
    let after = &lower[at + "style".len()..];
    let eq = after.find('=')?;
    if !after[..eq].trim().is_empty() {
        return None;
    }
    let value_at = at + "style".len() + eq + 1;
    let rest = tag[value_at..].trim_start();
    let skipped = tag[value_at..].len() - rest.len();
    let bytes = rest.as_bytes();
    let quote = *bytes.first()?;
    if quote == b'"' || quote == b'\'' {
        let close = rest[1..].find(char::from(quote))?;
        let start = value_at + skipped;
        return Some((at, start + 1 + close + 1));
    }
    None
}

fn style_is_remote(tag_slice: &str) -> bool {
    let compact: String = tag_slice
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    compact.contains("http://") || compact.contains("https://") || compact.contains("url(//")
}
