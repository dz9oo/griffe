//! Contrôle de structure d'un FEC — art. A. 47 A-1 du livre des procédures fiscales.
//!
//! Requête pure : des octets en entrée, un rapport en sortie. Rien n'est écrit. La conformité
//! structurelle **ne présage pas** de la régularité de la comptabilité, et ce rapport n'est
//! **pas** une attestation de l'administration — c'est la lecture de l'arrêté, le même rôle
//! que `xmllint --schema` pour le CII d'une Factur-X.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Serialize;
use time::Date;

use rusqlite::Connection;

use crate::app::AppError;
use crate::billing::import::decode;

use super::{HEADER, build_fec};

/// Phrase reprise de Test Compta Demat, à afficher telle quelle : le contrôle ne dit rien
/// de la comptabilité, seulement du fichier.
pub const DISCLAIMER: &str = "La conformité structurelle du FEC ne présage pas de la régularité \
    de la comptabilité, ni de sa valeur probante.";

/// Les champs que l'arrêté impose de renseigner (lettrage et devise restent facultatifs ;
/// débit/crédit vides valent zéro).
const REQUIRED: &[&str] = &[
    "JournalCode",
    "JournalLib",
    "EcritureNum",
    "EcritureDate",
    "CompteNum",
    "CompteLib",
    "PieceRef",
    "PieceDate",
    "EcritureLib",
    "ValidDate",
];

/// Le rapport d'un contrôle — toujours produit, même sur un fichier vide ou illisible.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FecCheck {
    pub file_name: Option<String>,
    pub encoding: String,
    /// `"|"`, `"tab"`, `";"`, `","` — vide si aucun séparateur n'a été reconnu.
    pub separator: String,
    /// Lignes d'écriture (hors en-tête, hors lignes vides de fin de fichier).
    pub lines: usize,
    pub total_debit_cents: i64,
    pub total_credit_cents: i64,
    pub conformant: bool,
    pub error_count: usize,
    pub warning_count: usize,
    pub findings: Vec<FecFinding>,
    pub disclaimer: String,
}

impl FecCheck {
    /// Aucune erreur de structure — des alertes peuvent rester.
    #[must_use]
    pub const fn is_conformant(&self) -> bool {
        self.conformant
    }
}

/// Une anomalie, rattachée à une ligne et éventuellement à une colonne.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FecFinding {
    pub severity: FecSeverity,
    /// 1 = en-tête. `None` : le fichier entier (nom, séparateur, vide).
    pub line: Option<u32>,
    pub column: Option<String>,
    pub message: String,
}

/// Erreur = le fichier n'est pas conforme à l'arrêté. Alerte = un inspecteur le remarquerait,
/// Test Compta Demat ne le bloque plus (ou ne le vérifie pas).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FecSeverity {
    Error,
    Warning,
}

