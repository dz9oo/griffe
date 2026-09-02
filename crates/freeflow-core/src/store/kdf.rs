//! Sidecar `<db>.kdf` : sel Argon2id, paramètres de coût, identifiant de coffre, et — selon le
//! format — vérificateur de clé ou clé maître enveloppée. Trois formats coexistent sur disque :
//!
//! - **v1** (17 octets) — `[version=1][sel:16]`. Aucun identifiant de coffre propre (l'appelant
//!   retombe sur le hash du chemin), aucun vérificateur, paramètres Argon2 câblés en dur
//!   ([`Argon2Cost::LEGACY`]). La clé du coffre est la clé dérivée elle-même.
//! - **v2** (77 octets) — `[version=2][vault_id:16][sel:16][m_cost:4][t_cost:4][p_cost:4]
//!   [vérificateur:32]`, tout en little-endian. Un coffre supprimé puis recréé au même chemin
//!   obtient un `vault_id` aléatoire neuf : l'ancienne entrée du trousseau, indexée sur cet id,
//!   ne peut plus jamais être servie par erreur — c'est ce qui corrige la divergence
//!   trousseau/sidecar de l'ancien format. Le vérificateur permet de rejeter une mauvaise
//!   passphrase *avant* de toucher `SQLCipher`, y compris sur un coffre dont le fichier `.db`
//!   serait vide ou absent de page chiffrée lisible — chose que la seule lecture SQL ne peut pas
//!   garantir. La clé du coffre est là aussi la clé dérivée elle-même.
//! - **v3** (133 octets, lot 24) — `[version=3][vault_id:16][sel:16][m_cost:4][t_cost:4]
//!   [p_cost:4][key_id:16][nonce:24][clé_maître_enveloppée:48]`. Modèle LUKS : le coffre est
//!   chiffré par une **clé maître aléatoire**, qui ne dérive d'aucune passphrase ; la passphrase
//!   ne sert qu'à dériver (Argon2id) la clé d'enveloppement sous laquelle la clé maître est
//!   scellée par XChaCha20-Poly1305, tout l'en-tête du fichier servant de données authentifiées.
//!   Une mauvaise passphrase fait échouer le tag d'authentification — le rôle que le
//!   vérificateur v2 jouait — et un changement de passphrase devient le ré-enveloppement de la
//!   même clé maître sous un sel neuf : la réécriture atomique de ce seul fichier, sans jamais
//!   ré-chiffrer la base. `key_id` (empreinte tronquée, à domaine séparé, de la clé maître) rend
//!   une clé en cache dans le trousseau vérifiable contre le sidecar sans passphrase, comme le
//!   vérificateur v2 le permettait.
//!
//! Un coffre v1 est migré vers v2 au premier déverrouillage réussi par passphrase, en conservant
//! exactement le même sel et les mêmes paramètres : la clé dérivée ne change donc pas, aucun
//! re-chiffrement `SQLCipher` n'est nécessaire. Un coffre v2, lui, ne migre vers v3 qu'au
//! premier changement de passphrase — passer à une clé maître réellement aléatoire exige un
//! re-chiffrement complet, que seul `passphrase change` (qui re-chiffre déjà) peut assumer ;
//! jamais une migration silencieuse au déverrouillage.
//!
//! Un changement de passphrase **avec re-chiffrement** (v2 → v3, [`stage_rekey`]/
//! [`commit_staged`]) passe par un sidecar en attente `<db>.kdf.new`, écrit et synchronisé sur
//! disque *avant* que la base elle-même ne soit ré-chiffrée : si le process est interrompu entre
//! la bascule de la base et celle du sidecar, les deux clés (ancienne et nouvelle) restent
//! retrouvables sur disque, ce qui rend la reprise possible sans intervention (voir
//! `Store::open_with_passphrase_with`). Un changement **sans re-chiffrement** (coffre déjà v3,
//! [`Sidecar::rewrap`]) n'a pas besoin de ce mécanisme : un unique [`write_atomic`] suffit, il
//! n'existe plus de fenêtre où base et sidecar pourraient diverger.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce};
use rand::Rng;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use zeroize::Zeroize;

use super::error::StoreError;
use super::secret::{Passphrase, VaultKey};

pub const SALT_LEN: usize = 16;
pub const VAULT_ID_LEN: usize = 16;
const VERIFIER_LEN: usize = 32;
const KEY_ID_LEN: usize = 16;
const NONCE_LEN: usize = 24;
/// Clé maître (32) + tag Poly1305 (16).
const WRAPPED_LEN: usize = VaultKey::LEN + 16;

const V1_VERSION: u8 = 1;
const V1_LEN: usize = 1 + SALT_LEN;

const V2_VERSION: u8 = 2;
const V2_LEN: usize = 1 + VAULT_ID_LEN + SALT_LEN + 4 + 4 + 4 + VERIFIER_LEN;

