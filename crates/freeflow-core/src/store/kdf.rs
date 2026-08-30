//! Sidecar `<db>.kdf` : sel Argon2id, paramètres de coût, identifiant de coffre, vérificateur de
//! clé. Deux formats coexistent sur disque :
//!
//! - **v1** (17 octets) — `[version=1][sel:16]`. Aucun identifiant de coffre propre (l'appelant
//!   retombe sur le hash du chemin), aucun vérificateur, paramètres Argon2 câblés en dur
//!   ([`Argon2Cost::LEGACY`]).
//! - **v2** (77 octets) — `[version=2][vault_id:16][sel:16][m_cost:4][t_cost:4][p_cost:4]
//!   [vérificateur:32]`, tout en little-endian. Un coffre supprimé puis recréé au même chemin
//!   obtient un `vault_id` aléatoire neuf : l'ancienne entrée du trousseau, indexée sur cet id,
//!   ne peut plus jamais être servie par erreur — c'est ce qui corrige la divergence
//!   trousseau/sidecar de l'ancien format. Le vérificateur permet de rejeter une mauvaise
//!   passphrase *avant* de toucher `SQLCipher`, y compris sur un coffre dont le fichier `.db`
//!   serait vide ou absent de page chiffrée lisible — chose que la seule lecture SQL ne peut pas
//!   garantir.
//!
//! Un coffre v1 est migré vers v2 au premier déverrouillage réussi par passphrase, en conservant
//! exactement le même sel et les mêmes paramètres : la clé dérivée ne change donc pas, aucun
//! re-chiffrement `SQLCipher` n'est nécessaire.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use argon2::{Algorithm, Argon2, Params, Version};
use rand::Rng;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use super::error::StoreError;
use super::secret::{Passphrase, VaultKey};

pub const SALT_LEN: usize = 16;
pub const VAULT_ID_LEN: usize = 16;
const VERIFIER_LEN: usize = 32;

const V1_VERSION: u8 = 1;
const V1_LEN: usize = 1 + SALT_LEN;

const V2_VERSION: u8 = 2;
const V2_LEN: usize = 1 + VAULT_ID_LEN + SALT_LEN + 4 + 4 + 4 + VERIFIER_LEN;

/// Coût Argon2id : mémoire (Kio), itérations, parallélisme. Les noms de champs reprennent la
/// terminologie d'Argon2 elle-même (`m`/`t`/`p` cost) — les renommer perdrait en clarté pour qui
/// connaît l'algorithme.
#[allow(clippy::struct_field_names)]
#[derive(Debug, Clone, Copy)]
pub struct Argon2Cost {
    pub m_cost: u32,
    pub t_cost: u32,
    pub p_cost: u32,
}

impl Argon2Cost {
    /// Paramètres câblés en dur des coffres v1, jamais rejoués pour un coffre neuf mais toujours
    /// lus depuis le sidecar : les coffres v1 existants continuent de s'ouvrir à l'identique.
    pub const LEGACY: Self = Self {
        m_cost: 19_456,
        t_cost: 2,
        p_cost: 1,
    };

    /// Paramètres des coffres créés désormais — recommandation OWASP de premier rang
    /// (~64 Mio, 3 itérations). Stockés dans le sidecar : les relever à l'avenir ne cassera
    /// jamais les coffres déjà créés, qui gardent les leurs.
    pub const CURRENT: Self = Self {
        m_cost: 65_536,
        t_cost: 3,
        p_cost: 1,
    };

    /// # Panics
    ///
    /// Ne panique jamais en pratique : `m_cost`/`t_cost`/`p_cost` proviennent soit des
    /// constantes ci-dessus, soit d'un sidecar déjà accepté par `SQLCipher` par le passé —
    /// les deux sont toujours des paramètres Argon2id valides.
    fn build(self) -> Argon2<'static> {
        let params = Params::new(self.m_cost, self.t_cost, self.p_cost, Some(VaultKey::LEN))
            .expect("paramètres Argon2id du sidecar toujours valides");
        Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
    }
}

/// Sidecar chargé : sel, coût, identifiant de coffre, et vérificateur si le format le porte
/// (absent seulement pour un sidecar v1 pas encore migré).
#[derive(Debug, Clone)]
pub struct Sidecar {
    pub vault_id: VaultId,
    salt: [u8; SALT_LEN],
    cost: Argon2Cost,
    verifier: Option<[u8; VERIFIER_LEN]>,
}

