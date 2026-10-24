//! HTML reçu, réduit à ce que la lettre peut montrer. Aucune IO.
//! La politique du document interdit les scripts. Le filtre retire en plus
//! ce qui ne doit pas partir vers le réseau.

/// Images, polices et `url()` hors du message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteContent {
    /// Rien ne sort du message. Les images `data:` restent.
    Blocked,
    /// `http` et `https` peuvent se charger.
    Allowed,
}

const POLICY_BLOCKED: &str = "default-src 'none'; script-src 'none'; style-src 'unsafe-inline'; \
img-src data:; font-src data:; connect-src 'none'; form-action 'none'; base-uri 'none'";

const POLICY_ALLOWED: &str = "default-src 'none'; script-src 'none'; style-src 'unsafe-inline'; \
img-src data: http: https:; font-src data: http: https:; connect-src 'none'; \
form-action 'none'; base-uri 'none'";

/// Politique du document de la lettre. Elle ne s'applique qu'à ce document.
#[must_use]
pub const fn received_policy(remote: RemoteContent) -> &'static str {
    match remote {
        RemoteContent::Blocked => POLICY_BLOCKED,
        RemoteContent::Allowed => POLICY_ALLOWED,
    }
}

/// Fragment montrable. `None` : il ne reste ni texte ni image.
#[must_use]
pub fn received_fragment(html: &str, remote: RemoteContent) -> Option<String> {
    let cleaned = sanitize(html, remote);
    if showable(&cleaned) {
        Some(cleaned)
    } else {
        None
    }
}

/// Document complet, politique comprise dans l'en-tête.
#[must_use]
pub fn received_document(fragment: &str, remote: RemoteContent) -> String {
    let policy = received_policy(remote);
    format!(
        "<!DOCTYPE html><html lang=\"fr\"><head><meta charset=\"utf-8\"><meta http-equiv=\"Content-Security-Policy\" content=\"{policy}\"></head><body>{fragment}</body></html>"
    )
}

fn showable(html: &str) -> bool {
    if !visible_text(html).is_empty() {
        return true;
    }
    let lower = html.to_ascii_lowercase();
    (lower.contains("<img") || lower.contains("<source")) && lower.contains("src=\"")
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

fn sanitize(html: &str, remote: RemoteContent) -> String {
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
        if html[i..].starts_with("<!") {
            i = match html[i..].find('>') {
                Some(end) => i + end + 1,
                None => html.len(),
            };
            continue;
        }
        let Some(tag) = parse_tag(&html[i..]) else {
            out.push('<');
            i += 1;
            continue;
        };
        if tag.name == "style" && !tag.closing {
            let rest = &html[i + tag.len..];
            let (css, consumed) = take_style_body(rest);
            let clean = sanitize_css(&css, remote);
            if !clean.trim().is_empty() {
                out.push_str("<style>");
                out.push_str(&clean);
                out.push_str("</style>");
            }
            i += tag.len + consumed;
            continue;
        }
        if dropped(&tag.name) {
            i += skip_element(&html[i..], &tag);
            continue;
        }
        if unwraps(&tag.name) {
            i += tag.len;
            continue;
        }
        if let Some(piece) = rewrite_tag(&tag, remote) {
            out.push_str(&piece);
        }
        i += tag.len;
    }
    out
}

fn dropped(name: &str) -> bool {
    matches!(
        name,
        "script"
            | "iframe"
            | "object"
            | "embed"
            | "link"
            | "meta"
            | "base"
            | "form"
            | "svg"
            | "title"
            | "applet"
            | "frame"
            | "frameset"
    )
}

fn unwraps(name: &str) -> bool {
    matches!(name, "html" | "head" | "body")
}

