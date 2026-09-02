//! Import de relevés bancaires (CSV, OFX) : uniquement des fonctions pures d'analyse, aucune
//! IO, aucune écriture en base — c'est [`super::commands::ImportBankTransactions`] qui
//! persiste le résultat.

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
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ImportError {
    #[error("ligne {line} : {reason}")]
    MalformedLine { line: usize, reason: String },
    #[error("le fichier ne contient aucune transaction exploitable")]
    Empty,
}

/// Analyse un export CSV au format `date;description;montant` (en-tête obligatoire à ignorer,
/// séparateur point-virgule, date `AAAA-MM-JJ`, montant décimal à `.` ou `,`, positif =
/// encaissement).
///
/// # Errors
///
/// Retourne une erreur si une ligne non vide ne respecte pas ce format, ou si le fichier ne
/// contient aucune transaction après l'en-tête.
pub fn parse_csv_bank_statement(input: &str) -> Result<Vec<ParsedTransaction>, ImportError> {
    let mut lines = input.lines().enumerate();
    lines.next(); // ignore l'en-tête

    let mut transactions = Vec::new();
    for (index, raw) in lines {
        let line_number = index + 1;
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        let fields: Vec<&str> = raw.split(';').collect();
        if fields.len() != 3 {
            return Err(ImportError::MalformedLine {
                line: line_number,
                reason: format!("attendu 3 champs, trouvé {}", fields.len()),
            });
        }
        let occurred_on =
            domain::parse_date(fields[0].trim()).map_err(|e| ImportError::MalformedLine {
                line: line_number,
                reason: format!("date invalide : {e}"),
            })?;
        let description = fields[1].trim().to_string();
        let amount_cents =
            parse_decimal_amount(fields[2].trim()).ok_or_else(|| ImportError::MalformedLine {
                line: line_number,
                reason: format!("montant invalide : {}", fields[2]),
            })?;
        transactions.push(ParsedTransaction {
            occurred_on,
            amount_cents,
            description,
        });
    }
    if transactions.is_empty() {
        return Err(ImportError::Empty);
    }
    Ok(transactions)
}

/// Analyse un export OFX 1.x (SGML), sous-ensemble pragmatique : extrait `DTPOSTED`, `TRNAMT`
/// et `NAME`/`MEMO` de chaque bloc `<STMTTRN>`. Ne vise pas la conformité complète à la
/// spécification OFX — seulement ce qu'il faut pour rapprocher un relevé.
///
/// # Errors
///
/// Retourne une erreur si un bloc `<STMTTRN>` est incomplet, ou si le fichier n'en contient
/// aucun.
pub fn parse_ofx_bank_statement(input: &str) -> Result<Vec<ParsedTransaction>, ImportError> {
    let mut transactions = Vec::new();
    for block in input.split("<STMTTRN>").skip(1) {
        let block = block.split("</STMTTRN>").next().unwrap_or(block);

        let amount_str =
            extract_ofx_tag(block, "TRNAMT").ok_or_else(|| ImportError::MalformedLine {
                line: 0,
                reason: "TRNAMT manquant".to_string(),
            })?;
        let date_str =
            extract_ofx_tag(block, "DTPOSTED").ok_or_else(|| ImportError::MalformedLine {
                line: 0,
                reason: "DTPOSTED manquant".to_string(),
            })?;
        let description = extract_ofx_tag(block, "NAME")
            .or_else(|| extract_ofx_tag(block, "MEMO"))
            .unwrap_or_default();

        let amount_cents =
            parse_decimal_amount(&amount_str).ok_or_else(|| ImportError::MalformedLine {
                line: 0,
                reason: format!("montant OFX invalide : {amount_str}"),
            })?;
        let occurred_on = parse_ofx_date(&date_str).ok_or_else(|| ImportError::MalformedLine {
            line: 0,
            reason: format!("date OFX invalide : {date_str}"),
        })?;

        transactions.push(ParsedTransaction {
            occurred_on,
            amount_cents,
            description: description.trim().to_string(),
        });
    }
    if transactions.is_empty() {
        return Err(ImportError::Empty);
    }
    Ok(transactions)
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

/// Montant décimal (`.` ou `,` comme séparateur, deux décimales maximum) en centimes.
fn parse_decimal_amount(s: &str) -> Option<i64> {
    crate::domain::Money::parse_decimal(s)
        .ok()
        .map(crate::domain::Money::cents)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

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
        assert_eq!(
            err,
            ImportError::MalformedLine {
                line: 3,
                reason: "attendu 3 champs, trouvé 1".to_string()
            }
        );
    }

    #[test]
    fn empty_csv_after_header_is_rejected() {
        assert_eq!(
            parse_csv_bank_statement("date;description;montant\n"),
            Err(ImportError::Empty)
        );
    }

    #[test]
    fn parses_a_well_formed_ofx_statement() {
        let ofx = "<OFX><BANKTRANLIST>\n\
                   <STMTTRN><TRNTYPE>CREDIT<DTPOSTED>20260830120000<TRNAMT>7800.00<NAME>Lumen Bank</STMTTRN>\n\
                   <STMTTRN><TRNTYPE>DEBIT<DTPOSTED>20260831<TRNAMT>-59.99<MEMO>Adobe</STMTTRN>\n\
                   </BANKTRANLIST></OFX>";
        let transactions = parse_ofx_bank_statement(ofx).unwrap();
        assert_eq!(transactions.len(), 2);
        assert_eq!(transactions[0].amount_cents, 780_000);
        assert_eq!(transactions[0].description, "Lumen Bank");
        assert_eq!(transactions[1].amount_cents, -5_999);
        assert_eq!(transactions[1].description, "Adobe");
    }

    #[test]
    fn ofx_without_any_transaction_block_is_rejected() {
        assert_eq!(
            parse_ofx_bank_statement("<OFX></OFX>"),
            Err(ImportError::Empty)
        );
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
