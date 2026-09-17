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

/// Qui produit la pièce : `FreeFlow` la fige, ou l'utilisateur l'apporte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaperWhence {
    /// Originaux nés ici (facture, FEC, PV, liasse…).
    BornHere,
    /// Pièces extérieures à déposer (Kbis, statuts, relevé, justificatif…).
    Bring,
}

/// Ce qu'un fondateur de SASU a besoin de lire sur une nature de pièce.
/// Une seule source pour la fenêtre, la CLI et le lexique des papiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaperBrief {
    /// Titre en français courant, pas le sigle.
    pub title: &'static str,
    /// Intitulé exact, celui qu'on trouverait sur le document.
    pub official: &'static str,
    /// Une ou deux phrases : à quoi ça sert.
    pub what: &'static str,
    /// D'où ça vient, concrètement.
    pub whence: &'static str,
    pub origin: PaperWhence,
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

    /// Libellé français d'une nature — une seule source pour CLI, fenêtre et checklist.
    /// Compact (listes, JSON humain). Pour expliquer la pièce, voir [`Self::brief`].
    #[must_use]
    pub const fn label_fr(self) -> &'static str {
        match self {
            Self::IssuedInvoice => "facture émise",
            Self::CreditNote => "avoir",
            Self::Fec => "FEC",
            Self::Minutes => "PV",
            Self::Appropriation => "affectation",
            Self::Synthesis => "synthèse",
            Self::BalanceSheet => "bilan",
            Self::Inventory => "inventaire",
            Self::EfiNotice => "notice EFI",
            Self::Liasse => "liasse",
            Self::BankStatement => "relevé",
            Self::ExpenseReceipt => "justificatif",
            Self::Statutes => "statuts",
            Self::Kbis => "Kbis",
            Self::ShareLedger => "registre des mouvements de titres",
            Self::ClientContract => "contrat",
            Self::Insurance => "assurance",
            Self::TaxNotice => "avis d'imposition",
            Self::FilingAck => "accusé de dépôt",
            Self::Payroll => "bulletin de paie",
            Self::Other => "autre",
        }
    }

    /// Fiche lisible par quelqu'un qui vient de créer sa SASU.
    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub const fn brief(self) -> PaperBrief {
        match self {
            Self::IssuedInvoice => PaperBrief {
                title: "La facture que vous avez envoyée",
                official: "Facture (Factur-X, norme EN 16931)",
                what: "Le document qui dit ce que le client vous doit, et la TVA. Une facture \
                       émise ne se modifie plus : un avoir l'annule.",
                whence: "Née ici si vous émettez dans Griffe (le premier rendu gagne). Sinon \
                         vous l'apportez : le PDF de la PA, collé au dossier.",
                origin: PaperWhence::BornHere,
            },
            Self::CreditNote => PaperBrief {
                title: "L'avoir qui annule une facture",
                official: "Avoir (note de crédit)",
                what: "Le document qui annule une facture déjà émise. La facture d'origine reste ; \
                       l'avoir la contre-écrit.",
                whence: "Né ici si vous émettez l'avoir dans Griffe (le premier rendu gagne). \
                         Sinon vous l'apportez : le PDF de la PA, collé au dossier.",
                origin: PaperWhence::BornHere,
            },
            Self::Fec => PaperBrief {
                title: "Le fichier que le contrôleur demandera",
                official: "Fichier des Écritures Comptables (FEC)",
                what: "Toutes les écritures de l'exercice, dans le format que l'administration \
                       impose. Ce n'est pas une attestation : c'est le fichier qu'on tend le \
                       jour d'un contrôle.",
                whence: "FreeFlow le fige à la clôture. Vous n'allez nulle part le chercher.",
                origin: PaperWhence::BornHere,
            },
            Self::Minutes => PaperBrief {
                title: "La décision écrite de l'associé unique",
                official: "Procès-verbal des décisions de l'associé unique",
                what: "En SASU, c'est vous qui arrêtez les comptes. Le PV consigne cette \
                       décision, à prendre dans les six mois de la clôture. Signez-le, datez-le, \
                       tenez-le au registre des décisions.",
                whence: "FreeFlow le rédige à l'approbation.",
                origin: PaperWhence::BornHere,
            },
            Self::Appropriation => PaperBrief {
                title: "Ce que devient le bénéfice — ou la perte",
                official: "Décision d'affectation du résultat",
                what: "Répartir le bénéfice : réserve légale d'abord, puis dividendes et/ou \
                       report à nouveau. Une perte va en entier au report à nouveau.",
                whence: "FreeFlow le rédige à l'approbation, d'après ce que vous avez décidé à \
                         la clôture.",
                origin: PaperWhence::BornHere,
            },
            Self::Synthesis => PaperBrief {
                title: "Ce que l'année a rapporté",
                official: "Compte de résultat simplifié",
                what: "Les ventes moins les charges, puis l'impôt. La photo de l'exercice en une \
                       page, avec l'année d'avant si elle est close.",
                whence: "FreeFlow le fige à la clôture.",
                origin: PaperWhence::BornHere,
            },
            Self::BalanceSheet => PaperBrief {
                title: "La photo de la société au dernier jour",
                official: "Bilan (imprimé 2033-A)",
                what: "Ce que la société possède (banque, créances) et ce qu'elle doit (dettes, \
                       capital, bénéfices non distribués). Les deux totaux sont toujours égaux.",
                whence: "FreeFlow le dérive du grand livre, à la clôture.",
                origin: PaperWhence::BornHere,
            },
            Self::Inventory => PaperBrief {
                title: "La liste des comptes, arrêtée",
                official: "Inventaire (balance des comptes)",
                what: "Le détail, compte par compte, qui justifie le bilan. La loi demande de le \
                       tenir.",
                whence: "FreeFlow le produit à la clôture, avec le bilan.",
                origin: PaperWhence::BornHere,
            },
            Self::EfiNotice => PaperBrief {
                title: "La notice pour recopier les cases",
                official: "Notice de saisie EFI (espace professionnel impots.gouv.fr)",
                what: "Chaque case de la liasse, le montant, et où la saisir sur le site des \
                       impôts. FreeFlow ne transmet rien : vous recopiez.",
                whence: "FreeFlow la rédige à la clôture. Le dépôt se fait sur impots.gouv.fr.",
                origin: PaperWhence::BornHere,
            },
            Self::Liasse => PaperBrief {
                title: "Les chiffres à déclarer pour l'impôt",
                official: "Liasse fiscale (formulaire 2065 et tableaux 2033)",
                what: "La déclaration annuelle de résultat : bilan, compte de résultat, suivi \
                       des déficits. À télétransmettre dans les trois mois de la clôture.",
                whence: "FreeFlow en fournit les cases. Vous les recopiez sur impots.gouv.fr \
                         (régime simplifié, en EFI).",
                origin: PaperWhence::BornHere,
            },
            Self::BankStatement => PaperBrief {
                title: "Le relevé de votre banque",
                official: "Relevé de compte bancaire",
                what: "La liste des mouvements telle que la banque l'a vue. C'est elle qui fait \
                       foi pour le solde.",
                whence: "Exportez-le depuis votre banque (CSV, OFX ou Excel), puis importez-le \
                         ici. FreeFlow fige le fichier importé.",
                origin: PaperWhence::Bring,
            },
            Self::ExpenseReceipt => PaperBrief {
                title: "La preuve d'une dépense",
                official: "Justificatif de dépense (facture fournisseur, ticket)",
                what: "Sans cette pièce, la dépense et sa TVA ne tiennent pas en cas de \
                       contrôle. Un scan déposé est une copie de travail ; le papier reste \
                       l'original.",
                whence: "Joignez le PDF ou la photo depuis la dépense, ou déposez-le ici.",
                origin: PaperWhence::Bring,
            },
            Self::Statutes => PaperBrief {
                title: "Les règles de la société",
                official: "Statuts de la SASU",
                what: "L'acte constitutif : dénomination, siège, capital, objet, président. À \
                       garder jusqu'à la radiation.",
                whence: "Chez le notaire ou l'avocat qui les a rédigés, ou au guichet unique \
                         (INPI) où ils ont été déposés à la création.",
                origin: PaperWhence::Bring,
            },
            Self::Kbis => PaperBrief {
                title: "L'extrait officiel de la société",
                official: "Extrait Kbis (registre du commerce et des sociétés)",
                what: "La carte d'identité de la SASU : SIREN, siège, président, capital. Un \
                       extrait de moins de trois mois est souvent demandé.",
                whence: "Sur le guichet unique de l'INPI (formalites.entreprises.gouv.fr) ou \
                         infogreffe.fr — extraire un Kbis.",
                origin: PaperWhence::Bring,
            },
            Self::ShareLedger => PaperBrief {
                title: "Qui détient les actions",
                official: "Registre des mouvements de titres",
                what: "En SASU, souvent une seule ligne : vous, toutes les actions. Il se tient \
                       dès la création, même s'il ne bouge pas.",
                whence: "Vous le tenez. Un modèle se trouve souvent dans le kit de constitution ; \
                         déposez-en une copie ici.",
                origin: PaperWhence::Bring,
            },
            Self::ClientContract => PaperBrief {
                title: "Le contrat avec un client",
                official: "Contrat de prestation / mission",
                what: "L'accord signé qui dit le travail, le prix, les délais. Utile le jour \
                       d'un contrôle, et le jour d'un désaccord.",
                whence: "Votre exemplaire signé. Déposez-en une copie.",
                origin: PaperWhence::Bring,
            },
            Self::Insurance => PaperBrief {
                title: "L'attestation d'assurance",
                official: "Attestation d'assurance (responsabilité civile professionnelle)",
                what: "La preuve que la société est couverte. Un client ou un bailleur la \
                       demande souvent.",
                whence: "Chez votre assureur, espace client, attestation à jour.",
                origin: PaperWhence::Bring,
            },
            Self::TaxNotice => PaperBrief {
                title: "Ce que l'impôt vous a notifié",
                official: "Avis d'imposition (impôt sur les sociétés, CFE, TVA)",
                what: "Le document de l'administration qui dit le montant dû, ou le crédit. À \
                       garder avec l'exercice.",
                whence: "Votre espace professionnel sur impots.gouv.fr, messagerie ou \
                         « Consulter mes avis ».",
                origin: PaperWhence::Bring,
            },
            Self::FilingAck => PaperBrief {
                title: "La preuve que c'est déposé",
                official: "Accusé de dépôt (liasse, comptes au greffe)",
                what: "Le récépissé du site après un dépôt. C'est ce qui prouve que vous avez \
                       déclaré à temps.",
                whence: "Après le dépôt : impots.gouv.fr (liasse) ou le guichet unique INPI \
                         (comptes). Téléchargez l'accusé, déposez-le ici.",
                origin: PaperWhence::Bring,
            },
            Self::Payroll => PaperBrief {
                title: "Le bulletin de paie",
                official: "Bulletin de paie",
                what: "Si le président est rémunéré, chaque bulletin. FreeFlow ne fait pas la \
                       paie : il en archive la pièce.",
                whence: "Chez l'expert-paie ou l'outil de paie. Déposez le PDF.",
                origin: PaperWhence::Bring,
            },
            Self::Other => PaperBrief {
                title: "Une autre pièce",
                official: "Autre document",
                what: "Tout ce qui n'a pas de case, et que vous voulez quand même au coffre.",
                whence: "Vous le déposez. Dites dans la note ce que c'est.",
                origin: PaperWhence::Bring,
            },
        }
    }

    /// `true` si `FreeFlow` produit et fige cette nature.
    #[must_use]
    pub const fn is_born_here(self) -> bool {
        matches!(self.brief().origin, PaperWhence::BornHere)
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

    #[test]
    fn every_sasu_kind_has_a_brief_a_founder_can_read() {
        for kind in PaperKind::sasu_kinds() {
            let b = kind.brief();
            assert!(!b.title.is_empty(), "{kind}");
            assert!(!b.official.is_empty(), "{kind}");
            assert!(!b.what.is_empty(), "{kind}");
            assert!(!b.whence.is_empty(), "{kind}");
            assert_ne!(
                b.title,
                kind.label_fr(),
                "le titre n'est pas le sigle : {kind}"
            );
        }
    }

    #[test]
    fn issued_invoice_brief_mentions_a_brought_original() {
        let b = PaperKind::IssuedInvoice.brief();
        assert!(
            b.whence.contains("apporte") || b.whence.contains("PA"),
            "{}",
            b.whence
        );
        assert_eq!(b.origin, PaperWhence::BornHere);
    }

    #[test]
    fn the_fec_brief_names_the_official_file_and_is_born_here() {
        let b = PaperKind::Fec.brief();
        assert!(
            b.official.contains("Fichier des Écritures Comptables"),
            "{}",
            b.official
        );
        assert_eq!(b.origin, PaperWhence::BornHere);
        assert!(
            b.whence.contains("clôture") || b.whence.contains("FreeFlow"),
            "{}",
            b.whence
        );
    }

    #[test]
    fn the_kbis_brief_points_to_the_inpi_and_is_brought() {
        let b = PaperKind::Kbis.brief();
        assert!(b.official.contains("Kbis"), "{}", b.official);
        assert!(b.whence.contains("INPI"), "{}", b.whence);
        assert_eq!(b.origin, PaperWhence::Bring);
        assert!(!PaperKind::Kbis.is_born_here());
        assert!(PaperKind::Fec.is_born_here());
    }
}