fn void_name(name: &str) -> bool {
    matches!(
        name,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

struct Tag {
    name: String,
    closing: bool,
    self_closing: bool,
    len: usize,
    attrs: Vec<Attr>,
}

struct Attr {
    name: String,
    value: Option<String>,
}

fn parse_tag(html: &str) -> Option<Tag> {
    let bytes = html.as_bytes();
    if bytes.first() != Some(&b'<') {
        return None;
    }
    let mut i = 1;
    let mut closing = false;
    if bytes.get(i) == Some(&b'/') {
        closing = true;
        i += 1;
    }
    let start = i;
    while bytes.get(i).is_some_and(u8::is_ascii_alphanumeric) {
        i += 1;
    }
    if i == start {
        return None;
    }
    let name = html[start..i].to_ascii_lowercase();
    let mut attrs = Vec::new();
    let mut self_closing = false;
    while i < bytes.len() {
        while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        match bytes.get(i).copied() {
            Some(b'>') => {
                i += 1;
                break;
            }
            Some(b'/') if bytes.get(i + 1) == Some(&b'>') => {
                self_closing = true;
                i += 2;
                break;
            }
            Some(_) => {}
            None => break,
        }
        let name_at = i;
        while bytes.get(i).is_some_and(|byte| is_attr_name(*byte)) {
            i += 1;
        }
        if i == name_at {
            i += 1;
            continue;
        }
        let attr_name = html[name_at..i].to_ascii_lowercase();
        while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        if bytes.get(i) == Some(&b'=') {
            i += 1;
            while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
                i += 1;
            }
            let value = read_attr_value(html, &mut i);
            attrs.push(Attr {
                name: attr_name,
                value: Some(value),
            });
        } else {
            attrs.push(Attr {
                name: attr_name,
                value: None,
            });
        }
    }
    Some(Tag {
        name,
        closing,
        self_closing,
        len: i.max(1),
        attrs,
    })
}

fn is_attr_name(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b':' | b'_')
}

fn read_attr_value(html: &str, i: &mut usize) -> String {
    let bytes = html.as_bytes();
    let Some(quote) = bytes.get(*i).copied() else {
        return String::new();
    };
    if quote == b'"' || quote == b'\'' {
        *i += 1;
        let start = *i;
        while bytes.get(*i).is_some_and(|byte| *byte != quote) {
            *i += 1;
        }
        let value = html[start..*i].to_string();
        if bytes.get(*i) == Some(&quote) {
            *i += 1;
        }
        return value;
    }
    let start = *i;
    while bytes
        .get(*i)
        .is_some_and(|byte| !byte.is_ascii_whitespace() && *byte != b'>')
    {
        *i += 1;
    }
    html[start..*i].to_string()
}

fn skip_element(html: &str, tag: &Tag) -> usize {
    if tag.closing || tag.self_closing || void_name(&tag.name) {
        return tag.len;
    }
    let mut depth = 1_u32;
    let mut i = tag.len;
    while i < html.len() && depth > 0 {
        if !html[i..].starts_with('<') {
            let Some(ch) = html[i..].chars().next() else {
                break;
            };
            i += ch.len_utf8();
            continue;
        }
        let Some(inner) = parse_tag(&html[i..]) else {
            i += 1;
            continue;
        };
        if inner.name == tag.name {
            if inner.closing {
                depth = depth.saturating_sub(1);
            } else if !inner.self_closing && !void_name(&tag.name) {
                depth = depth.saturating_add(1);
            }
        }
        i += inner.len.max(1);
    }
    i
}

fn take_style_body(rest: &str) -> (String, usize) {
    let Some(at) = find_close(rest, "style") else {
        return (rest.to_string(), rest.len());
    };
    let close = parse_tag(&rest[at..]).map_or(0, |tag| tag.len);
    (rest[..at].to_string(), at + close)
}

fn find_close(html: &str, name: &str) -> Option<usize> {
    let needle = format!("</{name}");
    let lower = html.to_ascii_lowercase();
    let mut search = 0;
    while let Some(at) = lower[search..].find(&needle) {
        let abs = search + at;
        let after = abs + needle.len();
        let boundary = lower.as_bytes().get(after).copied();
        if boundary.is_none_or(|byte| !byte.is_ascii_alphanumeric()) {
            return Some(abs);
        }
        search = abs + needle.len();
    }
    None
}

fn rewrite_tag(tag: &Tag, remote: RemoteContent) -> Option<String> {
    if tag.closing {
        return Some(format!("</{}>", tag.name));
    }
    let attrs = sanitize_attrs(&tag.attrs, &tag.name, remote);
    if replaced(&tag.name) && !has_source(&attrs) {
        return None;
    }
    let mut out = format!("<{}", tag.name);
    for attr in &attrs {
        out.push(' ');
        out.push_str(&attr.name);
        if let Some(value) = &attr.value {
            out.push_str("=\"");
            out.push_str(&escape_attr(value));
            out.push('"');
        }
    }
    if tag.self_closing && !void_name(&tag.name) {
        out.push_str(" /");
    }
    out.push('>');
    Some(out)
}

fn replaced(name: &str) -> bool {
    matches!(name, "img" | "source" | "image")
}

