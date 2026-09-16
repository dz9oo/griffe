//! Import de relevés bancaires (CSV, OFX, **xlsx Tiime**) : uniquement des fonctions pures
//! d'analyse, aucune IO, aucune écriture en base — c'est
//! [`super::commands::ImportBankTransactions`] qui persiste le résultat.
//!
//! **Lot 38 : un export de banque réelle s'importe tel quel.** Jusqu'ici seul le format maison
//! `date;description;montant` (UTF-8, dates ISO) passait ; l'audit du 2 septembre 2026 a montré
//! qu'aucun export de banque française — Qonto (virgule, en-tête anglais), Boursorama, Crédit
//! Agricole (latin-1, colonnes débit/crédit, `JJ/MM/AAAA`, lignes de préambule), BNP, LCL (sans
//! en-tête), La Banque Postale — ne s'importait, et que le message d'erreur ne disait pas quel
//! format produire. [`parse_bank_statement`] prend des **octets** et devine tout ce qui peut
//! l'être : l'encodage (UTF-8 avec ou sans BOM, sinon Windows-1252 — le latin-1 « à la main »
//! laisserait faux les caractères 0x80-0x9F que les banques émettent, `€` en tête), le format
//! (OFX ou CSV), le séparateur (`;`, `,`, tabulation — celui qui donne le même nombre de champs
//! sur l'en-tête et les premières lignes), les guillemets, la décimale (`,` ou `.`), les
//! séparateurs de milliers (espace, espace insécable), le format de date, et **les colonnes par
//! leur nom** (dictionnaire français/anglais : date d'opération, libellé, montant, débit,
//! crédit, identifiant). Un fichier sans en-tête reconnu retombe sur `date;description;montant`,
//! ou sur « la première colonne est la date, la première colonne numérique le montant, le reste
//! le libellé » (export LCL). Les lignes de préambule (avant l'en-tête) sont ignorées ; une
//! ligne de données illisible est **sautée et rapportée** ([`ParsedStatement::skipped`]) plutôt
//! que de faire échouer tout l'import — sauf s'il ne reste rien.

use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::Date;

use crate::domain;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParsedTransaction {
    #[serde(with = "crate::domain::serde_date::date")]
    pub occurred_on: Date,
    /// Positif pour une entrée d'argent, négatif pour une sortie.
    pub amount_cents: i64,
    pub description: String,
    /// Identifiant unique donné par la banque (`FITID` en OFX, colonne « Transaction ID » de
    /// certains CSV) — lot 38 : la clé de dédoublonnage quand elle existe, plus fiable que le
    /// triplet date/montant/libellé (un libellé diffère entre l'export CSV et l'OFX de la même
    /// opération).
    #[serde(default)]
    pub fitid: Option<String>,
}

/// Le format attendu, tel qu'il est dit à l'utilisateur dans chaque erreur.
pub const EXPECTED_FORMAT: &str = "un export CSV de votre banque avec une ligne d'en-tête qui \
    nomme la date, le libellé et le montant (ou deux colonnes débit/crédit) — par exemple \
    « date;description;montant » puis « 2026-01-15;VIR CABINET;-600,00 » — un fichier OFX, \
    ou l'export Excel (xlsx) de Tiime (feuille Transactions : date, intitulé, montant ; la \
    feuille Soldes bancaires est ignorée). Séparateur (; , tabulation), décimale (, ou .), \
    format de date (AAAA-MM-JJ, JJ/MM/AAAA, JJ-MM-AAAA, JJ.MM.AAAA) et encodage (UTF-8, \
    Windows-1252) sont détectés automatiquement.";

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ImportError {
    #[error("ligne {line} : {reason}")]
    MalformedLine { line: usize, reason: String },
    #[error(
        "le fichier ne contient aucune transaction exploitable{detail}. Attendu : {EXPECTED_FORMAT}"
    )]
    Empty { detail: String },
    #[error("format de relevé non reconnu : {reason}. Attendu : {EXPECTED_FORMAT}")]
    Unrecognized { reason: String },
}

/// Le format à forcer, quand la détection ne convient pas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StatementFormat {
    Csv,
    Ofx,
    Xlsx,
}

impl std::str::FromStr for StatementFormat {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "csv" => Ok(Self::Csv),
            "ofx" => Ok(Self::Ofx),
            "xlsx" | "xls" => Ok(Self::Xlsx),
            other => Err(format!(
                "format inconnu : {other} (attendu csv, ofx ou xlsx)"
            )),
        }
    }
}

/// Ce que la détection a trouvé — pour l'aperçu (`--dry-run`, panneau de la fenêtre) : dire
/// *comment* le fichier a été lu est ce qui permet à l'utilisateur de repérer une colonne mal
/// choisie avant d'importer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetectedDialect {
    pub format: StatementFormat,
    /// `utf-8` ou `windows-1252`.
    pub encoding: String,
    pub separator: Option<char>,
    pub decimal: Option<char>,
    /// Le format de date reconnu sur la première ligne de données (`AAAA-MM-JJ`, …).
    pub date_format: Option<String>,
    /// Numéro de la ligne d'en-tête (1-indexé), `None` sans en-tête reconnu.
    pub header_line: Option<usize>,
    /// Les colonnes retenues, en clair (« date ← Date operation, montant ← Montant… »).
    pub columns: String,
}

/// Le résultat d'une analyse : les transactions lues, comment, et ce qui a été sauté.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParsedStatement {
    pub transactions: Vec<ParsedTransaction>,
    pub dialect: DetectedDialect,
    /// `(numéro de ligne, motif)` des lignes de données illisibles, dans l'ordre du fichier.
    pub skipped: Vec<(usize, String)>,
}

/// Analyse un relevé depuis ses octets, en devinant tout ce qui peut l'être (voir le
/// commentaire de module). `hint` force le format quand la détection se trompe.
///
/// # Errors
///
/// [`ImportError::Unrecognized`] si ni un OFX ni un CSV lisible n'y est reconnu,
/// [`ImportError::Empty`] s'il n'en sort aucune transaction — chaque message dit le format
/// attendu.
pub fn parse_bank_statement(
    bytes: &[u8],
    hint: Option<StatementFormat>,
) -> Result<ParsedStatement, ImportError> {
    if hint == Some(StatementFormat::Xlsx) || (hint.is_none() && looks_like_spreadsheet(bytes)) {
        return parse_xlsx(bytes);
    }
    let (text, encoding) = decode(bytes);
    let format = hint.unwrap_or_else(|| {
        if looks_like_ofx(&text) {
            StatementFormat::Ofx
        } else {
            StatementFormat::Csv
        }
    });
    match format {
        StatementFormat::Ofx => parse_ofx(&text, encoding),
        StatementFormat::Csv => parse_csv(&text, encoding),
        StatementFormat::Xlsx => parse_xlsx(bytes),
    }
}

