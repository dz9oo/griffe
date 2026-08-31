//! Résolution de références lisibles par un humain vers les identifiants du domaine — un UUID
//! complet, un préfixe d'UUID (comme un hash git court), ou un nom (insensible à la casse et aux
//! accents, exact puis par préfixe). C'est ce qui évite à chaque façade d'inventer sa propre
//! notion de « désigner un client par son nom » : la CLI, le serveur MCP et la GUI appellent
//! toutes ce module plutôt que d'exiger un UUID complet pour chaque référence.

use rusqlite::Connection;

use crate::app::AppError;
use crate::clients;
use crate::domain::ClientId;

/// Résultat de la résolution d'une référence texte vers un identifiant typé. `label` (dans
/// `Ambiguous`) est le libellé lisible du candidat — le nom d'un client, par exemple — pour que
/// la façade appelante puisse lister les candidats sans requête supplémentaire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefMatch<T> {
    Unique(T),
    Ambiguous(Vec<(T, String)>),
    NotFound,
}

/// Longueur minimale d'un préfixe d'UUID pour être tenté comme tel plutôt que comme un nom —
/// en-dessous, un nom de client purement hexadécimal (peu probable, mais possible) resterait
/// résolu par nom plutôt que pris pour un fragment d'UUID.
const MIN_UUID_PREFIX_LEN: usize = 4;

/// Replie les diacritiques latins les plus courants en français sur leur lettre de base — pas
/// une translittération Unicode complète, seulement de quoi rendre `"Societe"` et `"Société"`
/// équivalents pour une recherche par nom.
fn fold_accents(c: char) -> char {
    match c {
        'à' | 'â' | 'ä' | 'á' | 'ã' => 'a',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'î' | 'ï' | 'í' | 'ì' => 'i',
        'ô' | 'ö' | 'ó' | 'ò' | 'õ' => 'o',
        'ù' | 'û' | 'ü' | 'ú' => 'u',
        'ç' => 'c',
        'ñ' => 'n',
        other => other,
    }
}

fn normalize(s: &str) -> String {
    s.to_lowercase().chars().map(fold_accents).collect()
}

/// Résout `needle` en identifiant de client.
///
/// Ordre de résolution : UUID complet ; sinon, si `needle` est un fragment hexadécimal d'au
/// moins [`MIN_UUID_PREFIX_LEN`] caractères, préfixe d'UUID (à la manière d'un hash git court) ;
/// sinon nom exact, insensible à la casse et aux accents ; sinon préfixe de nom, avec la même
/// tolérance.
///
/// # Errors
///
/// Retourne une erreur si la lecture en base échoue.
pub fn resolve_client(conn: &Connection, needle: &str) -> Result<RefMatch<ClientId>, AppError> {
    let trimmed = needle.trim();

    if let Ok(id) = trimmed.parse::<ClientId>() {
        let found = clients::client_by_id(conn, id)?.is_some();
        return Ok(if found {
            RefMatch::Unique(id)
        } else {
            RefMatch::NotFound
        });
    }

    let all = clients::list_clients(conn)?;

    if trimmed.len() >= MIN_UUID_PREFIX_LEN && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
        let prefix = trimmed.to_ascii_lowercase();
        let by_id_prefix: Vec<(ClientId, String)> = all
            .iter()
            .filter(|c| c.id.to_string().starts_with(&prefix))
            .map(|c| (c.id, c.name.clone()))
            .collect();
        if !by_id_prefix.is_empty() {
            return Ok(as_ref_match(by_id_prefix));
        }
    }

    let needle_norm = normalize(trimmed);
    let by_exact_name: Vec<(ClientId, String)> = all
        .iter()
        .filter(|c| normalize(&c.name) == needle_norm)
        .map(|c| (c.id, c.name.clone()))
        .collect();
    if !by_exact_name.is_empty() {
        return Ok(as_ref_match(by_exact_name));
    }

    let by_name_prefix: Vec<(ClientId, String)> = all
        .iter()
        .filter(|c| normalize(&c.name).starts_with(&needle_norm))
        .map(|c| (c.id, c.name.clone()))
        .collect();
    Ok(as_ref_match(by_name_prefix))
}

