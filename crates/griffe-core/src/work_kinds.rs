//! Types de travaux d'un dossier. Le genre classe les phrases. Le type dit le métier.
//! Plusieurs types tiennent sur une même fiche. Le catalogue reste quand on décoche.

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::app::{AppError, Command};
use crate::clients::client_by_id;
use crate::domain::ClientId;

const MAX_NAME_CHARS: usize = 40;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum WorkKindError {
    #[error("Ce type n'a pas de nom.")]
    Empty,
    #[error("Ce type existe déjà.")]
    Duplicate,
    #[error("Ce type dépasse 40 caractères.")]
    TooLong,
    #[error("Ce type n'existe pas.")]
    Unknown,
    #[error("Ce dossier n'existe pas.")]
    UnknownDossier,
}

impl From<WorkKindError> for AppError {
    fn from(error: WorkKindError) -> Self {
        Self::Domain(error.to_string())
    }
}

/// Un type du catalogue. `created_at` est un horodatage RFC 3339, non affiché.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkKind {
    pub id: String,
    pub name: String,
    pub created_at: String,
}

/// Une fiche qui porte un type. `name` est le « Qui » du dossier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkKindDossier {
    pub client: ClientId,
    pub name: String,
}

/// Crée un type. Le nom vide est refusé, le nom trop long aussi.
/// Un nom déjà présent, sans tenir compte de la casse, est refusé.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateWorkKind {
    pub name: String,
}

impl Command for CreateWorkKind {
    type Output = WorkKind;
    const NAME: &'static str = "people.create_work_kind";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let name = checked_name(&self.name)?;
        if id_of(&work_kinds(conn)?, &name).is_some() {
            return Err(WorkKindError::Duplicate.into());
        }
        insert_kind(conn, &name)
    }
}

/// Renomme un type. Les dossiers gardent le lien. La date et l'identifiant restent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenameWorkKind {
    pub id: String,
    pub name: String,
}

impl Command for RenameWorkKind {
    type Output = WorkKind;
    const NAME: &'static str = "people.rename_work_kind";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let name = checked_name(&self.name)?;
        let id = self.id.trim();
        let Some(current) = work_kind_by_id(conn, id)? else {
            return Err(WorkKindError::Unknown.into());
        };
        if let Some(other) = id_of(&work_kinds(conn)?, &name)
            && other != current.id
        {
            return Err(WorkKindError::Duplicate.into());
        }
        if current.name == name {
            return Ok(current);
        }
        conn.execute(
            "UPDATE work_kinds SET name = ?1 WHERE id = ?2",
            params![name, current.id],
        )?;
        Ok(WorkKind { name, ..current })
    }
}

/// Retire le type et tous les liens. Le catalogue ne le porte plus.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteWorkKind {
    pub id: String,
}

impl Command for DeleteWorkKind {
    type Output = ();
    const NAME: &'static str = "people.delete_work_kind";

    fn requires_confirmation(&self) -> bool {
        true
    }

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        let id = self.id.trim();
        if work_kind_by_id(conn, id)?.is_none() {
            return Err(WorkKindError::Unknown.into());
        }
        conn.execute("DELETE FROM dossier_work_kinds WHERE kind_id = ?1", [id])?;
        conn.execute("DELETE FROM work_kinds WHERE id = ?1", [id])?;
        Ok(())
    }
}

/// Remplace l'ensemble des types du dossier.
/// Un nom inconnu est créé. Un nom vide dans la liste est refusé, et rien n'est écrit.
/// Une liste vide retire les liens et laisse le catalogue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetDossierWorkKinds {
    pub client: ClientId,
    pub names: Vec<String>,
}

impl Command for SetDossierWorkKinds {
    type Output = Vec<WorkKind>;
    const NAME: &'static str = "people.set_work_kinds";

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        if client_by_id(conn, self.client)?.is_none() {
            return Err(WorkKindError::UnknownDossier.into());
        }
        let wanted = prepare_names(&self.names)?;
        let mut catalog = work_kinds(conn)?;
        let mut chosen = Vec::new();
        for name in wanted {
            if let Some(id) = id_of(&catalog, &name) {
                chosen.push(id);
            } else {
                let created = insert_kind(conn, &name)?;
                chosen.push(created.id.clone());
                catalog.push(created);
            }
        }
        conn.execute(
            "DELETE FROM dossier_work_kinds WHERE client_id = ?1",
            [self.client.to_string()],
        )?;
        for id in &chosen {
            conn.execute(
                "INSERT INTO dossier_work_kinds (client_id, kind_id) VALUES (?1, ?2)",
                params![self.client.to_string(), id],
            )?;
        }
        dossier_work_kinds(conn, self.client)
    }
}

