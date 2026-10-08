//! Erreurs du courrier, avant tout contact avec un serveur.

use thiserror::Error;

use crate::app::AppError;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum MailError {
    #[error("l'adresse est illisible")]
    Address,

    #[error("le serveur d'envoi est incomplet")]
    Incomplete,

    #[error("une connexion en clair est refusée")]
    Cleartext,

    #[error("le mot de passe est vide")]
    EmptySecret,

    #[error("cette lettre n'est plus annulable")]
    NotArmed,

    #[error("cette lettre n'attend pas de décision")]
    NotWaiting,

    #[error("un agent ne poste pas le courrier")]
    AgentCannotPost,

    #[error("lettre introuvable")]
    Missing,
}

impl From<MailError> for AppError {
    fn from(error: MailError) -> Self {
        AppError::Domain(error.to_string())
    }
}
