//! Le type du dossier et les phrases portent le même nom.
//!
//! Poser un type crée les phrases de ce nom si elles manquent. Retirer le type
//! retire ces phrases. Le dossier qui porte encore d'autres types prend les
//! phrases du premier, par nom, sauf si les phrases déjà posées sont encore
//! l'un de ses types.

use rusqlite::{Connection, OptionalExtension, params};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::app::AppError;
use crate::domain::{ClientId, OpportunityId};

use super::row::{self, ProspectGenre};

const MAX_TYPE_CHARS: usize = 40;

fn folded(name: &str) -> String {
    name.to_lowercase()
}

/// Le genre de phrases de ce nom, sans tenir compte de la casse.
///
/// # Errors
///
/// Lecture SQL impossible.
pub(crate) fn phrase_genre_named(
    conn: &Connection,
    name: &str,
) -> Result<Option<ProspectGenre>, AppError> {
    conn.query_row(
        "SELECT id, name FROM prospect_genres
          WHERE name = ?1 COLLATE NOCASE
          ORDER BY name ASC, id ASC
          LIMIT 1",
        [name],
        |row| {
            Ok(ProspectGenre {
                id: row.get(0)?,
                name: row.get(1)?,
            })
        },
    )
    .optional()
    .map_err(AppError::from)
}

/// Crée les phrases de ce type si ce nom n'en a pas. Les mots de départ sont
/// ceux de la série commune.
///
/// # Errors
///
/// Écriture SQL impossible.
pub(crate) fn ensure_phrase_genre(
    conn: &Connection,
    name: &str,
) -> Result<ProspectGenre, AppError> {
    if let Some(existing) = phrase_genre_named(conn, name)? {
        return Ok(existing);
    }
    let genre = row::insert_genre(conn, name)?;
    row::copy_genre_words(conn, &genre.id, None)?;
    Ok(genre)
}

/// Crée le type de ce nom s'il manque. L'encre, sans note.
///
/// # Errors
///
/// Le nom dépasse 40 caractères, ou l'écriture SQL échoue.
pub(crate) fn ensure_type_named(conn: &Connection, name: &str) -> Result<(), AppError> {
    if type_id_by_folded(conn, name)?.is_some() {
        return Ok(());
    }
    if name.chars().count() > MAX_TYPE_CHARS {
        return Err(crate::work_kinds::WorkKindError::TooLong.into());
    }
    let id = uuid::Uuid::now_v7().to_string();
    let created_at = OffsetDateTime::now_utc().format(&Rfc3339)?;
    conn.execute(
        "INSERT INTO work_kinds (id, name, tint, note, created_at) VALUES (?1, ?2, 'encre', '', ?3)",
        params![id, name, created_at],
    )?;
    Ok(())
}

/// Ajoute ce type sur le dossier, sans retirer les autres.
///
/// # Errors
///
/// Le nom est trop long, ou l'écriture SQL échoue.
pub(crate) fn link_type(conn: &Connection, client: ClientId, name: &str) -> Result<(), AppError> {
    ensure_type_named(conn, name)?;
    let Some(id) = type_id_by_folded(conn, name)? else {
        return Ok(());
    };
    conn.execute(
        "INSERT OR IGNORE INTO dossier_work_kinds (client_id, kind_id) VALUES (?1, ?2)",
        params![client.to_string(), id],
    )?;
    Ok(())
}

/// Retire les types du dossier. Le catalogue reste.
///
/// # Errors
///
/// Écriture SQL impossible.
pub(crate) fn clear_dossier_types(conn: &Connection, client: ClientId) -> Result<(), AppError> {
    conn.execute(
        "DELETE FROM dossier_work_kinds WHERE client_id = ?1",
        [client.to_string()],
    )?;
    Ok(())
}

/// Le genre de phrases suit le nouveau nom du type.
///
/// # Errors
///
/// Écriture SQL impossible.
pub(crate) fn rename_phrase_genre(conn: &Connection, from: &str, to: &str) -> Result<(), AppError> {
    if folded(from) == folded(to) {
        if let Some(genre) = phrase_genre_named(conn, from)? {
            conn.execute(
                "UPDATE prospect_genres SET name = ?1 WHERE id = ?2",
                params![to, genre.id],
            )?;
        }
        return Ok(());
    }
    match (
        phrase_genre_named(conn, from)?,
        phrase_genre_named(conn, to)?,
    ) {
        (Some(old), None) => {
            conn.execute(
                "UPDATE prospect_genres SET name = ?1 WHERE id = ?2",
                params![to, old.id],
            )?;
        }
        (Some(old), Some(_)) => row::delete_genre(conn, &old.id)?,
        (None, _) => {
            ensure_phrase_genre(conn, to)?;
        }
    }
    Ok(())
}