/// Relit `bytes` comme un FEC. `file_name` est le nom (ou chemin) du fichier s'il y en a un :
/// l'arrêté impose `<SIREN>FEC<AAAAMMJJ>` ; `None` saute ce contrôle (aperçu, stdin).
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn check_fec(bytes: &[u8], file_name: Option<&str>) -> FecCheck {
    let mut findings = Vec::new();
    let display_name = file_name.map(|n| {
        Path::new(n)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(n)
            .to_string()
    });
    if let Some(name) = display_name.as_deref() {
        check_file_name(name, &mut findings);
    }

    let (text, encoding) = decode(bytes);
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let raw_lines: Vec<&str> = text.lines().collect();
    let mut records: Vec<&str> = raw_lines
        .iter()
        .copied()
        .rev()
        .skip_while(|l| l.trim().is_empty())
        .collect();
    records.reverse();

    if records.is_empty() {
        findings.push(file_error("le fichier est vide"));
        return finish(display_name, encoding, String::new(), 0, 0, 0, findings);
    }

    let header = records[0];
    let Some((sep, sep_label)) = detect_separator(header) else {
        findings.push(file_error(
            "aucun séparateur reconnu (| ou tabulation) — l'en-tête ne se découpe pas en 18 colonnes",
        ));
        return finish(display_name, encoding, String::new(), 0, 0, 0, findings);
    };
    if sep == ';' || sep == ',' {
        findings.push(file_error(
            "séparateur interdit depuis le 1er janvier 2013 : | ou tabulation attendu",
        ));
    }

    let names: Vec<String> = split_record(header, sep);
    check_header(&names, &mut findings);

    let mut total_debit: i64 = 0;
    let mut total_credit: i64 = 0;
    let mut data_lines = 0_usize;
    let mut entries: BTreeMap<(String, String), EntryAgg> = BTreeMap::new();
    let mut last_date: Option<Date> = None;

    for (offset, record) in records.iter().copied().enumerate().skip(1) {
        let line_no = u32::try_from(offset + 1).unwrap_or(u32::MAX);
        if record.trim().is_empty() {
            findings.push(finding(
                FecSeverity::Error,
                Some(line_no),
                None,
                "ligne vide",
            ));
            continue;
        }
        data_lines += 1;
        let fields = split_record(record, sep);
        if fields.len() < HEADER.len() {
            findings.push(finding(
                FecSeverity::Error,
                Some(line_no),
                None,
                format!(
                    "la structure du fichier est incorrecte : {} champ(s) au lieu des {} attendus",
                    fields.len(),
                    HEADER.len()
                ),
            ));
            continue;
        }
        if fields.len() > names.len() {
            findings.push(finding(
                FecSeverity::Error,
                Some(line_no),
                None,
                format!(
                    "la structure du fichier est incorrecte : {} champ(s) au lieu des {} de l'en-tête",
                    fields.len(),
                    names.len()
                ),
            ));
        }

        let get = |name: &str| -> &str {
            names
                .iter()
                .position(|n| n.eq_ignore_ascii_case(name))
                .and_then(|i| fields.get(i))
                .map_or("", String::as_str)
        };

        for col in REQUIRED {
            if get(col).is_empty() {
                findings.push(finding(
                    FecSeverity::Error,
                    Some(line_no),
                    Some((*col).to_string()),
                    format!("le champ {col} n'est pas renseigné"),
                ));
            }
        }

        let compte = get("CompteNum");
        if !compte.is_empty() && !compte_num_starts_with_three_digits(compte) {
            findings.push(finding(
                FecSeverity::Warning,
                Some(line_no),
                Some("CompteNum".into()),
                format!("le numéro de compte doit commencer par trois chiffres (reçu {compte})"),
            ));
        }

        for col in ["EcritureDate", "PieceDate", "ValidDate", "DateLet"] {
            let raw = get(col);
            if raw.is_empty() {
                continue;
            }
            match parse_fec_date(raw) {
                DateParse::Ok(date) => {
                    if col == "EcritureDate" {
                        if last_date.is_some_and(|prev| date < prev) {
                            findings.push(finding(
                                FecSeverity::Warning,
                                Some(line_no),
                                Some(col.into()),
                                format!(
                                    "date de comptabilisation antérieure à la ligne précédente ({raw})"
                                ),
                            ));
                        }
                        last_date = Some(date);
                    }
                }
                DateParse::OutOfRange => {
                    findings.push(finding(
                        FecSeverity::Warning,
                        Some(line_no),
                        Some(col.into()),
                        format!("date hors période 1900–2099 ({raw})"),
                    ));
                }
                DateParse::Invalid => {
                    findings.push(finding(
                        FecSeverity::Error,
                        Some(line_no),
                        Some(col.into()),
                        format!("format de date incorrect ({raw}) — attendu AAAAMMJJ"),
                    ));
                }
            }
        }

        let debit = match parse_fec_amount(get("Debit")) {
            Ok(v) => v,
            Err(msg) => {
                findings.push(finding(
                    FecSeverity::Error,
                    Some(line_no),
                    Some("Debit".into()),
                    msg,
                ));
                0
            }
        };
        let credit = match parse_fec_amount(get("Credit")) {
            Ok(v) => v,
            Err(msg) => {
                findings.push(finding(
                    FecSeverity::Error,
                    Some(line_no),
                    Some("Credit".into()),
                    msg,
                ));
                0
            }
        };
        if (debit == 0 && credit == 0) || (debit != 0 && credit != 0) {
            findings.push(finding(
                FecSeverity::Warning,
                Some(line_no),
                None,
                format!(
                    "débit et crédit renseignés sur la même ligne, ou tous deux nuls ({}/{})",
                    format_cents(debit),
                    format_cents(credit)
                ),
            ));
        }
        total_debit = total_debit.saturating_add(debit.abs());
        total_credit = total_credit.saturating_add(credit.abs());

        let journal = get("JournalCode").to_string();
        let number = get("EcritureNum").to_string();
        if !journal.is_empty() && !number.is_empty() {
            let entry = entries
                .entry((journal.clone(), number.clone()))
                .or_insert(EntryAgg {
                    first_line: line_no,
                    journal,
                    number,
                    debit: 0,
                    credit: 0,
                    dates: BTreeSet::new(),
                });
            entry.debit = entry.debit.saturating_add(debit.abs());
            entry.credit = entry.credit.saturating_add(credit.abs());
            let date = get("EcritureDate");
            if !date.is_empty() {
                entry.dates.insert(date.to_string());
            }
        }
    }

    if data_lines == 0 {
        findings.push(file_error("le fichier ne contient pas de ligne d'écriture"));
    }

    if total_debit != total_credit {
        findings.push(finding(
            FecSeverity::Warning,
            None,
            None,
            format!(
                "total débit ({}) ≠ total crédit ({})",
                format_cents(total_debit),
                format_cents(total_credit)
            ),
        ));
    }

    for entry in entries.values() {
        if entry.debit != entry.credit {
            findings.push(finding(
                FecSeverity::Warning,
                Some(entry.first_line),
                Some("EcritureNum".into()),
                format!(
                    "écriture {} / {} non équilibrée (débit {}, crédit {})",
                    entry.journal,
                    entry.number,
                    format_cents(entry.debit),
                    format_cents(entry.credit)
                ),
            ));
        }
        if entry.dates.len() > 1 {
            findings.push(finding(
                FecSeverity::Warning,
                Some(entry.first_line),
                Some("EcritureDate".into()),
                format!(
                    "écriture {} / {} : dates de comptabilisation différentes",
                    entry.journal, entry.number
                ),
            ));
        }
    }

    check_numbering(&entries, &mut findings);

    finish(
        display_name,
        encoding,
        sep_label.to_string(),
        data_lines,
        total_debit,
        total_credit,
        findings,
    )
}