fn has_source(attrs: &[Attr]) -> bool {
    attrs
        .iter()
        .any(|attr| matches!(attr.name.as_str(), "src" | "srcset") && attr.value.is_some())
}

fn sanitize_attrs(attrs: &[Attr], element: &str, remote: RemoteContent) -> Vec<Attr> {
    let mut kept = Vec::new();
    for attr in attrs {
        if let Some(next) = keep_attr(attr, element, remote) {
            kept.push(next);
        }
    }
    if element == "a" && kept.iter().any(|attr| attr.name == "href") {
        kept.retain(|attr| !matches!(attr.name.as_str(), "target" | "rel"));
        kept.push(Attr {
            name: "target".to_string(),
            value: Some("_blank".to_string()),
        });
        kept.push(Attr {
            name: "rel".to_string(),
            value: Some("noopener noreferrer".to_string()),
        });
    }
    kept
}

fn keep_attr(attr: &Attr, element: &str, remote: RemoteContent) -> Option<Attr> {
    if event_handler(&attr.name) || attr.name == "srcdoc" {
        return None;
    }
    let Some(raw) = attr.value.as_deref() else {
        return Some(Attr {
            name: attr.name.clone(),
            value: None,
        });
    };
    let decoded = decode_entities(raw);
    if attr.name == "style" {
        let css = sanitize_css(&decoded, remote);
        let trimmed = css.trim();
        if trimmed.is_empty() || !trimmed.chars().any(|ch| ch.is_ascii_alphanumeric()) {
            return None;
        }
        return Some(Attr {
            name: attr.name.clone(),
            value: Some(css),
        });
    }
    if is_script_url(&decoded) {
        return None;
    }
    if matches!(attr.name.as_str(), "src" | "poster" | "background")
        || (attr.name == "href" && replaced(element))
    {
        return resource_value(&decoded, remote).map(|value| Attr {
            name: attr.name.clone(),
            value: Some(value),
        });
    }
    if attr.name == "srcset" {
        return srcset_value(&decoded, remote).map(|value| Attr {
            name: attr.name.clone(),
            value: Some(value),
        });
    }
    if attr.name == "href" {
        if !href_kept(&decoded) {
            return None;
        }
        return Some(Attr {
            name: attr.name.clone(),
            value: Some(normalize_href(&decoded)),
        });
    }
    Some(Attr {
        name: attr.name.clone(),
        value: Some(decoded),
    })
}

fn event_handler(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() > 2 && bytes.starts_with(b"on") && bytes[2].is_ascii_alphabetic()
}

fn resource_value(value: &str, remote: RemoteContent) -> Option<String> {
    let compact = compact(value);
    if is_script_url(&compact) {
        return None;
    }
    if compact.starts_with("data:image/") || compact.starts_with("cid:") {
        return Some(value.trim().to_string());
    }
    if remote == RemoteContent::Allowed && remote_url(&compact) {
        return Some(normalize_remote(value.trim()));
    }
    None
}