/// Identifiant de compte trousseau. `V2` est un tirage aléatoire propre au sidecar ; `LegacyV1`
/// reproduit l'ancien calcul (hash du chemin absolu) pour que les coffres v1 pas encore migrés
/// retrouvent l'entrée déjà présente dans le trousseau d'un utilisateur existant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultId {
    V2([u8; VAULT_ID_LEN]),
    LegacyV1(String),
}

impl VaultId {
    #[must_use]
    pub fn as_account(&self) -> String {
        match self {
            Self::V2(id) => format!("v2:{}", hex::encode(id)),
            Self::LegacyV1(hash) => hash.clone(),
        }
    }

    fn legacy_for(db_path: &Path) -> Self {
        let absolute = std::path::absolute(db_path).unwrap_or_else(|_| db_path.to_path_buf());
        let digest = Sha256::digest(absolute.to_string_lossy().as_bytes());
        Self::LegacyV1(hex::encode(digest))
    }
}

impl Sidecar {
    #[must_use]
    pub fn cost(&self) -> Argon2Cost {
        self.cost
    }

    #[must_use]
    pub fn salt(&self) -> &[u8; SALT_LEN] {
        &self.salt
    }

    #[must_use]
    pub fn is_legacy(&self) -> bool {
        self.verifier.is_none()
    }

    /// Vérifie `key` contre le vérificateur embarqué. Un sidecar v1 n'en porte aucun : dans ce
    /// cas cette fonction ne peut rien affirmer, l'appelant continue de compter sur la
    /// validation `SQLCipher` (lecture réelle d'une page déjà chiffrée).
    #[must_use]
    pub fn verify(&self, key: &VaultKey) -> bool {
        match &self.verifier {
            Some(expected) => {
                let vault_id = match &self.vault_id {
                    VaultId::V2(id) => *id,
                    VaultId::LegacyV1(_) => return false, // incohérent : jamais produit ainsi
                };
                bool::from(compute_verifier(&vault_id, key).ct_eq(expected))
            }
            None => true,
        }
    }
}

fn compute_verifier(vault_id: &[u8; VAULT_ID_LEN], key: &VaultKey) -> [u8; VERIFIER_LEN] {
    // Hachage à domaine séparé plutôt qu'un HMAC : l'entrée (32 octets de clé pleine entropie)
    // n'est jamais choisie par un attaquant, donc l'extension de longueur n'est pas un vecteur
    // ici, et ça évite d'ajouter `hmac` pour une seule comparaison d'égalité.
    let mut hasher = Sha256::new();
    hasher.update(b"freeflow.kdf.v2.verifier\0");
    hasher.update(vault_id);
    hasher.update(key.as_bytes());
    let digest = hasher.finalize();
    let mut out = [0u8; VERIFIER_LEN];
    out.copy_from_slice(&digest);
    out
}

/// Dérive la clé de chiffrement depuis `passphrase`, `salt` et `cost`.
///
/// # Errors
///
/// Retourne une erreur si Argon2id rejette la passphrase (en pratique : passphrase vide selon
/// la configuration de la bibliothèque).
pub fn derive_key(
    passphrase: &Passphrase,
    salt: &[u8; SALT_LEN],
    cost: Argon2Cost,
) -> Result<VaultKey, StoreError> {
    let mut bytes = [0u8; VaultKey::LEN];
    cost.build()
        .hash_password_into(passphrase.expose().as_bytes(), salt, &mut bytes)
        .map_err(|e| StoreError::KeyDerivation(e.to_string()))?;
    Ok(VaultKey::new(bytes))
}

pub fn sidecar_path(db_path: &Path) -> PathBuf {
    let mut os_string = db_path.as_os_str().to_owned();
    os_string.push(".kdf");
    PathBuf::from(os_string)
}