/// Construit le FEC de l'exercice puis le relit — la requête des façades `fec check --period`.
///
/// # Errors
///
/// Celles de [`build_fec`].
pub fn check_fec_of(conn: &Connection, period: i32) -> Result<FecCheck, AppError> {
    Ok(build_fec(conn, period)?.check())
}

struct EntryAgg {
    first_line: u32,
    journal: String,
    number: String,
    debit: i64,
    credit: i64,
    dates: BTreeSet<String>,
}

fn finish(
    file_name: Option<String>,
    encoding: &str,
    separator: String,
    lines: usize,
    total_debit_cents: i64,
    total_credit_cents: i64,
    findings: Vec<FecFinding>,
) -> FecCheck {
    let error_count = findings
        .iter()
        .filter(|f| f.severity == FecSeverity::Error)
        .count();
    let warning_count = findings
        .iter()
        .filter(|f| f.severity == FecSeverity::Warning)
        .count();
    FecCheck {
        file_name,
        encoding: encoding.to_string(),
        separator,
        lines,
        total_debit_cents,
        total_credit_cents,
        conformant: error_count == 0 && lines > 0,
        error_count,
        warning_count,
        findings,
        disclaimer: DISCLAIMER.to_string(),
    }
}

fn file_error(message: impl Into<String>) -> FecFinding {
    finding(FecSeverity::Error, None, None, message)
}

fn finding(
    severity: FecSeverity,
    line: Option<u32>,
    column: Option<String>,
    message: impl Into<String>,
) -> FecFinding {
    FecFinding {
        severity,
        line,
        column,
        message: message.into(),
    }
}

fn check_file_name(name: &str, findings: &mut Vec<FecFinding>) {
    let stem = name
        .strip_suffix(".txt")
        .or_else(|| name.strip_suffix(".TXT"))
        .unwrap_or(name);
    let valid = match stem.split_once("FEC").or_else(|| stem.split_once("fec")) {
        Some((siren, date)) => {
            siren.len() == 9
                && siren.bytes().all(|b| b.is_ascii_digit())
                && date.len() == 8
                && matches!(parse_fec_date(date), DateParse::Ok(_))
        }
        None => false,
    };
    if !valid {
        findings.push(file_error(format!(
            "nom de fichier incorrect ({name}) — attendu SIREN(9)FEC + AAAAMMJJ, ex. \
             552100554FEC20261231.txt"
        )));
    }
}

