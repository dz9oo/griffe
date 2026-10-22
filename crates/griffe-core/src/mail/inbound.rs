//! Lecture d'un message et rattachement à un dossier. Aucune IO, aucune socket.
//! Le texte est préféré au HTML. Un avis de non-remise certain se lit dans la
//! structure RFC 3464, pas dans l'objet seul.

use crate::domain::parse_email;

/// Lettre déjà partie, reconnue par son `Message-ID`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownLetter {
    pub message_id: String,
    pub client_id: String,
    pub opportunity_id: Option<String>,
}

/// Adresse portée par une fiche.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FicheAddress {
    pub client_id: String,
    pub address: String,
}

/// Avis de non-remise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    /// `Action: failed` et un destinataire final.
    Certain { address: String },
    /// L'objet le suggère, la structure ne le confirme pas.
    Probable,
}

/// Où poser le message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DossierLink {
    One {
        client_id: String,
        opportunity_id: Option<String>,
    },
    /// Plusieurs fiches portent l'adresse. On ne choisit pas.
    Choice {
        client_ids: Vec<String>,
    },
    Unlinked,
}

/// Ce que le cœur retient d'un message, sans son corps dans un journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filing {
    pub link: DossierLink,
    /// Vrai pour une réponse humaine rattachée à une seule fiche.
    pub hold: bool,
    pub notice: Option<Notice>,
}

/// Message lu. Les identifiants sont normalisés, sans chevrons.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedMessage {
    pub message_id: Option<String>,
    pub referenced: Vec<String>,
    pub from_name: Option<String>,
    pub from_address: Option<String>,
    pub subject: String,
    pub text: String,
    pub html: Option<String>,
    pub auto_submitted: bool,
    pub notice: Option<Notice>,
    /// En-tête `Date`, tel qu'il est écrit. Absent s'il manque.
    pub sent_on: Option<String>,
    /// `Content-Type: multipart/report`. L'avis certain attend le corps.
    pub delivery_report: bool,
}

/// Lit un message RFC 822. Le HTML n'est gardé que s'il est présent.
#[must_use]
pub fn read_message(raw: &str) -> ParsedMessage {
    let raw = raw.replace("\r\n", "\n").replace('\r', "\n");
    let (header_text, body) = split_headers(&raw);
    let headers = unfold(header_text);
    let from =
        header(&headers, "from").map_or((None, None), |value| address_of(&decode_words(value)));
    let top_type = header(&headers, "content-type").unwrap_or_default();
    let report = top_type.to_ascii_lowercase().contains("multipart/report");
    let parts = explode(&headers, body);
    let (text, html) = plain_and_html(&parts);
    let notice = notice_of(
        &parts,
        report,
        header(&headers, "subject").unwrap_or_default(),
    );
    let mut referenced = Vec::new();
    for name in ["in-reply-to", "references"] {
        if let Some(value) = header(&headers, name) {
            push_ids(&mut referenced, value);
        }
    }
    for part in &parts {
        let kind = header(&part.headers, "content-type").unwrap_or_default();
        if kind.to_ascii_lowercase().contains("message/rfc822") {
            let (nested_headers, _) = split_headers(&part.body);
            if let Some(value) = header(&unfold(nested_headers), "message-id") {
                push_ids(&mut referenced, value);
            }
        }
    }
    ParsedMessage {
        message_id: header(&headers, "message-id").and_then(first_id),
        referenced,
        from_name: from.0,
        from_address: from.1,
        subject: decode_words(header(&headers, "subject").unwrap_or_default()),
        text: text.trim().to_string(),
        html: html
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
        auto_submitted: auto_submitted(header(&headers, "auto-submitted")),
        notice,
        sent_on: header(&headers, "date")
            .map(decode_words)
            .filter(|value| !value.is_empty()),
        delivery_report: report,
    }
}