/// Retire les phrases de ce nom. Les dossiers qui les portaient n'ont plus
/// cette série.
///
/// # Errors
///
/// Écriture SQL impossible.
pub(crate) fn drop_phrase_genre_named(conn: &Connection, name: &str) -> Result<(), AppError> {
    if let Some(genre) = phrase_genre_named(conn, name)? {
        row::delete_genre(conn, &genre.id)?;
    }
    Ok(())
}

/// Retire le type de ce nom et ses liens. Rend les dossiers qui le portaient.
///
/// # Errors
///
/// Lecture ou écriture SQL impossible.
pub(crate) fn drop_type_named(conn: &Connection, name: &str) -> Result<Vec<ClientId>, AppError> {
    let Some(id) = type_id_by_folded(conn, name)? else {
        return Ok(Vec::new());
    };
    let clients = clients_of_type(conn, &id)?;
    conn.execute("DELETE FROM dossier_work_kinds WHERE kind_id = ?1", [&id])?;
    conn.execute("DELETE FROM work_kinds WHERE id = ?1", [&id])?;
    Ok(clients)
}

/// Les dossiers qui portent encore ce nom de type.
///
/// # Errors
///
/// Lecture SQL impossible.
pub(crate) fn clients_of_type_named(
    conn: &Connection,
    name: &str,
) -> Result<Vec<ClientId>, AppError> {
    let Some(id) = type_id_by_folded(conn, name)? else {
        return Ok(Vec::new());
    };
    clients_of_type(conn, &id)
}

/// Les phrases du dossier suivent ses types.
///
/// Un dossier sans type n'a plus de phrases propres. S'il en porte plusieurs,
/// les phrases déjà posées restent lorsqu'elles sont encore l'un des types.
/// Sinon, le premier type par nom fournit les phrases.
///
/// # Errors
///
/// Lecture ou écriture SQL impossible.
pub(crate) fn phrases_follow_types(conn: &Connection, client: ClientId) -> Result<(), AppError> {
    let names = type_names_of_client(conn, client)?;
    for name in &names {
        ensure_phrase_genre(conn, name)?;
    }
    for id in opportunity_ids_of_client(conn, client)? {
        if names.is_empty() {
            row::set_opportunity_genre(conn, id, None)?;
            continue;
        }
        let current = row::genre_of_opportunity(conn, id)?;
        let chosen = current
            .as_ref()
            .filter(|genre| names.iter().any(|name| folded(name) == folded(&genre.name)))
            .map_or_else(|| names[0].clone(), |genre| genre.name.clone());
        let genre = ensure_phrase_genre(conn, &chosen)?;
        row::set_opportunity_genre(conn, id, Some(&genre.id))?;
    }
    Ok(())
}

fn type_id_by_folded(conn: &Connection, name: &str) -> Result<Option<String>, AppError> {
    conn.query_row(
        "SELECT id FROM work_kinds WHERE name = ?1 COLLATE NOCASE ORDER BY name ASC, id ASC LIMIT 1",
        [name],
        |row| row.get(0),
    )
    .optional()
    .map_err(AppError::from)
}

fn clients_of_type(conn: &Connection, id: &str) -> Result<Vec<ClientId>, AppError> {
    let mut stmt = conn.prepare("SELECT client_id FROM dossier_work_kinds WHERE kind_id = ?1")?;
    let rows = stmt.query_map([id], |row| row.get::<_, String>(0))?;
    rows.map(|row| parse_client(&row?))
        .collect::<Result<Vec<_>, _>>()
}

fn type_names_of_client(conn: &Connection, client: ClientId) -> Result<Vec<String>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT k.name
           FROM dossier_work_kinds AS l
           JOIN work_kinds AS k ON k.id = l.kind_id
          WHERE l.client_id = ?1
          ORDER BY k.name COLLATE NOCASE ASC, k.id ASC",
    )?;
    let rows = stmt.query_map([client.to_string()], |row| row.get(0))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
}

fn opportunity_ids_of_client(
    conn: &Connection,
    client: ClientId,
) -> Result<Vec<OpportunityId>, AppError> {
    let mut stmt = conn.prepare("SELECT id FROM opportunities WHERE client_id = ?1")?;
    let rows = stmt.query_map([client.to_string()], |row| row.get::<_, String>(0))?;
    rows.map(|row| parse_opportunity(&row?))
        .collect::<Result<Vec<_>, _>>()
}

fn parse_client(value: &str) -> Result<ClientId, AppError> {
    value
        .parse()
        .map_err(|error: uuid::Error| AppError::Domain(error.to_string()))
}

fn parse_opportunity(value: &str) -> Result<OpportunityId, AppError> {
    value
        .parse()
        .map_err(|error: uuid::Error| AppError::Domain(error.to_string()))
}
