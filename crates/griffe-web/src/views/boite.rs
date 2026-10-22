//! La boîte. Le texte reçu est montré tel quel. Le HTML reçu passe dans un cadre fermé.

use griffe_core::mail::{BoxCard, BoxPage, BoxVue, ClassChoice, LetterPlace};
use maud::{Markup, html};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc2822;

use crate::views::copy::month_fr;
use crate::views::gens::person_href;

/// Ce que la page doit garder dans ses liens.
pub struct Keep {
    pub vue: BoxVue,
    pub q: String,
    pub lettre: String,
    pub dossier: String,
}

#[must_use]
pub fn render(page: &BoxPage, keep: &Keep, note: Option<&str>) -> Markup {
    let open_id = page.open.as_ref().map(|letter| letter.id.as_str());
    html! {
        div class="boite" data-view="boite" {
            article class="plateau-status" {
                (status(page.dossier_letters, page.failed_addresses))
            }
            @if let Some(note) = note {
                p class="note" role="alert" { (note) }
            }
            div class="board" {
                (vues(page, keep))
                (list(page, keep, open_id))
                (reading(page, keep))
            }
        }
    }
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

fn vues(page: &BoxPage, keep: &Keep) -> Markup {
    let folder_on = !keep.dossier.is_empty();
    html! {
        nav class="vues" aria-label="Vues de la boîte" {
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

fn list(page: &BoxPage, keep: &Keep, open_id: Option<&str>) -> Markup {
    html! {
        div class="list" {
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
                p class="empty" { "Aucune lettre dans cette vue." }
            } @else {
                ol class="letters" {
                    @for card in &page.cards {
                        (row(card, keep, open_id))
                    }
                }
            }
        }
    }
}

fn row(card: &BoxCard, keep: &Keep, open_id: Option<&str>) -> Markup {
    let target = href(keep.vue, &keep.q, &card.id, &keep.dossier);
    let current = open_id == Some(card.id.as_str());
    html! {
        li class={ @if card.notice.as_deref() == Some("certain") { "fail" } @if card.seen { " read" } }
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

fn reading(page: &BoxPage, keep: &Keep) -> Markup {
    let Some(letter) = &page.open else {
        return html! { article class="reading" { p class="empty" { "Rien à ouvrir." } } };
    };
    let html_frame = letter.html.as_deref().and_then(received_html);
    html! {
        article class="reading" {
            (kicker(letter))
            h2 { (letter.subject) }
            p class="meta" { (meta(letter)) }
            @if let Some(address) = letter.dead_address.as_deref() {
                p class="note" {
                    "L'adresse " (address) " ne reçoit pas. L'envoi vers elle est retenu."
                }
            }
            @if !letter.text.trim().is_empty() {
                div class="body" { (letter.text) }
            }
            @if let Some(document) = html_frame {
                iframe class="received-frame" sandbox="" title="Le message reçu" srcdoc=(document) {}
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
            html! { p class="soft" { "Ce message n'a pas d'identifiant. Il reste dans la boîte." } }
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
                    p class="soft" { "Ce message n'a pas d'identifiant. Il reste dans la boîte." }
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