fn compte_num_starts_with_three_digits(compte: &str) -> bool {
    let mut chars = compte.chars();
    (0..3).all(|_| chars.next().is_some_and(|c| c.is_ascii_digit()))
}

fn detect_separator(header: &str) -> Option<(char, &'static str)> {
    let ranked = [('|', "|"), ('\t', "tab"), (';', ";"), (',', ",")];
    ranked
        .into_iter()
        .map(|(sep, label)| (sep, label, split_record(header, sep).len()))
        .filter(|(_, _, n)| *n >= 2)
        .max_by_key(|(_, _, n)| *n)
        .map(|(sep, label, _)| (sep, label))
}

fn split_record(line: &str, sep: char) -> Vec<String> {
    line.split(sep).map(|f| f.trim().to_string()).collect()
}

fn check_header(names: &[String], findings: &mut Vec<FecFinding>) {
    for (i, expected) in HEADER.iter().enumerate() {
        match names.get(i) {
            None => findings.push(finding(
                FecSeverity::Error,
                Some(1),
                Some((*expected).to_string()),
                format!("colonne {} manquante ({expected})", i + 1),
            )),
            Some(got) if got.eq_ignore_ascii_case(expected) => {}
            Some(got) => findings.push(finding(
                FecSeverity::Error,
                Some(1),
                Some((*expected).to_string()),
                format!("colonne {} : attendu {expected}, trouvé {got}", i + 1),
            )),
        }
    }
}

fn check_numbering(entries: &BTreeMap<(String, String), EntryAgg>, findings: &mut Vec<FecFinding>) {
    let mut by_journal: BTreeMap<&str, Vec<(&str, u32)>> = BTreeMap::new();
    for entry in entries.values() {
        by_journal
            .entry(entry.journal.as_str())
            .or_default()
            .push((entry.number.as_str(), entry.first_line));
    }
    for (journal, nums) in by_journal {
        let parsed: Option<Vec<(u32, u32)>> = nums
            .iter()
            .map(|(n, line)| n.parse::<u32>().ok().map(|v| (v, *line)))
            .collect();
        let Some(mut parsed) = parsed else {
            continue;
        };
        parsed.sort_by_key(|(n, _)| *n);
        parsed.dedup_by_key(|(n, _)| *n);
        for (i, (n, line)) in parsed.iter().enumerate() {
            let expected = u32::try_from(i + 1).unwrap_or(u32::MAX);
            if *n != expected {
                findings.push(finding(
                    FecSeverity::Warning,
                    Some(*line),
                    Some("EcritureNum".into()),
                    format!("numérotation discontinue dans le journal {journal} (attendu {expected}, trouvé {n})"),
                ));
                break;
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DateParse {
    Ok(Date),
    OutOfRange,
    Invalid,
}

fn parse_fec_date(raw: &str) -> DateParse {
    let trimmed = raw.trim();
    let digits: String = trimmed.chars().filter(char::is_ascii_digit).collect();
    if digits.len() < 8 {
        return DateParse::Invalid;
    }
    let ymd = &digits[..8];
    let year: i32 = ymd[..4].parse().unwrap_or(0);
    let month: u8 = ymd[4..6].parse().unwrap_or(0);
    let day: u8 = ymd[6..8].parse().unwrap_or(0);
    // AAAAMMJJ vs JJMMAAAA : si le premier groupe > 31, c'est une année.
    let (year, month, day) = if year >= 1900 {
        (year, month, day)
    } else {
        // JJMMAAAA compacté : digits = DDMMAAAA
        let day = ymd[..2].parse().unwrap_or(0);
        let month = ymd[2..4].parse().unwrap_or(0);
        let year = ymd[4..8].parse().unwrap_or(0);
        (year, month, day)
    };
    if !(1900..=2099).contains(&year) {
        return DateParse::OutOfRange;
    }
    let Some(month) = time::Month::try_from(month).ok() else {
        return DateParse::Invalid;
    };
    Date::from_calendar_date(year, month, day).map_or(DateParse::Invalid, DateParse::Ok)
}

fn parse_fec_amount(raw: &str) -> Result<i64, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(0);
    }
    if trimmed.contains('.') {
        return Err(format!(
            "n'est pas au bon format ({trimmed}) : virgule décimale attendue, pas un point"
        ));
    }
    if trimmed.chars().any(char::is_whitespace) {
        return Err(format!(
            "n'est pas au bon format ({trimmed}) : sans séparateur de milliers"
        ));
    }
    let (sign, unsigned) = match trimmed.strip_prefix('-') {
        Some(rest) => (-1_i64, rest),
        None => match trimmed.strip_suffix('-') {
            Some(rest) => (-1, rest),
            None => (1, trimmed.strip_prefix('+').unwrap_or(trimmed)),
        },
    };
    if unsigned.chars().filter(|c| *c == ',').count() > 1 {
        return Err(format!(
            "n'est pas au bon format ({unsigned}) : un format numérique est attendu"
        ));
    }
    let mut parts = unsigned.splitn(2, ',');
    let integer = parts.next().unwrap_or("");
    let frac = parts.next().unwrap_or("0");
    if integer.is_empty() || !integer.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!(
            "n'est pas au bon format ({trimmed}) : un format numérique est attendu"
        ));
    }
    if frac.len() > 2 || !frac.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!(
            "n'est pas au bon format ({trimmed}) : au plus deux décimales"
        ));
    }
    let integer: i64 = integer.parse().map_err(|_| {
        format!("n'est pas au bon format ({trimmed}) : un format numérique est attendu")
    })?;
    let frac_cents: i64 = match frac.len() {
        0 => 0,
        1 => frac.parse::<i64>().unwrap_or(0) * 10,
        _ => frac.parse().unwrap_or(0),
    };
    integer
        .checked_mul(100)
        .and_then(|c| c.checked_add(frac_cents))
        .and_then(|c| c.checked_mul(sign))
        .ok_or_else(|| format!("montant hors bornes ({trimmed})"))
}