/// Analyse un export CSV au format maison `date;description;montant` — conservée pour les
/// appelants historiques, c'est désormais [`parse_bank_statement`] qui fait le travail (et
/// accepte bien plus). Les lignes sautées deviennent une erreur, comme avant.
///
/// # Errors
///
/// Voir [`parse_bank_statement`] ; une ligne illisible est une erreur
/// ([`ImportError::MalformedLine`]).
pub fn parse_csv_bank_statement(input: &str) -> Result<Vec<ParsedTransaction>, ImportError> {
    strict(parse_bank_statement(
        input.as_bytes(),
        Some(StatementFormat::Csv),
    )?)
}

/// Analyse un export OFX (1.x SGML ou 2.x XML), sous-ensemble pragmatique : `DTPOSTED`,
/// `TRNAMT`, `NAME`/`MEMO` et `FITID` de chaque bloc `<STMTTRN>`.
///
/// # Errors
///
/// Voir [`parse_bank_statement`].
pub fn parse_ofx_bank_statement(input: &str) -> Result<Vec<ParsedTransaction>, ImportError> {
    strict(parse_bank_statement(
        input.as_bytes(),
        Some(StatementFormat::Ofx),
    )?)
}

fn strict(parsed: ParsedStatement) -> Result<Vec<ParsedTransaction>, ImportError> {
    if let Some((line, reason)) = parsed.skipped.into_iter().next() {
        return Err(ImportError::MalformedLine { line, reason });
    }
    Ok(parsed.transactions)
}

// ---------------------------------------------------------------------------------------------
// Encodage et format
// ---------------------------------------------------------------------------------------------

/// UTF-8 (BOM retiré) si le fichier en est, sinon Windows-1252 — jamais une erreur : un relevé
/// n'est pas moins lisible pour avoir été exporté par une banque en latin-1.
pub(crate) fn decode(bytes: &[u8]) -> (String, &'static str) {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    if let Ok(text) = std::str::from_utf8(bytes) {
        return (text.to_string(), "utf-8");
    }
    let (decoded, _, _) = encoding_rs::WINDOWS_1252.decode(bytes);
    (decoded.into_owned(), "windows-1252")
}

fn looks_like_ofx(text: &str) -> bool {
    let head: String = text
        .chars()
        .take(4096)
        .collect::<String>()
        .to_ascii_uppercase();
    head.contains("<OFX") || head.contains("OFXHEADER") || head.contains("<STMTTRN>")
}

/// ZIP xlsx (`PK`) ou OLE Compound File xls (`D0 CF 11 E0`).
fn looks_like_spreadsheet(bytes: &[u8]) -> bool {
    bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(&[0xD0, 0xCF, 0x11, 0xE0])
}

/// Ouvre le classeur, retient la feuille Transactions (Tiime), ignore Soldes, et recycle
/// le sniffer CSV.
fn parse_xlsx(bytes: &[u8]) -> Result<ParsedStatement, ImportError> {
    use std::io::Cursor;

    use calamine::{Reader, open_workbook_auto_from_rs};

    let mut workbook =
        open_workbook_auto_from_rs(Cursor::new(bytes)).map_err(|e| ImportError::Unrecognized {
            reason: format!(
                "classeur Excel illisible ({e}). Attendu : l'export Tiime « état du compte » \
                 (xlsx, feuille Transactions : date, intitulé, montant)"
            ),
        })?;
    let names = workbook.sheet_names();
    if names.is_empty() {
        return Err(ImportError::Unrecognized {
            reason: "classeur Excel sans feuille. Attendu : une feuille Transactions \
                     (date, intitulé, montant) — l'export Tiime en a deux, Soldes ignorée"
                .to_string(),
        });
    }
    let mut last_err = None;
    for name in ordered_sheet_names(&names) {
        let Ok(range) = workbook.worksheet_range(name) else {
            continue;
        };
        let csv = sheet_to_csv(&range);
        if csv.trim().is_empty() {
            continue;
        }
        match parse_csv(&csv, "xlsx") {
            Ok(mut parsed) if !parsed.transactions.is_empty() => {
                parsed.dialect.format = StatementFormat::Xlsx;
                parsed.dialect.encoding = "xlsx".to_string();
                if !parsed.dialect.columns.is_empty() {
                    parsed.dialect.columns = format!("feuille {name} — {}", parsed.dialect.columns);
                }
                return Ok(parsed);
            }
            Ok(_) | Err(_) => {
                last_err = Some(name.to_string());
            }
        }
    }
    Err(ImportError::Unrecognized {
        reason: format!(
            "aucune feuille exploitable{} : attendu une feuille Transactions (date, intitulé, \
             montant). La feuille Soldes bancaires de Tiime est ignorée à dessein",
            last_err.map_or(String::new(), |n| format!(" (essayé « {n} »)"))
        ),
    })
}

/// Transactions d'abord, Soldes/Balance jamais en premier.
fn ordered_sheet_names(names: &[String]) -> Vec<&str> {
    let mut preferred = Vec::new();
    let mut rest = Vec::new();
    for name in names {
        let n = normalize(name);
        if n.contains("solde") || n.contains("balance") {
            continue;
        }
        if n.contains("transaction") {
            preferred.push(name.as_str());
        } else {
            rest.push(name.as_str());
        }
    }
    preferred.extend(rest);
    preferred
}

