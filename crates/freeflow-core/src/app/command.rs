//! Le contrat `Command` : la seule façon de muter l'état de `FreeFlow`. La CLI, le serveur MCP
//! et la GUI construisent tous des `Command` et les font exécuter par [`super::Executor`] — la
//! logique de dry-run, d'idempotence, de confirmation et d'audit ne vit qu'à un seul endroit.

use rusqlite::Connection;
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::error::AppError;

pub trait Command: Serialize + DeserializeOwned {
    type Output: Serialize + DeserializeOwned;

    /// Nom stable de la commande (ex. `"prospect.win"`), utilisé dans le journal d'audit, les
    /// actions en attente, et comme identifiant de dispatch pour la confirmation différée.
    const NAME: &'static str;

    /// Clé d'idempotence optionnelle : deux exécutions portant la même clé ne produisent
    /// qu'un seul effet, la seconde renvoyant le résultat mémorisé de la première.
    fn idempotency_key(&self) -> Option<&str> {
        None
    }

    /// Vrai si cette commande a un effet qu'un agent ne doit jamais déclencher sans validation
    /// humaine explicite (envoi d'email, émission de facture...). Sans effet quand l'acteur est
    /// [`super::Actor::Human`] ou [`super::Actor::System`] : agir directement EST déjà la
    /// confirmation dans ces cas-là.
    fn requires_confirmation(&self) -> bool {
        false
    }

    /// Applique effectivement la commande sur la connexion fournie (déjà dans une transaction
    /// ouverte par l'exécuteur). N'est jamais appelée directement par un appelant — toujours
    /// via [`super::Executor::execute`].
    ///
    /// # Errors
    ///
    /// Retourne une erreur si la mutation échoue.
    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError>;
}