/// Rattache le message. Une adresse partagée reste un choix.
#[must_use]
pub fn classify(
    message: &ParsedMessage,
    known: &[KnownLetter],
    fiches: &[FicheAddress],
    own_addresses: &[&str],
) -> Filing {
    if let Some(Notice::Certain { address }) = &message.notice {
        return Filing {
            link: link_address(address, message, known, fiches),
            hold: false,
            notice: message.notice.clone(),
        };
    }
    if let Some(letter) = message
        .message_id
        .as_deref()
        .and_then(|id| find_known(known, id))
    {
        return Filing {
            link: link_known(letter),
            hold: false,
            notice: message.notice.clone(),
        };
    }
    if let Some(letter) = message
        .referenced
        .iter()
        .find_map(|id| find_known(known, id))
    {
        let human = is_human(message, own_addresses);
        return Filing {
            link: link_known(letter),
            hold: human && letter.opportunity_id.is_some(),
            notice: message.notice.clone(),
        };
    }
    let Some(address) = message.from_address.as_deref() else {
        return Filing {
            link: DossierLink::Unlinked,
            hold: false,
            notice: message.notice.clone(),
        };
    };
    let matched = fiches_for(fiches, address);
    let link = link_matched(&matched, None);
    let hold = is_human(message, own_addresses) && matches!(link, DossierLink::One { .. });
    Filing {
        link,
        hold,
        notice: message.notice.clone(),
    }
}

fn link_address(
    address: &str,
    message: &ParsedMessage,
    known: &[KnownLetter],
    fiches: &[FicheAddress],
) -> DossierLink {
    let matched = fiches_for(fiches, address);
    let known_letter = message
        .referenced
        .iter()
        .find_map(|id| find_known(known, id));
    if matched.is_empty() {
        return known_letter.map_or(DossierLink::Unlinked, link_known);
    }
    let opportunity_id = known_letter.and_then(|letter| {
        matched
            .iter()
            .any(|fiche| fiche.client_id == letter.client_id)
            .then(|| letter.opportunity_id.clone())
            .flatten()
    });
    link_matched(&matched, opportunity_id)
}

fn link_known(letter: &KnownLetter) -> DossierLink {
    DossierLink::One {
        client_id: letter.client_id.clone(),
        opportunity_id: letter.opportunity_id.clone(),
    }
}

fn link_matched(matched: &[&FicheAddress], opportunity_id: Option<String>) -> DossierLink {
    match matched {
        [] => DossierLink::Unlinked,
        [one] => DossierLink::One {
            client_id: one.client_id.clone(),
            opportunity_id,
        },
        many => DossierLink::Choice {
            client_ids: many.iter().map(|fiche| fiche.client_id.clone()).collect(),
        },
    }
}

fn fiches_for<'a>(fiches: &'a [FicheAddress], address: &str) -> Vec<&'a FicheAddress> {
    let mut matched = Vec::new();
    for fiche in fiches {
        if fiche.address.eq_ignore_ascii_case(address)
            && matched
                .iter()
                .all(|seen: &&FicheAddress| seen.client_id != fiche.client_id)
        {
            matched.push(fiche);
        }
    }
    matched
}

fn find_known<'a>(known: &'a [KnownLetter], id: &str) -> Option<&'a KnownLetter> {
    let needle = normalize_id(id);
    known
        .iter()
        .find(|letter| normalize_id(&letter.message_id) == needle)
}

fn is_human(message: &ParsedMessage, own_addresses: &[&str]) -> bool {
    if message.auto_submitted || message.notice.is_some() {
        return false;
    }
    let Some(from) = message.from_address.as_deref() else {
        return false;
    };
    let local = from.split('@').next().unwrap_or_default();
    if local.eq_ignore_ascii_case("mailer-daemon") || local.eq_ignore_ascii_case("postmaster") {
        return false;
    }
    !own_addresses
        .iter()
        .any(|address| address.eq_ignore_ascii_case(from))
}

fn notice_of(parts: &[Part], report: bool, subject: &str) -> Option<Notice> {
    for part in parts {
        let kind = header(&part.headers, "content-type")
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !(report || kind.contains("delivery-status")) {
            continue;
        }
        if let Some(address) = failed_recipient(&part_text(part)) {
            return Some(Notice::Certain { address });
        }
    }
    if subject
        .to_ascii_lowercase()
        .contains("undelivered mail returned to sender")
    {
        return Some(Notice::Probable);
    }
    None
}

fn failed_recipient(body: &str) -> Option<String> {
    let mut failed = false;
    let mut recipient = None;
    for line in body.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("action") && value.trim().eq_ignore_ascii_case("failed")
        {
            failed = true;
        }
        if name.trim().eq_ignore_ascii_case("final-recipient") {
            recipient = address_from_recipient(value.trim());
        }
    }
    recipient.filter(|_| failed)
}