fn sheet_to_csv(range: &calamine::Range<calamine::Data>) -> String {
    range
        .rows()
        .filter_map(|row| {
            let fields: Vec<String> = row.iter().map(cell_text).collect();
            if fields.iter().all(String::is_empty) {
                None
            } else {
                Some(csv_join(&fields))
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn csv_join(fields: &[String]) -> String {
    fields
        .iter()
        .map(|f| {
            if f.contains([';', '"', '\n']) {
                format!("\"{}\"", f.replace('"', "\"\""))
            } else {
                f.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(";")
}

fn cell_text(cell: &calamine::Data) -> String {
    match cell {
        calamine::Data::Empty | calamine::Data::Error(_) => String::new(),
        calamine::Data::String(s)
        | calamine::Data::DateTimeIso(s)
        | calamine::Data::DurationIso(s) => s.trim().to_string(),
        calamine::Data::Int(i) => i.to_string(),
        calamine::Data::Bool(b) => b.to_string(),
        calamine::Data::Float(f) => format_excel_number(*f),
        calamine::Data::DateTime(dt) => excel_serial_date(dt.as_f64()),
    }
}

fn format_excel_number(value: f64) -> String {
    if value.is_finite() && (value - value.round()).abs() < 1e-9 {
        format!("{value:.0}")
    } else {
        format!("{value:.2}")
    }
}

/// Série Excel (jours depuis le 1899-12-30) → `AAAA-MM-JJ`. Les durées et les hors-plage
/// deviennent une chaîne vide : le sniffer CSV les sautera.
fn excel_serial_date(serial: f64) -> String {
    if !serial.is_finite() {
        return String::new();
    }
    let Ok(days) = format!("{:.0}", serial.trunc()).parse::<i64>() else {
        return String::new();
    };
    // Excel : 1 = 1900-01-01 ; 80 000 ≈ 2119 — hors d'un relevé bancaire.
    if !(1..=80_000).contains(&days) {
        return String::new();
    }
    let Ok(base) = Date::from_calendar_date(1899, time::Month::December, 30) else {
        return String::new();
    };
    base.saturating_add(time::Duration::days(days)).to_string()
}

// ---------------------------------------------------------------------------------------------
// OFX
// ---------------------------------------------------------------------------------------------

fn parse_ofx(text: &str, encoding: &'static str) -> Result<ParsedStatement, ImportError> {
    let mut transactions = Vec::new();
    let mut skipped = Vec::new();
    for (index, block) in text.split("<STMTTRN>").skip(1).enumerate() {
        let number = index + 1;
        let block = block.split("</STMTTRN>").next().unwrap_or(block);
        let Some(amount_str) = extract_ofx_tag(block, "TRNAMT") else {
            skipped.push((number, "TRNAMT manquant".to_string()));
            continue;
        };
        let Some(date_str) = extract_ofx_tag(block, "DTPOSTED") else {
            skipped.push((number, "DTPOSTED manquant".to_string()));
            continue;
        };
        let description = extract_ofx_tag(block, "NAME")
            .or_else(|| extract_ofx_tag(block, "MEMO"))
            .unwrap_or_default();
        let Some(amount_cents) = parse_amount(&amount_str, None) else {
            skipped.push((number, format!("montant OFX invalide : {amount_str}")));
            continue;
        };
        let Some(occurred_on) = parse_ofx_date(&date_str) else {
            skipped.push((number, format!("date OFX invalide : {date_str}")));
            continue;
        };
        transactions.push(ParsedTransaction {
            occurred_on,
            amount_cents,
            description: description.trim().to_string(),
            fitid: extract_ofx_tag(block, "FITID").filter(|f| !f.is_empty()),
        });
    }
    if transactions.is_empty() {
        return Err(ImportError::Empty {
            detail: if skipped.is_empty() {
                " (aucun bloc <STMTTRN>)".to_string()
            } else {
                format!(" ({} bloc(s) OFX illisible(s))", skipped.len())
            },
        });
    }
    Ok(ParsedStatement {
        transactions,
        dialect: DetectedDialect {
            format: StatementFormat::Ofx,
            encoding: encoding.to_string(),
            separator: None,
            decimal: Some('.'),
            date_format: Some("AAAAMMJJ".to_string()),
            header_line: None,
            columns: "DTPOSTED, TRNAMT, NAME/MEMO, FITID".to_string(),
        },
        skipped,
    })
}

fn extract_ofx_tag(block: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let start = block.find(&open)? + open.len();
    let rest = &block[start..];
    let end = rest.find('<').unwrap_or(rest.len());
    Some(rest[..end].trim().to_string())
}

fn parse_ofx_date(s: &str) -> Option<Date> {
    // OFX : AAAAMMJJ, éventuellement suivi d'une heure/fuseau qu'on ignore. On prend les 8
    // premiers *caractères* (pas octets) pour rester valide sur une entrée arbitraire.
    let digits: String = s.chars().take(8).collect();
    if digits.chars().count() != 8 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let year: i32 = digits[0..4].parse().ok()?;
    let month: u8 = digits[4..6].parse().ok()?;
    let day: u8 = digits[6..8].parse().ok()?;
    time::Month::try_from(month)
        .ok()
        .and_then(|m| Date::from_calendar_date(year, m, day).ok())
}

// ---------------------------------------------------------------------------------------------
// CSV : découpage, en-tête, colonnes
// ---------------------------------------------------------------------------------------------

pub(crate) const SEPARATORS: [char; 3] = [';', ',', '\t'];

/// Découpe une ligne en champs selon `sep`, en respectant les guillemets doubles (un `;` ou une
/// `,` entre guillemets fait partie du champ, `""` est un guillemet échappé).
pub(crate) fn split_fields(line: &str, sep: char) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '"' {
            if quoted && chars.peek() == Some(&'"') {
                current.push('"');
                chars.next();
            } else {
                quoted = !quoted;
            }
        } else if c == sep && !quoted {
            fields.push(std::mem::take(&mut current));
        } else {
            current.push(c);
        }
    }
    fields.push(current);
    fields.into_iter().map(|f| f.trim().to_string()).collect()
}

/// Minuscules, sans accents ni ponctuation superflue : la clé de comparaison des noms de
/// colonnes (« Date d'opération », « DATE OPERATION », « dateOp » se rejoignent).
pub(crate) fn normalize(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        let mapped = match c {
            'é' | 'è' | 'ê' | 'ë' | 'É' | 'È' | 'Ê' => 'e',
            'à' | 'â' | 'ä' | 'À' => 'a',
            'î' | 'ï' => 'i',
            'ô' | 'ö' => 'o',
            'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            c if c.is_alphanumeric() => c.to_ascii_lowercase(),
            _ => ' ',
        };
        out.push(mapped);
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Date,
    ValueDate,
    Amount,
    Debit,
    Credit,
    Label,
    Counterparty,
    Id,
}

/// Le rôle d'une colonne d'après son nom normalisé — `None` pour une colonne ignorée
/// (catégorie, devise, solde, note…).
fn role_of(name: &str) -> Option<Role> {
    let n = normalize(name);
    let has = |w: &str| n.split(' ').any(|t| t == w);
    let contains = |w: &str| n.contains(w);
    if n.is_empty() {
        return None;
    }
    if contains("solde") || contains("balance") || contains("devise") || contains("currency") {
        return None;
    }
    if contains("transaction id")
        || contains("id transaction")
        || contains("identifiant")
        || n == "id"
    {
        return Some(Role::Id);
    }
    if has("debit") {
        return Some(Role::Debit);
    }
    if has("credit") {
        return Some(Role::Credit);
    }
    if has("date") || n.starts_with("date") {
        if contains("valeur") || contains("value") || contains("settlement") || n == "dateval" {
            return Some(Role::ValueDate);
        }
        return Some(Role::Date);
    }
    if contains("local amount") || contains("montant local") {
        return None;
    }
    if has("montant") || has("amount") {
        return Some(Role::Amount);
    }
    if contains("counterparty") || contains("tiers") || contains("beneficiaire") {
        return Some(Role::Counterparty);
    }
    if contains("libell")
        || contains("label")
        || contains("description")
        || contains("motif")
        || contains("intitul")
        || contains("reference")
        || contains("operation")
        || contains("nature")
    {
        return Some(Role::Label);
    }
    None
}

/// Les colonnes retenues sur une ligne d'en-tête reconnue.
#[derive(Debug, Clone, Default)]
struct Columns {
    date: Option<usize>,
    amount: Option<usize>,
    debit: Option<usize>,
    credit: Option<usize>,
    labels: Vec<usize>,
    counterparty: Option<usize>,
    id: Option<usize>,
    names: Vec<String>,
}

impl Columns {
    fn from_header(fields: &[String]) -> Option<Self> {
        let mut cols = Self {
            names: fields.to_vec(),
            ..Self::default()
        };
        let mut value_date = None;
        for (i, name) in fields.iter().enumerate() {
            match role_of(name) {
                Some(Role::Date) if cols.date.is_none() => cols.date = Some(i),
                Some(Role::ValueDate) if value_date.is_none() => value_date = Some(i),
                Some(Role::Amount) if cols.amount.is_none() => cols.amount = Some(i),
                Some(Role::Debit) if cols.debit.is_none() => cols.debit = Some(i),
                Some(Role::Credit) if cols.credit.is_none() => cols.credit = Some(i),
                Some(Role::Label) => cols.labels.push(i),
                Some(Role::Counterparty) if cols.counterparty.is_none() => {
                    cols.counterparty = Some(i);
                }
                Some(Role::Id) if cols.id.is_none() => cols.id = Some(i),
                _ => {}
            }
        }
        if cols.date.is_none() {
            cols.date = value_date;
        }
        let has_amount = cols.amount.is_some() || (cols.debit.is_some() && cols.credit.is_some());
        (cols.date.is_some() && has_amount).then_some(cols)
    }

    /// `date;description;montant` sans en-tête nommé (ou avec un en-tête inconnu de trois
    /// colonnes), ou la lecture « première colonne = date, première colonne numérique =
    /// montant, le reste = libellé » d'un export sans en-tête (LCL).
    fn guess_headerless(rows: &[Vec<String>]) -> Option<Self> {
        let width = rows.first()?.len();
        if width < 2 {
            return None;
        }
        let all = |pred: &dyn Fn(&str) -> bool, col: usize| {
            rows.iter().all(|r| r.get(col).is_some_and(|f| pred(f)))
        };
        if !all(&|f| parse_date_any(f).is_some(), 0) {
            return None;
        }
        let amount = (1..width).find(|&col| all(&|f| parse_amount(f, None).is_some(), col))?;
        let labels: Vec<usize> = (1..width).filter(|&c| c != amount).collect();
        Some(Self {
            date: Some(0),
            amount: Some(amount),
            labels,
            names: (0..width).map(|i| format!("colonne {}", i + 1)).collect(),
            ..Self::default()
        })
    }

    fn describe(&self) -> String {
        let name = |i: usize| self.names.get(i).cloned().unwrap_or_default();
        let mut parts = Vec::new();
        if let Some(d) = self.date {
            parts.push(format!("date ← {}", name(d)));
        }
        if let Some(a) = self.amount {
            parts.push(format!("montant ← {}", name(a)));
        }
        if let (Some(d), Some(c)) = (self.debit, self.credit) {
            parts.push(format!("montant ← {} / {}", name(c), name(d)));
        }
        let labels: Vec<String> = self
            .counterparty
            .into_iter()
            .chain(self.labels.iter().copied())
            .map(name)
            .collect();
        if !labels.is_empty() {
            parts.push(format!("libellé ← {}", labels.join(" + ")));
        }
        if let Some(i) = self.id {
            parts.push(format!("identifiant ← {}", name(i)));
        }
        parts.join(", ")
    }
}

/// La première ligne (parmi les vingt premières non vides) qui, découpée par `sep`, nomme une
/// date et un montant (ou débit/crédit) — les lignes qui la précèdent sont un préambule
/// (« Numéro de compte », « Solde au … ») que plusieurs banques ajoutent.
fn find_header(lines: &[(usize, &str)], sep: char) -> Option<(usize, Columns)> {
    lines
        .iter()
        .take(20)
        .enumerate()
        .find_map(|(idx, (_, line))| {
            Columns::from_header(&split_fields(line, sep)).map(|c| (idx, c))
        })
}

#[allow(clippy::too_many_lines)]
fn parse_csv(text: &str, encoding: &'static str) -> Result<ParsedStatement, ImportError> {
    let lines: Vec<(usize, &str)> = text
        .lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l.trim_end_matches('\r')))
        .filter(|(_, l)| !l.trim().is_empty())
        .collect();
    if lines.is_empty() {
        return Err(ImportError::Empty {
            detail: " (fichier vide)".to_string(),
        });
    }

    // Séparateur et en-tête : le premier séparateur qui révèle un en-tête reconnu gagne ;
    // sinon celui qui donne le plus de champs, stables sur les premières lignes.
    let mut chosen: Option<(char, Option<usize>, Columns)> = None;
    for sep in SEPARATORS {
        if let Some((header_idx, cols)) = find_header(&lines, sep) {
            chosen = Some((sep, Some(header_idx), cols));
            break;
        }
    }
    if chosen.is_none() {
        let mut best: Option<(char, usize)> = None;
        for sep in SEPARATORS {
            let widths: Vec<usize> = lines
                .iter()
                .take(6)
                .map(|(_, l)| split_fields(l, sep).len())
                .collect();
            let width = widths[0];
            if width >= 2 && widths.iter().all(|w| *w == width) && best.is_none_or(|b| width > b.1)
            {
                best = Some((sep, width));
            }
        }
        let (sep, _) = best.ok_or_else(|| ImportError::Unrecognized {
            reason: "aucune ligne d'en-tête nommant la date et le montant, et aucun séparateur \
                     (; , tabulation) ne découpe les premières lignes en colonnes régulières"
                .to_string(),
        })?;
        let rows: Vec<Vec<String>> = lines
            .iter()
            .take(6)
            .map(|(_, l)| split_fields(l, sep))
            .collect();
        // Un en-tête inconnu de trois colonnes (« date;description;montant ») : on saute la
        // première ligne si elle ne se lit pas comme une donnée.
        let (skip_first, sample): (bool, &[Vec<String>]) = if rows
            .first()
            .is_some_and(|r| parse_date_any(&r[0]).is_none())
        {
            (true, &rows[1..])
        } else {
            (false, &rows[..])
        };
        let cols = if sample.is_empty() {
            None
        } else if sample[0].len() == 3 {
            Columns::guess_headerless(sample).or_else(|| {
                Some(Columns {
                    date: Some(0),
                    amount: Some(2),
                    labels: vec![1],
                    names: vec![
                        "date".to_string(),
                        "description".to_string(),
                        "montant".to_string(),
                    ],
                    ..Columns::default()
                })
            })
        } else {
            Columns::guess_headerless(sample)
        };
        let cols = cols.ok_or_else(|| ImportError::Unrecognized {
            reason: format!(
                "aucune ligne d'en-tête reconnue (première ligne : « {} »), et les lignes ne se \
                 lisent pas comme « date ; montant ; libellé »",
                lines[0].1.chars().take(80).collect::<String>()
            ),
        })?;
        chosen = Some((sep, skip_first.then_some(0), cols));
    }
    let (sep, header_idx, cols) = chosen.expect("choisi ou erreur retournée ci-dessus");
    let data_start = header_idx.map_or(0, |h| h + 1);

    let mut transactions = Vec::new();
    let mut skipped = Vec::new();
    let mut date_format = None;
    let mut decimal = None;
    let expected_width = cols.names.len();
    for (line_number, line) in &lines[data_start..] {
        let fields = split_fields(line, sep);
        if fields.len() < expected_width.min(3) || fields.iter().all(String::is_empty) {
            skipped.push((
                *line_number,
                format!(
                    "{} champ(s) au lieu de {expected_width} — ligne de pied de page ou de \
                     solde ?",
                    fields.len()
                ),
            ));
            continue;
        }
        let field = |i: Option<usize>| i.and_then(|i| fields.get(i)).map_or("", String::as_str);
        let raw_date = field(cols.date);
        let Some((occurred_on, fmt)) = parse_date_with_format(raw_date) else {
            skipped.push((*line_number, format!("date illisible : « {raw_date} »")));
            continue;
        };
        date_format.get_or_insert(fmt);
        let amount = match (cols.amount, cols.debit, cols.credit) {
            (Some(a), _, _) => parse_amount(field(Some(a)), Some(&mut decimal)).map(Some),
            (None, Some(d), Some(c)) => {
                let debit = field(Some(d));
                let credit = field(Some(c));
                let mut parse = |s: &str| -> Option<i64> {
                    if s.is_empty() {
                        Some(0)
                    } else {
                        parse_amount(s, Some(&mut decimal))
                    }
                };
                let d = parse(debit);
                let c = parse(credit);
                match (d, c) {
                    (Some(d), Some(c)) => Some(Some(c.abs() - d.abs())),
                    _ => None,
                }
            }
            _ => Some(None),
        };
        let Some(Some(amount_cents)) = amount else {
            skipped.push((
                *line_number,
                format!(
                    "montant illisible : « {} »",
                    field(cols.amount.or(cols.debit))
                ),
            ));
            continue;
        };
        let counterparty = field(cols.counterparty).trim().to_string();
        let label = cols
            .labels
            .iter()
            .map(|&i| field(Some(i)).trim())
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join(" — ");
        let description = match (counterparty.is_empty(), label.is_empty()) {
            (false, false) => format!("{counterparty} — {label}"),
            (false, true) => counterparty,
            _ => label,
        };
        transactions.push(ParsedTransaction {
            occurred_on,
            amount_cents,
            description,
            fitid: Some(field(cols.id).trim().to_string()).filter(|f| !f.is_empty()),
        });
    }
    if transactions.is_empty() {
        return Err(ImportError::Empty {
            detail: if skipped.is_empty() {
                String::new()
            } else {
                format!(
                    " ({} ligne(s) illisible(s), la première : ligne {} — {})",
                    skipped.len(),
                    skipped[0].0,
                    skipped[0].1
                )
            },
        });
    }
    Ok(ParsedStatement {
        transactions,
        dialect: DetectedDialect {
            format: StatementFormat::Csv,
            encoding: encoding.to_string(),
            separator: Some(sep),
            decimal,
            date_format: date_format.map(str::to_string),
            header_line: header_idx.map(|h| lines[h].0),
            columns: cols.describe(),
        },
        skipped,
    })
}

// ---------------------------------------------------------------------------------------------
// Dates et montants
// ---------------------------------------------------------------------------------------------

/// Les formats de date reconnus, avec leur nom pour l'aperçu.
fn parse_date_with_format(s: &str) -> Option<(Date, &'static str)> {
    let s = s.trim();
    // Une heure après la date (« 2026-01-31 10:22:05 », « 31/01/2026 10:22 ») est ignorée.
    let s = s.split([' ', 'T']).next().unwrap_or(s);
    let digits: Vec<&str> = s.split(['-', '/', '.']).collect();
    if s.len() == 8 && s.bytes().all(|b| b.is_ascii_digit()) {
        return parse_ofx_date(s).map(|d| (d, "AAAAMMJJ"));
    }
    if digits.len() != 3 {
        return None;
    }
    let sep = s.chars().find(|c| matches!(c, '-' | '/' | '.'))?;
    let parse = |y: &str, m: &str, d: &str| -> Option<Date> {
        let year: i32 = y.parse().ok()?;
        let year = if y.len() == 2 { 2000 + year } else { year };
        let month: u8 = m.parse().ok()?;
        let day: u8 = d.parse().ok()?;
        time::Month::try_from(month)
            .ok()
            .and_then(|m| Date::from_calendar_date(year, m, day).ok())
    };
    if digits[0].len() == 4 {
        let fmt = match sep {
            '-' => "AAAA-MM-JJ",
            '/' => "AAAA/MM/JJ",
            _ => "AAAA.MM.JJ",
        };
        return parse(digits[0], digits[1], digits[2]).map(|d| (d, fmt));
    }
    let fmt = match (sep, digits[2].len()) {
        ('/', 4) => "JJ/MM/AAAA",
        ('-', 4) => "JJ-MM-AAAA",
        ('.', 4) => "JJ.MM.AAAA",
        ('/', _) => "JJ/MM/AA",
        ('-', _) => "JJ-MM-AA",
        (_, _) => "JJ.MM.AA",
    };
    parse(digits[2], digits[1], digits[0]).map(|d| (d, fmt))
}

fn parse_date_any(s: &str) -> Option<Date> {
    parse_date_with_format(s).map(|(d, _)| d)
}

/// Un montant tel qu'une banque l'écrit : `-12,50`, `1 200,00`, `1,200.50`, `-59.99`, `+7 800`,
/// `12,50 €`, `EUR -12.50`, `(12,50)`. La décimale est le dernier séparateur quand les deux
/// apparaissent, sinon `,` ou `.` — sauf un `.` ou une `,` suivi d'exactement trois chiffres
/// *et* précédé d'un groupe de milliers plausible, qui est un séparateur de milliers. Le
/// séparateur décimal retenu est mémorisé dans `decimal` pour l'aperçu.
pub(crate) fn parse_amount(raw: &str, decimal: Option<&mut Option<char>>) -> Option<i64> {
    let mut s: String = raw
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '\u{a0}' && *c != '\u{202f}')
        .collect();
    for token in ["€", "EUR", "eur", "Eur", "E"] {
        s = s.replace(token, "");
    }
    let negative_paren = s.starts_with('(') && s.ends_with(')');
    let s = s.trim_matches(|c| c == '(' || c == ')');
    let (sign, s) = match s.strip_prefix('-') {
        Some(rest) => (-1i64, rest),
        None => (1i64, s.strip_prefix('+').unwrap_or(s)),
    };
    let (sign, s) = match s.strip_suffix('-') {
        Some(rest) => (-1i64, rest),
        None => (sign, s),
    };
    let sign = if negative_paren { -1 } else { sign };
    if s.is_empty()
        || !s
            .chars()
            .all(|c| c.is_ascii_digit() || c == ',' || c == '.')
    {
        return None;
    }
    let last_comma = s.rfind(',');
    let last_dot = s.rfind('.');
    let dec = match (last_comma, last_dot) {
        (Some(c), Some(d)) => Some(if c > d { ',' } else { '.' }),
        (Some(c), None) => {
            // « 1,200 » seul : ambigu — trois chiffres après une virgule *et* au moins un
            // chiffre avant, avec une seule virgule, est lu comme un montant anglais en
            // milliers seulement si la partie entière fait plus de trois chiffres ; « 1,200 »
            // reste 1,20 (les banques françaises n'écrivent jamais de milliers sans décimales).
            let _ = c;
            Some(',')
        }
        (None, Some(_)) => Some('.'),
        (None, None) => None,
    };
    let (integer, fraction) = match dec {
        Some(d) => {
            if s.matches(d).count() != 1 {
                return None;
            }
            let idx = s.rfind(d)?;
            (&s[..idx], &s[idx + 1..])
        }
        None => (s, ""),
    };
    // Dans la partie entière, l'autre séparateur ne peut être qu'un séparateur de milliers :
    // des groupes de trois chiffres exactement.
    if let Some(first) = integer.find([',', '.']) {
        let groups: Vec<&str> = integer[first + 1..].split([',', '.']).collect();
        if groups.iter().any(|g| g.len() != 3) {
            return None;
        }
    }
    let integer: String = integer.chars().filter(char::is_ascii_digit).collect();
    if fraction.len() > 2 || fraction.contains(['.', ',']) {
        return None;
    }
    let normalized = format!(
        "{}.{}",
        if integer.is_empty() { "0" } else { &integer },
        if fraction.is_empty() { "0" } else { fraction }
    );
    let cents = domain::Money::parse_decimal(&normalized).ok()?.cents();
    if let (Some(slot), Some(d)) = (decimal, dec) {
        slot.get_or_insert(d);
    }
    cents.checked_mul(sign)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn date(y: i32, m: u8, d: u8) -> Date {
        Date::from_calendar_date(y, time::Month::try_from(m).unwrap(), d).unwrap()
    }

    #[test]
    fn parses_a_well_formed_csv_statement() {
        let csv = "date;description;montant\n2026-08-30;Virement Lumen Bank;7800.00\n2026-08-31;Prélèvement Adobe;-59,99\n";
        let transactions = parse_csv_bank_statement(csv).unwrap();
        assert_eq!(transactions.len(), 2);
        assert_eq!(transactions[0].amount_cents, 780_000);
        assert_eq!(transactions[1].amount_cents, -5_999);
        assert_eq!(transactions[1].description, "Prélèvement Adobe");
    }

    #[test]
    fn rejects_a_malformed_csv_line_with_its_line_number() {
        let csv = "date;description;montant\n2026-08-30;Virement;7800.00\npas assez de champs\n";
        let err = parse_csv_bank_statement(csv).unwrap_err();
        assert!(
            matches!(err, ImportError::MalformedLine { line: 3, .. }),
            "{err}"
        );
    }

    #[test]
    fn empty_csv_after_header_is_rejected_with_the_expected_format() {
        let err = parse_csv_bank_statement("date;description;montant\n").unwrap_err();
        assert!(matches!(err, ImportError::Empty { .. }));
        assert!(
            err.to_string().contains("date;description;montant"),
            "{err}"
        );
    }

    #[test]
    fn parses_a_well_formed_ofx_statement_with_fitid() {
        let ofx = "<OFX><BANKTRANLIST>\n\
                   <STMTTRN><TRNTYPE>CREDIT<DTPOSTED>20260830120000<TRNAMT>7800.00<FITID>2026083001<NAME>Lumen Bank</STMTTRN>\n\
                   <STMTTRN><TRNTYPE>DEBIT<DTPOSTED>20260831<TRNAMT>-59.99<MEMO>Adobe</STMTTRN>\n\
                   </BANKTRANLIST></OFX>";
        let transactions = parse_ofx_bank_statement(ofx).unwrap();
        assert_eq!(transactions.len(), 2);
        assert_eq!(transactions[0].amount_cents, 780_000);
        assert_eq!(transactions[0].description, "Lumen Bank");
        assert_eq!(transactions[0].fitid.as_deref(), Some("2026083001"));
        assert_eq!(transactions[1].amount_cents, -5_999);
        assert_eq!(transactions[1].description, "Adobe");
        assert_eq!(transactions[1].fitid, None);
    }

    #[test]
    fn ofx_without_any_transaction_block_is_rejected() {
        assert!(matches!(
            parse_ofx_bank_statement("<OFX></OFX>"),
            Err(ImportError::Empty { .. })
        ));
    }

    #[test]
    fn ofx_is_detected_without_a_hint_including_xml_flavour() {
        let ofx2 = include_bytes!("fixtures/ofx2.ofx");
        let parsed = parse_bank_statement(ofx2, None).unwrap();
        assert_eq!(parsed.dialect.format, StatementFormat::Ofx);
        assert_eq!(parsed.transactions.len(), 3);
        assert_eq!(
            parsed.transactions[0].fitid.as_deref(),
            Some("20260115-0001")
        );
        assert_eq!(parsed.transactions[0].amount_cents, -1_250);
        let ofx1 = include_bytes!("fixtures/ofx1.ofx");
        let parsed = parse_bank_statement(ofx1, None).unwrap();
        assert_eq!(parsed.transactions.len(), 3);
        assert!(parsed.transactions.iter().all(|t| t.fitid.is_some()));
    }

    /// Les sept exports de banques françaises (anonymisés, d'après les colonnes documentées
    /// par chaque banque) s'importent sans option : même relevé de trois opérations partout.
    #[test]
    fn real_bank_exports_import_without_any_option() {
        let fixtures: [(&str, &[u8], usize); 8] = [
            ("qonto", include_bytes!("fixtures/qonto.csv"), 3),
            ("qonto-fr", include_bytes!("fixtures/qonto-fr.csv"), 3),
            ("shine", include_bytes!("fixtures/shine.csv"), 3),
            ("boursorama", include_bytes!("fixtures/boursorama.csv"), 3),
            (
                "credit-agricole",
                include_bytes!("fixtures/credit-agricole.csv"),
                3,
            ),
            ("bnp", include_bytes!("fixtures/bnp.csv"), 3),
            ("lcl", include_bytes!("fixtures/lcl.csv"), 3),
            (
                "banque-postale",
                include_bytes!("fixtures/banque-postale.csv"),
                3,
            ),
        ];
        for (name, bytes, expected) in fixtures {
            let parsed =
                parse_bank_statement(bytes, None).unwrap_or_else(|e| panic!("{name} : {e}"));
            assert_eq!(parsed.transactions.len(), expected, "{name}: {parsed:#?}");
            // Seule une ligne de pied de page (solde) peut être sautée — jamais un mouvement.
            assert!(
                parsed
                    .skipped
                    .iter()
                    .all(|(_, r)| r.contains("pied de page")),
                "{name}: {:?}",
                parsed.skipped
            );
            let amounts: Vec<i64> = parsed.transactions.iter().map(|t| t.amount_cents).collect();
            assert_eq!(amounts, vec![-1_250, -60_000, 120_000], "{name}");
            assert_eq!(
                parsed.transactions[0].occurred_on,
                date(2026, 1, 15),
                "{name}"
            );
            assert!(
                parsed.transactions[1]
                    .description
                    .to_uppercase()
                    .contains("CABINET"),
                "{name}: {}",
                parsed.transactions[1].description
            );
        }
    }

    #[test]
    fn a_latin1_export_with_debit_credit_columns_and_a_preamble_is_read() {
        let parsed =
            parse_bank_statement(include_bytes!("fixtures/credit-agricole.csv"), None).unwrap();
        assert_eq!(parsed.dialect.encoding, "windows-1252");
        assert_eq!(parsed.dialect.separator, Some(';'));
        assert_eq!(parsed.dialect.decimal, Some(','));
        assert_eq!(parsed.dialect.date_format.as_deref(), Some("JJ/MM/AAAA"));
        assert_eq!(parsed.dialect.header_line, Some(4));
        assert!(
            parsed
                .dialect
                .columns
                .contains("Crédit euros / Débit euros")
        );
        assert_eq!(
            parsed.transactions[1].description,
            "VIR SEPA CABINET COMPTA — Prélèvement"
        );
    }

    #[test]
    fn qonto_english_export_uses_the_transaction_id_as_fitid_and_the_counterparty() {
        let parsed = parse_bank_statement(include_bytes!("fixtures/qonto.csv"), None).unwrap();
        assert_eq!(parsed.dialect.separator, Some(','));
        assert_eq!(parsed.dialect.decimal, Some('.'));
        assert_eq!(
            parsed.transactions[0].fitid.as_deref(),
            Some("qonto-tx-0001")
        );
        assert!(
            parsed.transactions[1]
                .description
                .starts_with("Cabinet Compta —"),
            "{}",
            parsed.transactions[1].description
        );
    }

    #[test]
    fn a_footer_line_is_skipped_and_reported_not_fatal() {
        let csv = "Date;Libellé;Montant\n15/01/2026;FRAIS;-12,50\nSolde au 31/01/2026;;1 234,56\n\
                   TOTAL\n";
        let parsed = parse_bank_statement(csv.as_bytes(), None).unwrap();
        assert_eq!(parsed.transactions.len(), 1);
        assert_eq!(parsed.skipped.len(), 2, "{:?}", parsed.skipped);
        assert_eq!(parsed.skipped[0].0, 3);
        assert!(parsed.skipped[0].1.contains("date illisible"));
    }

    #[test]
    fn a_bom_and_thousands_separators_are_handled() {
        let csv =
            "\u{feff}date;description;montant\n15/01/2026;VIR;1 200,00\n16/01/2026;\"A;B\";-12,5\n";
        let parsed = parse_bank_statement(csv.as_bytes(), None).unwrap();
        assert_eq!(parsed.dialect.encoding, "utf-8");
        assert_eq!(parsed.transactions[0].amount_cents, 120_000);
        assert_eq!(parsed.transactions[1].amount_cents, -1_250);
        assert_eq!(parsed.transactions[1].description, "A;B");
    }

    #[test]
    fn amounts_in_every_bank_notation_are_read() {
        let cases = [
            ("-12,50", -1_250),
            ("1 200,00", 120_000),
            ("1\u{a0}200,00", 120_000),
            ("1,200.50", 120_050),
            ("-59.99", -5_999),
            ("+7800", 780_000),
            ("12,50 €", 1_250),
            ("EUR -12.50", -1_250),
            ("(12,50)", -1_250),
            ("12,5", 1_250),
            ("1.200,00", 120_000),
        ];
        for (raw, cents) in cases {
            assert_eq!(parse_amount(raw, None), Some(cents), "{raw}");
        }
        assert_eq!(parse_amount("abc", None), None);
        assert_eq!(parse_amount("1,2,3", None), None);
        assert_eq!(parse_amount("", None), None);
    }

    #[test]
    fn dates_in_every_bank_notation_are_read() {
        for (raw, fmt) in [
            ("2026-01-15", "AAAA-MM-JJ"),
            ("15/01/2026", "JJ/MM/AAAA"),
            ("15-01-2026", "JJ-MM-AAAA"),
            ("15.01.2026", "JJ.MM.AAAA"),
            ("15/01/26", "JJ/MM/AA"),
            ("20260115", "AAAAMMJJ"),
            ("2026-01-15 10:22:05", "AAAA-MM-JJ"),
            ("2026-01-15T10:22:05Z", "AAAA-MM-JJ"),
        ] {
            let (d, f) = parse_date_with_format(raw).unwrap_or_else(|| panic!("{raw}"));
            assert_eq!(d, date(2026, 1, 15), "{raw}");
            assert_eq!(f, fmt, "{raw}");
        }
        assert_eq!(parse_date_any("31/02/2026"), None);
        assert_eq!(parse_date_any("hier"), None);
    }

    #[test]
    fn a_tiime_xlsx_export_reads_the_transactions_sheet_and_ignores_balances() {
        let parsed = parse_bank_statement(include_bytes!("fixtures/tiime-transactions.xlsx"), None)
            .expect("un export Tiime xlsx doit s'importer");
        assert_eq!(parsed.dialect.format, StatementFormat::Xlsx);
        assert_eq!(parsed.transactions.len(), 3);
        assert_eq!(
            parsed
                .transactions
                .iter()
                .map(|t| t.amount_cents)
                .collect::<Vec<_>>(),
            vec![-1_250, -60_000, 120_000]
        );
        assert_eq!(parsed.transactions[0].occurred_on, date(2026, 1, 15));
        assert!(
            parsed.transactions[1]
                .description
                .to_uppercase()
                .contains("CABINET"),
            "{}",
            parsed.transactions[1].description
        );
        // La feuille « Soldes bancaires » (5 000 €, 5 587,50 €) ne doit pas devenir des mouvements.
        assert!(
            parsed
                .transactions
                .iter()
                .all(|t| t.amount_cents.abs() != 500_000 && t.amount_cents.abs() != 558_750),
            "{:?}",
            parsed.transactions
        );
    }

    #[test]
    fn a_truncated_xlsx_gives_a_message_with_the_expected_format() {
        let err = parse_bank_statement(b"PK\x03\x04not-a-workbook", None).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Attendu :"), "{err}");
    }

    #[test]
    fn a_binary_or_truncated_file_gives_a_message_with_the_expected_format() {
        let err = parse_bank_statement(b"\x00\x01\x02\xff\xfe garbage", None).unwrap_err();
        assert!(err.to_string().contains("Attendu :"), "{err}");
        let err = parse_bank_statement(b"date;description;montant\n2026-01-", None).unwrap_err();
        assert!(matches!(err, ImportError::Empty { .. }), "{err}");
        assert!(err.to_string().contains("ligne 2"), "{err}");
        let err = parse_bank_statement(b"", None).unwrap_err();
        assert!(matches!(err, ImportError::Empty { .. }), "{err}");
    }

    proptest! {
        /// Un parseur d'import ne doit jamais paniquer, quelle que soit l'entrée — c'est la
        /// propriété de robustesse attendue d'un parseur exposé à des fichiers externes.
        #[test]
        fn csv_parser_never_panics_on_arbitrary_input(input in ".*") {
            let _ = parse_csv_bank_statement(&input);
        }

        #[test]
        fn ofx_parser_never_panics_on_arbitrary_input(input in ".*") {
            let _ = parse_ofx_bank_statement(&input);
        }

        #[test]
        fn csv_parser_never_panics_on_arbitrary_multiline_input(lines in proptest::collection::vec(".*", 0..20)) {
            let _ = parse_csv_bank_statement(&lines.join("\n"));
        }

        /// Lot 38 : sur des **octets** arbitraires (pas seulement de l'UTF-8 valide), avec ou
        /// sans indice de format.
        #[test]
        fn statement_parser_never_panics_on_arbitrary_bytes(
            bytes in proptest::collection::vec(any::<u8>(), 0..400),
            hint in prop_oneof![Just(None), Just(Some(StatementFormat::Csv)), Just(Some(StatementFormat::Ofx)), Just(Some(StatementFormat::Xlsx))],
        ) {
            let _ = parse_bank_statement(&bytes, hint);
        }

        /// Des lignes CSV formées de champs arbitraires, séparateurs et guillemets compris.
        #[test]
        fn statement_parser_never_panics_on_arbitrary_csv_like_input(
            rows in proptest::collection::vec(proptest::collection::vec("[^\\n]{0,12}", 1..6), 0..12),
            sep in prop::sample::select(vec![';', ',', '\t']),
        ) {
            let text = rows.iter().map(|r| r.join(&sep.to_string())).collect::<Vec<_>>().join("\n");
            let _ = parse_bank_statement(text.as_bytes(), None);
        }

        /// Comme ci-dessus, mais en forçant des délimiteurs de bloc `<STMTTRN>`/`</STMTTRN>`
        /// dans le mélange : la fonction sur `.*` seule construit rarement ces motifs, alors
        /// que c'est précisément la logique de découpage en blocs qui doit rester robuste face
        /// à des balises tronquées, imbriquées ou dupliquées.
        #[test]
        fn ofx_parser_never_panics_on_arbitrary_input_with_block_delimiters(
            fragments in proptest::collection::vec(
                prop_oneof![".*", Just("<STMTTRN>".to_string()), Just("</STMTTRN>".to_string())],
                0..30,
            )
        ) {
            let _ = parse_ofx_bank_statement(&fragments.join(""));
        }
    }
}
