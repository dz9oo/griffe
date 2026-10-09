//! Erreurs du courrier, avant tout contact avec un serveur.

use thiserror::Error;

use crate::app::AppError;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum MailError {
    #[error("l'adresse est illisible")]
    Address,

    #[error("Il manque l'adresse de la personne.")]
    MissingAddress,

    #[error("Le port des copies est 993.")]
    CopyPort,

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

    #[error("la formule dépasse 2 000 caractères")]
    SignatureLength,

    #[error("la formule contient <signature>")]
    SignatureToken,
}

impl From<MailError> for AppError {
    fn from(error: MailError) -> Self {
        AppError::Domain(error.to_string())
    }
}
