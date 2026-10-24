//! Parties utiles d'un message IMAP. Le texte, le HTML, et les images qui ont
//! un `Content-ID`. Les pièces jointes ne sont pas demandées.

use griffe_core::mail::BODY_CAP_BYTES;

pub(crate) struct KeptSection {
    pub section: String,
    pub octets: u64,
}

pub(crate) enum KeptBody {
    Parts(Vec<KeptSection>),
    TooLarge,
    Unreadable,
}

pub(crate) struct KeptPiece {
    pub mime: String,
    pub body: String,
}

/// Ce qu'il faut lire. Le total des parties gardées ne dépasse pas 15 Mo.
pub(crate) fn kept_from_fetch(line: &str) -> KeptBody {
    let upper = line.to_ascii_uppercase();
    let Some(at) = upper.find("BODYSTRUCTURE") else {
        return KeptBody::Unreadable;
    };
    let rest = &line[at + "BODYSTRUCTURE".len()..];
    let Some(start) = rest.find('(') else {
        return KeptBody::Unreadable;
    };
    let mut cursor = Cursor {
        bytes: &rest.as_bytes()[start..],
        index: 0,
    };
    let Some(node) = parse_node(&mut cursor) else {
        return KeptBody::Unreadable;
    };
    let mut plain = false;
    let mut html = false;
    let mut parts = Vec::new();
    match &node {
        Node::Leaf { .. } => collect(&node, "1", &mut plain, &mut html, &mut parts),
        Node::Multi { .. } => collect(&node, "", &mut plain, &mut html, &mut parts),
    }
    let sum = parts
        .iter()
        .fold(0_u64, |acc, part| acc.saturating_add(part.octets));
    if parts.iter().any(|part| part.octets > BODY_CAP_BYTES) || sum > BODY_CAP_BYTES {
        KeptBody::TooLarge
    } else {
        KeptBody::Parts(parts)
    }
}

/// Message RFC 822 réduit aux parties gardées. Le HTML y retrouve ses images.
pub(crate) fn assemble_kept(header: &str, pieces: &[KeptPiece]) -> String {
    let mut out = rewrite_header(header);
    for piece in pieces {
        out.push_str("--griffe-kept\r\n");
        out.push_str(piece.mime.trim_end_matches(['\r', '\n']));
        out.push_str("\r\n\r\n");
        out.push_str(piece.body.trim_end_matches(['\r', '\n']));
        out.push_str("\r\n");
    }
    out.push_str("--griffe-kept--\r\n");
    out
}

/// Valeurs `BODY[...]` d'une ligne FETCH, dans l'ordre, littéral ou chaîne.
pub(crate) fn fetched_parts(line: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut index = 0;
    while index < line.len() {
        if !line[index..].starts_with(b"BODY[") {
            index += 1;
            continue;
        }
        let Some(close) = line[index..].iter().position(|byte| *byte == b']') else {
            break;
        };
        index += close + 1;
        while index < line.len() && line[index].is_ascii_whitespace() {
            index += 1;
        }
        if index < line.len() && line[index] == b'{' {
            let Some(end) = line[index..].iter().position(|byte| *byte == b'}') else {
                break;
            };
            let number = std::str::from_utf8(&line[index + 1..index + end]).unwrap_or("");
            let Ok(len) = number.parse::<usize>() else {
                break;
            };
            let start = index + end + 1;
            let stop = start.saturating_add(len);
            if stop > line.len() {
                break;
            }
            out.push(line[start..stop].to_vec());
            index = stop;
            continue;
        }
        if index < line.len() && line[index] == b'"' {
            index += 1;
            let start = index;
            while index < line.len() && line[index] != b'"' {
                if line[index] == b'\\' && index + 1 < line.len() {
                    index += 2;
                    continue;
                }
                index += 1;
            }
            out.push(line[start..index.min(line.len())].to_vec());
            if index < line.len() {
                index += 1;
            }
            continue;
        }
        if line[index..].starts_with(b"NIL") {
            out.push(Vec::new());
            index += 3;
        }
    }
    out
}

enum Node {
    Leaf {
        typ: String,
        sub: String,
        id: Option<String>,
        octets: u64,
    },
    Multi {
        children: Vec<Node>,
    },
}

