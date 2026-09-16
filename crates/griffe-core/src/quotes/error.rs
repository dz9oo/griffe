//! Erreurs métier des devis.

use thiserror::Error;

use crate::app::AppError;
use crate::domain::QuoteId;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum QuoteError {
    #[error("devis introuvable : {0}")]
    NotFound(QuoteId),

    #[error("un devis doit avoir au moins une ligne")]
    EmptyQuote,

    #[error("le devis {0} est déjà clos (accepté, décliné ou expiré)")]
    AlreadyClosed(QuoteId),

    #[error("seul un devis envoyé peut être accepté ou décliné (devis {0})")]
    NotSent(QuoteId),

    #[error(
        "impossible de dériver une mission du devis {0} : les lignes mélangent plusieurs types de facturation, ou une mission régie/récurrente doit tenir sur une seule ligne"
    )]
    CannotDeriveMission(QuoteId),
}

impl From<QuoteError> for AppError {
    fn from(e: QuoteError) -> Self {
        Self::Domain(e.to_string())
    }
}
