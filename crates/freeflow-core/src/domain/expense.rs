//! Dépenses professionnelles et TVA déductible.

use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::{Date, OffsetDateTime};

use super::ids::ExpenseId;
use super::money::Money;
use super::vat::VatRate;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ExpenseCategory {
    Software,
    Equipment,
    Travel,
    Meals,
    Office,
    /// Formations, cotisations professionnelles, assurances.
    Professional,
    /// Honoraires (expert-comptable, avocat, sous-traitance intellectuelle) — lot 33 : compte
    /// 622600, distingué de `Professional` parce qu'une SASU qui clôture seule paie au moins un
    /// cabinet, et que le 2033-B les isole.
    Fees,
    /// Frais bancaires (tenue de compte, commissions) — lot 33 : compte 627000 ; le plus souvent
    /// exonérés de TVA, d'où un taux `zero` attendu mais jamais imposé.
    BankCharges,
    Other,
}

#[derive(Debug, Error, PartialEq, Eq)]
#[error("catégorie de dépense inconnue : {0}")]
pub struct UnknownExpenseCategory(pub String);

impl ExpenseCategory {
    /// Toutes les catégories, dans l'ordre de présentation des façades.
    pub const ALL: [Self; 9] = [
        Self::Software,
        Self::Equipment,
        Self::Travel,
        Self::Meals,
        Self::Office,
        Self::Professional,
        Self::Fees,
        Self::BankCharges,
        Self::Other,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Software => "software",
            Self::Equipment => "equipment",
            Self::Travel => "travel",
            Self::Meals => "meals",
            Self::Office => "office",
            Self::Professional => "professional",
            Self::Fees => "fees",
            Self::BankCharges => "bank_charges",
            Self::Other => "other",
        }
    }
}

impl std::str::FromStr for ExpenseCategory {
    type Err = UnknownExpenseCategory;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "software" => Ok(Self::Software),
            "equipment" => Ok(Self::Equipment),
            "travel" => Ok(Self::Travel),
            "meals" => Ok(Self::Meals),
            "office" => Ok(Self::Office),
            "professional" => Ok(Self::Professional),
            "fees" => Ok(Self::Fees),
            "bank_charges" => Ok(Self::BankCharges),
            "other" => Ok(Self::Other),
            other => Err(UnknownExpenseCategory(other.to_string())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Expense {
    pub id: ExpenseId,
    pub label: String,
    pub category: ExpenseCategory,
    /// Montant total TTC réellement payé.
    pub amount: Money,
    pub vat_rate: VatRate,
    /// TVA effectivement déductible — pas nécessairement `amount × taux` : la loi française
    /// plafonne ou exclut la déduction pour certaines dépenses (véhicules de tourisme,
    /// restauration au-delà d'un barème, etc.). Explicite plutôt que recalculé, pour ne jamais
    /// prêter au domaine une règle fiscale qu'il ne connaît pas.
    pub vat_deductible: Money,
    pub incurred_on: Date,
    /// Hash SHA-256 du contenu du justificatif attaché, et son nom de fichier d'origine — la
    /// preuve d'intégrité, pas une copie : le stockage adressé par contenu (répertoire
    /// `receipts/` à côté du coffre) est un souci d'adaptateur, fait par la CLI avant de
    /// construire la commande (voir `freeflow-cli::expense::archive_receipt`).
    pub receipt_hash: Option<String>,
    pub receipt_filename: Option<String>,
    pub created_at: OffsetDateTime,
    /// Révision optimiste (lot 21) — voir `crate::app::revision`. La colonne SQL existait depuis
    /// la migration `0008` (posée par anticipation) ; elle n'est lue et écrite que depuis que
    /// `UpdateExpense`/`DeleteExpense` existent.
    pub revision: i64,
}