const V3_VERSION: u8 = 3;
/// Tout ce qui précède le nonce — c'est aussi, à l'octet près, l'AAD de l'enveloppe : un sidecar
/// v3 dont l'en-tête aurait été altéré (identifiant, sel, coûts, `key_id`) fait échouer le tag
/// d'authentification exactement comme une mauvaise passphrase.
const V3_HEADER_LEN: usize = 1 + VAULT_ID_LEN + SALT_LEN + 4 + 4 + 4 + KEY_ID_LEN;
const V3_LEN: usize = V3_HEADER_LEN + NONCE_LEN + WRAPPED_LEN;

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
    /// Ne panique jamais : `m_cost`/`t_cost`/`p_cost` proviennent soit des constantes ci-dessus,
    /// soit d'un sidecar dont [`parse_sidecar`] a déjà validé les bornes via
    /// [`validate_argon2_params`] avant de construire ce `Argon2Cost` — les deux sont donc
    /// toujours des paramètres Argon2id valides et bornés.
    fn build(self) -> Argon2<'static> {
        let params = Params::new(self.m_cost, self.t_cost, self.p_cost, Some(VaultKey::LEN))
            .expect("paramètres Argon2id déjà validés par validate_argon2_params");
        Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
    }
}

/// Plafond mémoire accepté pour un sidecar (en Kio) : 1 Gio. Très au-dessus du profil courant
/// (64 Mio) — un utilisateur peut légitimement relever ses paramètres — mais loin de l'allocation
/// délirante qui provoquerait un OOM.
const MAX_M_COST_KIB: u32 = 1024 * 1024;

/// Valide les paramètres Argon2id lus depuis un sidecar non fiable. Reproduit les invariants de
/// `argon2::Params::new` (`t_cost >= 1`, `p_cost >= 1`, `m_cost >= 8 * p_cost`) et ajoute un
/// plafond mémoire pour empêcher un déni de service par épuisement. `Err(())` = paramètre à
/// rejeter comme corrompu.
fn validate_argon2_params(m_cost: u32, t_cost: u32, p_cost: u32) -> Result<(), ()> {
    if t_cost < 1 || p_cost < 1 {
        return Err(());
    }
    // `saturating_mul` : `p_cost` vient d'un fichier non fiable, `8 * p_cost` pourrait déborder
    // u32 (et paniquer sous `overflow-checks`) — la saturation le plafonne proprement à u32::MAX,
    // ce qui fait échouer la comparaison suivante comme voulu.
    let min_m_cost = p_cost.saturating_mul(8);
    if m_cost < min_m_cost || m_cost > MAX_M_COST_KIB {
        return Err(());
    }
    Ok(())
}

/// Sidecar chargé : sel, coût, identifiant de coffre, et corps propre au format — vérificateur
/// de clé (v2), clé maître enveloppée (v3), ou rien (v1 pas encore migré).
#[derive(Debug, Clone)]
pub struct Sidecar {
    pub vault_id: VaultId,
    salt: [u8; SALT_LEN],
    cost: Argon2Cost,
    body: SidecarBody,
}

