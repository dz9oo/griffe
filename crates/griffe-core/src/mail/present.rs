//! Habit HTML d'une lettre, et le texte qui reste quand le client n'affiche pas le HTML.
//! Aucune IO. Le corps stocké ne change pas : l'habit se calcule à l'envoi.

use super::model::LetterChrome;
use crate::domain::{display_phone, parse_email, tel_href};

/// Phrase sous la zone d'écriture. Une adresse de suivi se tape en entier.
pub const LINK_HINT: &str = "Une adresse suivie de ?utm_… reste cliquable en entier. \
La personne lit l'adresse sans cette partie.";

const SERIF: &str =
    "Georgia,'Iowan Old Style','Palatino Linotype',Palatino,'Times New Roman',serif";
const PAPER: &str = "#f6f1e7";
const MARGIN: &str = "#efe8dc";
const INK: &str = "#1c1814";
const RULE: &str = "#e3d9c8";

/// Ce qu'il faut pour habiller une lettre. Le corps est le texte écrit, formule comprise.
pub struct LetterParts<'a> {
    /// Nom en tête. Vide : pas de nom.
    pub from_name: &'a str,
    /// Sujet, repris dans le titre du document.
    pub subject: &'a str,
    /// Texte de la lettre, tel qu'il part aussi en `text/plain`.
    pub body: &'a str,
    /// Couleur, métier, site, formule.
    pub chrome: &'a LetterChrome,
}

/// HTML de la lettre. Le texte seul reste le corps d'origine, avec les adresses entières.
#[must_use]
pub fn letter_html(letter: &LetterParts<'_>) -> String {
    let body = normalize(letter.body);
    let (prose, _) = peel_signature(&body, &letter.chrome.signature);
    let preheader = first_line(prose);
    let mut html = String::new();
    html.push_str("<!DOCTYPE html><html lang=\"fr\"><head><meta charset=\"utf-8\">");
    html.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">");
    html.push_str("<title>");
    html.push_str(&escape(letter.subject));
    html.push_str("</title></head>");
    html.push_str("<body style=\"margin:0;padding:0;background:");
    html.push_str(MARGIN);
    html.push_str(";\">");
    if !preheader.is_empty() {
        html.push_str("<div style=\"display:none;max-height:0;overflow:hidden;mso-hide:all;\">");
        html.push_str(&escape(preheader));
        html.push_str("</div>");
    }
    html.push_str(&letter_card(letter));
    html.push_str("</body></html>");
    html
}

/// La carte, marge comprise, sans le document. La fenêtre l'insère telle quelle.
#[must_use]
pub fn letter_card(letter: &LetterParts<'_>) -> String {
    let body = normalize(letter.body);
    let (prose, signature) = peel_signature(&body, &letter.chrome.signature);
    let mut html = String::new();
    html.push_str(
        "<table role=\"presentation\" width=\"100%\" cellpadding=\"0\" cellspacing=\"0\" style=\"background:",
    );
    html.push_str(MARGIN);
    html.push_str(
        ";border-collapse:collapse;\"><tr><td align=\"center\" style=\"padding:32px 16px;\">",
    );
    html.push_str(
        "<table role=\"presentation\" width=\"560\" cellpadding=\"0\" cellspacing=\"0\" style=\"width:100%;max-width:560px;background:",
    );
    html.push_str(PAPER);
    html.push_str(";border-collapse:collapse;\"><tr><td style=\"padding:32px 28px;\">");
    header(&mut html, letter.from_name);
    html.push_str("<div style=\"margin:20px 0 0;\">");
    prose_blocks(&mut html, prose, INK, INK);
    html.push_str("</div>");
    if let Some(signature) = signature {
        signature_block(&mut html, signature);
    }
    html.push_str("</td></tr></table></td></tr></table>");
    html
}

/// Ajoute la formule quand le texte ne l'a pas déjà. Une formule vide ne change rien.
#[must_use]
pub fn close_letter(body: &str, signature: &str) -> String {
    let signature = normalize(signature).trim().to_string();
    let body = normalize(body);
    if signature.is_empty() || signature_in_body(body.trim(), &signature) {
        return body;
    }
    let trimmed = body.trim();
    if trimmed.is_empty() {
        signature
    } else {
        format!("{trimmed}\n\n{signature}")
    }
}

