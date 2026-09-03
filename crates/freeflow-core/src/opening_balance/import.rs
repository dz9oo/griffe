//! Reprise du bilan d'ouverture depuis un fichier du cabinet (lot 40) : une **balance
//! générale** (CSV : compte / libellé / débit / crédit, éventuellement soldes) ou le **FEC** de
//! l'exercice précédent (18 colonnes, `|` ou tabulation — ce que Tiime et Indy exportent). Le
//! cœur ne fait que **prévisualiser** ([`ImportPreview`]) : les comptes de bilan (classes 1 à
//! 5) sont repris tels quels, les comptes de gestion (6 et 7) agrégés en un résultat posé en 120
//! (bénéfice) ou 129 (perte) — sauf si la balance porte déjà un 120/129, cas d'une balance
//! **après** affectation —, le reste écarté avec son motif. L'adaptateur soumet ensuite
//! `RecordOpeningBalance` (même rail de confirmation qu'une saisie à la main) ; l'utilisateur
//! peut corriger les lignes avant.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::Date;

use crate::billing::import::{SEPARATORS, decode, normalize, parse_amount, split_fields};
use crate::domain::{AccountCode, Money, OpeningBalance, OpeningBalanceLine, Side};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum OpeningImportError {
    #[error(
        "format non reconnu : {reason}. Attendu : une balance générale en CSV (colonnes compte, \
         libellé, débit, crédit — ou soldes débiteur/créditeur) ou un FEC (18 colonnes, \
         séparées par | ou tabulation), tels que les exportent Tiime, Indy ou votre cabinet."
    )]
    Unrecognized { reason: String },
    #[error("le fichier ne contient aucun compte exploitable{detail}")]
    Empty { detail: String },
}

/// La source reconnue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OpeningImportFormat {
    Balance,
    Fec,
}

impl std::str::FromStr for OpeningImportFormat {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "balance" => Ok(Self::Balance),
            "fec" => Ok(Self::Fec),
            other => Err(format!("format inconnu : {other} (attendu balance ou fec)")),
        }
    }
}

/// Ce que l'import propose — jamais une écriture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportPreview {
    pub format: OpeningImportFormat,
    #[serde(with = "crate::domain::serde_date::date")]
    pub opens_on: Date,
    /// Les lignes retenues, dans l'ordre des comptes — modifiables avant enregistrement.
    pub lines: Vec<OpeningBalanceLine>,
    /// `(compte ou ligne, motif)` : ce qui n'a pas été repris.
    pub dropped: Vec<(String, String)>,
    /// Le résultat dérivé des comptes 6/7 (produits − charges), posé en 120/129 — `None` si la
    /// balance était déjà après affectation (ou sans comptes de gestion).
    pub derived_result: Option<Money>,
    pub warnings: Vec<String>,
}

impl ImportPreview {
    /// Le bilan d'ouverture correspondant, à valider par `OpeningBalance::validate` avant
    /// enregistrement (l'adaptateur passe par `RecordOpeningBalance`, qui le fait).
    #[must_use]
    pub fn to_opening_balance(&self, source: Option<String>) -> OpeningBalance {
        OpeningBalance {
            opens_on: self.opens_on,
            source,
            lines: self.lines.clone(),
            tax_losses: Money::ZERO,
        }
    }
}

/// Un compte tel que lu : libellé et solde signé (débit positif).
#[derive(Debug, Default, Clone)]
struct Read {
    label: String,
    balance: i64,
}