#[derive(Debug, Clone)]
enum SidecarBody {
    V1,
    V2 {
        verifier: [u8; VERIFIER_LEN],
    },
    V3 {
        key_id: [u8; KEY_ID_LEN],
        nonce: [u8; NONCE_LEN],
        wrapped: [u8; WRAPPED_LEN],
    },
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
        matches!(self.body, SidecarBody::V1)
    }

    /// Version du format sur disque — exposée par `vault status`.
    #[must_use]
    pub fn version(&self) -> u8 {
        match self.body {
            SidecarBody::V1 => V1_VERSION,
            SidecarBody::V2 { .. } => V2_VERSION,
            SidecarBody::V3 { .. } => V3_VERSION,
        }
    }

    /// `true` si ce sidecar enveloppe une clé maître (v3) — le format où changer de passphrase
    /// se fait par [`Self::rewrap`], sans jamais toucher la base.
    #[must_use]
    pub fn wraps_master_key(&self) -> bool {
        matches!(self.body, SidecarBody::V3 { .. })
    }

    /// Vérifie que `key` est bien la clé du coffre décrit par ce sidecar, **sans passphrase** —
    /// c'est le contrôle appliqué à une clé sortie du cache trousseau. v2 : hash-vérificateur de
    /// la clé dérivée ; v3 : empreinte `key_id` de la clé maître. Un sidecar v1 ne porte rien :
    /// dans ce cas cette fonction ne peut rien affirmer, l'appelant continue de compter sur la
    /// validation `SQLCipher` (lecture réelle d'une page déjà chiffrée).
    #[must_use]
    pub fn verify(&self, key: &VaultKey) -> bool {
        let vault_id = match &self.vault_id {
            VaultId::V2(id) => *id,
            // Incohérent pour v2/v3 (jamais produits avec un id legacy) ; v1 n'en a pas besoin.
            VaultId::LegacyV1(_) => return self.is_legacy(),
        };
        match &self.body {
            SidecarBody::V1 => true,
            SidecarBody::V2 { verifier } => {
                bool::from(compute_verifier(&vault_id, key).ct_eq(verifier))
            }
            SidecarBody::V3 { key_id, .. } => {
                bool::from(compute_key_id(&vault_id, key).ct_eq(key_id))
            }
        }
    }

    /// Résout la clé du coffre depuis la passphrase, selon le format :
    ///
    /// - v1 : la clé dérivée elle-même, **sans aucune garantie** (pas de vérificateur — une
    ///   mauvaise passphrase ne se détecte qu'à la lecture `SQLCipher`) ;
    /// - v2 : la clé dérivée, vérifiée contre le hash embarqué ;
    /// - v3 : la clé maître, déballée de son enveloppe AEAD par la clé d'enveloppement dérivée.
    ///
    /// # Errors
    ///
    /// [`StoreError::WrongPassphrase`] si le vérificateur (v2) ou le tag d'authentification (v3)
    /// rejette la passphrase — pour v3, un sidecar altéré est indistinguable d'une mauvaise
    /// passphrase, c'est la nature d'un AEAD. [`StoreError::KeyDerivation`] si Argon2id échoue.
    pub fn key_from_passphrase(&self, passphrase: &Passphrase) -> Result<VaultKey, StoreError> {
        let derived = derive_key(passphrase, &self.salt, self.cost)?;
        match &self.body {
            SidecarBody::V1 => Ok(derived),
            SidecarBody::V2 { .. } => {
                if self.verify(&derived) {
                    Ok(derived)
                } else {
                    Err(StoreError::WrongPassphrase)
                }
            }
            SidecarBody::V3 { nonce, wrapped, .. } => {
                let header = self.v3_header()?;
                unwrap_master_key(&derived, nonce, &header, wrapped)
                    .ok_or(StoreError::WrongPassphrase)
            }
        }
    }

    /// Ré-enveloppe la clé maître d'un sidecar v3 sous une nouvelle passphrase : sel neuf, coûts
    /// [`Argon2Cost::CURRENT`] (un coffre resté à d'anciens paramètres en profite pour se mettre
    /// à niveau), nonce neuf — puis réécrit `<db>.kdf` de façon atomique. La base n'est jamais
    /// touchée : la clé maître, le `vault_id` et le `key_id` sont conservés à l'identique.
    ///
    /// C'est ici — au plus près des deux dérivations déjà payées — que « nouvelle passphrase
    /// identique à l'ancienne » est détectée, comme `Store::change_passphrase` le fait pour un
    /// coffre v2 : en comparant les clés d'enveloppement dérivées sous le même sel, en temps
    /// constant.
    ///
    /// # Errors
    ///
    /// [`StoreError::WrongPassphrase`] si `current` ne déballe pas la clé maître ;
    /// [`StoreError::KeyDerivation`] si les deux passphrases sont identiques ou si Argon2id
    /// échoue ; [`StoreError::CorruptKdfParams`] si ce sidecar n'est pas un v3 (l'appelant a
    /// choisi la mauvaise branche) ; une erreur d'IO si l'écriture échoue.
    pub fn rewrap(
        &self,
        db_path: &Path,
        current: &Passphrase,
        new: &Passphrase,
    ) -> Result<Self, StoreError> {
        let path = sidecar_path(db_path);
        let (SidecarBody::V3 { key_id, .. }, VaultId::V2(vault_id)) = (&self.body, &self.vault_id)
        else {
            return Err(StoreError::CorruptKdfParams(path));
        };
        let master = self.key_from_passphrase(current)?;

        let current_kek = derive_key(current, &self.salt, self.cost)?;
        let new_kek_under_current_salt = derive_key(new, &self.salt, self.cost)?;
        if bool::from(
            new_kek_under_current_salt
                .as_bytes()
                .ct_eq(current_kek.as_bytes()),
        ) {
            return Err(StoreError::KeyDerivation(
                "la nouvelle passphrase est identique à l'ancienne".to_string(),
            ));
        }

        let mut salt = [0u8; SALT_LEN];
        rand::rng().fill_bytes(&mut salt);
        let cost = Argon2Cost::CURRENT;
        let new_kek = derive_key(new, &salt, cost)?;
        let body = write_v3(&path, vault_id, &salt, cost, key_id, &new_kek, &master)?;

        Ok(Self {
            vault_id: self.vault_id.clone(),
            salt,
            cost,
            body,
        })
    }

    /// L'en-tête v3 tel qu'il apparaît sur disque — et donc l'AAD de l'enveloppe.
    fn v3_header(&self) -> Result<Vec<u8>, StoreError> {
        let (SidecarBody::V3 { key_id, .. }, VaultId::V2(vault_id)) = (&self.body, &self.vault_id)
        else {
            return Err(StoreError::CorruptKdfParams(PathBuf::new()));
        };
        Ok(v3_header_bytes(vault_id, &self.salt, self.cost, key_id))
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

/// Empreinte de la clé **maître** d'un sidecar v3 — même construction à domaine séparé que le
/// vérificateur v2, tronquée à 16 octets : ce n'est pas une barrière cryptographique (la clé
/// maître est déjà secrète, l'empreinte d'une clé de 256 bits d'entropie ne fuit rien
/// d'inversible), seulement de quoi détecter qu'une clé sortie du trousseau OS ne correspond
/// plus au sidecar courant.
fn compute_key_id(vault_id: &[u8; VAULT_ID_LEN], master: &VaultKey) -> [u8; KEY_ID_LEN] {
    let mut hasher = Sha256::new();
    hasher.update(b"freeflow.kdf.v3.key-id\0");
    hasher.update(vault_id);
    hasher.update(master.as_bytes());
    let digest = hasher.finalize();
    let mut out = [0u8; KEY_ID_LEN];
    out.copy_from_slice(&digest[..KEY_ID_LEN]);
    out
}

fn v3_header_bytes(
    vault_id: &[u8; VAULT_ID_LEN],
    salt: &[u8; SALT_LEN],
    cost: Argon2Cost,
    key_id: &[u8; KEY_ID_LEN],
) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(V3_HEADER_LEN);
    bytes.push(V3_VERSION);
    bytes.extend_from_slice(vault_id);
    bytes.extend_from_slice(salt);
    bytes.extend_from_slice(&cost.m_cost.to_le_bytes());
    bytes.extend_from_slice(&cost.t_cost.to_le_bytes());
    bytes.extend_from_slice(&cost.p_cost.to_le_bytes());
    bytes.extend_from_slice(key_id);
    bytes
}

