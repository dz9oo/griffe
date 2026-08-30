//! Trace d'audit des opérations sur le coffre lui-même (aujourd'hui : le changement de
//! passphrase) — pas un module métier au sens des autres (`clients`, `missions`, ...), mais le
//! seul point d'entrée dans le journal d'audit chaîné par hash, qui n'accepte des écritures que
//! via [`crate::app::Executor`].

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::app::{AppError, Command};

/// Consigne dans le journal d'audit qu'un changement de passphrase a eu lieu. Ne mute rien du
/// domaine : le travail réel (sauvegarde préalable, ré-chiffrement, bascule du sidecar) est de
/// l'IO fichier faite par l'adaptateur *avant* de construire cette commande — exactement comme
/// `expenses::hash_receipt`/la copie du justificatif sont faites par la CLI avant de construire
/// `RecordExpense`. `apply` ne touche donc que `&Connection`, comme l'exige la doctrine du dépôt,
/// et n'a en pratique rien à y écrire : son seul rôle est de faire exister l'événement pour
/// [`crate::app::audit`].
///
/// Appendue *après* que le changement a réellement abouti, sur le coffre rouvert sous la
/// nouvelle clé : un changement interrompu ne laisse donc aucune entrée trompeuse, et la
/// sauvegarde préalable — antérieure à cette commande — ne la contient jamais.
///
/// `requires_confirmation` reste `false` à dessein : le rail de confirmation protège une
/// *décision* qu'un agent ne doit pas prendre seul, mais ici la vraie barrière est déjà passée
/// — la preuve de l'ancienne passphrase, exigée par `Store::change_passphrase` avant tout, et
/// qu'un `Actor::Agent` ne peut structurellement pas fournir (le serveur MCP n'a ni terminal ni
/// source de passphrase). Différer cette commande différerait seulement le *registre* de
/// l'événement, jamais son *effet* déjà produit — ne pas la faire passer par `PendingAction`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PassphraseChanged {
    /// Chemin de la sauvegarde écrite avant le changement. Aucun secret : ni sel, ni
    /// `vault_id`, ni clé, ni passphrase.
    pub backup_path: String,
    pub argon2_m_cost: u32,
    pub argon2_t_cost: u32,
    pub argon2_p_cost: u32,
}

impl Command for PassphraseChanged {
    type Output = ();
    const NAME: &'static str = "vault.passphrase_changed";

    fn apply(&self, _conn: &Connection) -> Result<Self::Output, AppError> {
        Ok(())
    }
}