fn format_cents(cents: i64) -> String {
    crate::domain::Money::from_cents(cents).to_string()
}

#[cfg(test)]
mod tests {
    use super::super::Fec;
    use super::*;
    use crate::company::CompanyProfile;
    use crate::domain::{
        Address, Client, ClientId, Expense, ExpenseCategory, FiscalYear, FiscalYearEnd, Invoice,
        InvoiceId, InvoiceLine, InvoiceOrigin, InvoiceStatus, Money, Payment, PaymentId,
        PaymentMethod, Siren, VatRate,
    };
    use time::{Date, Month, OffsetDateTime};

    fn header() -> String {
        HEADER.join("|")
    }

    fn valid_body() -> String {
        format!(
            "{}\nVE|Ventes|1|20260310|411000|Clients|||FA-1|20260310|Facture|100,00|0,00|||20260310||\n\
             VE|Ventes|1|20260310|706000|Prestations|||FA-1|20260310|Facture|0,00|100,00|||20260310||\n",
            header()
        )
    }

    #[test]
    fn an_empty_file_is_not_conformant() {
        let check = check_fec(b"", None);
        assert!(!check.is_conformant());
        assert_eq!(check.error_count, 1);
        assert!(check.findings[0].message.contains("vide"), "{check:?}");
    }

    #[test]
    fn a_header_only_file_is_not_conformant() {
        let check = check_fec(header().as_bytes(), None);
        assert!(!check.is_conformant());
        assert!(
            check
                .findings
                .iter()
                .any(|f| f.message.contains("ligne d'écriture")),
            "{check:?}"
        );
    }

    #[test]
    fn a_minimal_balanced_file_is_conformant() {
        let check = check_fec(valid_body().as_bytes(), Some("552100554FEC20261231.txt"));
        assert!(check.is_conformant(), "{:?}", check.findings);
        assert_eq!(check.lines, 2);
        assert_eq!(check.total_debit_cents, 10_000);
        assert_eq!(check.total_credit_cents, 10_000);
        assert_eq!(check.separator, "|");
        assert_eq!(check.encoding, "utf-8");
        assert!(check.disclaimer.contains("régularité"));
    }