/// Lit le sidecar existant, s'il y en a un. `None` si le fichier n'existe pas — l'appelant en
/// déduit que le coffre lui-même n'existe pas.
///
/// # Errors
///
/// [`StoreError::CorruptKdfParams`] si le fichier existe mais ne correspond à aucun format connu.
pub fn read(db_path: &Path) -> Result<Option<Sidecar>, StoreError> {
    let path = sidecar_path(db_path);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(StoreError::Io(e)),
    };

    match bytes.first() {
        Some(&V1_VERSION) if bytes.len() == V1_LEN => {
            let mut salt = [0u8; SALT_LEN];
            salt.copy_from_slice(&bytes[1..]);
            Ok(Some(Sidecar {
                vault_id: VaultId::legacy_for(db_path),
                salt,
                cost: Argon2Cost::LEGACY,
                verifier: None,
            }))
        }
        Some(&V2_VERSION) if bytes.len() == V2_LEN => {
            let mut vault_id = [0u8; VAULT_ID_LEN];
            vault_id.copy_from_slice(&bytes[1..=VAULT_ID_LEN]);
            let mut offset = 1 + VAULT_ID_LEN;
            let mut salt = [0u8; SALT_LEN];
            salt.copy_from_slice(&bytes[offset..offset + SALT_LEN]);
            offset += SALT_LEN;
            let m_cost = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
            offset += 4;
            let t_cost = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
            offset += 4;
            let p_cost = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
            offset += 4;
            let mut verifier = [0u8; VERIFIER_LEN];
            verifier.copy_from_slice(&bytes[offset..offset + VERIFIER_LEN]);
            Ok(Some(Sidecar {
                vault_id: VaultId::V2(vault_id),
                salt,
                cost: Argon2Cost {
                    m_cost,
                    t_cost,
                    p_cost,
                },
                verifier: Some(verifier),
            }))
        }
        _ => Err(StoreError::CorruptKdfParams(path)),
    }
}

/// Crée un sidecar v2 neuf pour un coffre en cours de création : sel et identifiant aléatoires,
/// paramètres de coût courants. Dérive la clé, calcule le vérificateur, écrit le fichier de
/// façon atomique (0600, refuse d'écraser un sidecar existant).
///
/// # Errors
///
/// [`StoreError::VaultAlreadyExists`] si un sidecar existe déjà à cet emplacement ; une erreur
/// d'IO sinon.
pub fn create(db_path: &Path, passphrase: &Passphrase) -> Result<(Sidecar, VaultKey), StoreError> {
    let path = sidecar_path(db_path);
    if path.exists() {
        return Err(StoreError::VaultAlreadyExists(db_path.to_path_buf()));
    }

    let mut vault_id = [0u8; VAULT_ID_LEN];
    rand::rng().fill_bytes(&mut vault_id);
    let mut salt = [0u8; SALT_LEN];
    rand::rng().fill_bytes(&mut salt);
    let cost = Argon2Cost::CURRENT;

    let key = derive_key(passphrase, &salt, cost)?;
    let verifier = compute_verifier(&vault_id, &key);

    write_v2(&path, &vault_id, &salt, cost, &verifier)?;

    Ok((
        Sidecar {
            vault_id: VaultId::V2(vault_id),
            salt,
            cost,
            verifier: Some(verifier),
        },
        key,
    ))
}

/// Réécrit un sidecar v1 en v2 en conservant le même sel et les mêmes paramètres — donc la même
/// clé — après un déverrouillage réussi par passphrase. Écriture atomique.
///
/// # Errors
///
/// Une erreur d'IO si l'écriture échoue.
pub fn upgrade_v1_to_v2(
    db_path: &Path,
    sidecar: &Sidecar,
    key: &VaultKey,
) -> Result<Sidecar, StoreError> {
    let mut vault_id = [0u8; VAULT_ID_LEN];
    rand::rng().fill_bytes(&mut vault_id);
    let verifier = compute_verifier(&vault_id, key);
    let path = sidecar_path(db_path);
    write_v2(&path, &vault_id, &sidecar.salt, sidecar.cost, &verifier)?;
    Ok(Sidecar {
        vault_id: VaultId::V2(vault_id),
        salt: sidecar.salt,
        cost: sidecar.cost,
        verifier: Some(verifier),
    })
}