fn srcset_value(value: &str, remote: RemoteContent) -> Option<String> {
    if is_script_url(value) {
        return None;
    }
    if remote == RemoteContent::Blocked && contains_remote(value) {
        return None;
    }
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn href_kept(value: &str) -> bool {
    let compact = compact(value);
    if is_script_url(&compact) {
        return false;
    }
    compact.starts_with("https://")
        || compact.starts_with("http://")
        || compact.starts_with("mailto:")
        || compact.starts_with("tel:")
        || compact.starts_with('#')
        || compact.starts_with("//")
}

fn normalize_href(value: &str) -> String {
    let trimmed = value.trim();
    if compact(trimmed).starts_with("//") {
        normalize_remote(trimmed)
    } else {
        trimmed.to_string()
    }
}

fn normalize_remote(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.starts_with("//") {
        format!("https:{trimmed}")
    } else {
        trimmed.to_string()
    }
}

fn remote_url(compact_value: &str) -> bool {
    compact_value.starts_with("http://")
        || compact_value.starts_with("https://")
        || compact_value.starts_with("//")
}

fn contains_remote(value: &str) -> bool {
    let lower = compact(value);
    lower.contains("http://") || lower.contains("https://") || scheme_relative(value)
}

fn scheme_relative(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.windows(2).enumerate().any(|(index, pair)| {
        pair == b"//"
            && index > 0
            && matches!(
                bytes[index - 1],
                b' ' | b'\t' | b'\n' | b',' | b'(' | b'\'' | b'"'
            )
            || pair == b"//" && index == 0
    })
}

fn is_script_url(value: &str) -> bool {
    let compact = compact(value);
    compact.starts_with("javascript:") || compact.starts_with("vbscript:")
}

fn compact(value: &str) -> String {
    value
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase()
}

fn sanitize_css(css: &str, remote: RemoteContent) -> String {
    let mut out = String::with_capacity(css.len());
    let mut i = 0;
    while i < css.len() {
        if !css.is_char_boundary(i) {
            i += 1;
            continue;
        }
        if css[i..].starts_with("/*") {
            i = match css[i..].find("*/") {
                Some(end) => i + end + 2,
                None => css.len(),
            };
            continue;
        }
        if at_word(css, i, "@import") {
            i += skip_semi(&css[i..]);
            continue;
        }
        if at_word(css, i, "-moz-binding") || at_property(css, i, "behavior") {
            i += skip_semi(&css[i..]);
            continue;
        }
        if at_function(css, i, "expression") {
            i += skip_call(&css[i..]);
            continue;
        }
        if at_function(css, i, "url") {
            let (len, keep) = take_url(&css[i..], remote);
            if keep {
                out.push_str(&css[i..i + len]);
            }
            i += len.max(1);
            continue;
        }
        let Some(ch) = css[i..].chars().next() else {
            break;
        };
        out.push(ch);
        i += ch.len_utf8();
    }
    out.replace("</", "<\\/")
}

fn at_word(css: &str, i: usize, word: &str) -> bool {
    let rest = &css[i..];
    if rest.len() < word.len() || !rest[..word.len()].eq_ignore_ascii_case(word) {
        return false;
    }
    let prev_ok = i == 0 || !is_ident(css.as_bytes()[i - 1]);
    let next_ok = rest
        .as_bytes()
        .get(word.len())
        .is_none_or(|byte| !is_ident(*byte));
    prev_ok && next_ok
}

fn is_ident(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'
}

fn at_function(css: &str, i: usize, word: &str) -> bool {
    if !at_word(css, i, word) {
        return false;
    }
    css[i + word.len()..].trim_start().starts_with('(')
}

fn at_property(css: &str, i: usize, word: &str) -> bool {
    if !at_word(css, i, word) {
        return false;
    }
    let rest = css[i + word.len()..].trim_start();
    rest.starts_with(':')
}

fn skip_semi(css: &str) -> usize {
    let mut quote: Option<u8> = None;
    for (index, byte) in css.bytes().enumerate() {
        if let Some(mark) = quote {
            if byte == mark {
                quote = None;
            }
            continue;
        }
        match byte {
            b'"' | b'\'' => quote = Some(byte),
            b';' => return index + 1,
            _ => {}
        }
    }
    css.len()
}

fn skip_call(css: &str) -> usize {
    let Some(open) = css.find('(') else {
        return css.len();
    };
    let mut depth = 0_i32;
    let mut quote: Option<u8> = None;
    for (index, byte) in css.bytes().enumerate().skip(open) {
        if let Some(mark) = quote {
            if byte == mark {
                quote = None;
            }
            continue;
        }
        match byte {
            b'"' | b'\'' => quote = Some(byte),
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return index + 1;
                }
            }
            _ => {}
        }
    }
    css.len()
}

fn take_url(css: &str, remote: RemoteContent) -> (usize, bool) {
    let len = skip_call(css);
    let Some(open) = css.find('(') else {
        return (len, false);
    };
    let end = len.saturating_sub(1).max(open + 1);
    let inner = css.get(open + 1..end).unwrap_or("");
    let decoded = decode_entities(inner);
    let trimmed = decoded
        .trim()
        .trim_matches(|ch| ch == '"' || ch == '\'')
        .trim();
    (len, resource_value(trimmed, remote).is_some())
}

fn escape_attr(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '<' => out.push_str("&lt;"),
            _ => out.push(ch),
        }
    }
    out
}

fn decode_entities(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        if let Some(end) = after.find(';')
            && let Some(ch) = entity_char(&after[..end])
        {
            out.push(ch);
            rest = &after[end + 1..];
            continue;
        }
        out.push('&');
        rest = after;
    }
    out.push_str(rest);
    out
}

fn entity_char(body: &str) -> Option<char> {
    match body {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some('\u{00a0}'),
        _ => numeric_entity(body),
    }
}