/// Construit le chiffreur d'enveloppe depuis la clé d'enveloppement, en effaçant la copie
/// intermédiaire de clé (le chiffreur efface la sienne à sa destruction, feature `zeroize`).
fn envelope_cipher(kek: &VaultKey) -> XChaCha20Poly1305 {
    let mut key = chacha20poly1305::Key::from(*kek.as_bytes());
    let cipher = XChaCha20Poly1305::new(&key);
    key.zeroize();
    cipher
}

/// Scelle la clé maître sous la clé d'enveloppement. L'en-tête complet du sidecar sert d'AAD.
fn wrap_master_key(
    kek: &VaultKey,
    nonce: &[u8; NONCE_LEN],
    header: &[u8],
    master: &VaultKey,
) -> Result<[u8; WRAPPED_LEN], StoreError> {
    let sealed = envelope_cipher(kek)
        .encrypt(
            <&XNonce>::from(nonce),
            Payload {
                msg: master.as_bytes(),
                aad: header,
            },
        )
        .map_err(|_| StoreError::KeyDerivation("échec du scellement de la clé maître".into()))?;
    let mut out = [0u8; WRAPPED_LEN];
    out.copy_from_slice(&sealed);
    Ok(out)
}

/// Déballe la clé maître. `None` si le tag d'authentification échoue — mauvaise passphrase ou
/// sidecar altéré, indistinguables par construction.
fn unwrap_master_key(
    kek: &VaultKey,
    nonce: &[u8; NONCE_LEN],
    header: &[u8],
    wrapped: &[u8; WRAPPED_LEN],
) -> Option<VaultKey> {
    let mut opened = envelope_cipher(kek)
        .decrypt(
            <&XNonce>::from(nonce),
            Payload {
                msg: wrapped,
                aad: header,
            },
        )
        .ok()?;
    let mut bytes = [0u8; VaultKey::LEN];
    bytes.copy_from_slice(&opened);
    opened.zeroize();
    let master = VaultKey::new(bytes);
    bytes.zeroize();
    Some(master)
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

/// Ajoute `suffix` à l'`OsString` complète de `path`, plutôt que de remplacer son extension —
/// c'est ce qui permet à `<db>.kdf` et `<db>.kdf.new` d'avoir chacun leur propre fichier
/// temporaire (`<db>.kdf.tmp` / `<db>.kdf.new.tmp`) sans jamais se les disputer.
fn append_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut os_string = path.as_os_str().to_owned();
    os_string.push(suffix);
    PathBuf::from(os_string)
}

#[must_use]
pub fn sidecar_path(db_path: &Path) -> PathBuf {
    append_suffix(db_path, ".kdf")
}

/// Sidecar en attente d'un changement de passphrase en cours : écrit par [`stage_rekey`],
/// basculé sur [`sidecar_path`] par [`commit_staged`]. Sa seule présence signale un changement
/// interrompu — voir `Store::open_with_passphrase_with`.
#[must_use]
pub fn staged_sidecar_path(db_path: &Path) -> PathBuf {
    append_suffix(db_path, ".kdf.new")
}

fn read_sidecar_file(path: &Path, db_path: &Path) -> Result<Option<Sidecar>, StoreError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(StoreError::Io(e)),
    };
    parse_sidecar(&bytes, db_path, path).map(Some)
}

