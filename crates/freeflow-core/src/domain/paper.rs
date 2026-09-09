//! Catalogue des pièces d'une SASU et durées de conservation — **pur**, aucune IO.
//!
//! Deux horloges (spec archives légales) : 10 ans après la clôture de l'exercice
//! ([C. com. L123-22](https://www.legifrance.gouv.fr/codes/article_lc/LEGIARTI000006219327/))
//! pour presque tout ce qui est comptable ; jusqu'à la radiation pour les statuts et le Kbis.
//! On code le plus long : L102 B à 6 ans n'autorise pas à jeter à 7 ans.
//!
//! V1 SASU seulement : pas de `PaperKind` micro (livre des recettes) dans ce module.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::Date;

use super::asset::add_months;

/// Nature d'une pièce au coffre. Serde `snake_case` : contrat JSON CLI / MCP / audit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaperKind {
    IssuedInvoice,
    CreditNote,
    Fec,
    Minutes,
    Appropriation,
    Synthesis,
    BalanceSheet,
    Inventory,
    EfiNotice,
    Liasse,
    BankStatement,
    ExpenseReceipt,
    Statutes,
    Kbis,
    ShareLedger,
    ClientContract,
    Insurance,
    TaxNotice,
    FilingAck,
    Payroll,
    Other,
}

/// Provenance de la pièce : née ici, importée (relevé, justificatif), ou déposée à la main.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaperOrigin {
    Issued,
    Imported,
    Uploaded,
}

/// Horloge de conservation d'une nature de pièce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetentionClock {
    /// 10 ans après la clôture de `period` (année civile de l'exercice).
    TenYearsAfterYearEnd,
    /// Pas de date de fin tant que la société existe (statuts, Kbis).
    UntilDissolution,
}

#[derive(Debug, Error, PartialEq, Eq)]
#[error("nature de pièce inconnue : {0}")]
pub struct UnknownPaperKind(pub String);

#[derive(Debug, Error, PartialEq, Eq)]
#[error("origine de pièce inconnue : {0}")]
pub struct UnknownPaperOrigin(pub String);

impl PaperKind {
    /// Toutes les natures v1 SASU, dans un ordre stable (présentation des façades).
    #[must_use]
    pub const fn sasu_kinds() -> &'static [Self] {
        &[
            Self::IssuedInvoice,
            Self::CreditNote,
            Self::Fec,
            Self::Minutes,
            Self::Appropriation,
            Self::Synthesis,
            Self::BalanceSheet,
            Self::Inventory,
            Self::EfiNotice,
            Self::Liasse,
            Self::BankStatement,
            Self::ExpenseReceipt,
            Self::Statutes,
            Self::Kbis,
            Self::ShareLedger,
            Self::ClientContract,
            Self::Insurance,
            Self::TaxNotice,
            Self::FilingAck,
            Self::Payroll,
            Self::Other,
        ]
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::IssuedInvoice => "issued_invoice",
            Self::CreditNote => "credit_note",
            Self::Fec => "fec",
            Self::Minutes => "minutes",
            Self::Appropriation => "appropriation",
            Self::Synthesis => "synthesis",
            Self::BalanceSheet => "balance_sheet",
            Self::Inventory => "inventory",
            Self::EfiNotice => "efi_notice",
            Self::Liasse => "liasse",
            Self::BankStatement => "bank_statement",
            Self::ExpenseReceipt => "expense_receipt",
            Self::Statutes => "statutes",
            Self::Kbis => "kbis",
            Self::ShareLedger => "share_ledger",
            Self::ClientContract => "client_contract",
            Self::Insurance => "insurance",
            Self::TaxNotice => "tax_notice",
            Self::FilingAck => "filing_ack",
            Self::Payroll => "payroll",
            Self::Other => "other",
        }
    }

    #[must_use]
    pub const fn clock(self) -> RetentionClock {
        match self {
            Self::Statutes | Self::Kbis => RetentionClock::UntilDissolution,
            Self::IssuedInvoice
            | Self::CreditNote
            | Self::Fec
            | Self::Minutes
            | Self::Appropriation
            | Self::Synthesis
            | Self::BalanceSheet
            | Self::Inventory
            | Self::EfiNotice
            | Self::Liasse
            | Self::BankStatement
            | Self::ExpenseReceipt
            | Self::ShareLedger
            | Self::ClientContract
            | Self::Insurance
            | Self::TaxNotice
            | Self::FilingAck
            | Self::Payroll
            | Self::Other => RetentionClock::TenYearsAfterYearEnd,
        }
    }
}

