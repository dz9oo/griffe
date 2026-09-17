//! Types métier purs (aucune IO) : argent, TVA, identifiants, entités.

mod asset;
mod calendar;
mod client;
mod expense;
mod follow_up;
mod ids;
mod interaction;
mod invoice;
mod mission;
mod money;
mod opening;
mod opportunity;
mod paper;
mod payment;
mod period;
mod quote;
pub mod serde_date;
mod settlement;
mod siren;
mod time_entry;
mod vat;
mod vat_filing;

pub use asset::{
    AssetCandidate, AssetRow, DEFAULT_DURATION_MONTHS, FixedAsset, FixedAssetError,
    SMALL_EQUIPMENT_THRESHOLD, add_months, asset_account_label, days360, depreciation_account,
    fixed_asset_candidates, inferred_acquired_on, is_depreciable_account, sub_months,
};
pub use calendar::{
    easter_sunday, french_business_days_in, french_public_holidays, is_french_business_day,
    next_french_business_day_on_or_after,
};
pub use client::{Address, Client, Contact};
pub use expense::{
    Expense, ExpenseCategory, ExpensePaidBy, UnknownExpenseCategory, UnknownExpensePaidBy,
};
pub use follow_up::{
    CadenceStep, EmlDraft, FollowUpCursor, FollowUpEvent, FollowUpFact, FollowUpKind,
    FollowUpSubject, INVOICE_CADENCE, InvalidEmail, PROSPECT_CADENCE, SnoozePreset,
    TemplateContext, UnknownFollowUpFact, UnknownFollowUpKind, active_events, derive_cursor,
    format_date_fr, parse_email, render_eml, render_template, snooze_date,
};
pub use ids::{
    BankTransactionId, ClientId, ContactId, DutyFilingId, ExpenseId, FiscalYearId, FixedAssetId,
    FollowUpEventId, InteractionId, InvoiceId, MissionId, OpportunityId, PaperId, PaymentId,
    QuoteId, TimeEntryId, WriteOffId,
};
pub use interaction::{Interaction, InteractionKind, UnknownInteractionKind};
pub use invoice::{
    Invoice, InvoiceLine, InvoiceOrigin, InvoiceStatus, InvoiceWriteOff, UnknownInvoiceOrigin,
};
pub use mission::{Milestone, MilestoneParseError, Mission, MissionKind};
pub use money::{Money, MoneyParseError};
pub use opening::{
    AccountCode, AccountCodeError, OpeningBalance, OpeningBalanceError, OpeningBalanceLine,
    OpeningBalanceLineParseError, OpeningEquity, Side, UnknownSide,
};
pub use opportunity::{
    LossReason, Opportunity, OpportunityStage, Probability, ProbabilityError, UnknownStage,
};
pub use paper::{
    PaperBrief, PaperKind, PaperOrigin, PaperWhence, RetentionClock, UnknownPaperKind,
    UnknownPaperOrigin, retained_until,
};
pub use payment::{BankTransaction, Payment, PaymentMethod, UnknownPaymentMethod};
pub use period::{
    FiscalYear, FiscalYearEnd, FiscalYearEndError, Month, MonthError, UnknownVatRegime, VatRegime,
    format_date, parse_date,
};
pub use quote::{
    Discount, LineKind, Quote, QuoteLine, QuoteLineParseError, QuoteStatus, UnknownQuoteStatus,
};
pub use settlement::{SettlementAccount, SettlementAccountError};
pub use siren::{Siren, SirenError, VatNumber, VatNumberError};
pub use time_entry::{TimeCategory, TimeEntry, UnknownTimeCategory};
pub use vat::{UnknownVatRate, VatRate};
pub use vat_filing::{Ca3FilingRule, VatFilerCategory, VatFilingZone};
