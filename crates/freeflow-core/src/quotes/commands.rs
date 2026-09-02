//! Commandes de devis : création, révision (nouvelle version), envoi, acceptation, déclin.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use time::{Date, OffsetDateTime};

use crate::app::{AppError, Command};
use crate::domain::{
    ClientId, Discount, MissionId, OpportunityId, Quote, QuoteId, QuoteLine, QuoteStatus,
};

use super::error::QuoteError;
use super::pricing::derive_mission;
use super::row;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateQuote {
    pub client_id: ClientId,
    pub opportunity_id: Option<OpportunityId>,
    pub lines: Vec<QuoteLine>,
    pub discount: Option<Discount>,
    pub terms: Option<String>,
    #[serde(with = "crate::domain::serde_date::date")]
    pub valid_until: Date,
}

impl Command for CreateQuote {
    type Output = QuoteId;
    const NAME: &'static str = "quotes.create_quote";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        if self.lines.is_empty() {
            return Err(QuoteError::EmptyQuote.into());
        }
        let id = QuoteId::new();
        let quote = Quote {
            id,
            root_id: id,
            client_id: self.client_id,
            opportunity_id: self.opportunity_id,
            version: 1,
            status: QuoteStatus::Draft,
            lines: self.lines.clone(),
            discount: self.discount,
            terms: self.terms.clone(),
            valid_until: self.valid_until,
            created_at: OffsetDateTime::now_utc(),
        };
        row::insert_quote(conn, &quote)?;
        Ok(id)
    }
}

/// Crée une nouvelle version d'un devis existant — jamais une modification en place : le
/// contenu d'une version est immuable dès sa création (contrainte imposée par le store).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviseQuote {
    pub root_id: QuoteId,
    pub lines: Vec<QuoteLine>,
    pub discount: Option<Discount>,
    pub terms: Option<String>,
    #[serde(with = "crate::domain::serde_date::date")]
    pub valid_until: Date,
}

impl Command for ReviseQuote {
    type Output = QuoteId;
    const NAME: &'static str = "quotes.revise_quote";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        if self.lines.is_empty() {
            return Err(QuoteError::EmptyQuote.into());
        }
        let previous_version = row::latest_version(conn, self.root_id)?;
        if previous_version == 0 {
            return Err(QuoteError::NotFound(self.root_id).into());
        }
        let original =
            row::quote_by_id(conn, self.root_id)?.ok_or(QuoteError::NotFound(self.root_id))?;

        let id = QuoteId::new();
        let quote = Quote {
            id,
            root_id: self.root_id,
            client_id: original.client_id,
            opportunity_id: original.opportunity_id,
            version: previous_version + 1,
            status: QuoteStatus::Draft,
            lines: self.lines.clone(),
            discount: self.discount,
            terms: self.terms.clone(),
            valid_until: self.valid_until,
            created_at: OffsetDateTime::now_utc(),
        };
        row::insert_quote(conn, &quote)?;
        Ok(id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendQuote {
    pub quote_id: QuoteId,
}

impl Command for SendQuote {
    type Output = ();
    const NAME: &'static str = "quotes.send_quote";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let quote =
            row::quote_by_id(conn, self.quote_id)?.ok_or(QuoteError::NotFound(self.quote_id))?;
        if quote.status != QuoteStatus::Draft {
            return Err(QuoteError::AlreadyClosed(self.quote_id).into());
        }
        row::set_status(conn, self.quote_id, QuoteStatus::Sent)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeclineQuote {
    pub quote_id: QuoteId,
}

impl Command for DeclineQuote {
    type Output = ();
    const NAME: &'static str = "quotes.decline_quote";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let quote =
            row::quote_by_id(conn, self.quote_id)?.ok_or(QuoteError::NotFound(self.quote_id))?;
        if quote.status != QuoteStatus::Sent {
            return Err(QuoteError::NotSent(self.quote_id).into());
        }
        row::set_status(conn, self.quote_id, QuoteStatus::Declined)?;
        Ok(())
    }
}

/// Accepte un devis envoyé et crée la mission correspondante, avec l'échéancier de
/// facturation dérivé des lignes (voir [`derive_mission`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcceptQuote {
    pub quote_id: QuoteId,
    #[serde(with = "crate::domain::serde_date::date")]
    pub started_on: Date,
}

impl Command for AcceptQuote {
    type Output = MissionId;
    const NAME: &'static str = "quotes.accept_quote";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let quote =
            row::quote_by_id(conn, self.quote_id)?.ok_or(QuoteError::NotFound(self.quote_id))?;
        if quote.status != QuoteStatus::Sent {
            return Err(QuoteError::NotSent(self.quote_id).into());
        }
        let mission = derive_mission(&quote, self.started_on)?;
        crate::missions::row::insert_mission(conn, &mission)?;
        row::set_status(conn, self.quote_id, QuoteStatus::Accepted)?;
        Ok(mission.id)
    }
}
