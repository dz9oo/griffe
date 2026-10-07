//! Rendu du récit des travaux. Le markdown devient du HTML échappé : pas de script,
//! pas d'image à aller chercher, pas de lien qui quitte la fenêtre.

use pulldown_cmark::{Event, Parser, Tag, TagEnd, html};

/// HTML du récit. Les balises du texte source restent du texte. Une image disparaît.
/// Un lien s'écrit en toutes lettres, adresse comprise, sans balise `<a>`.
#[must_use]
pub fn render_work_markdown(source: &str) -> String {
    let events = rewrite(Parser::new(source));
    let mut out = String::new();
    html::push_html(&mut out, events.into_iter());
    out
}

fn rewrite<'a>(parser: Parser<'a>) -> Vec<Event<'a>> {
    let mut out = Vec::new();
    let mut image_depth = 0_i32;
    let mut link_url: Option<String> = None;
    let mut link_shows_url = false;
    for event in parser {
        if image_depth > 0 {
            match event {
                Event::Start(Tag::Image { .. }) => image_depth += 1,
                Event::End(TagEnd::Image) => image_depth -= 1,
                _ => {}
            }
            continue;
        }
        match event {
            Event::Start(Tag::Image { .. }) => image_depth = 1,
            Event::Start(Tag::Link { dest_url, .. }) => {
                link_url = Some(dest_url.to_string());
                link_shows_url = false;
            }
            Event::End(TagEnd::Link) => {
                if let Some(url) = link_url.take()
                    && !link_shows_url
                {
                    out.push(Event::Text(format!(" ({url})").into()));
                }
                link_shows_url = false;
            }
            Event::Html(raw) | Event::InlineHtml(raw) => {
                if link_url.as_ref().is_some_and(|url| raw.contains(url)) {
                    link_shows_url = true;
                }
                out.push(Event::Text(raw));
            }
            Event::Text(text) => {
                if link_url.as_ref().is_some_and(|url| text.contains(url)) {
                    link_shows_url = true;
                }
                out.push(Event::Text(text));
            }
            Event::Code(text) => {
                if link_url.as_ref().is_some_and(|url| text.contains(url)) {
                    link_shows_url = true;
                }
                out.push(Event::Code(text));
            }
            other => out.push(other),
        }
    }
    out
}