/// Le même texte, avec chaque adresse de suivi écrite sans ses paramètres `utm_`.
/// Sert à l'aperçu. Le texte envoyé aux clients sans HTML garde l'adresse entière.
#[must_use]
pub fn readable_links(text: &str) -> String {
    let mut out = String::new();
    for piece in pieces(&normalize(text)) {
        match piece {
            Piece::Gap(gap) => out.push_str(&gap),
            Piece::Url(url) => out.push_str(&without_utm(&url)),
        }
    }
    out
}

fn header(html: &mut String, from_name: &str) {
    let name = from_name.trim();
    if name.is_empty() {
        return;
    }
    html.push_str("<p style=\"margin:0;font-family:");
    html.push_str(SERIF);
    html.push_str(";font-size:22px;line-height:1.2;letter-spacing:-0.02em;color:");
    html.push_str(INK);
    html.push_str(";\">");
    html.push_str(&escape(name));
    html.push_str("</p>");
    html.push_str("<p style=\"margin:16px 0 0;border-top:1px solid ");
    html.push_str(RULE);
    html.push_str(";font-size:0;line-height:0;\">&nbsp;</p>");
}

/// La formule, dans la même encre et le même serif que la lettre. Un mail, un
/// numéro ou une adresse, seuls sur leur ligne, restent cliquables, sans
/// soulignement et sans autre couleur. La première ligne de texte est un peu
/// plus présente.
fn signature_block(html: &mut String, signature: &str) {
    let mut started = false;
    let mut named = false;
    for line in signature.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if !started {
            html.push_str("<div style=\"margin:28px 0 0;\">");
            started = true;
        }
        let strong =
            !named && whole_url(line).is_none() && !whole_email(line) && tel_href(line).is_none();
        if strong {
            named = true;
        }
        signature_line(html, line, strong);
    }
    if started {
        html.push_str("</div>");
    }
}

fn signature_line(html: &mut String, line: &str, strong: bool) {
    html.push_str("<p style=\"margin:");
    html.push_str(if strong { "0" } else { "4px 0 0" });
    html.push_str(";font-family:");
    html.push_str(SERIF);
    html.push_str(";font-size:");
    html.push_str(if strong { "17px" } else { "16px" });
    html.push_str(";line-height:1.45;color:");
    html.push_str(INK);
    if strong {
        html.push_str(";font-weight:600");
    }
    html.push_str(";\">");
    if let Some(url) = whole_url(line) {
        html.push_str(&anchor(&url, &without_utm(&url), INK));
    } else if whole_email(line) {
        html.push_str(&anchor(&format!("mailto:{line}"), line, INK));
    } else if let Some(href) = tel_href(line) {
        html.push_str(&anchor(&href, &display_phone(line), INK));
    } else {
        linked_line(html, line, INK);
    }
    html.push_str("</p>");
}

fn whole_url(line: &str) -> Option<String> {
    let line = line.trim();
    if !(line.starts_with("https://") || line.starts_with("http://"))
        || line.chars().any(char::is_whitespace)
    {
        return None;
    }
    let chars: Vec<char> = line.chars().collect();
    let (url, next) = take_url(&chars, 0);
    let trailing = chars[next..]
        .iter()
        .all(|ch| matches!(ch, '.' | ',' | ';' | ':' | '!' | '?' | ')' | ']'));
    (!url.is_empty() && trailing).then_some(url)
}

fn whole_email(line: &str) -> bool {
    parse_email(line).is_ok()
}

fn signature_in_body(body: &str, signature: &str) -> bool {
    if body == signature {
        return true;
    }
    if let Some(rest) = body.strip_suffix(signature) {
        return rest.is_empty() || rest.ends_with('\n');
    }
    body.contains(&format!("\n{signature}\n"))
}

fn prose_blocks(html: &mut String, text: &str, text_ink: &str, link_ink: &str) {
    let mut started = false;
    for para in text.split("\n\n") {
        if para.trim().is_empty() {
            continue;
        }
        if started {
            html.push_str("<p style=\"margin:16px 0 0;font-family:");
        } else {
            html.push_str("<p style=\"margin:0;font-family:");
        }
        started = true;
        html.push_str(SERIF);
        html.push_str(";font-size:16px;line-height:1.55;color:");
        html.push_str(text_ink);
        html.push_str(";\">");
        let mut first_line = true;
        for line in para.split('\n') {
            if !first_line {
                html.push_str("<br>");
            }
            first_line = false;
            linked_line(html, line, link_ink);
        }
        html.push_str("</p>");
    }
}