fn collect(
    node: &Node,
    section: &str,
    plain: &mut bool,
    html: &mut bool,
    out: &mut Vec<KeptSection>,
) {
    match node {
        Node::Leaf {
            typ,
            sub,
            id,
            octets,
        } => {
            let kind = format!("{}/{}", typ.to_ascii_lowercase(), sub.to_ascii_lowercase());
            let image = kind.starts_with("image/")
                && id
                    .as_deref()
                    .is_some_and(|value| !cid_token(value).is_empty());
            let keep = if kind == "text/plain" && !*plain {
                *plain = true;
                true
            } else if kind == "text/html" && !*html {
                *html = true;
                true
            } else {
                image
            };
            if keep {
                out.push(KeptSection {
                    section: section.to_string(),
                    octets: *octets,
                });
            }
        }
        Node::Multi { children } => {
            for (index, child) in children.iter().enumerate() {
                let number = index + 1;
                let next = if section.is_empty() {
                    number.to_string()
                } else {
                    format!("{section}.{number}")
                };
                collect(child, &next, plain, html, out);
            }
        }
    }
}

fn cid_token(raw: &str) -> String {
    raw.trim()
        .trim_matches(|ch| ch == '<' || ch == '>')
        .trim()
        .to_string()
}

fn rewrite_header(raw: &str) -> String {
    let raw = raw.replace("\r\n", "\n").replace('\r', "\n");
    let mut out = String::new();
    let mut skip = false;
    for line in raw.split('\n') {
        if line.is_empty() {
            break;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            if !skip {
                out.push_str(line);
                out.push_str("\r\n");
            }
            continue;
        }
        let name = line.split(':').next().unwrap_or("").trim();
        skip = name.eq_ignore_ascii_case("content-type")
            || name.eq_ignore_ascii_case("content-transfer-encoding")
            || name.eq_ignore_ascii_case("mime-version");
        if !skip {
            out.push_str(line);
            out.push_str("\r\n");
        }
    }
    out.push_str("MIME-Version: 1.0\r\n");
    out.push_str("Content-Type: multipart/mixed; boundary=\"griffe-kept\"\r\n");
    out.push_str("\r\n");
    out
}

struct Cursor<'a> {
    bytes: &'a [u8],
    index: usize,
}

impl Cursor<'_> {
    fn skip_ws(&mut self) {
        while self
            .bytes
            .get(self.index)
            .is_some_and(|byte| byte.is_ascii_whitespace())
        {
            self.index += 1;
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.skip_ws();
        self.bytes.get(self.index).copied()
    }

    fn eat(&mut self, byte: u8) -> bool {
        if self.peek() == Some(byte) {
            self.index += 1;
            true
        } else {
            false
        }
    }

    fn atom(&mut self) -> Option<Atom> {
        self.skip_ws();
        if self.eat(b'"') {
            let mut text = String::new();
            while let Some(byte) = self.bytes.get(self.index).copied() {
                self.index += 1;
                if byte == b'\\' {
                    if let Some(next) = self.bytes.get(self.index).copied() {
                        text.push(char::from(next));
                        self.index += 1;
                    }
                    continue;
                }
                if byte == b'"' {
                    break;
                }
                text.push(char::from(byte));
            }
            return Some(Atom::Text(text));
        }
        let start = self.index;
        while self
            .bytes
            .get(self.index)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
        {
            self.index += 1;
        }
        if start == self.index {
            return None;
        }
        let text = std::str::from_utf8(&self.bytes[start..self.index]).ok()?;
        if text.eq_ignore_ascii_case("NIL") {
            Some(Atom::Nil)
        } else {
            Some(Atom::Text(text.to_string()))
        }
    }

    fn skip_field(&mut self) -> bool {
        if self.peek() == Some(b'(') {
            self.skip_list()
        } else {
            self.atom().is_some()
        }
    }

    fn skip_list(&mut self) -> bool {
        if !self.eat(b'(') {
            return false;
        }
        while self.peek().is_some_and(|byte| byte != b')') {
            if !self.skip_field() {
                return false;
            }
        }
        self.eat(b')')
    }
}

enum Atom {
    Nil,
    Text(String),
}

/// En-têtes MIME et corps. Un corps binaire part en base64, pour que le HTML
/// puisse réécrire `cid:` sans perdre les octets.
pub(crate) fn piece_of(mime: &[u8], body: &[u8]) -> KeptPiece {
    match std::str::from_utf8(body) {
        Ok(text) => KeptPiece {
            mime: String::from_utf8_lossy(mime).into_owned(),
            body: text.to_string(),
        },
        Err(_) => KeptPiece {
            mime: force_base64(&String::from_utf8_lossy(mime)),
            body: encode_base64(body),
        },
    }
}