#[allow(clippy::too_many_lines)]
fn aggregate(
    format: OpeningImportFormat,
    opens_on: Date,
    accounts: &BTreeMap<String, Read>,
    mut dropped: Vec<(String, String)>,
) -> Result<ImportPreview, OpeningImportError> {
    let mut lines = Vec::new();
    let mut warnings = Vec::new();
    let mut income: i64 = 0;
    let mut has_result_account = false;
    let mut has_depreciation = false;
    for (number, read) in accounts {
        let Ok(account) = AccountCode::parse(number) else {
            dropped.push((number.clone(), "numéro de compte invalide".to_string()));
            continue;
        };
        if read.balance == 0 {
            dropped.push((number.clone(), "solde nul".to_string()));
            continue;
        }
        match account.class() {
            1..=5 => {
                if account.starts_with("120") || account.starts_with("129") {
                    has_result_account = true;
                }
                if account.starts_with("28") || account.starts_with("29") {
                    has_depreciation = true;
                }
                let (side, amount) = if read.balance > 0 {
                    (Side::Debit, read.balance)
                } else {
                    (Side::Credit, -read.balance)
                };
                lines.push(OpeningBalanceLine {
                    account,
                    label: if read.label.trim().is_empty() {
                        format!("Compte {number}")
                    } else {
                        read.label.trim().to_string()
                    },
                    side,
                    amount: Money::from_cents(amount),
                });
            }
            6 | 7 => {
                // Un produit est un solde créditeur (négatif), une charge un solde débiteur.
                income = income.saturating_sub(read.balance);
            }
            _ => dropped.push((
                number.clone(),
                "compte spécial (classe 8 ou 9), hors bilan".to_string(),
            )),
        }
    }
    let has_management = accounts
        .keys()
        .any(|n| n.starts_with('6') || n.starts_with('7'));
    let derived_result = if has_management && income != 0 {
        if has_result_account {
            warnings.push(
                "La balance porte déjà un résultat en 120/129 *et* des comptes de gestion : les \
                 comptes 6/7 sont ignorés (balance après affectation)."
                    .to_string(),
            );
            None
        } else {
            let result = Money::from_cents(income);
            let (account, label, side) = if income >= 0 {
                ("120000", "Résultat de l'exercice (bénéfice)", Side::Credit)
            } else {
                ("129000", "Résultat de l'exercice (perte)", Side::Debit)
            };
            lines.push(OpeningBalanceLine {
                account: AccountCode::parse(account).expect("compte fixe valide"),
                label: label.to_string(),
                side,
                amount: Money::from_cents(income.abs()),
            });
            Some(result)
        }
    } else {
        None
    };
    if has_depreciation {
        warnings.push(
            "Un compte d'amortissement ou de dépréciation (28x/29x) est repris : FreeFlow ne \
             calcule pas la dotation annuelle — le résultat des exercices suivants sera \
             surestimé de ce montant tant que les amortissements ne sont pas modélisés."
                .to_string(),
        );
    }
    if lines.is_empty() {
        return Err(OpeningImportError::Empty {
            detail: if dropped.is_empty() {
                String::new()
            } else {
                format!(" ({} ligne(s) écartée(s))", dropped.len())
            },
        });
    }
    lines.sort_by(|a, b| a.account.as_str().cmp(b.account.as_str()));
    let total_debit: i64 = lines
        .iter()
        .filter(|l| l.side == Side::Debit)
        .map(|l| l.amount.cents())
        .sum();
    let total_credit: i64 = lines
        .iter()
        .filter(|l| l.side == Side::Credit)
        .map(|l| l.amount.cents())
        .sum();
    if total_debit != total_credit {
        warnings.push(format!(
            "Le bilan repris n'est pas équilibré (débit {}, crédit {}) : la balance lue est \
             incomplète ou porte un résultat non affecté — corrigez les lignes avant \
             d'enregistrer.",
            Money::from_cents(total_debit),
            Money::from_cents(total_credit)
        ));
    }
    Ok(ImportPreview {
        format,
        opens_on,
        lines,
        dropped,
        derived_result,
        warnings,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Column {
    Account,
    Label,
    Debit,
    Credit,
    DebitBalance,
    CreditBalance,
    Balance,
}

fn column_of(name: &str) -> Option<Column> {
    let n = normalize(name);
    let has = |w: &str| n.split(' ').any(|t| t == w);
    if n.is_empty() {
        return None;
    }
    if (has("solde") || has("balance")) && (has("debiteur") || has("debit")) {
        return Some(Column::DebitBalance);
    }
    if (has("solde") || has("balance")) && (has("crediteur") || has("credit")) {
        return Some(Column::CreditBalance);
    }
    if has("solde") || n == "balance" {
        return Some(Column::Balance);
    }
    if has("debit") {
        return Some(Column::Debit);
    }
    if has("credit") {
        return Some(Column::Credit);
    }
    if has("compte")
        || has("account")
        || n.starts_with('n') && n.contains("compte")
        || n == "numero"
    {
        return Some(Column::Account);
    }
    if n.contains("libell") || n.contains("intitul") || n.contains("label") || n.contains("nom") {
        return Some(Column::Label);
    }
    None
}

/// Une balance générale (CSV), telle que Tiime, Indy ou un cabinet l'exportent.
///
/// # Errors
///
/// [`OpeningImportError::Unrecognized`] sans en-tête nommant compte et débit/crédit ou soldes,
/// [`OpeningImportError::Empty`] s'il n'en sort aucun compte.
pub fn from_balance_csv(bytes: &[u8], opens_on: Date) -> Result<ImportPreview, OpeningImportError> {
    let (text, _) = decode(bytes);
    let lines: Vec<&str> = text
        .lines()
        .map(|l| l.trim_end_matches('\r'))
        .filter(|l| !l.trim().is_empty())
        .collect();
    let mut found: Option<(usize, char, BTreeMap<Column, usize>, usize)> = None;
    'outer: for sep in SEPARATORS {
        for (idx, line) in lines.iter().take(20).enumerate() {
            let fields = split_fields(line, sep);
            let mut cols: BTreeMap<Column, usize> = BTreeMap::new();
            for (i, f) in fields.iter().enumerate() {
                if let Some(c) = column_of(f) {
                    cols.entry(c).or_insert(i);
                }
            }
            let has_amounts = cols.contains_key(&Column::Balance)
                || (cols.contains_key(&Column::DebitBalance)
                    && cols.contains_key(&Column::CreditBalance))
                || (cols.contains_key(&Column::Debit) && cols.contains_key(&Column::Credit));
            if cols.contains_key(&Column::Account) && has_amounts {
                found = Some((idx, sep, cols, fields.len()));
                break 'outer;
            }
        }
    }
    let Some((header_idx, sep, cols, width)) = found else {
        return Err(OpeningImportError::Unrecognized {
            reason: "aucune ligne d'en-tête nommant le compte et les colonnes débit/crédit ou \
                     soldes"
                .to_string(),
        });
    };
    let mut accounts: BTreeMap<String, Read> = BTreeMap::new();
    let mut dropped = Vec::new();
    for (offset, line) in lines[header_idx + 1..].iter().enumerate() {
        let line_number = header_idx + 2 + offset;
        let fields = split_fields(line, sep);
        if fields.len() < width.min(3) {
            continue;
        }
        let field = |c: Column| {
            cols.get(&c)
                .and_then(|i| fields.get(*i))
                .map_or("", String::as_str)
        };
        let raw_account: String = field(Column::Account)
            .trim()
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        if raw_account.len() < 3 {
            let head = field(Column::Account).trim();
            if !head.is_empty() && !normalize(head).starts_with("total") {
                dropped.push((
                    format!("ligne {line_number}"),
                    format!("pas un numéro de compte : « {head} »"),
                ));
            }
            continue;
        }
        let amount = |c: Column| -> Option<i64> {
            let raw = field(c).trim();
            if raw.is_empty() {
                Some(0)
            } else {
                parse_amount(raw, None)
            }
        };
        let balance = if cols.contains_key(&Column::DebitBalance)
            && cols.contains_key(&Column::CreditBalance)
        {
            match (amount(Column::DebitBalance), amount(Column::CreditBalance)) {
                (Some(d), Some(c)) => Some(d.abs() - c.abs()),
                _ => None,
            }
        } else if cols.contains_key(&Column::Balance) {
            amount(Column::Balance)
        } else {
            match (amount(Column::Debit), amount(Column::Credit)) {
                (Some(d), Some(c)) => Some(d.abs() - c.abs()),
                _ => None,
            }
        };
        let Some(balance) = balance else {
            dropped.push((
                raw_account,
                format!("montant illisible (ligne {line_number})"),
            ));
            continue;
        };
        let entry = accounts.entry(raw_account).or_default();
        if entry.label.is_empty() {
            entry.label = field(Column::Label).trim().to_string();
        }
        entry.balance = entry.balance.saturating_add(balance);
    }
    aggregate(OpeningImportFormat::Balance, opens_on, &accounts, dropped)
}

/// Le FEC de l'exercice précédent (18 colonnes réglementaires, `|`, tabulation ou `;`) : les
/// soldes de chaque compte au dernier jour, à-nouveaux compris.
///
/// # Errors
///
/// [`OpeningImportError::Unrecognized`] sans en-tête `JournalCode…CompteNum…Debit|Credit`,
/// [`OpeningImportError::Empty`] s'il n'en sort aucun compte.
pub fn from_fec(bytes: &[u8], opens_on: Date) -> Result<ImportPreview, OpeningImportError> {
    let (text, _) = decode(bytes);
    let mut lines = text
        .lines()
        .map(|l| l.trim_end_matches('\r'))
        .filter(|l| !l.trim().is_empty());
    let Some(header) = lines.next() else {
        return Err(OpeningImportError::Empty {
            detail: " (fichier vide)".to_string(),
        });
    };
    let sep = ['|', '\t', ';']
        .into_iter()
        .find(|s| split_fields(header, *s).len() >= 13)
        .ok_or_else(|| OpeningImportError::Unrecognized {
            reason: "l'en-tête ne se découpe pas en 18 colonnes".to_string(),
        })?;
    let names: Vec<String> = split_fields(header, sep)
        .iter()
        .map(|n| normalize(n))
        .collect();
    let index = |wanted: &str| names.iter().position(|n| n == wanted);
    let (Some(account_col), Some(label_col), Some(debit_col), Some(credit_col)) = (
        index("comptenum"),
        index("comptelib"),
        index("debit"),
        index("credit"),
    ) else {
        return Err(OpeningImportError::Unrecognized {
            reason: "l'en-tête ne nomme pas CompteNum, CompteLib, Debit et Credit".to_string(),
        });
    };
    let mut accounts: BTreeMap<String, Read> = BTreeMap::new();
    let mut dropped = Vec::new();
    for (offset, line) in lines.enumerate() {
        let line_number = offset + 2;
        let fields = split_fields(line, sep);
        let get = |i: usize| fields.get(i).map_or("", String::as_str).trim();
        let account = get(account_col).to_string();
        if account.is_empty() {
            continue;
        }
        let money = |raw: &str| -> Option<i64> {
            if raw.is_empty() {
                Some(0)
            } else {
                parse_amount(raw, None)
            }
        };
        let (Some(debit), Some(credit)) = (money(get(debit_col)), money(get(credit_col))) else {
            dropped.push((account, format!("montant illisible (ligne {line_number})")));
            continue;
        };
        let entry = accounts.entry(account).or_default();
        if entry.label.is_empty() {
            entry.label = get(label_col).to_string();
        }
        entry.balance = entry.balance.saturating_add(debit.abs() - credit.abs());
    }
    aggregate(OpeningImportFormat::Fec, opens_on, &accounts, dropped)
}

/// Devine le format (FEC si l'en-tête commence par `JournalCode`, balance sinon) — `hint`
/// force.
///
/// # Errors
///
/// Voir [`from_balance_csv`] et [`from_fec`].
pub fn import_opening_balance(
    bytes: &[u8],
    opens_on: Date,
    hint: Option<OpeningImportFormat>,
) -> Result<ImportPreview, OpeningImportError> {
    let format = hint.unwrap_or_else(|| {
        let (text, _) = decode(&bytes[..bytes.len().min(512)]);
        if normalize(text.lines().next().unwrap_or_default()).starts_with("journalcode") {
            OpeningImportFormat::Fec
        } else {
            OpeningImportFormat::Balance
        }
    });
    match format {
        OpeningImportFormat::Balance => from_balance_csv(bytes, opens_on),
        OpeningImportFormat::Fec => from_fec(bytes, opens_on),
    }
}

/// La vue JSON partagée par `year opening import --dry-run --json` et l'outil MCP.
#[must_use]
pub fn preview_json(preview: &ImportPreview) -> serde_json::Value {
    serde_json::json!({
        "format": preview.format,
        "opens_on": crate::domain::format_date(preview.opens_on),
        "lines": preview.lines.iter().map(|l| serde_json::json!({
            "account": l.account.as_str(),
            "label": l.label,
            "side": l.side.as_str(),
            "amount_cents": l.amount.cents(),
            "spec": l.to_string(),
        })).collect::<Vec<_>>(),
        "dropped": preview.dropped,
        "derived_result_cents": preview.derived_result.map(Money::cents),
        "warnings": preview.warnings,
        "balanced": preview.to_opening_balance(None).validate().is_ok(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn opens() -> Date {
        time::macros::date!(2025 - 10 - 01)
    }

    fn line_of<'a>(preview: &'a ImportPreview, account: &str) -> &'a OpeningBalanceLine {
        preview
            .lines
            .iter()
            .find(|l| l.account.as_str() == account)
            .unwrap_or_else(|| panic!("{account} absent : {:#?}", preview.lines))
    }

    /// La balance du cabinet (14 comptes, avant affectation, avec un préambule et un total) :
    /// bilan d'ouverture équilibré, résultat de 1 200 € dérivé en 120, capitaux propres
    /// reconstitués (report à nouveau 5 150 + 1 200 = 6 350 €, comme le scénario de l'audit),
    /// avertissement sur l'amortissement.
    #[test]
    fn a_cabinet_balance_becomes_a_balanced_opening_balance_with_the_result_in_120() {
        let preview =
            from_balance_csv(include_bytes!("fixtures/balance-cabinet.csv"), opens()).unwrap();
        assert_eq!(preview.format, OpeningImportFormat::Balance);
        assert_eq!(preview.derived_result, Some(Money::from_cents(120_000)));
        assert_eq!(preview.lines.len(), 11, "{:#?}", preview.lines);
        assert_eq!(
            line_of(&preview, "401000").amount,
            Money::from_cents(60_000)
        );
        assert_eq!(line_of(&preview, "401000").side, Side::Credit);
        assert_eq!(
            line_of(&preview, "512000").amount,
            Money::from_cents(854_000)
        );
        assert_eq!(line_of(&preview, "120000").side, Side::Credit);
        assert_eq!(
            line_of(&preview, "120000").amount,
            Money::from_cents(120_000)
        );
        assert!(preview.dropped.is_empty(), "{:?}", preview.dropped);
        assert!(preview.warnings.iter().any(|w| w.contains("amortissement")));
        let balance = preview.to_opening_balance(Some("cabinet".into()));
        balance.validate().unwrap();
        let equity = balance.equity();
        assert_eq!(equity.share_capital, Money::from_cents(100_000));
        assert_eq!(equity.legal_reserve, Money::from_cents(10_000));
        assert_eq!(equity.retained_earnings, Money::from_cents(635_000));
    }

    /// Le FEC de l'exercice précédent (export Tiime, `|`) donne exactement le même bilan.
    #[test]
    fn the_previous_fec_gives_the_same_opening_balance() {
        let from_fec = from_fec(include_bytes!("fixtures/fec-tiime.txt"), opens()).unwrap();
        let from_balance =
            from_balance_csv(include_bytes!("fixtures/balance-cabinet.csv"), opens()).unwrap();
        let strip = |p: &ImportPreview| -> Vec<(String, Side, i64)> {
            p.lines
                .iter()
                .map(|l| (l.account.to_string(), l.side, l.amount.cents()))
                .collect()
        };
        assert_eq!(strip(&from_fec), strip(&from_balance));
        assert_eq!(from_fec.derived_result, Some(Money::from_cents(120_000)));
        // Détection du format sans indice, et en tabulation.
        let tabbed =
            String::from_utf8_lossy(include_bytes!("fixtures/fec-tiime.txt")).replace('|', "\t");
        let detected = import_opening_balance(tabbed.as_bytes(), opens(), None).unwrap();
        assert_eq!(detected.format, OpeningImportFormat::Fec);
        assert_eq!(strip(&detected), strip(&from_balance));
    }

    /// Une balance déjà affectée (120 présent, 6/7 encore là) ne dérive pas de second résultat.
    #[test]
    fn a_balance_after_appropriation_keeps_its_120_and_ignores_management_accounts() {
        let csv = "Compte;Libellé;Débit;Crédit\n101000;Capital;;1000\n120000;Résultat;;200\n\
                   512000;Banque;1200;\n706000;Ventes;;200\n";
        let preview = from_balance_csv(csv.as_bytes(), opens()).unwrap();
        assert_eq!(preview.derived_result, None);
        assert_eq!(preview.lines.len(), 3);
        assert!(
            preview
                .warnings
                .iter()
                .any(|w| w.contains("après affectation"))
        );
        assert!(preview.to_opening_balance(None).validate().is_ok());
    }

    #[test]
    fn unreadable_input_says_what_is_expected_and_never_panics() {
        let err = from_balance_csv(b"n'importe quoi\nsans en-tete", opens()).unwrap_err();
        assert!(err.to_string().contains("Attendu :"), "{err}");
        let err = import_opening_balance(b"JournalCode|x", opens(), None).unwrap_err();
        assert!(
            matches!(err, OpeningImportError::Unrecognized { .. }),
            "{err}"
        );
        assert_eq!(
            preview_json(
                &from_balance_csv(include_bytes!("fixtures/balance-cabinet.csv"), opens()).unwrap()
            )["balanced"],
            true
        );
    }

    proptest! {
        #[test]
        fn the_importers_never_panic_on_arbitrary_bytes(
            bytes in proptest::collection::vec(any::<u8>(), 0..400),
            hint in prop_oneof![Just(None), Just(Some(OpeningImportFormat::Balance)), Just(Some(OpeningImportFormat::Fec))],
        ) {
            let _ = import_opening_balance(&bytes, opens(), hint);
        }
    }
}