fn linked_line(html: &mut String, line: &str, ink: &str) {
    for piece in pieces(line) {
        match piece {
            Piece::Gap(gap) => html.push_str(&escape(&gap)),
            Piece::Url(url) => html.push_str(&anchor(&url, &without_utm(&url), ink)),
        }
    }
}

fn anchor(href: &str, label: &str, ink: &str) -> String {
    let mut out = String::new();
    out.push_str("<a href=\"");
    out.push_str(&escape(href));
    out.push_str("\" style=\"color:");
    out.push_str(ink);
    out.push_str(";text-decoration:none;\">");
    out.push_str(&escape(label));
    out.push_str("</a>");
    out
}

fn normalize(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn first_line(text: &str) -> &str {
    text.lines()
        .find(|line| !line.trim().is_empty())
        .map_or("", str::trim)
}

fn peel_signature<'a>(body: &'a str, signature: &str) -> (&'a str, Option<&'a str>) {
    let signature = signature.trim();
    if signature.is_empty() {
        return (body, None);
    }
    let trimmed = body.trim_end();
    let Some(head) = trimmed.strip_suffix(signature) else {
        return (body, None);
    };
    let signature_in_body = &trimmed[head.len()..];
    (head.trim_end(), Some(signature_in_body))
}

/// Adresse affichée : le même lien, sans les paramètres dont le nom commence par `utm_`.
fn without_utm(url: &str) -> String {
    let (main, fragment) = match url.split_once('#') {
        Some((main, fragment)) => (main, Some(fragment)),
        None => (url, None),
    };
    let Some((path, query)) = main.split_once('?') else {
        return url.to_string();
    };
    let kept: Vec<&str> = query
        .split('&')
        .filter(|part| !part.is_empty())
        .filter(|part| {
            let name = part.split_once('=').map_or(*part, |(name, _)| name);
            !name.to_ascii_lowercase().starts_with("utm_")
        })
        .collect();
    let mut label = path.to_string();
    if !kept.is_empty() {
        label.push('?');
        label.push_str(&kept.join("&"));
    }
    if let Some(fragment) = fragment {
        label.push('#');
        label.push_str(fragment);
    }
    label
}

enum Piece {
    Gap(String),
    Url(String),
}

fn pieces(text: &str) -> Vec<Piece> {
    let chars: Vec<char> = text.chars().collect();
    let mut found = Vec::new();
    let mut index = 0;
    let mut gap_at = 0;
    while index < chars.len() {
        if url_at(&chars, index) {
            if gap_at < index {
                found.push(Piece::Gap(chars[gap_at..index].iter().collect()));
            }
            let (url, next) = take_url(&chars, index);
            found.push(Piece::Url(url));
            index = next;
            gap_at = index;
        } else {
            index += 1;
        }
    }
    if gap_at < chars.len() {
        found.push(Piece::Gap(chars[gap_at..].iter().collect()));
    }
    found
}

fn url_at(chars: &[char], index: usize) -> bool {
    let scheme = starts_at(chars, index, "https://") || starts_at(chars, index, "http://");
    if !scheme {
        return false;
    }
    index == 0 || chars[index - 1].is_whitespace() || !chars[index - 1].is_ascii_alphanumeric()
}

fn starts_at(chars: &[char], index: usize, prefix: &str) -> bool {
    let mut rest = chars.iter().skip(index).copied();
    prefix.chars().all(|expected| rest.next() == Some(expected))
}

