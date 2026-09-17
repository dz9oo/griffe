//! Justificatifs de dépense **chiffrés** (lot 39). Jusqu'ici une pièce (facture fournisseur,
//! note de frais) était copiée en clair dans `receipts/` à côté du coffre — lisible par
//! quiconque lit le disque, alors que tout le reste du produit est chiffré. Désormais chaque
//! pièce vit dans `<coffre>.receipts/` (un dossier **par coffre**) sous XChaCha20-Poly1305,
//! avec une clé dérivée de la clé maître ([`Store::receipts_key`], HKDF-SHA256, domaine
//! séparé) : la promesse « un seul fichier chiffré » du README, ramenée à « le coffre et ses
//! pièces, chiffrés ». Le nom du fichier reste `<hash SHA-256 du contenu en clair>-<nom
//! d'origine>` (stockage adressé par contenu, comme avant) ; le hash est aussi ce que la dépense
//! persiste (`receipt_hash`), donc l'intégrité se vérifie sans déchiffrer.
//!
//! De l'IO de fichier, comme [`Store::backup_to`] : jamais appelée depuis un `Command::apply`
//! (qui ne touche que `&Connection`), toujours par l'adaptateur — CLI, fenêtre et serveur MCP
//! partagent cette seule implémentation.

use std::fs;
use std::path::{Path, PathBuf};

use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce};
use rand::Rng as _;
use thiserror::Error;

use crate::expenses::hash_receipt;
use crate::store::Store;

/// En-tête d'un fichier de pièce chiffrée : magie, version, puis 24 octets de nonce.
const MAGIC: &[u8; 4] = b"FFR1";
const NONCE_LEN: usize = 24;

#[derive(Debug, Error)]
pub enum ReceiptError {
    #[error("justificatif introuvable : {0}")]
    NotFound(String),
    #[error("justificatif illisible ({0}) : ce n'est pas une pièce chiffrée par ce coffre")]
    Corrupt(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

/// Ce que l'archivage rend : le hash d'intégrité et le nom du fichier archivé, tels que la
/// dépense les persiste.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchivedReceipt {
    pub hash: String,
    pub filename: String,
}

fn cipher(store: &Store) -> XChaCha20Poly1305 {
    let key = chacha20poly1305::Key::from(*store.receipts_key().as_bytes());
    XChaCha20Poly1305::new(&key)
}

/// Le nom d'origine réduit à son composant final : un nom venu d'un navigateur ou d'un agent ne
/// doit pas pouvoir sortir du dossier (« ../x »).
fn safe_name(original_name: &str) -> String {
    Path::new(original_name)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty() && n != "." && n != "..")
        .unwrap_or_else(|| "justificatif".to_string())
}

#[cfg(unix)]
fn tighten(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
}

#[cfg(not(unix))]
fn tighten(_path: &Path, _mode: u32) {}

/// Chiffre et archive `content` dans `<coffre>.receipts/<hash>-<nom>` ; renvoie ce qu'il faut
/// persister sur la dépense. Idempotent : le même contenu sous le même nom réécrit le même
/// fichier (autre nonce, même hash).
///
/// # Errors
///
/// Une erreur d'IO (dossier non créable, fichier non inscriptible).
pub fn archive(
    store: &Store,
    original_name: &str,
    content: &[u8],
) -> Result<ArchivedReceipt, ReceiptError> {
    let hash = hash_receipt(content);
    let filename = format!("{hash}-{}", safe_name(original_name));
    let dir = store.receipts_dir();
    fs::create_dir_all(&dir)?;
    tighten(&dir, 0o700);

    let mut nonce = [0u8; NONCE_LEN];
    rand::rng().fill_bytes(&mut nonce);
    let mut header = Vec::with_capacity(4 + NONCE_LEN);
    header.extend_from_slice(MAGIC);
    header.extend_from_slice(&nonce);
    let ciphertext = cipher(store)
        .encrypt(
            <&XNonce>::from(&nonce),
            Payload {
                msg: content,
                aad: filename.as_bytes(),
            },
        )
        .map_err(|_| ReceiptError::Corrupt(filename.clone()))?;
    let mut bytes = header;
    bytes.extend_from_slice(&ciphertext);
    let path = dir.join(&filename);
    fs::write(&path, bytes)?;
    tighten(&path, 0o600);
    Ok(ArchivedReceipt { hash, filename })
}

/// Le chemin d'une pièce archivée (chiffrée) de ce coffre.
#[must_use]
pub fn path_of(store: &Store, filename: &str) -> PathBuf {
    store.receipts_dir().join(safe_name(filename))
}