    #[test]
    fn a_bad_file_name_is_an_error() {
        let check = check_fec(valid_body().as_bytes(), Some("export.txt"));
        assert!(!check.is_conformant());
        assert!(
            check
                .findings
                .iter()
                .any(|f| f.message.contains("nom de fichier")),
            "{check:?}"
        );
    }

    #[test]
    fn a_decimal_point_is_an_error() {
        let body = format!(
            "{}\nVE|Ventes|1|20260310|411000|Clients|||FA-1|20260310|Facture|100.00|0,00|||20260310||\n",
            header()
        );
        let check = check_fec(body.as_bytes(), None);
        assert!(!check.is_conformant());
        assert!(
            check
                .findings
                .iter()
                .any(|f| f.column.as_deref() == Some("Debit") && f.message.contains("point")),
            "{check:?}"
        );
    }

    #[test]
    fn seventeen_columns_is_an_error() {
        let body = "JournalCode|JournalLib|EcritureNum|EcritureDate|CompteNum|CompteLib|CompAuxNum|CompAuxLib|PieceRef|PieceDate|EcritureLib|Debit|Credit|EcritureLet|DateLet|ValidDate|Montantdevise\n";
        let check = check_fec(body.as_bytes(), None);
        assert!(!check.is_conformant());
        assert!(
            check
                .findings
                .iter()
                .any(|f| f.message.contains("Idevise") || f.message.contains("manquante")),
            "{check:?}"
        );
    }

    #[test]
    fn semicolon_separator_is_an_error() {
        let header = HEADER.join(";");
        let body = format!(
            "{header}\nVE;Ventes;1;20260310;411000;Clients;;;FA-1;20260310;Facture;100,00;0,00;;;20260310;;\n\
             VE;Ventes;1;20260310;706000;Prestations;;;FA-1;20260310;Facture;0,00;100,00;;;20260310;;\n"
        );
        let check = check_fec(body.as_bytes(), None);
        assert!(!check.is_conformant());
        assert!(
            check
                .findings
                .iter()
                .any(|f| f.message.contains("séparateur interdit")),
            "{check:?}"
        );
    }

    #[test]
    fn debit_and_credit_on_the_same_line_is_a_warning() {
        let body = format!(
            "{}\nVE|Ventes|1|20260310|411000|Clients|||FA-1|20260310|Facture|50,00|50,00|||20260310||\n",
            header()
        );
        let check = check_fec(body.as_bytes(), None);
        assert!(check.is_conformant(), "ce n'est pas bloquant : {check:?}");
        assert!(
            check.findings.iter().any(
                |f| f.severity == FecSeverity::Warning && f.message.contains("débit et crédit")
            ),
            "{check:?}"
        );
    }

    #[test]
    fn an_unbalanced_entry_is_a_warning() {
        let body = format!(
            "{}\nVE|Ventes|1|20260310|411000|Clients|||FA-1|20260310|Facture|100,00|0,00|||20260310||\n\
             VE|Ventes|1|20260310|706000|Prestations|||FA-1|20260310|Facture|0,00|80,00|||20260310||\n",
            header()
        );
        let check = check_fec(body.as_bytes(), None);
        assert!(check.is_conformant());
        assert!(
            check
                .findings
                .iter()
                .any(|f| f.message.contains("non équilibrée")),
            "{check:?}"
        );
    }

    #[test]
    fn a_compte_num_without_three_leading_digits_is_a_warning() {
        let body = format!(
            "{}\nVE|Ventes|1|20260310|AB|Clients|||FA-1|20260310|Facture|100,00|0,00|||20260310||\n\
             VE|Ventes|1|20260310|706000|Prestations|||FA-1|20260310|Facture|0,00|100,00|||20260310||\n",
            header()
        );
        let check = check_fec(body.as_bytes(), None);
        assert!(
            check
                .findings
                .iter()
                .any(|f| f.column.as_deref() == Some("CompteNum")),
            "{check:?}"
        );
    }

