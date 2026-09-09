//! Identifiants typés : chaque entité a son propre type d'id (impossible de confondre un
//! `ClientId` avec un `InvoiceId` à la compilation), porté par un `UUIDv7` triable par le temps.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! define_id {
    ($name:ident) => {
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }

            #[must_use]
            pub const fn from_uuid(id: Uuid) -> Self {
                Self(id)
            }

            #[must_use]
            pub const fn as_uuid(self) -> Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(s).map(Self)
            }
        }
    };
}

define_id!(ClientId);
define_id!(ContactId);
define_id!(OpportunityId);
define_id!(InteractionId);
define_id!(QuoteId);
define_id!(MissionId);
define_id!(TimeEntryId);
define_id!(InvoiceId);
define_id!(PaymentId);
define_id!(ExpenseId);
define_id!(BankTransactionId);
define_id!(FiscalYearId);
define_id!(FixedAssetId);
define_id!(FollowUpEventId);
define_id!(DutyFilingId);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_string() {
        let id = MissionId::new();
        let parsed: MissionId = id.to_string().parse().unwrap();
        assert_eq!(id, parsed);
    }
}