fn parse_sidecar(bytes: &[u8], db_path: &Path, path: &Path) -> Result<Sidecar, StoreError> {
    match bytes.first() {
        Some(&V1_VERSION) if bytes.len() == V1_LEN => {
            let mut salt = [0u8; SALT_LEN];
            salt.copy_from_slice(&bytes[1..]);
            Ok(Sidecar {
                vault_id: VaultId::legacy_for(db_path),
                salt,
                cost: Argon2Cost::LEGACY,
                body: SidecarBody::V1,
            })
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
            // Un `.kdf` est un fichier de 77 octets qu'un autre process local peut écrire : ses
            // paramètres de coût ne sont pas fiables. Sans ce contrôle, `t_cost = 0` faisait
            // paniquer `Params::new` au déverrouillage, et `m_cost = 0xFFFF_FFFF` (4 Tio) tentait
            // une allocation géante (OOM, potentiellement OOM-killer sur d'autres process). On
            // rejette tout paramètre hors des bornes acceptées par Argon2, avec un plafond mémoire
            // très au-dessus du profil courant (64 Mio) mais loin de l'épuisement.
            validate_argon2_params(m_cost, t_cost, p_cost)
                .map_err(|()| StoreError::CorruptKdfParams(path.to_path_buf()))?;
            Ok(Sidecar {
                vault_id: VaultId::V2(vault_id),
                salt,
                cost: Argon2Cost {
                    m_cost,
                    t_cost,
                    p_cost,
                },
                body: SidecarBody::V2 { verifier },
            })
        }
        Some(&V3_VERSION) if bytes.len() == V3_LEN => {
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
            let mut key_id = [0u8; KEY_ID_LEN];
            key_id.copy_from_slice(&bytes[offset..offset + KEY_ID_LEN]);
            offset += KEY_ID_LEN;
            let mut nonce = [0u8; NONCE_LEN];
            nonce.copy_from_slice(&bytes[offset..offset + NONCE_LEN]);
            offset += NONCE_LEN;
            let mut wrapped = [0u8; WRAPPED_LEN];
            wrapped.copy_from_slice(&bytes[offset..offset + WRAPPED_LEN]);
            // Mêmes bornes que pour v2 : les coûts viennent d'un fichier non fiable. L'enveloppe
            // AEAD authentifie l'en-tête, mais seulement une fois la passphrase fournie — un
            // paramètre délirant doit être rejeté avant de tenter la dérivation, pas après.
            validate_argon2_params(m_cost, t_cost, p_cost)
                .map_err(|()| StoreError::CorruptKdfParams(path.to_path_buf()))?;
            Ok(Sidecar {
                vault_id: VaultId::V2(vault_id),
                salt,
                cost: Argon2Cost {
                    m_cost,
                    t_cost,
                    p_cost,
                },
                body: SidecarBody::V3 {
                    key_id,
                    nonce,
                    wrapped,
                },
            })
        }
        _ => Err(StoreError::CorruptKdfParams(path.to_path_buf())),
    }
}

/// Lit le sidecar existant, s'il y en a un. `None` si le fichier n'existe pas — l'appelant en
/// déduit que le coffre lui-même n'existe pas.
///
/// # Errors
///
/// [`StoreError::CorruptKdfParams`] si le fichier existe mais ne correspond à aucun format connu.
pub fn read(db_path: &Path) -> Result<Option<Sidecar>, StoreError> {
    read_sidecar_file(&sidecar_path(db_path), db_path)
}

/// Lit le sidecar en attente d'un changement de passphrase, s'il y en a un. `None` si aucun
/// changement n'est en cours.
///
/// # Errors
///
/// [`StoreError::CorruptKdfParams`] si le fichier existe mais ne correspond à aucun format connu
/// — ne devrait jamais arriver en pratique, [`stage_rekey`] n'écrit que des sidecars valides
/// (v3 depuis le lot 24 ; un `<db>.kdf.new` v2 laissé par une version antérieure reste lisible).
pub fn read_staged(db_path: &Path) -> Result<Option<Sidecar>, StoreError> {
    read_sidecar_file(&staged_sidecar_path(db_path), db_path)
}

/// Crée un sidecar v3 neuf pour un coffre en cours de création : identifiant, sel et **clé
/// maître** aléatoires, paramètres de coût courants. Dérive la clé d'enveloppement, scelle la
/// clé maître, écrit le fichier de façon atomique (0600, refuse d'écraser un sidecar existant).
/// Retourne la clé maître — c'est elle, et jamais une clé dérivée, qui chiffre le coffre.
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
    write_fresh_v3(&path, &vault_id, passphrase)
}

/// Le tronc commun de [`create`] et [`stage_rekey`] : sel, clé maître et nonce neufs, coûts
/// courants, enveloppe scellée sous la passphrase fournie, écriture atomique à `path`.
fn write_fresh_v3(
    path: &Path,
    vault_id: &[u8; VAULT_ID_LEN],
    passphrase: &Passphrase,
) -> Result<(Sidecar, VaultKey), StoreError> {
    let mut salt = [0u8; SALT_LEN];
    rand::rng().fill_bytes(&mut salt);
    let cost = Argon2Cost::CURRENT;

    let mut master_bytes = [0u8; VaultKey::LEN];
    rand::rng().fill_bytes(&mut master_bytes);
    let master = VaultKey::new(master_bytes);
    master_bytes.zeroize();

    let kek = derive_key(passphrase, &salt, cost)?;
    let key_id = compute_key_id(vault_id, &master);
    let body = write_v3(path, vault_id, &salt, cost, &key_id, &kek, &master)?;

    Ok((
        Sidecar {
            vault_id: VaultId::V2(*vault_id),
            salt,
            cost,
            body,
        },
        master,
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
        body: SidecarBody::V2 { verifier },
    })
}