/// Déchiffre une pièce archivée. Le nom de fichier fait partie des données authentifiées : une
/// pièce renommée ou déplacée entre coffres est refusée.
///
/// # Errors
///
/// [`ReceiptError::NotFound`] si le fichier n'existe pas, [`ReceiptError::Corrupt`] s'il n'est
/// pas une pièce chiffrée par ce coffre (en-tête inconnu, tag invalide).
pub fn read(store: &Store, filename: &str) -> Result<Vec<u8>, ReceiptError> {
    let filename = safe_name(filename);
    let path = store.receipts_dir().join(&filename);
    let bytes = match fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(ReceiptError::NotFound(filename));
        }
        Err(e) => return Err(e.into()),
    };
    let body = bytes
        .strip_prefix(MAGIC)
        .filter(|b| b.len() >= NONCE_LEN)
        .ok_or_else(|| ReceiptError::Corrupt(filename.clone()))?;
    let (nonce, ciphertext) = body.split_at(NONCE_LEN);
    let nonce: [u8; NONCE_LEN] = nonce
        .try_into()
        .map_err(|_| ReceiptError::Corrupt(filename.clone()))?;
    cipher(store)
        .decrypt(
            <&XNonce>::from(&nonce),
            Payload {
                msg: ciphertext,
                aad: filename.as_bytes(),
            },
        )
        .map_err(|_| ReceiptError::Corrupt(filename))
}

/// Migre les pièces en clair de l'ancien `receipts/` (à côté du coffre, lots 11 à 38) vers
/// `<coffre>.receipts/` chiffré — au premier déverrouillage, **idempotente** : une pièce déjà
/// présente sous son nom dans le nouveau dossier est simplement retirée de l'ancien, et
/// l'ancien dossier disparaît quand il est vide. Renvoie le nombre de pièces migrées.
///
/// # Errors
///
/// Une erreur d'IO — la pièce en cours reste alors en clair, à reprendre au prochain
/// déverrouillage.
pub fn migrate_legacy(store: &Store) -> Result<usize, ReceiptError> {
    let legacy = store
        .db_path()
        .parent()
        .map_or_else(|| PathBuf::from("receipts"), |p| p.join("receipts"));
    if !legacy.is_dir() {
        return Ok(0);
    }
    let mut migrated = 0;
    for entry in fs::read_dir(&legacy)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let target = store.receipts_dir().join(&name);
        if !target.exists() {
            let content = fs::read(entry.path())?;
            // Le nom archivé est déjà `<hash>-<nom>` : on le conserve tel quel pour que la
            // dépense qui le référence le retrouve.
            let (_, original) = name.split_once('-').unwrap_or(("", &name));
            let archived = archive(store, original, &content)?;
            if archived.filename != name {
                // Contenu altéré depuis son archivage (le hash ne colle plus) : on garde le nom
                // que la dépense connaît, en réécrivant sous ce nom.
                fs::rename(store.receipts_dir().join(&archived.filename), &target)?;
            }
            migrated += 1;
        }
        fs::remove_file(entry.path())?;
    }
    if fs::read_dir(&legacy)?.next().is_none() {
        let _ = fs::remove_dir(&legacy);
    }
    Ok(migrated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::testing::test_store;

    #[test]
    fn a_receipt_is_stored_encrypted_beside_the_vault_and_read_back() {
        let store = test_store("roundtrip");
        let archived = archive(&store, "../../facture.pdf", b"%PDF-1.4 secret").unwrap();
        assert!(
            archived.filename.ends_with("-facture.pdf"),
            "{}",
            archived.filename
        );
        assert_eq!(archived.hash, hash_receipt(b"%PDF-1.4 secret"));
        let path = path_of(&store, &archived.filename);
        assert!(path.starts_with(store.receipts_dir()));
        let raw = fs::read(&path).unwrap();
        assert!(raw.starts_with(b"FFR1"));
        assert!(
            !raw.windows(4).any(|w| w == b"%PDF"),
            "le contenu ne doit pas apparaître en clair"
        );
        assert_eq!(
            read(&store, &archived.filename).unwrap(),
            b"%PDF-1.4 secret"
        );
        // Un autre coffre ne lit pas cette pièce (autre clé maître).
        let other = test_store("other");
        fs::create_dir_all(other.receipts_dir()).unwrap();
        fs::copy(&path, other.receipts_dir().join(&archived.filename)).unwrap();
        assert!(matches!(
            read(&other, &archived.filename),
            Err(ReceiptError::Corrupt(_))
        ));
        assert!(matches!(
            read(&store, "absent.pdf"),
            Err(ReceiptError::NotFound(_))
        ));
    }

    #[test]
    fn legacy_plaintext_receipts_are_migrated_once() {
        let store = test_store("migrate");
        let legacy = store.db_path().parent().unwrap().join("receipts");
        fs::create_dir_all(&legacy).unwrap();
        let hash = hash_receipt(b"note de frais");
        let name = format!("{hash}-note.pdf");
        fs::write(legacy.join(&name), b"note de frais").unwrap();
        assert_eq!(migrate_legacy(&store).unwrap(), 1);
        assert!(!legacy.exists(), "l'ancien dossier vide disparaît");
        assert_eq!(read(&store, &name).unwrap(), b"note de frais");
        assert_eq!(migrate_legacy(&store).unwrap(), 0, "idempotente");
    }
}