fn numeric_entity(body: &str) -> Option<char> {
    let digits = body.strip_prefix('#')?;
    let code = if let Some(hex) = digits
        .strip_prefix('x')
        .or_else(|| digits.strip_prefix('X'))
    {
        u32::from_str_radix(hex, 16).ok()?
    } else {
        digits.parse().ok()?
    };
    char::from_u32(code).filter(|ch| *ch != '\0')
}

#[cfg(test)]
mod tests {
    use super::{RemoteContent, received_document, received_fragment, received_policy};

    #[test]
    fn a_pixel_leaves_the_sentence_and_the_color() {
        let raw = "<style>p{color:#112233;background:url(https://pixel.evil.test/c.png)}</style>\
<p style=\"color:#112233;background:url(https://pixel.evil.test/b.png)\">Bonjour Camille</p>\
<img src=\"https://pixel.evil.test/a.png\"><script>pixel.evil.test</script>";
        let fragment = received_fragment(raw, RemoteContent::Blocked).expect("fragment");
        assert!(fragment.contains("Bonjour Camille"), "{fragment}");
        assert!(fragment.contains("#112233"), "{fragment}");
        assert!(!fragment.contains("pixel.evil.test"), "{fragment}");
        assert!(!fragment.contains("<script"), "{fragment}");
        assert!(!fragment.contains("<img"), "{fragment}");
        let document = received_document(&fragment, RemoteContent::Blocked);
        assert!(document.contains("img-src data:"), "{document}");
        assert!(!document.contains("https:"), "{document}");
    }

    #[test]
    fn an_image_only_letter_has_no_fragment_while_remote_content_is_blocked() {
        let raw = "<p><img src=\"https://pixel.evil.test/a.png\"></p>";
        assert_eq!(received_fragment(raw, RemoteContent::Blocked), None);
    }

    #[test]
    fn a_data_image_stays_when_remote_content_is_blocked() {
        let raw = "<img src=\"data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7\" alt=\"point\">";
        let fragment = received_fragment(raw, RemoteContent::Blocked).expect("fragment");
        assert!(fragment.contains("data:image/gif"), "{fragment}");
    }

    #[test]
    fn remote_images_stay_only_when_they_are_allowed() {
        let raw = "<img src=\"https://pixel.evil.test/a.png\" alt=\"marque\"><p>Bonjour</p>";
        let blocked = received_fragment(raw, RemoteContent::Blocked).expect("texte");
        assert!(!blocked.contains("pixel.evil.test"), "{blocked}");
        let allowed = received_fragment(raw, RemoteContent::Allowed).expect("fragment");
        assert!(
            allowed.contains("https://pixel.evil.test/a.png"),
            "{allowed}"
        );
        assert!(
            received_policy(RemoteContent::Allowed).contains("https:"),
            "{}",
            received_policy(RemoteContent::Allowed)
        );
    }

    #[test]
    fn a_script_link_is_removed_and_a_real_link_opens_beside() {
        let raw = "<a href=\"javascript:pixel.evil.test\">ici</a>\
<a href=\"https://atelier.example/note\" target=\"_top\" onclick=\"pixel.evil.test\">la note</a>";
        let fragment = received_fragment(raw, RemoteContent::Blocked).expect("fragment");
        assert!(!fragment.contains("javascript:"), "{fragment}");
        assert!(!fragment.contains("onclick"), "{fragment}");
        assert!(!fragment.contains("_top"), "{fragment}");
        assert!(
            fragment.contains("https://atelier.example/note"),
            "{fragment}"
        );
        assert!(fragment.contains("target=\"_blank\""), "{fragment}");
        assert!(fragment.contains(">ici</a>"), "{fragment}");
    }

    #[test]
    fn an_encoded_image_address_is_still_remote() {
        let raw = "<img src=\"https&#58;//pixel.evil.test/a.png\"><p>Bonjour</p>";
        let fragment = received_fragment(raw, RemoteContent::Blocked).expect("fragment");
        assert!(!fragment.contains("pixel.evil.test"), "{fragment}");
    }

    #[test]
    fn an_import_and_a_binding_leave_the_style() {
        let raw = "<style>@import url(https://pixel.evil.test/a.css);p{color:#112233;-moz-binding:url(https://pixel.evil.test/b.xml);}</style><p>Bonjour</p>";
        let fragment = received_fragment(raw, RemoteContent::Blocked).expect("fragment");
        assert!(fragment.contains("#112233"), "{fragment}");
        assert!(!fragment.contains("pixel.evil.test"), "{fragment}");
        assert!(!fragment.contains("@import"), "{fragment}");
    }
}