/// Prépare — sans committer — le sidecar de la nouvelle passphrase d'un changement **avec
/// re-chiffrement** (migration v2 → v3) : même `vault_id` (c'est l'identité du coffre, pas de la
/// clé), sel, **clé maître** et nonce neufs, paramètres de coût courants. Le sidecar produit est
/// un v3 : c'est ainsi qu'un coffre v2 migre, au moment où la base est de toute façon
/// ré-chiffrée. Écrit `<db>.kdf.new` de façon durable (fsync du fichier et du répertoire
/// parent) : c'est ce qui rend la fenêtre entre les deux renames de `Store::change_passphrase`
/// récupérable après une interruption, plutôt qu'un simple espoir.
///
/// # Errors
///
/// Une erreur d'IO si l'écriture échoue.
pub fn stage_rekey(
    db_path: &Path,
    vault_id: &VaultId,
    new: &Passphrase,
) -> Result<(Sidecar, VaultKey), StoreError> {
    let path = staged_sidecar_path(db_path);
    let VaultId::V2(vault_id_bytes) = vault_id else {
        // N'arrive jamais en pratique : `Store::open_with_passphrase` migre tout sidecar v1 en
        // v2 avant de rendre un `Store` à l'appelant, donc `change_passphrase` ne voit jamais de
        // `VaultId::LegacyV1`.
        return Err(StoreError::CorruptKdfParams(path));
    };
    write_fresh_v3(&path, vault_id_bytes, new)
}

/// Bascule le sidecar en attente sur le sidecar committé : `rename(<db>.kdf.new, <db>.kdf)` +
/// fsync du répertoire. C'est *le* point de commit d'un changement de passphrase — avant cet
/// appel, l'ancienne passphrase reste celle qui fait autorité.
///
/// # Errors
///
/// Une erreur d'IO si le rename échoue (le sidecar en attente reste alors en place, inchangé).
pub fn commit_staged(db_path: &Path) -> Result<(), StoreError> {
    let staged = staged_sidecar_path(db_path);
    let committed = sidecar_path(db_path);
    fs::rename(&staged, &committed)?;
    sync_dir(committed.parent())?;
    Ok(())
}