fn force_base64(mime: &str) -> String {
    let mime = mime.replace("\r\n", "\n").replace('\r', "\n");
    let mut out = String::new();
    let mut saw = false;
    let mut skip = false;
    for line in mime.split('\n') {
        if line.is_empty() {
            continue;
        }
        if line.starts_with([' ', '\t']) {
            if !skip {
                out.push_str(line);
                out.push_str("\r\n");
            }
            continue;
        }
        let name = line.split(':').next().unwrap_or("").trim();
        if name.eq_ignore_ascii_case("content-transfer-encoding") {
            skip = true;
            saw = true;
            out.push_str("Content-Transfer-Encoding: base64\r\n");
            continue;
        }
        skip = false;
        out.push_str(line);
        out.push_str("\r\n");
    }
    if !saw {
        out.push_str("Content-Transfer-Encoding: base64\r\n");
    }
    out
}

pub(crate) fn encode_base64(bytes: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    let mut index = 0;
    while index < bytes.len() {
        let first = u32::from(bytes[index]);
        let second = bytes.get(index + 1).copied().map(u32::from);
        let third = bytes.get(index + 2).copied().map(u32::from);
        let packed = (first << 16) | (second.unwrap_or(0) << 8) | third.unwrap_or(0);
        out.push(base64_digit((packed >> 18) & 63, TABLE));
        out.push(base64_digit((packed >> 12) & 63, TABLE));
        if second.is_some() {
            out.push(base64_digit((packed >> 6) & 63, TABLE));
        } else {
            out.push('=');
        }
        if third.is_some() {
            out.push(base64_digit(packed & 63, TABLE));
        } else {
            out.push('=');
        }
        index += 3;
    }
    out
}

fn base64_digit(value: u32, table: &[u8]) -> char {
    let index = usize::try_from(value & 63).unwrap_or(0);
    char::from(table[index])
}

fn parse_node(cursor: &mut Cursor<'_>) -> Option<Node> {
    if !cursor.eat(b'(') {
        return None;
    }
    if cursor.peek() == Some(b'(') {
        let mut children = Vec::new();
        while cursor.peek() == Some(b'(') {
            children.push(parse_node(cursor)?);
        }
        if !matches!(cursor.atom(), Some(Atom::Text(_))) {
            return None;
        }
        while cursor.peek().is_some_and(|byte| byte != b')') {
            if !cursor.skip_field() {
                return None;
            }
        }
        if !cursor.eat(b')') {
            return None;
        }
        return Some(Node::Multi { children });
    }
    let Atom::Text(typ) = cursor.atom()? else {
        return None;
    };
    let Atom::Text(sub) = cursor.atom()? else {
        return None;
    };
    if !cursor.skip_field() {
        return None;
    }
    let id = match cursor.atom()? {
        Atom::Nil => None,
        Atom::Text(text) => Some(text),
    };
    cursor.atom()?;
    cursor.atom()?;
    let Atom::Text(octets) = cursor.atom()? else {
        return None;
    };
    let octets = octets.parse().ok()?;
    while cursor.peek().is_some_and(|byte| byte != b')') {
        if !cursor.skip_field() {
            return None;
        }
    }
    if !cursor.eat(b')') {
        return None;
    }
    Some(Node::Leaf {
        typ,
        sub,
        id,
        octets,
    })
}

#[cfg(test)]
mod tests {
    use super::{KeptBody, KeptPiece, assemble_kept, fetched_parts, kept_from_fetch, piece_of};
    use griffe_core::mail::{BODY_CAP_BYTES, read_message};