/// Le catalogue, par nom.
///
/// # Errors
///
/// Retourne une erreur si la lecture SQL échoue.
pub fn work_kinds(conn: &Connection) -> Result<Vec<WorkKind>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, name, created_at FROM work_kinds ORDER BY name COLLATE NOCASE ASC, id ASC",
    )?;
    let rows = stmt.query_map([], read_kind)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
}

/// Les types d'un dossier, par nom. Un dossier sans lien donne une liste vide.
///
/// # Errors
///
/// Retourne une erreur si la lecture SQL échoue.
pub fn dossier_work_kinds(conn: &Connection, client: ClientId) -> Result<Vec<WorkKind>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT k.id, k.name, k.created_at
           FROM dossier_work_kinds AS l
           JOIN work_kinds AS k ON k.id = l.kind_id
          WHERE l.client_id = ?1
          ORDER BY k.name COLLATE NOCASE ASC, k.id ASC",
    )?;
    let rows = stmt.query_map([client.to_string()], read_kind)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
}

/// Les dossiers qui portent ce type. Un identifiant inconnu est refusé.
///
/// # Errors
///
/// Retourne une erreur si le type n'existe pas, ou si la lecture SQL échoue.
pub fn dossiers_of_work_kind(
    conn: &Connection,
    id: &str,
) -> Result<Vec<WorkKindDossier>, AppError> {
    let id = id.trim();
    if work_kind_by_id(conn, id)?.is_none() {
        return Err(WorkKindError::Unknown.into());
    }
    let mut stmt = conn.prepare(
        "SELECT c.id, c.name
           FROM dossier_work_kinds AS l
           JOIN clients AS c ON c.id = l.client_id
          WHERE l.kind_id = ?1
          ORDER BY c.name COLLATE NOCASE ASC, c.id ASC",
    )?;
    let rows = stmt.query_map([id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut dossiers = Vec::new();
    for row in rows {
        let (client, name) = row?;
        let client = parse_client(&client)?;
        dossiers.push(WorkKindDossier { client, name });
    }
    Ok(dossiers)
}

/// Chaque lien, dans l'ordre du dossier puis du nom. Sert à remplir la liste des affaires.
///
/// # Errors
///
/// Retourne une erreur si la lecture SQL échoue.
pub(crate) fn attached_kind_names(conn: &Connection) -> Result<Vec<(ClientId, String)>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT l.client_id, k.name
           FROM dossier_work_kinds AS l
           JOIN work_kinds AS k ON k.id = l.kind_id
          ORDER BY l.client_id ASC, k.name COLLATE NOCASE ASC, k.id ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut links = Vec::new();
    for row in rows {
        let (client, name) = row?;
        links.push((parse_client(&client)?, name));
    }
    Ok(links)
}

fn read_kind(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkKind> {
    Ok(WorkKind {
        id: row.get(0)?,
        name: row.get(1)?,
        created_at: row.get(2)?,
    })
}

fn work_kind_by_id(conn: &Connection, id: &str) -> Result<Option<WorkKind>, AppError> {
    conn.query_row(
        "SELECT id, name, created_at FROM work_kinds WHERE id = ?1",
        [id],
        read_kind,
    )
    .optional()
    .map_err(AppError::from)
}

fn insert_kind(conn: &Connection, name: &str) -> Result<WorkKind, AppError> {
    let kind = WorkKind {
        id: uuid::Uuid::now_v7().to_string(),
        name: name.to_string(),
        created_at: OffsetDateTime::now_utc().format(&Rfc3339)?,
    };
    conn.execute(
        "INSERT INTO work_kinds (id, name, created_at) VALUES (?1, ?2, ?3)",
        params![kind.id, kind.name, kind.created_at],
    )?;
    Ok(kind)
}

fn checked_name(raw: &str) -> Result<String, WorkKindError> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(WorkKindError::Empty);
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(WorkKindError::TooLong);
    }
    Ok(name.to_string())
}

fn folded(name: &str) -> String {
    name.to_lowercase()
}

fn id_of(catalog: &[WorkKind], name: &str) -> Option<String> {
    let needle = folded(name);
    catalog
        .iter()
        .find(|kind| folded(&kind.name) == needle)
        .map(|kind| kind.id.clone())
}

fn prepare_names(raw: &[String]) -> Result<Vec<String>, WorkKindError> {
    let mut names: Vec<String> = Vec::new();
    for item in raw {
        let name = checked_name(item)?;
        if names
            .iter()
            .any(|existing| folded(existing) == folded(&name))
        {
            continue;
        }
        names.push(name);
    }
    Ok(names)
}

fn parse_client(value: &str) -> Result<ClientId, AppError> {
    value
        .parse()
        .map_err(|error: uuid::Error| AppError::Domain(error.to_string()))
}

#[cfg(test)]
mod tests {
    use time::Date;