/// Supprime un sidecar en attente orphelin (changement de passphrase interrompu avant la bascule
/// de la base, ou déjà mené à son terme). Best-effort : un échec ici ne doit jamais faire
/// échouer l'appelant, l'orphelin sera de toute façon réessayé au prochain succès d'ouverture.
pub fn discard_staged(db_path: &Path) {
    let _ = fs::remove_file(staged_sidecar_path(db_path));
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

/// Scelle `master` sous `kek` (nonce aléatoire neuf, en-tête complet en AAD) et écrit le sidecar
/// v3 de façon atomique. Retourne le corps écrit, pour que l'appelant construise un [`Sidecar`]
/// fidèle au fichier.
fn write_v3(
    path: &Path,
    vault_id: &[u8; VAULT_ID_LEN],
    salt: &[u8; SALT_LEN],
    cost: Argon2Cost,
    key_id: &[u8; KEY_ID_LEN],
    kek: &VaultKey,
    master: &VaultKey,
) -> Result<SidecarBody, StoreError> {
    let mut nonce = [0u8; NONCE_LEN];
    rand::rng().fill_bytes(&mut nonce);
    let header = v3_header_bytes(vault_id, salt, cost, key_id);
    let wrapped = wrap_master_key(kek, &nonce, &header, master)?;

    let mut bytes = Vec::with_capacity(V3_LEN);
    bytes.extend_from_slice(&header);
    bytes.extend_from_slice(&nonce);
    bytes.extend_from_slice(&wrapped);
    write_atomic(path, &bytes)?;

    Ok(SidecarBody::V3 {
        key_id: *key_id,
        nonce,
        wrapped,
    })
}

/// Écrit `bytes` dans `path` de façon atomique et durable : fichier temporaire créé 0600 dès sa
/// création (jamais un umask plus permissif, même transitoirement), `fsync` du fichier puis
/// `rename`, puis `fsync` du répertoire parent — sans ce dernier, un rename fraîchement fait
/// peut ne pas survivre à une coupure d'alimentation, ce qui viderait de son sens la conception
/// « sidecar en attente » du changement de passphrase.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp_path = append_suffix(path, ".tmp");
    {
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp_path)?;
        #[cfg(unix)]
        {
            // `mode()` ne s'applique qu'à la création : un `.tmp` résiduel d'un run précédent
            // (process tué avant le rename) garderait sinon ses anciennes permissions.
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    fs::rename(&tmp_path, path)?;
    sync_dir(path.parent())?;
    Ok(())
}

#[cfg(unix)]
fn sync_dir(dir: Option<&Path>) -> io::Result<()> {
    if let Some(dir) = dir {
        // `Path::new("nom.db").parent()` vaut `Some("")` : un chemin nu désigne le répertoire
        // courant, que `File::open("")` ne sait pas ouvrir (lot 36 — `freeflow init --db nom.db`
        // laissait un sidecar orphelin après cet échec).
        let dir = if dir.as_os_str().is_empty() {
            Path::new(".")
        } else {
            dir
        };
        fs::File::open(dir)?.sync_all()?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn sync_dir(_dir: Option<&Path>) -> io::Result<()> {
    // Pas d'équivalent portable simple à l'ouverture d'un répertoire comme fichier ; documenté
    // comme limite Windows, au même titre que `tighten_permissions` dans `store.rs`.
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
    fn argon2_params_out_of_bounds_are_rejected() {
        // Bornes légitimes (profils réels).
        assert!(validate_argon2_params(65_536, 3, 1).is_ok());
        assert!(validate_argon2_params(19_456, 2, 1).is_ok());
        // Invalides : feraient paniquer `Params::new` ou provoqueraient un OOM.
        assert!(validate_argon2_params(65_536, 0, 1).is_err(), "t_cost = 0");
        assert!(validate_argon2_params(65_536, 3, 0).is_err(), "p_cost = 0");
        assert!(validate_argon2_params(4, 3, 1).is_err(), "m_cost < 8*p");
        assert!(
            validate_argon2_params(u32::MAX, 3, 1).is_err(),
            "m_cost délirant (OOM)"
        );
        // Ne panique pas malgré un p_cost qui ferait déborder 8*p_cost.
        assert!(validate_argon2_params(65_536, 3, u32::MAX).is_err());
    }

    #[test]
    fn a_v2_sidecar_with_forged_argon2_params_is_rejected_not_panicked() {
        let db_path = temp_db_path("forged-params");
        // Reconstruit la disposition V2 avec t_cost = 0 (paramètre illégal qu'un attaquant
        // local pourrait écrire pour faire paniquer le déverrouillage).
        let mut bytes = vec![V2_VERSION];
        bytes.extend_from_slice(&[9u8; VAULT_ID_LEN]);
        bytes.extend_from_slice(&[7u8; SALT_LEN]);
        bytes.extend_from_slice(&65_536u32.to_le_bytes()); // m_cost
        bytes.extend_from_slice(&0u32.to_le_bytes()); // t_cost = 0 (illégal)
        bytes.extend_from_slice(&1u32.to_le_bytes()); // p_cost
        bytes.extend_from_slice(&[0u8; VERIFIER_LEN]);
        assert_eq!(bytes.len(), V2_LEN, "disposition V2 attendue");
        fs::write(sidecar_path(&db_path), &bytes).unwrap();

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

    #[test]
    fn a_staged_sidecar_carries_the_same_vault_id_but_a_new_salt_and_current_cost() {
        let db_path = temp_db_path("stage-rekey");
        let (committed, _) = create(&db_path, &"s3cret".into()).unwrap();

        let (staged, new_key) =
            stage_rekey(&db_path, &committed.vault_id, &"new-s3cret".into()).unwrap();
        assert_eq!(staged.vault_id, committed.vault_id);
        assert_ne!(staged.salt(), committed.salt());
        assert!(staged.verify(&new_key));
        // Le sidecar committé n'a pas bougé : le sidecar en attente est un fichier à part.
        let reread_committed = read(&db_path).unwrap().unwrap();
        assert_eq!(reread_committed.salt(), committed.salt());
    }

    #[test]
    fn committing_a_staged_sidecar_replaces_the_committed_one_atomically() {
        let db_path = temp_db_path("commit-staged");
        create(&db_path, &"s3cret".into()).unwrap();
        let sidecar = read(&db_path).unwrap().unwrap();

        let (staged, new_key) =
            stage_rekey(&db_path, &sidecar.vault_id, &"new-s3cret".into()).unwrap();
        commit_staged(&db_path).unwrap();

        assert!(read_staged(&db_path).unwrap().is_none());
        let committed = read(&db_path).unwrap().unwrap();
        assert_eq!(committed.salt(), staged.salt());
        assert!(committed.verify(&new_key));
    }

    #[test]
    #[cfg(unix)]
    fn a_sidecar_is_never_left_world_readable_even_transiently() {
        use std::os::unix::fs::PermissionsExt;
        let db_path = temp_db_path("permissions");
        create(&db_path, &"s3cret".into()).unwrap();

        let mode = fs::metadata(sidecar_path(&db_path))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        assert!(!append_suffix(&sidecar_path(&db_path), ".tmp").exists());
    }

    // -- Sidecar v3 : clé maître enveloppée (lot 24) -------------------------------------------

    #[test]
    fn a_fresh_sidecar_is_v3_and_round_trips_the_master_key_through_the_passphrase() {
        let db_path = temp_db_path("v3-round-trip");
        let (sidecar, master) = create(&db_path, &"s3cret".into()).unwrap();
        assert_eq!(sidecar.version(), 3);
        assert!(sidecar.wraps_master_key());
        assert_eq!(fs::read(sidecar_path(&db_path)).unwrap().len(), V3_LEN);

        let reread = read(&db_path).unwrap().unwrap();
        let unwrapped = reread.key_from_passphrase(&"s3cret".into()).unwrap();
        assert_eq!(unwrapped.as_bytes(), master.as_bytes());
        assert!(matches!(
            reread.key_from_passphrase(&"wrong".into()),
            Err(StoreError::WrongPassphrase)
        ));
    }

    #[test]
    fn any_tampered_byte_of_a_v3_sidecar_is_rejected() {
        // L'enveloppe AEAD authentifie l'en-tête entier (AAD) en plus du scellé : altérer
        // n'importe quel octet — identifiant, sel, coûts, key_id, nonce, scellé — doit rejeter
        // la passphrase pourtant correcte (ou signaler un fichier corrompu pour les octets que
        // le parseur borne lui-même, version et coûts).
        let db_path = temp_db_path("v3-tamper");
        create(&db_path, &"s3cret".into()).unwrap();
        let original = fs::read(sidecar_path(&db_path)).unwrap();
        assert_eq!(original.len(), V3_LEN);

        // Un octet représentatif par zone du format, pas tous les 133 : chaque essai coûte une
        // dérivation Argon2 complète (64 Mio), les zones restantes sont couvertes par symétrie.
        let one_byte_per_zone = [
            0,                         // version
            1,                         // vault_id
            1 + VAULT_ID_LEN,          // sel
            34,                        // m_cost (2e octet : reste borné, la dérivation diverge)
            37,            // t_cost (1er octet : 3 → 2, dérivation divergente et bon marché)
            41,            // p_cost (1 → 0 : rejeté par les bornes avant toute dérivation)
            45,            // key_id
            V3_HEADER_LEN, // nonce
            V3_HEADER_LEN + NONCE_LEN, // scellé
            V3_LEN - 1,    // tag
        ];
        for index in one_byte_per_zone {
            let mut tampered = original.clone();
            tampered[index] ^= 0x01;
            fs::write(sidecar_path(&db_path), &tampered).unwrap();
            let outcome = read(&db_path)
                .and_then(|sidecar| sidecar.unwrap().key_from_passphrase(&"s3cret".into()));
            assert!(outcome.is_err(), "octet {index} altéré accepté à tort");
        }
    }

    #[test]
    fn rewrap_changes_the_envelope_but_never_the_master_key_nor_the_vault_id() {
        let db_path = temp_db_path("v3-rewrap");
        let (before, master) = create(&db_path, &"old-s3cret".into()).unwrap();

        let after = before
            .rewrap(&db_path, &"old-s3cret".into(), &"new-s3cret".into())
            .unwrap();
        assert_eq!(after.vault_id, before.vault_id);
        assert_ne!(after.salt(), before.salt());

        let reread = read(&db_path).unwrap().unwrap();
        assert_eq!(reread.version(), 3);
        let unwrapped = reread.key_from_passphrase(&"new-s3cret".into()).unwrap();
        assert_eq!(
            unwrapped.as_bytes(),
            master.as_bytes(),
            "la clé maître survit au changement de passphrase — c'est tout l'intérêt du v3"
        );
        assert!(matches!(
            reread.key_from_passphrase(&"old-s3cret".into()),
            Err(StoreError::WrongPassphrase)
        ));
        // `verify` (le contrôle sans passphrase d'une clé sortie du cache) reconnaît toujours
        // la même clé maître, avant comme après.
        assert!(before.verify(&master));
        assert!(reread.verify(&master));
    }

    #[test]
    fn rewrap_rejects_a_wrong_current_passphrase_and_an_identical_new_one() {
        let db_path = temp_db_path("v3-rewrap-refusals");
        let (sidecar, _master) = create(&db_path, &"s3cret".into()).unwrap();
        let bytes_before = fs::read(sidecar_path(&db_path)).unwrap();

        assert!(matches!(
            sidecar.rewrap(&db_path, &"wrong".into(), &"new-s3cret".into()),
            Err(StoreError::WrongPassphrase)
        ));
        assert!(matches!(
            sidecar.rewrap(&db_path, &"s3cret".into(), &"s3cret".into()),
            Err(StoreError::KeyDerivation(_))
        ));
        assert_eq!(
            fs::read(sidecar_path(&db_path)).unwrap(),
            bytes_before,
            "un refus ne réécrit jamais le sidecar"
        );
    }

    #[test]
    fn a_staged_rekey_now_produces_a_v3_sidecar_with_a_fresh_master_key() {
        let db_path = temp_db_path("stage-produces-v3");
        let (committed, old_master) = create(&db_path, &"s3cret".into()).unwrap();

        let (staged, staged_key) =
            stage_rekey(&db_path, &committed.vault_id, &"new-s3cret".into()).unwrap();
        assert_eq!(staged.version(), 3);
        assert_eq!(staged.vault_id, committed.vault_id);
        assert_ne!(
            staged_key.as_bytes(),
            old_master.as_bytes(),
            "la migration tire une clé maître neuve — jamais une clé dérivée d'une passphrase"
        );
        let reread = read_staged(&db_path).unwrap().unwrap();
        let unwrapped = reread.key_from_passphrase(&"new-s3cret".into()).unwrap();
        assert_eq!(unwrapped.as_bytes(), staged_key.as_bytes());
    }
}