fn as_ref_match<T>(mut candidates: Vec<(T, String)>) -> RefMatch<T> {
    match candidates.len() {
        0 => RefMatch::NotFound,
        1 => {
            let (id, _) = candidates.remove(0);
            RefMatch::Unique(id)
        }
        _ => RefMatch::Ambiguous(candidates),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Actor, ExecutionContext, Executor};
    use crate::clients::CreateClient;
    use crate::store::{Passphrase, Store};

    fn test_store(label: &str) -> Store {
        let dir = std::env::temp_dir().join(format!(
            "freeflow-reference-test-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ));
        Store::create(&dir.join("vault.db"), &Passphrase::from("s3cret")).unwrap()
    }

    fn create_client(store: &mut Store, name: &str) -> ClientId {
        let cmd = CreateClient {
            name: name.to_string(),
            siren: None,
            vat_number: None,
            address: None,
        };
        let crate::app::Outcome::Applied(id) = Executor::new(store)
            .execute(&cmd, &ExecutionContext::new(Actor::Human, false))
            .unwrap()
        else {
            panic!("expected Applied")
        };
        id
    }

    #[test]
    fn resolves_a_full_uuid() {
        let mut store = test_store("full-uuid");
        let id = create_client(&mut store, "Acme");
        assert_eq!(
            resolve_client(store.connection(), &id.to_string()).unwrap(),
            RefMatch::Unique(id)
        );
    }

    #[test]
    fn a_well_formed_but_unknown_uuid_is_not_found() {
        let store = test_store("unknown-uuid");
        let unknown = ClientId::new();
        assert_eq!(
            resolve_client(store.connection(), &unknown.to_string()).unwrap(),
            RefMatch::NotFound
        );
    }

    #[test]
    fn resolves_a_short_uuid_prefix() {
        let mut store = test_store("uuid-prefix");
        let id = create_client(&mut store, "Acme");
        let prefix = &id.to_string()[..8];
        assert_eq!(
            resolve_client(store.connection(), prefix).unwrap(),
            RefMatch::Unique(id)
        );
    }

    #[test]
    fn resolves_by_exact_name_ignoring_case_and_accents() {
        let mut store = test_store("exact-name");
        let id = create_client(&mut store, "Société Générale");
        assert_eq!(
            resolve_client(store.connection(), "societe generale").unwrap(),
            RefMatch::Unique(id)
        );
    }

    #[test]
    fn resolves_by_name_prefix() {
        let mut store = test_store("name-prefix");
        let id = create_client(&mut store, "Argon Digital");
        assert_eq!(
            resolve_client(store.connection(), "argon").unwrap(),
            RefMatch::Unique(id)
        );
    }

    #[test]
    fn an_ambiguous_name_prefix_lists_every_candidate() {
        let mut store = test_store("ambiguous");
        let a = create_client(&mut store, "Argon Digital");
        let b = create_client(&mut store, "Argon Studio");
        let result = resolve_client(store.connection(), "argon").unwrap();
        let RefMatch::Ambiguous(mut candidates) = result else {
            panic!("expected Ambiguous, got a unique or absent match")
        };
        candidates.sort_by_key(|(id, _)| *id);
        let mut expected = vec![
            (a, "Argon Digital".to_string()),
            (b, "Argon Studio".to_string()),
        ];
        expected.sort_by_key(|(id, _)| *id);
        assert_eq!(candidates, expected);
    }

    #[test]
    fn an_unmatched_name_is_not_found() {
        let mut store = test_store("not-found");
        create_client(&mut store, "Acme");
        assert_eq!(
            resolve_client(store.connection(), "totally-unknown").unwrap(),
            RefMatch::NotFound
        );
    }
}