fn address_from_recipient(value: &str) -> Option<String> {
    let raw = value
        .split_once(';')
        .map_or(value, |(_, address)| address.trim());
    let raw = raw.trim_matches(|c: char| c == '<' || c == '>' || c.is_whitespace());
    parse_email(raw).ok()
}

fn auto_submitted(value: Option<&str>) -> bool {
    value.is_some_and(|value| !value.trim().eq_ignore_ascii_case("no"))
}

struct Part {
    headers: Vec<(String, String)>,
    body: String,
}

fn explode(headers: &[(String, String)], body: &str) -> Vec<Part> {
    let kind = header(headers, "content-type").unwrap_or_default();
    let Some(boundary) = boundary_of(kind) else {
        return vec![Part {
            headers: headers.to_vec(),
            body: body.to_string(),
        }];
    };
    let mut parts = Vec::new();
    let marker = format!("--{boundary}");
    for chunk in body.split(&marker).skip(1) {
        let chunk = chunk.trim_start_matches('\n');
        if chunk.is_empty() || chunk.starts_with("--") {
            continue;
        }
        let (header_text, part_body) = split_headers(chunk);
        let part_headers = unfold(header_text);
        let nested = header(&part_headers, "content-type").unwrap_or_default();
        if nested.to_ascii_lowercase().starts_with("multipart/") {
            parts.extend(explode(&part_headers, part_body));
        } else {
            parts.push(Part {
                headers: part_headers,
                body: part_body.to_string(),
            });
        }
    }
    if parts.is_empty() {
        parts.push(Part {
            headers: headers.to_vec(),
            body: body.to_string(),
        });
    }
    parts
}

fn plain_and_html(parts: &[Part]) -> (String, Option<String>) {
    let mut plain = None;
    let mut html = None;
    for part in parts {
        let kind = header(&part.headers, "content-type")
            .unwrap_or("text/plain")
            .to_ascii_lowercase();
        if kind.starts_with("text/plain") && plain.is_none() {
            plain = Some(part_text(part));
        } else if kind.starts_with("text/html") && html.is_none() {
            html = Some(part_text(part));
        }
    }
    let text = plain.unwrap_or_else(|| html.as_deref().map(html_to_text).unwrap_or_default());
    (text, html)
}

fn part_text(part: &Part) -> String {
    let encoding = header(&part.headers, "content-transfer-encoding")
        .unwrap_or("7bit")
        .trim()
        .to_ascii_lowercase();
    let charset = header(&part.headers, "content-type")
        .and_then(|value| parameter(value, "charset"))
        .unwrap_or_else(|| "utf-8".to_string());
    let bytes = match encoding.as_str() {
        "base64" => decode_base64(&part.body).unwrap_or_default(),
        "quoted-printable" => decode_qp(&part.body),
        _ => part.body.as_bytes().to_vec(),
    };
    decode_bytes(&bytes, &charset)
}