impl fmt::Display for PaperKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for PaperKind {
    type Err = UnknownPaperKind;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        for kind in Self::sasu_kinds() {
            if kind.as_str() == s {
                return Ok(*kind);
            }
        }
        Err(UnknownPaperKind(s.to_string()))
    }
}

impl PaperOrigin {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Issued => "issued",
            Self::Imported => "imported",
            Self::Uploaded => "uploaded",
        }
    }
}

impl fmt::Display for PaperOrigin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for PaperOrigin {
    type Err = UnknownPaperOrigin;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "issued" => Ok(Self::Issued),
            "imported" => Ok(Self::Imported),
            "uploaded" => Ok(Self::Uploaded),
            other => Err(UnknownPaperOrigin(other.to_string())),
        }
    }
}

/// `year_end` = dernier jour de l'exercice portant la pièce (`None` si pas encore connu).
/// `UntilDissolution` → `None` (on ne purge pas). Sans `year_end`, une horloge à 10 ans
/// n'est pas encore calculable : `None`, on ne purge pas.
#[must_use]
pub fn retained_until(kind: PaperKind, year_end: Option<Date>) -> Option<Date> {
    match kind.clock() {
        RetentionClock::UntilDissolution => None,
        RetentionClock::TenYearsAfterYearEnd => year_end.map(|end| add_months(end, 120)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::Month;

    fn d(year: i32, month: u8, day: u8) -> Date {
        Date::from_calendar_date(year, Month::try_from(month).unwrap(), day).unwrap()
    }

    #[test]
    fn retained_until_an_issued_invoice_is_ten_years_after_year_end() {
        assert_eq!(
            retained_until(PaperKind::IssuedInvoice, Some(d(2026, 9, 30))),
            Some(d(2036, 9, 30))
        );
    }

    #[test]
    fn statutes_and_kbis_have_no_retention_end() {
        assert_eq!(
            retained_until(PaperKind::Statutes, Some(d(2026, 9, 30))),
            None
        );
        assert_eq!(retained_until(PaperKind::Kbis, Some(d(2026, 12, 31))), None);
        assert_eq!(
            PaperKind::Statutes.clock(),
            RetentionClock::UntilDissolution
        );
        assert_eq!(PaperKind::Kbis.clock(), RetentionClock::UntilDissolution);
    }

    #[test]
    fn ten_year_kinds_without_year_end_are_not_yet_calculable() {
        assert_eq!(retained_until(PaperKind::Fec, None), None);
        assert_eq!(retained_until(PaperKind::IssuedInvoice, None), None);
    }

    #[test]
    fn every_sasu_kind_has_the_catalogue_clock() {
        for kind in PaperKind::sasu_kinds() {
            match *kind {
                PaperKind::Statutes | PaperKind::Kbis => {
                    assert_eq!(kind.clock(), RetentionClock::UntilDissolution, "{kind}");
                    assert_eq!(retained_until(*kind, Some(d(2026, 9, 30))), None);
                }
                _ => {
                    assert_eq!(kind.clock(), RetentionClock::TenYearsAfterYearEnd, "{kind}");
                    assert_eq!(
                        retained_until(*kind, Some(d(2026, 9, 30))),
                        Some(d(2036, 9, 30)),
                        "{kind}"
                    );
                }
            }
        }
    }

    #[test]
    fn as_str_round_trips_through_serde_snake_case() {
        for kind in PaperKind::sasu_kinds() {
            let json = serde_json::to_string(kind).unwrap();
            assert_eq!(json, format!("\"{}\"", kind.as_str()), "{kind}");
            let parsed: PaperKind = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, *kind);
            assert_eq!(kind.as_str().parse::<PaperKind>().unwrap(), *kind);
        }
        for origin in [
            PaperOrigin::Issued,
            PaperOrigin::Imported,
            PaperOrigin::Uploaded,
        ] {
            let json = serde_json::to_string(&origin).unwrap();
            assert_eq!(json, format!("\"{}\"", origin.as_str()));
            let parsed: PaperOrigin = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, origin);
        }
    }

    #[test]
    fn from_str_refuses_micro_revenue_book() {
        assert_eq!(
            "micro_revenue_book".parse::<PaperKind>(),
            Err(UnknownPaperKind("micro_revenue_book".into()))
        );
    }
}
