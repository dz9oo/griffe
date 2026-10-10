//! Affichage d'un numéro de téléphone. Aucune IO.

fn is_separator(c: char) -> bool {
    matches!(
        c,
        ' ' | '\u{00A0}'
            | '\u{202F}'
            | '.'
            | '-'
            | '\u{2010}'
            | '\u{2011}'
            | '\u{2013}'
            | '\u{2014}'
            | '/'
            | '('
            | ')'
    )
}

fn ascii_digits(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}

/// Dix chiffres nationaux, ou `+33` / `0033` puis neuf chiffres sans le `0` de tête.
fn national_digits(compact: &str) -> Option<String> {
    if compact.len() == 10 && compact.starts_with('0') && ascii_digits(compact) {
        return Some(compact.to_string());
    }
    let rest = compact
        .strip_prefix("+33")
        .or_else(|| compact.strip_prefix("0033"))?;
    if rest.len() == 9 && !rest.starts_with('0') && ascii_digits(rest) {
        Some(format!("0{rest}"))
    } else {
        None
    }
}

fn pair_digits(digits: &str) -> String {
    let mut paired = String::with_capacity(digits.len() + 4);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && index.is_multiple_of(2) {
            paired.push(' ');
        }
        paired.push(digit);
    }
    paired
}

/// Forme affichée d'un numéro.
///
/// Un numéro français de dix chiffres, ou un indicatif `+33` / `0033` suivi de neuf chiffres
/// dont le premier n'est pas `0`, s'écrit par paires (`03 27 44 44 44`). Les espaces, y compris
/// insécables, les points, les tirets, les barres et les parenthèses ne comptent pas dans cette
/// lecture. Tout autre numéro revient tel que saisi, une fois les bords coupés. Une chaîne vide
/// reste vide.
#[must_use]
pub fn display_phone(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let compact: String = trimmed.chars().filter(|c| !is_separator(*c)).collect();
    national_digits(&compact).map_or_else(|| trimmed.to_string(), |digits| pair_digits(&digits))
}

/// Lien `tel:` quand la ligne n'est qu'un numéro. Huit chiffres au moins.
/// Un numéro français devient `tel:+33` suivi des neuf chiffres. Un autre garde son `+` et ses chiffres.
#[must_use]
pub fn tel_href(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let compact: String = trimmed.chars().filter(|c| !is_separator(*c)).collect();
    let digits = compact.strip_prefix('+').unwrap_or(compact.as_str());
    if digits.len() < 8 || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    if let Some(national) = national_digits(&compact) {
        return Some(format!("tel:+33{}", &national[1..]));
    }
    Some(format!("tel:{compact}"))
}

#[cfg(test)]
mod tests {
    use super::{display_phone, tel_href};

    #[test]
    fn a_french_number_is_paired_and_any_other_stays_as_typed() {
        let cases = [
            ("0327444444", "03 27 44 44 44"),
            ("03.27.44.44.44", "03 27 44 44 44"),
            ("+33327444444", "03 27 44 44 44"),
            ("0033327444444", "03 27 44 44 44"),
            ("+33 3 27 44 44 44", "03 27 44 44 44"),
            ("03-27-44-44-44", "03 27 44 44 44"),
            ("(03) 27/44.44.44", "03 27 44 44 44"),
            ("  0327444444  ", "03 27 44 44 44"),
            ("03\u{00a0}27 44 44 44", "03 27 44 44 44"),
            ("+1 415 555 0100", "+1 415 555 0100"),
            ("  +1 415 555 0100  ", "+1 415 555 0100"),
            ("03 27 44 44 44 poste 12", "03 27 44 44 44 poste 12"),
            ("+33 (0)3 27 44 44 44", "+33 (0)3 27 44 44 44"),
            ("", ""),
            ("   ", ""),
            ("06 12 34 56 78", "06 12 34 56 78"),
        ];
        for (raw, expected) in cases {
            assert_eq!(display_phone(raw), expected, "{raw:?}");
        }
    }

    #[test]
    fn a_line_that_is_only_a_number_becomes_a_tel_link() {
        assert_eq!(
            tel_href("06 70 12 32 60"),
            Some("tel:+33670123260".to_string())
        );
        assert_eq!(tel_href("0670123260"), Some("tel:+33670123260".to_string()));
        assert_eq!(
            tel_href("+1 415 555 0100"),
            Some("tel:+14155550100".to_string())
        );
        assert_eq!(tel_href("2026"), None);
        assert_eq!(tel_href("03 27 44 44 44 poste 12"), None);
        assert_eq!(tel_href("bonjour"), None);
    }
}