fn take_url(chars: &[char], index: usize) -> (String, usize) {
    let mut end = index;
    while end < chars.len() {
        let ch = chars[end];
        if ch.is_whitespace() || matches!(ch, '<' | '>' | '"' | '\'') {
            break;
        }
        end += 1;
    }
    while end > index
        && matches!(
            chars[end - 1],
            '.' | ',' | ';' | ':' | '!' | '?' | ')' | ']'
        )
    {
        end -= 1;
    }
    (chars[index..end].iter().collect(), end)
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{close_letter, letter_html, readable_links, without_utm};
    use crate::mail::{LetterChrome, LetterInk, LetterParts};

    const TRACKED: &str = "https://atelier.example/conformite?utm_source=mairie&utm_medium=email&utm_campaign=automne-2026";

    #[test]
    fn a_tracking_address_is_read_without_its_utm_parameters() {
        assert_eq!(without_utm(TRACKED), "https://atelier.example/conformite");
        assert_eq!(
            without_utm("https://atelier.example/doc?page=2&utm_source=email&x=1#part"),
            "https://atelier.example/doc?page=2&x=1#part"
        );
        assert_eq!(
            without_utm("https://atelier.example/conformite"),
            "https://atelier.example/conformite"
        );
        let sentence = format!("Le dossier est là : {TRACKED}.");
        let read = readable_links(&sentence);
        assert!(read.contains("https://atelier.example/conformite."));
        assert!(!read.contains("utm_"));
    }

    #[test]
    fn the_html_keeps_the_full_address_and_escapes_the_prose() {
        let chrome = LetterChrome {
            ink: LetterInk::Vert,
            metier: "Ingénieur logiciel".into(),
            site: "https://atelier.example".into(),
            signature: "Bien à vous,\nCamille".into(),
        };
        let body = format!(
            "Bonjour,\n\nLe dossier : {TRACKED}.\n\n<script>alert(1)</script>\n\nBien à vous,\nCamille"
        );
        let html = letter_html(&LetterParts {
            from_name: "Camille",
            subject: "Conformité",
            body: &body,
            chrome: &chrome,
        });
        assert!(html.contains("href=\"https://atelier.example/conformite?utm_source=mairie&amp;utm_medium=email&amp;utm_campaign=automne-2026\""));
        assert!(html.contains(">https://atelier.example/conformite</a>"));
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script>"));
        assert!(!html.contains("<img"));
        assert!(!html.contains("@import"));
        assert!(!html.contains("Ingénieur logiciel"));
        assert!(!html.contains("#3e6b34"));
        assert!(!html.contains("https://atelier.example</a>"));
        assert!(html.contains("text-decoration:none"));
        assert!(!html.contains("text-decoration:underline"));
        assert_eq!(html.matches("Bien à vous").count(), 1);
        assert!(html.contains("Bonjour"));
    }

    #[test]
    fn the_formula_closes_once_and_its_lines_become_links() {
        let signature = "\
Nicolas COLLIER
Ingénieur logiciel indépendant
hello@atelier.example
06 70 12 32 60
https://atelier.example/conformite?utm_source=mairie&utm_medium=email&utm_campaign=automne-2026";
        let closed = close_letter("Bonjour Camille,\n\nLe dossier avance.", signature);
        assert!(closed.ends_with(signature));
        assert_eq!(close_letter(&closed, signature), closed);
        assert_eq!(close_letter("Bonjour.", ""), "Bonjour.");

        let chrome = LetterChrome {
            ink: LetterInk::Sceau,
            metier: "Ingénieur logiciel indépendant".into(),
            site: "https://atelier.example/conformite".into(),
            signature: signature.into(),
        };
        let html = letter_html(&LetterParts {
            from_name: "Nicolas Collier",
            subject: "Essai",
            body: &closed,
            chrome: &chrome,
        });
        assert_eq!(html.matches("Ingénieur logiciel indépendant").count(), 1);
        assert_eq!(
            html.matches("https://atelier.example/conformite").count(),
            2
        );
        assert!(html.contains("href=\"mailto:hello@atelier.example\""));
        assert!(html.contains("href=\"tel:+33670123260\""));
        assert!(html.contains(">06 70 12 32 60</a>"));
        assert!(html.contains("href=\"https://atelier.example/conformite?utm_source=mairie&amp;utm_medium=email&amp;utm_campaign=automne-2026\""));
        assert!(html.contains(">https://atelier.example/conformite</a>"));
        assert!(!html.contains("#9f3218"));
        assert!(!html.contains("text-transform:uppercase"));
        assert!(html.contains("text-decoration:none"));
        assert!(!html.contains("text-decoration:underline"));
        assert!(html.contains("font-weight:600"));
        assert!(!html.contains("color:#0000ee"));
        let middle = format!("Avant.\n\n{signature}\n\nAprès.");
        assert_eq!(close_letter(&middle, signature), middle);
    }

    #[test]
    fn javascript_and_a_glued_word_stay_text() {
        let read = readable_links("voir javascript:alert(1) et voirhttps://atelier.example/x");
        assert!(read.contains("javascript:alert(1)"));
        assert!(read.contains("voirhttps://atelier.example/x"));
        assert!(!read.contains("href"));
    }
}
