//! Documents de clôture d'exercice (lot 20) : PV d'approbation, décision d'affectation,
//! synthèse comptable — rendus en PDF via Typst, sur le patron de `griffe-invoice` — et
//! export des données de liasse (2065/2033) en structure JSON. Comme `griffe-invoice`, ce
//! crate est l'« adaptateur documentaire » : il reçoit des données du domaine déjà lues
//! (`CompanyProfile`, `FiscalYearRecord`, `AccountingResult`, `BalanceSheet`), ne touche jamais
//! la base, et c'est lui qui fait l'IO process vers le binaire `typst`.

mod appropriation;
mod balance_sheet;
mod efi_notice;
mod error;
mod inventory;
mod liasse;
mod minutes;
mod synthesis;
mod typst;

pub use appropriation::render_appropriation_decision;
pub use balance_sheet::render_balance_sheet;
pub use efi_notice::render_efi_notice;
pub use error::DocsError;
pub use inventory::render_inventory;
pub use liasse::{LiasseEntry, LiasseExport, liasse_export};
pub use minutes::render_approval_minutes;
pub use synthesis::render_synthesis;