fn html_to_text(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('>') else {
            break;
        };
        let tag = after[..end].trim().to_ascii_lowercase();
        let name = tag
            .trim_start_matches('/')
            .split_whitespace()
            .next()
            .unwrap_or_default();
        if matches!(name, "br" | "p" | "div" | "tr" | "li" | "h1" | "h2" | "h3") {
            out.push('\n');
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    let decoded = out
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    decoded
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn split_headers(raw: &str) -> (&str, &str) {
    raw.split_once("\n\n")
        .map_or((raw, ""), |(headers, body)| (headers, body))
}

fn unfold(headers: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut name = String::new();
    let mut value = String::new();
    for line in headers.split('\n') {
        if (line.starts_with(' ') || line.starts_with('\t')) && !name.is_empty() {
            value.push(' ');
            value.push_str(line.trim());
            continue;
        }
        if let Some((next_name, next_value)) = line.split_once(':') {
            if !name.is_empty() {
                out.push((name.to_ascii_lowercase(), value.trim().to_string()));
            }
            name = next_name.trim().to_string();
            value = next_value.trim().to_string();
        }
    }
    if !name.is_empty() {
        out.push((name.to_ascii_lowercase(), value.trim().to_string()));
    }
    out
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

fn boundary_of(content_type: &str) -> Option<String> {
    let value = parameter(content_type, "boundary")?;
    let value = value.trim_matches('"').trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn parameter(header_value: &str, name: &str) -> Option<String> {
    for piece in header_value.split(';').skip(1) {
        let Some((key, value)) = piece.trim().split_once('=') else {
            continue;
        };
        if key.trim().eq_ignore_ascii_case(name) {
            return Some(value.trim().trim_matches('"').to_string());
        }
    }
    None
}

fn address_of(value: &str) -> (Option<String>, Option<String>) {
    if let Some(start) = value.rfind('<')
        && let Some(end) = value[start + 1..].find('>')
    {
        let raw = value[start + 1..start + 1 + end].trim();
        let name = value[..start].trim().trim_matches('"').trim();
        let name = (!name.is_empty()).then(|| name.to_string());
        return (name, parse_email(raw).ok());
    }
    (None, parse_email(value.trim()).ok())
}

fn push_ids(into: &mut Vec<String>, value: &str) {
    for id in ids_in(value) {
        if !into.iter().any(|known| known == &id) {
            into.push(id);
        }
    }
}

fn first_id(value: &str) -> Option<String> {
    ids_in(value).into_iter().next()
}

fn ids_in(value: &str) -> Vec<String> {
    let mut ids = Vec::new();
    if value.contains('<') {
        let mut rest = value;
        while let Some(start) = rest.find('<') {
            let after = &rest[start + 1..];
            let Some(end) = after.find('>') else {
                break;
            };
            let id = normalize_id(&after[..end]);
            if id.contains('@') {
                ids.push(id);
            }
            rest = &after[end + 1..];
        }
        return ids;
    }
    for token in value.split_whitespace() {
        let id = normalize_id(token);
        if id.contains('@') {
            ids.push(id);
        }
    }
    ids
}

fn normalize_id(value: &str) -> String {
    value
        .trim()
        .trim_matches(|c: char| c == '<' || c == '>')
        .trim()
        .to_ascii_lowercase()
}

fn decode_words(value: &str) -> String {
    let mut out = String::new();
    let mut rest = value;
    let mut previous_encoded = false;
    while let Some(start) = rest.find("=?") {
        let before = &rest[..start];
        if !(previous_encoded && before.trim().is_empty()) {
            out.push_str(before);
        }
        let after = &rest[start + 2..];
        let Some(end) = after.find("?=") else {
            out.push_str(&rest[start..]);
            return out;
        };
        if let Some(decoded) = decode_one_word(&after[..end]) {
            out.push_str(&decoded);
            previous_encoded = true;
        } else {
            out.push_str(&rest[start..start + 2 + end + 2]);
            previous_encoded = false;
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

fn decode_one_word(token: &str) -> Option<String> {
    let (charset, rest) = token.split_once('?')?;
    let (encoding, text) = rest.split_once('?')?;
    let bytes = match encoding {
        "Q" | "q" => decode_qp(&text.replace('_', " ")),
        "B" | "b" => decode_base64(text)?,
        _ => return None,
    };
    Some(decode_bytes(&bytes, charset))
}

fn decode_bytes(bytes: &[u8], charset: &str) -> String {
    let encoding =
        encoding_rs::Encoding::for_label(charset.as_bytes()).unwrap_or(encoding_rs::UTF_8);
    encoding.decode(bytes).0.into_owned()
}

fn decode_qp(input: &str) -> Vec<u8> {
    let bytes = input.as_bytes();
    let mut out = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'=' {
            if matches!(bytes.get(index + 1), Some(b'\n' | b'\r')) {
                index += 1;
                if bytes.get(index) == Some(&b'\r') {
                    index += 1;
                }
                if bytes.get(index) == Some(&b'\n') {
                    index += 1;
                }
                continue;
            }
            if let (Some(high), Some(low)) = (
                bytes.get(index + 1).and_then(|byte| hex_value(*byte)),
                bytes.get(index + 2).and_then(|byte| hex_value(*byte)),
            ) {
                out.push((high << 4) | low);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    out
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn decode_base64(input: &str) -> Option<Vec<u8>> {
    fn value(byte: u8) -> Option<u8> {
        match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let bytes: Vec<u8> = input
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace() && *byte != b'=')
        .collect();
    if bytes.is_empty() {
        return Some(Vec::new());
    }
    let mut out = Vec::new();
    let mut index = 0;
    while index + 1 < bytes.len() {
        let b0 = value(bytes[index])?;
        let b1 = value(bytes[index + 1])?;
        let b2 = bytes.get(index + 2).copied().and_then(value);
        let b3 = bytes.get(index + 3).copied().and_then(value);
        let packed = (u32::from(b0) << 18)
            | (u32::from(b1) << 12)
            | (u32::from(b2.unwrap_or(0)) << 6)
            | u32::from(b3.unwrap_or(0));
        out.push(u8::try_from((packed >> 16) & 0xff).unwrap_or(0));
        if b2.is_some() {
            out.push(u8::try_from((packed >> 8) & 0xff).unwrap_or(0));
        }
        if b3.is_some() {
            out.push(u8::try_from(packed & 0xff).unwrap_or(0));
        }
        index += 4;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::{DossierLink, FicheAddress, Filing, KnownLetter, Notice, classify, read_message};

    const REPLY: &str = "\
From: =?utf-8?Q?Camille_Rivi=C3=A8re?= <camille@riviere.test>\r\n\
To: nicolas@lumen.test\r\n\
Subject: Re: Votre proposition\r\n\
Message-ID: <camille-1@riviere.test>\r\n\
In-Reply-To: <abc@griffe.local>\r\n\
References: <abc@griffe.local>\r\n\
Content-Type: text/plain; charset=utf-8\r\n\
\r\n\
Bonjour Nicolas,\r\n\
\r\n\
Oui, je suis partante.\r\n";

    const ALT: &str = "\
From: Camille <camille@riviere.test>\n\
Subject: Re: suite\n\
Content-Type: multipart/alternative; boundary=alt\n\
\n\
--alt\n\
Content-Type: text/plain; charset=utf-8\n\
\n\
Le texte.\n\
--alt\n\
Content-Type: text/html; charset=utf-8\n\
\n\
<p>Le html</p>\n\
--alt--\n";

    const DSN: &str = "\
From: Mail Delivery System <MAILER-DAEMON@mx.example>\n\
Subject: Undelivered Mail Returned to Sender\n\
Content-Type: multipart/report; report-type=delivery-status;\n\
\tboundary=\"bound\"\n\
\n\
--bound\n\
Content-Type: text/plain; charset=utf-8\n\
\n\
This is the mail system.\n\
--bound\n\
Content-Type: message/delivery-status\n\
\n\
Final-Recipient: rfc822; contact@atelier-du-mail.fr\n\
Action: failed\n\
Status: 5.1.1\n\
\n\
--bound\n\
Content-Type: message/rfc822\n\
\n\
Message-ID: <abc@griffe.local>\n\
\n\
La lettre.\n\
--bound--\n";

    fn known() -> Vec<KnownLetter> {
        vec![KnownLetter {
            message_id: "<abc@griffe.local>".into(),
            client_id: "fiche-camille".into(),
            opportunity_id: Some("dossier-camille".into()),
        }]
    }

    fn fiches() -> Vec<FicheAddress> {
        vec![
            FicheAddress {
                client_id: "fiche-camille".into(),
                address: "camille@riviere.test".into(),
            },
            FicheAddress {
                client_id: "fiche-atelier".into(),
                address: "contact@atelier-du-mail.fr".into(),
            },
        ]
    }

    #[test]
    fn a_reply_keeps_the_plain_text_and_the_name() {
        let message = read_message(REPLY);
        assert_eq!(message.from_name.as_deref(), Some("Camille Rivière"));
        assert_eq!(
            message.from_address.as_deref(),
            Some("camille@riviere.test")
        );
        assert_eq!(
            message.message_id.as_deref(),
            Some("camille-1@riviere.test")
        );
        assert_eq!(message.referenced, vec!["abc@griffe.local".to_string()]);
        assert!(message.text.contains("je suis partante"));
        assert!(message.notice.is_none());
    }

    #[test]
    fn the_plain_part_wins_over_the_html() {
        let message = read_message(ALT);
        assert_eq!(message.text, "Le texte.");
        assert_eq!(message.html.as_deref(), Some("<p>Le html</p>"));
    }

    #[test]
    fn quoted_printable_utf8_is_read() {
        let raw = "\
From: Lea <lea@nord.test>\n\
Subject: =?utf-8?Q?Caf=C3=A9?=\n\
Content-Type: text/plain; charset=utf-8\n\
Content-Transfer-Encoding: quoted-printable\n\
\n\
Caf=C3=A9\n";
        let message = read_message(raw);
        assert_eq!(message.subject, "Café");
        assert_eq!(message.text, "Café");
    }

    #[test]
    fn a_delivery_status_is_certain_and_a_subject_alone_is_probable() {
        let certain = read_message(DSN);
        assert_eq!(
            certain.notice,
            Some(Notice::Certain {
                address: "contact@atelier-du-mail.fr".into(),
            })
        );
        assert!(certain.referenced.iter().any(|id| id == "abc@griffe.local"));
        let probable = read_message(
            "\
From: postmaster@mx.example\n\
Subject: Undelivered Mail Returned to Sender\n\
Content-Type: text/plain; charset=utf-8\n\
\n\
Le message n'est pas parti.\n",
        );
        assert_eq!(probable.notice, Some(Notice::Probable));
    }

    #[test]
    fn a_known_reply_holds_the_dossier_and_our_own_letter_does_not() {
        let reply = classify(
            &read_message(REPLY),
            &known(),
            &fiches(),
            &["nicolas@lumen.test"],
        );
        assert_eq!(
            reply,
            Filing {
                link: DossierLink::One {
                    client_id: "fiche-camille".into(),
                    opportunity_id: Some("dossier-camille".into()),
                },
                hold: true,
                notice: None,
            }
        );
        let own = read_message(
            "\
From: Nicolas <nicolas@lumen.test>\n\
Message-ID: <abc@griffe.local>\n\
Subject: Votre proposition\n\
Content-Type: text/plain\n\
\n\
Bonjour Camille.\n",
        );
        let filed = classify(&own, &known(), &fiches(), &["nicolas@lumen.test"]);
        assert!(!filed.hold);
        assert!(matches!(filed.link, DossierLink::One { .. }));
    }

    #[test]
    fn one_fiche_links_and_two_fiches_stay_a_choice() {
        let raw = "\
From: Inconnue <lea@nord.test>\n\
Subject: Disponibilité\n\
Content-Type: text/plain\n\
\n\
Bonjour.\n";
        let alone = classify(&read_message(raw), &[], &fiches(), &[]);
        assert_eq!(alone.link, DossierLink::Unlinked);
        assert!(!alone.hold);
        let one = classify(
            &read_message(&raw.replace("lea@nord.test", "camille@riviere.test")),
            &[],
            &fiches(),
            &[],
        );
        assert!(one.hold);
        assert_eq!(
            one.link,
            DossierLink::One {
                client_id: "fiche-camille".into(),
                opportunity_id: None,
            }
        );
        let shared = vec![
            fiches()[0].clone(),
            FicheAddress {
                client_id: "fiche-autre".into(),
                address: "camille@riviere.test".into(),
            },
        ];
        let choice = classify(
            &read_message(&raw.replace("lea@nord.test", "camille@riviere.test")),
            &[],
            &shared,
            &[],
        );
        assert!(!choice.hold);
        assert_eq!(
            choice.link,
            DossierLink::Choice {
                client_ids: vec!["fiche-camille".into(), "fiche-autre".into()],
            }
        );
    }

    #[test]
    fn a_bounce_marks_the_address_and_does_not_hold() {
        let filed = classify(
            &read_message(DSN),
            &known(),
            &fiches(),
            &["nicolas@lumen.test"],
        );
        assert!(!filed.hold);
        assert_eq!(
            filed.link,
            DossierLink::One {
                client_id: "fiche-atelier".into(),
                opportunity_id: None,
            }
        );
        let same_fiche = vec![KnownLetter {
            message_id: "<abc@griffe.local>".into(),
            client_id: "fiche-atelier".into(),
            opportunity_id: Some("dossier-atelier".into()),
        }];
        let linked = classify(&read_message(DSN), &same_fiche, &fiches(), &[]);
        assert_eq!(
            linked.link,
            DossierLink::One {
                client_id: "fiche-atelier".into(),
                opportunity_id: Some("dossier-atelier".into()),
            }
        );
    }

    #[test]
    fn an_automatic_reply_does_not_hold() {
        let raw = format!("Auto-Submitted: auto-replied\n{REPLY}");
        let filed = classify(&read_message(&raw), &known(), &fiches(), &[]);
        assert!(!filed.hold);
    }

    #[test]
    fn html_without_a_plain_part_becomes_text() {
        let raw = "\
From: Lea <lea@nord.test>\n\
Subject: Bonjour\n\
Content-Type: text/html; charset=utf-8\n\
\n\
<p>Bonjour &amp; merci.</p>\n";
        let message = read_message(raw);
        assert_eq!(message.text, "Bonjour & merci.");
    }
}