    use super::*;
    use crate::app::{Actor, ExecutionContext, Executor, Outcome};
    use crate::domain::{FollowUpSubject, Money, Probability};
    use crate::follow_up::{SetDossierGenre, prospect_genre_for};
    use crate::prospection::{CreateProspect, opportunity_by_id};
    use crate::store::testing::test_store;

    fn human() -> ExecutionContext {
        ExecutionContext::new(Actor::Human, false)
    }

    fn agent(dry_run: bool) -> ExecutionContext {
        ExecutionContext::new(
            Actor::Agent {
                session: "mcp-test".into(),
            },
            dry_run,
        )
    }

    fn applied<C: Command>(store: &mut crate::store::Store, command: &C) -> C::Output
    where
        C::Output: std::fmt::Debug,
    {
        match Executor::new(store).execute(command, &human()).unwrap() {
            Outcome::Applied(value) => value,
            other => panic!("application attendue, obtenu {other:?}"),
        }
    }

    fn domain<C>(store: &mut crate::store::Store, command: &C) -> String
    where
        C: Command,
        C::Output: std::fmt::Debug,
    {
        match Executor::new(store).execute(command, &human()).unwrap_err() {
            AppError::Domain(message) => message,
            other => panic!("refus de domaine attendu, obtenu {other}"),
        }
    }

    fn conversation(
        store: &mut crate::store::Store,
        qui: &str,
    ) -> (ClientId, crate::domain::OpportunityId) {
        let next = Date::from_calendar_date(2026, time::Month::October, 7).unwrap();
        let id = applied(
            store,
            &CreateProspect {
                prospect_name: qui.into(),
                address: None,
                representative: None,
                email: None,
                phone: None,
                name: format!("Affaire {qui}"),
                amount: Money::from_cents(100_000),
                probability: Probability::new(40).unwrap(),
                next_action_at: next,
                source: None,
            },
        );
        let client = opportunity_by_id(store.connection(), id)
            .unwrap()
            .unwrap()
            .client_id;
        (client, id)
    }

    fn kind_names(store: &crate::store::Store, client: ClientId) -> Vec<String> {
        dossier_work_kinds(store.connection(), client)
            .unwrap()
            .into_iter()
            .map(|kind| kind.name)
            .collect()
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn site_web_follows_the_dossier_through_rename_and_leaves_on_delete() {
        let mut store = test_store("types-parcours");
        let (quai, quai_affaire) = conversation(&mut store, "Atelier Quai");
        let (port, _) = conversation(&mut store, "Atelier Port");
        applied(
            &mut store,
            &SetDossierGenre {
                subject: FollowUpSubject::Opportunity(quai_affaire),
                name: "Mairie".into(),
            },
        );

        let created = applied(
            &mut store,
            &SetDossierWorkKinds {
                client: quai,
                names: vec!["site web".into()],
            },
        );
        assert_eq!(created.len(), 1);
        assert_eq!(created[0].name, "site web");
        let site_id = created[0].id.clone();
        let created_at = created[0].created_at.clone();
        assert_eq!(
            work_kinds(store.connection())
                .unwrap()
                .into_iter()
                .map(|kind| kind.name)
                .collect::<Vec<_>>(),
            vec!["site web".to_string()]
        );
        let listed = dossiers_of_work_kind(store.connection(), &site_id).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].client, quai);
        assert_eq!(listed[0].name, "Atelier Quai");

        let both = applied(
            &mut store,
            &SetDossierWorkKinds {
                client: port,
                names: vec!["site web".into(), "backend".into()],
            },
        );
        assert_eq!(
            both.iter()
                .map(|kind| kind.name.as_str())
                .collect::<Vec<_>>(),
            vec!["backend", "site web"]
        );
        assert_eq!(kind_names(&store, quai), vec!["site web".to_string()]);
        assert_eq!(work_kinds(store.connection()).unwrap().len(), 2);

        let renamed = applied(
            &mut store,
            &RenameWorkKind {
                id: site_id.clone(),
                name: "site".into(),
            },
        );
        assert_eq!(renamed.id, site_id);
        assert_eq!(renamed.created_at, created_at);
        assert_eq!(renamed.name, "site");
        assert_eq!(kind_names(&store, quai), vec!["site".to_string()]);
        assert_eq!(
            kind_names(&store, port),
            vec!["backend".to_string(), "site".to_string()]
        );

        applied(
            &mut store,
            &SetDossierWorkKinds {
                client: quai,
                names: Vec::new(),
            },
        );
        assert!(kind_names(&store, quai).is_empty());
        assert!(
            work_kinds(store.connection())
                .unwrap()
                .iter()
                .any(|kind| kind.name == "site")
        );