    #[test]
    fn an_attachment_is_skipped_and_the_cid_image_is_kept() {
        let line = "* 1 FETCH (UID 4 BODYSTRUCTURE (((\"TEXT\" \"HTML\" (\"CHARSET\" \"UTF-8\") NIL NIL \"7BIT\" 40 1)(\"IMAGE\" \"PNG\" NIL \"<logo@x>\" NIL \"BASE64\" 12) \"RELATED\" NIL NIL NIL)(\"APPLICATION\" \"PDF\" NIL NIL NIL \"BASE64\" 5000) \"MIXED\" NIL NIL NIL))";
        let KeptBody::Parts(parts) = kept_from_fetch(line) else {
            panic!("parties");
        };
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].section, "1.1");
        assert_eq!(parts[1].section, "1.2");
        let raw = assemble_kept(
            "From: Camille <camille@riviere.test>\r\nSubject: Bonjour\r\nContent-Type: multipart/mixed; boundary=old\r\n\r\n",
            &[
                KeptPiece {
                    mime: "Content-Type: text/html; charset=utf-8\r\n".into(),
                    body: "<p>Bonjour</p><img src=\"cid:logo@x\">".into(),
                },
                KeptPiece {
                    mime: "Content-Type: image/png\r\nContent-Transfer-Encoding: base64\r\nContent-ID: <logo@x>\r\n".into(),
                    body: "iVBORw0KGgo=".into(),
                },
            ],
        );
        assert!(!raw.contains("application/pdf"));
        let html = read_message(&raw).html.unwrap();
        assert!(html.contains("Bonjour"), "{html}");
        assert!(html.contains("data:image/png;base64,"), "{html}");
        assert!(!html.to_ascii_lowercase().contains("cid:logo"), "{html}");
    }

    #[test]
    fn a_useful_part_over_the_cap_is_refused_and_a_file_is_not() {
        let heavy = BODY_CAP_BYTES + 1;
        let line = format!(
            "* 1 FETCH (UID 4 BODYSTRUCTURE (\"TEXT\" \"HTML\" NIL NIL NIL \"7BIT\" {heavy} 1))"
        );
        assert!(matches!(kept_from_fetch(&line), KeptBody::TooLarge));
        let with_file = format!(
            "* 1 FETCH (UID 4 BODYSTRUCTURE ((\"TEXT\" \"HTML\" NIL NIL NIL \"7BIT\" 20 1)(\"APPLICATION\" \"PDF\" NIL NIL NIL \"BASE64\" {heavy}) \"MIXED\" NIL NIL NIL))"
        );
        let KeptBody::Parts(parts) = kept_from_fetch(&with_file) else {
            panic!("le fichier ne compte pas");
        };
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].section, "1");
        let pdf_only =
            "* 1 FETCH (UID 4 BODYSTRUCTURE (\"APPLICATION\" \"PDF\" NIL NIL NIL \"BASE64\" 20))";
        let KeptBody::Parts(empty) = kept_from_fetch(pdf_only) else {
            panic!("un fichier seul n'est pas illisible");
        };
        assert!(empty.is_empty());
    }

    #[test]
    fn a_fetch_line_yields_each_part_in_order() {
        let header = b"Subject: Hi";
        let mime = b"Content-Type: text/html\r\n";
        let body = b"<p>Bon</p>";
        let mut line = format!("* 1 FETCH (UID 4 BODY[HEADER] {{{}}}", header.len()).into_bytes();
        line.extend(header);
        line.extend(format!(" BODY[1.MIME] {{{}}}", mime.len()).into_bytes());
        line.extend(mime);
        line.extend(format!(" BODY[1] {{{}}}", body.len()).into_bytes());
        line.extend(body);
        line.push(b')');
        let parts = fetched_parts(&line);
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0], b"Subject: Hi");
        assert_eq!(parts[1], b"Content-Type: text/html\r\n");
        assert_eq!(parts[2], b"<p>Bon</p>");
    }

    #[test]
    fn a_binary_image_is_carried_as_base64() {
        let png = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        let piece = piece_of(
            b"Content-Type: image/png\r\nContent-Transfer-Encoding: 8bit\r\nContent-ID: <logo@x>\r\n",
            &png,
        );
        assert!(piece.mime.to_ascii_lowercase().contains("base64"));
        assert!(!piece.mime.to_ascii_lowercase().contains("8bit"));
        assert_eq!(piece.body, "iVBORw0KGgo=");
        let raw = assemble_kept(
            "From: Camille <camille@riviere.test>\r\nSubject: Bonjour\r\n\r\n",
            &[
                KeptPiece {
                    mime: "Content-Type: text/html; charset=utf-8\r\n".into(),
                    body: "<p>Bonjour</p><img src=\"cid:logo@x\">".into(),
                },
                piece,
            ],
        );
        let html = read_message(&raw).html.unwrap();
        assert!(
            html.contains("data:image/png;base64,iVBORw0KGgo="),
            "{html}"
        );
        assert!(!html.to_ascii_lowercase().contains("cid:logo"), "{html}");
    }
}