fn write_v2(
    path: &Path,
    vault_id: &[u8; VAULT_ID_LEN],
    salt: &[u8; SALT_LEN],
    cost: Argon2Cost,
    verifier: &[u8; VERIFIER_LEN],
) -> Result<(), StoreError> {
    let mut bytes = Vec::with_capacity(V2_LEN);
    bytes.push(V2_VERSION);
    bytes.extend_from_slice(vault_id);
    bytes.extend_from_slice(salt);
    bytes.extend_from_slice(&cost.m_cost.to_le_bytes());
    bytes.extend_from_slice(&cost.t_cost.to_le_bytes());
    bytes.extend_from_slice(&cost.p_cost.to_le_bytes());
    bytes.extend_from_slice(verifier);
    write_atomic(path, &bytes)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp_path = path.with_extension("kdf.tmp");
    fs::write(&tmp_path, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp_path, fs::Permissions::from_mode(0o600))?;
    }
    fs::rename(&tmp_path, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "freeflow-kdf-test-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ))
    }

    #[test]
    fn a_freshly_created_sidecar_verifies_the_correct_key_and_rejects_a_wrong_one() {
        let db_path = temp_db_path("verify");
        let (sidecar, key) = create(&db_path, &"s3cret".into()).unwrap();
        assert!(sidecar.verify(&key));

        let wrong_key =
            derive_key(&"not-the-passphrase".into(), sidecar.salt(), sidecar.cost()).unwrap();
        assert!(!sidecar.verify(&wrong_key));
    }

    #[test]
    fn create_refuses_to_overwrite_an_existing_sidecar() {
        let db_path = temp_db_path("no-overwrite");
        create(&db_path, &"s3cret".into()).unwrap();
        let err = create(&db_path, &"other".into()).unwrap_err();
        assert!(matches!(err, StoreError::VaultAlreadyExists(_)));
    }

    #[test]
    fn reading_a_missing_sidecar_returns_none() {
        let db_path = temp_db_path("missing");
        assert!(read(&db_path).unwrap().is_none());
    }

    #[test]
    fn reading_a_corrupt_sidecar_is_reported_not_silently_recreated() {
        let db_path = temp_db_path("corrupt");
        fs::write(sidecar_path(&db_path), b"not a valid sidecar at all").unwrap();
        let err = read(&db_path).unwrap_err();
        assert!(matches!(err, StoreError::CorruptKdfParams(_)));
    }

    #[test]
    fn a_v1_sidecar_reads_with_legacy_parameters_and_no_verifier() {
        let db_path = temp_db_path("v1-read");
        let salt = [7u8; SALT_LEN];
        let mut bytes = vec![V1_VERSION];
        bytes.extend_from_slice(&salt);
        fs::write(sidecar_path(&db_path), &bytes).unwrap();

        let sidecar = read(&db_path).unwrap().unwrap();
        assert!(sidecar.is_legacy());
        assert_eq!(sidecar.salt(), &salt);
        assert_eq!(sidecar.cost().m_cost, Argon2Cost::LEGACY.m_cost);
    }

    #[test]
    fn upgrading_a_v1_sidecar_preserves_salt_and_params_so_the_key_never_changes() {
        let db_path = temp_db_path("v1-upgrade");
        let salt = [9u8; SALT_LEN];
        let mut bytes = vec![V1_VERSION];
        bytes.extend_from_slice(&salt);
        fs::write(sidecar_path(&db_path), &bytes).unwrap();

        let v1 = read(&db_path).unwrap().unwrap();
        let key = derive_key(&"s3cret".into(), v1.salt(), v1.cost()).unwrap();

        let v2 = upgrade_v1_to_v2(&db_path, &v1, &key).unwrap();
        assert!(!v2.is_legacy());
        assert!(v2.verify(&key));

        let reread = read(&db_path).unwrap().unwrap();
        assert_eq!(reread.salt(), &salt);
        let rederived = derive_key(&"s3cret".into(), reread.salt(), reread.cost()).unwrap();
        assert_eq!(rederived.as_bytes(), key.as_bytes());
    }

    #[test]
    fn recreating_a_vault_at_the_same_path_never_reuses_the_previous_vault_id() {
        let db_path = temp_db_path("recreate");
        let (first, _) = create(&db_path, &"s3cret".into()).unwrap();
        fs::remove_file(sidecar_path(&db_path)).unwrap();
        let (second, _) = create(&db_path, &"other-passphrase".into()).unwrap();
        assert_ne!(first.vault_id.as_account(), second.vault_id.as_account());
    }
}
