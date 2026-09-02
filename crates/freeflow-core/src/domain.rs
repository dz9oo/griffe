//! Types métier purs (aucune IO) : argent, TVA, identifiants, entités.

mod calendar;
mod client;
mod expense;
mod ids;
mod interaction;
mod invoice;
mod mission;
mod money;
mod opening;
mod opportunity;
mod payment;
mod period;
mod quote;
pub mod serde_date;
mod siren;
mod time_entry;
mod vat;
mod vat_filing;

pub use calendar::{
    easter_sunday, french_business_days_in, french_public_holidays, is_french_business_day,
    next_french_business_day_on_or_after,
};
pub use client::{Address, Client, Contact};
pub use expense::{Expense, ExpenseCategory, UnknownExpenseCategory};
pub use ids::{
    BankTransactionId, ClientId, ContactId, ExpenseId, FiscalYearId, InteractionId, InvoiceId,
    MissionId, OpportunityId, PaymentId, QuoteId, TimeEntryId,
};
pub use interaction::{Interaction, InteractionKind, UnknownInteractionKind};
pub use invoice::{Invoice, InvoiceLine, InvoiceStatus};
pub use mission::{Milestone, MilestoneParseError, Mission, MissionKind};
pub use money::{Money, MoneyParseError};
pub use opening::{
    AccountCode, AccountCodeError, OpeningBalance, OpeningBalanceError, OpeningBalanceLine,
    OpeningBalanceLineParseError, OpeningEquity, Side, UnknownSide,
};
pub use opportunity::{
    LossReason, Opportunity, OpportunityStage, Probability, ProbabilityError, UnknownStage,
};
pub use payment::{BankTransaction, Payment, PaymentMethod, UnknownPaymentMethod};
pub use period::{
    FiscalYear, FiscalYearEnd, FiscalYearEndError, Month, MonthError, UnknownVatRegime, VatRegime,
    format_date, parse_date,
};
pub use quote::{
    Discount, LineKind, Quote, QuoteLine, QuoteLineParseError, QuoteStatus, UnknownQuoteStatus,
};
pub use siren::{Siren, SirenError, VatNumber, VatNumberError};
pub use time_entry::{TimeCategory, TimeEntry, UnknownTimeCategory};
pub use vat::{UnknownVatRate, VatRate};
pub use vat_filing::{Ca3FilingRule, VatFilerCategory, VatFilingZone};