        applied(
            &mut store,
            &DeleteWorkKind {
                id: site_id.clone(),
            },
        );
        assert!(kind_names(&store, quai).is_empty());
        assert_eq!(kind_names(&store, port), vec!["backend".to_string()]);
        assert!(
            !work_kinds(store.connection())
                .unwrap()
                .iter()
                .any(|kind| kind.id == site_id)
        );
        let missing = dossiers_of_work_kind(store.connection(), &site_id).unwrap_err();
        assert!(matches!(missing, AppError::Domain(message) if message == "Ce type n'existe pas."));
        assert_eq!(
            prospect_genre_for(store.connection(), quai_affaire)
                .unwrap()
                .unwrap()
                .name,
            "Mairie"
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn names_refuse_empty_long_and_case_duplicates_and_unknowns() {
        let mut store = test_store("types-gardes");
        let (quai, _) = conversation(&mut store, "Atelier des gardes");

        assert_eq!(
            domain(&mut store, &CreateWorkKind { name: "   ".into() }),
            "Ce type n'a pas de nom."
        );
        let forty = "é".repeat(40);
        let kept = applied(
            &mut store,
            &CreateWorkKind {
                name: forty.clone(),
            },
        );
        assert_eq!(kept.name, forty);
        assert_eq!(
            domain(
                &mut store,
                &CreateWorkKind {
                    name: "é".repeat(41),
                }
            ),
            "Ce type dépasse 40 caractères."
        );
        applied(
            &mut store,
            &CreateWorkKind {
                name: "Site Web".into(),
            },
        );
        assert_eq!(
            domain(
                &mut store,
                &CreateWorkKind {
                    name: "site web".into(),
                }
            ),
            "Ce type existe déjà."
        );
        assert_eq!(
            domain(
                &mut store,
                &RenameWorkKind {
                    id: "absent".into(),
                    name: "site".into(),
                }
            ),
            "Ce type n'existe pas."
        );
        assert_eq!(
            domain(
                &mut store,
                &DeleteWorkKind {
                    id: "absent".into(),
                }
            ),
            "Ce type n'existe pas."
        );
        assert_eq!(
            domain(
                &mut store,
                &RenameWorkKind {
                    id: kept.id.clone(),
                    name: "SITE WEB".into(),
                }
            ),
            "Ce type existe déjà."
        );
        assert_eq!(
            work_kind_by_id(store.connection(), &kept.id)
                .unwrap()
                .unwrap()
                .name,
            forty
        );

        let before = work_kinds(store.connection()).unwrap().len();
        assert_eq!(
            domain(
                &mut store,
                &SetDossierWorkKinds {
                    client: quai,
                    names: vec!["backend".into(), "x".repeat(41)],
                }
            ),
            "Ce type dépasse 40 caractères."
        );
        assert_eq!(work_kinds(store.connection()).unwrap().len(), before);
        assert!(kind_names(&store, quai).is_empty());
        assert_eq!(
            domain(
                &mut store,
                &SetDossierWorkKinds {
                    client: ClientId::new(),
                    names: vec!["atelier".into()],
                }
            ),
            "Ce dossier n'existe pas."
        );
        assert!(
            !work_kinds(store.connection())
                .unwrap()
                .iter()
                .any(|kind| kind.name == "atelier")
        );

        let assigned = applied(
            &mut store,
            &SetDossierWorkKinds {
                client: quai,
                names: vec!["SITE WEB".into(), "site web".into(), "  API  ".into()],
            },
        );
        assert_eq!(
            assigned
                .iter()
                .map(|kind| kind.name.as_str())
                .collect::<Vec<_>>(),
            vec!["API", "Site Web"]
        );
    }

    #[test]
    fn an_agent_delete_waits_and_a_dry_run_writes_nothing() {
        let mut store = test_store("types-agent");
        let (quai, _) = conversation(&mut store, "Atelier de l'agent");
        let created = applied(
            &mut store,
            &SetDossierWorkKinds {
                client: quai,
                names: vec!["site web".into()],
            },
        );
        let id = created[0].id.clone();
        let command = DeleteWorkKind { id: id.clone() };

        let dry = Executor::new(&mut store)
            .execute(&command, &agent(true))
            .unwrap();
        assert_eq!(dry, Outcome::DryRun);
        assert_eq!(kind_names(&store, quai), vec!["site web".to_string()]);

        let pending = Executor::new(&mut store)
            .execute(&command, &agent(false))
            .unwrap();
        let Outcome::PendingConfirmation(pending_id) = pending else {
            panic!("confirmation attendue, obtenu {pending:?}");
        };
        assert_eq!(kind_names(&store, quai), vec!["site web".to_string()]);

        let confirmed = Executor::new(&mut store)
            .confirm::<DeleteWorkKind>(pending_id)
            .unwrap();
        assert_eq!(confirmed, Outcome::Applied(()));
        assert!(kind_names(&store, quai).is_empty());
        assert!(work_kinds(store.connection()).unwrap().is_empty());
    }
}
