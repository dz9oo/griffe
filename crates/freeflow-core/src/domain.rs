//! Types métier purs (aucune IO) : argent, TVA, identifiants, entités.

mod calendar;
mod client;
mod expense;
mod ids;
mod interaction;
mod invoice;
mod mission;
mod money;
mod opportunity;
mod payment;
mod period;
mod quote;
mod siren;
mod time_entry;
mod vat;

pub use calendar::{
    easter_sunday, french_business_days_in, french_public_holidays, is_french_business_day,
};
pub use client::{Address, Client, Contact};
pub use expense::{Expense, ExpenseCategory, UnknownExpenseCategory};
pub use ids::{
    BankTransactionId, ClientId, ContactId, ExpenseId, InteractionId, InvoiceId, MissionId,
    OpportunityId, PaymentId, QuoteId, TimeEntryId,
};
pub use interaction::{Interaction, InteractionKind, UnknownInteractionKind};
pub use invoice::{Invoice, InvoiceLine, InvoiceStatus};
pub use mission::{Milestone, MilestoneParseError, Mission, MissionKind};
pub use money::{Money, MoneyParseError};
pub use opportunity::{
    LossReason, Opportunity, OpportunityStage, Probability, ProbabilityError, UnknownStage,
};
pub use payment::{BankTransaction, Payment, PaymentMethod, UnknownPaymentMethod};
pub use period::{FiscalYear, Month, MonthError, format_date, parse_date};
pub use quote::{Discount, LineKind, Quote, QuoteLine, QuoteStatus, UnknownQuoteStatus};
pub use siren::{Siren, SirenError, VatNumber, VatNumberError};
pub use time_entry::{TimeCategory, TimeEntry, UnknownTimeCategory};
pub use vat::{UnknownVatRate, VatRate};