    #[test]
    fn a_numbering_gap_is_a_warning() {
        let body = format!(
            "{}\nVE|Ventes|1|20260310|411000|Clients|||FA-1|20260310|Facture|100,00|0,00|||20260310||\n\
             VE|Ventes|1|20260310|706000|Prestations|||FA-1|20260310|Facture|0,00|100,00|||20260310||\n\
             VE|Ventes|3|20260311|411000|Clients|||FA-2|20260311|Facture|50,00|0,00|||20260311||\n\
             VE|Ventes|3|20260311|706000|Prestations|||FA-2|20260311|Facture|0,00|50,00|||20260311||\n",
            header()
        );
        let check = check_fec(body.as_bytes(), None);
        assert!(
            check
                .findings
                .iter()
                .any(|f| f.message.contains("numérotation discontinue")),
            "{check:?}"
        );
    }

    #[test]
    fn the_tiime_fixture_is_structurally_conformant() {
        let bytes = include_bytes!("../opening_balance/fixtures/fec-tiime.txt");
        let check = check_fec(bytes, None);
        assert!(check.is_conformant(), "{:?}", check.findings);
        assert_eq!(check.total_debit_cents, check.total_credit_cents);
    }

    fn date(year: i32, month: Month, day: u8) -> Date {
        Date::from_calendar_date(year, month, day).unwrap()
    }

    fn profile() -> CompanyProfile {
        CompanyProfile {
            name: "Argon Digital".to_string(),
            legal_form: "SASU".to_string(),
            siren: Siren::parse("552100554").unwrap(),
            vat_number: None,
            address: Address {
                street: "12 rue de la Paix".to_string(),
                postal_code: "75002".to_string(),
                city: "Paris".to_string(),
                country: "FR".to_string(),
            },
            share_capital: None,
            rcs_city: None,
            iban: None,
            fiscal_year_end: Some(FiscalYearEnd::CALENDAR),
            vat_regime: None,
            director_monthly_gross: None,
            director_charge_ratio_bps: None,
            president_name: None,
            sole_shareholder_name: None,
            sole_shareholder_address: None,
            share_count: None,
        }
    }

    #[test]
    fn a_rendered_fec_is_conformant() {
        let client_id = ClientId::new();
        let inv_id = InvoiceId::new();
        let fec = Fec::build(
            &profile(),
            FiscalYear::calendar(2026),
            &[Invoice {
                id: inv_id,
                number: "FA-2026-0001".into(),
                client_id,
                mission_id: None,
                lines: vec![InvoiceLine {
                    description: "Conseil".into(),
                    quantity: 1.0,
                    unit_price: Money::from_cents(200_000),
                    vat_rate: VatRate::Standard,
                }],
                status: InvoiceStatus::Issued,
                origin: InvoiceOrigin::Issued,
                issued_on: date(2026, Month::March, 10),
                due_on: date(2026, Month::March, 10),
                previous_hash: None,
                hash: String::new(),
                credited_invoice_id: None,
            }],
            &[Client {
                id: client_id,
                name: "Acme".into(),
                siren: None,
                vat_number: None,
                address: None,
                created_at: OffsetDateTime::UNIX_EPOCH,
                revision: 1,
                archived_at: None,
            }],
            &[Payment {
                id: PaymentId::new(),
                invoice_id: inv_id,
                amount: Money::from_cents(240_000),
                received_on: date(2026, Month::March, 31),
                method: PaymentMethod::BankTransfer,
                voided_at: None,
                bank_transaction_id: None,
            }],
            &[Expense {
                id: crate::domain::ExpenseId::new(),
                label: "Licence".into(),
                category: ExpenseCategory::Software,
                amount: Money::from_cents(12_000),
                vat_rate: VatRate::Standard,
                vat_deductible: Money::from_cents(2_000),
                incurred_on: date(2026, Month::March, 12),
                receipt_hash: None,
                receipt_filename: None,
                supplier: None,
                paid_by: crate::domain::ExpensePaidBy::Company,
                created_at: OffsetDateTime::UNIX_EPOCH,
                revision: 1,
            }],
            None,
        )
        .unwrap();
        let check = fec.check();
        assert!(check.is_conformant(), "{:?}", check.findings);
        assert_eq!(check.file_name.as_deref(), Some("552100554FEC20261231.txt"));
        assert_eq!(check.total_debit_cents, check.total_credit_cents);
        assert_eq!(check.warning_count, 0, "{:?}", check.findings);
    }
}
