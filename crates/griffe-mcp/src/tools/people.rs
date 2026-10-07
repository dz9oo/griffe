//! Outils `people.*` — liste, dossier, récit des travaux, types de travaux.
//! `today` est un argument d'adaptateur.

use griffe_core::app::Executor;
use griffe_core::clock::today_local;
use griffe_core::dossier_work::SaveDossierWork;
use griffe_core::people::{people_list, person};
use griffe_core::work_kinds::{
    CreateWorkKind, DeleteWorkKind, RenameWorkKind, SetDossierWorkKinds, dossiers_of_work_kind,
    work_kinds,
};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::server::FreeflowServer;
use crate::support::{err_text, ok_json, ok_or_return, outcome_json, resolve_client};

fn today_or(today: Option<String>) -> Result<time::Date, String> {
    match today {
        Some(s) => griffe_core::domain::parse_date(&s).map_err(|e| e.to_string()),
        None => Ok(today_local()),
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct TodayArgs {
    /// Date `AAAA-MM-JJ`. Défaut : aujourd'hui (heure locale).
    today: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ShowArgs {
    /// Nom, préfixe d'UUID, UUID (fiche, opportunité, mission, devis, facture) ou nom chez qui ça sort.
    reference: String,
    /// Date `AAAA-MM-JJ`. Défaut : aujourd'hui (heure locale).
    today: Option<String>,
}

#[tool_router(router = people_router, vis = "pub(crate)")]
impl FreeflowServer {
    /// Les affaires : trois chapitres (en conversation, en mission, chez qui ça sort). Faits typés,
    /// sans phrase française.
    #[tool(
        name = "people.list",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn people_list_tool(&self, Parameters(args): Parameters<TodayArgs>) -> CallToolResult {
        let today = match today_or(args.today) {
            Ok(d) => d,
            Err(e) => return err_text(e),
        };
        let store = self.store.lock().await;
        match people_list(store.connection(), today) {
            Ok(list) => ok_json(list),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Dossier d'une personne : en cours, projet, papiers, histoire. `reference` comme
    /// `freeflow people show`.
    #[tool(
        name = "people.show",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn people_show_tool(&self, Parameters(args): Parameters<ShowArgs>) -> CallToolResult {
        let today = match today_or(args.today) {
            Ok(d) => d,
            Err(e) => return err_text(e),
        };
        let store = self.store.lock().await;
        match person(store.connection(), &args.reference, today) {
            Ok(dossier) => ok_json(dossier),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Enregistre le récit markdown d'une fiche. `body` est le texte complet.
    /// `revision` est celle lue : une autre révision est refusée.
    #[tool(
        name = "people.save_work",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn people_save_work(&self, Parameters(args): Parameters<SaveWorkArgs>) -> CallToolResult {
        let mut store = self.store.lock().await;
        let id = ok_or_return!("client", resolve_client(&store, &args.reference));
        let cmd = SaveDossierWork {
            client: id,
            body: args.body,
            revision: args.revision,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Le catalogue des types de travaux.
    #[tool(
        name = "people.work_kinds",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn people_work_kinds(&self) -> CallToolResult {
        let store = self.store.lock().await;
        match work_kinds(store.connection()) {
            Ok(kinds) => ok_json(kinds),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Les dossiers qui portent ce type.
    #[tool(
        name = "people.work_kind_dossiers",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn people_work_kind_dossiers(
        &self,
        Parameters(args): Parameters<KindIdArgs>,
    ) -> CallToolResult {
        let store = self.store.lock().await;
        match dossiers_of_work_kind(store.connection(), &args.id) {
            Ok(rows) => ok_json(rows),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Crée un type. Un nom vide, trop long, ou déjà présent est refusé.
    #[tool(
        name = "people.create_work_kind",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn people_create_work_kind(
        &self,
        Parameters(args): Parameters<KindNameArgs>,
    ) -> CallToolResult {
        let mut store = self.store.lock().await;
        let cmd = CreateWorkKind { name: args.name };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Renomme un type. Les dossiers gardent le lien.
    #[tool(
        name = "people.rename_work_kind",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn people_rename_work_kind(
        &self,
        Parameters(args): Parameters<RenameKindArgs>,
    ) -> CallToolResult {
        let mut store = self.store.lock().await;
        let cmd = RenameWorkKind {
            id: args.id,
            name: args.name,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Retire le type et tous ses liens. Un agent dépose une confirmation.
    #[tool(
        name = "people.delete_work_kind",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn people_delete_work_kind(
        &self,
        Parameters(args): Parameters<DeleteKindArgs>,
    ) -> CallToolResult {
        let mut store = self.store.lock().await;
        let cmd = DeleteWorkKind { id: args.id };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Remplace les types d'une fiche. Les noms sont séparés par des virgules.
    /// Un nom inconnu est créé. Une chaîne vide retire les liens.
    #[tool(
        name = "people.set_work_kinds",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn people_set_work_kinds(
        &self,
        Parameters(args): Parameters<SetKindsArgs>,
    ) -> CallToolResult {
        let mut store = self.store.lock().await;
        let id = ok_or_return!("client", resolve_client(&store, &args.reference));
        let cmd = SetDossierWorkKinds {
            client: id,
            names: split_kind_names(&args.names),
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct SaveWorkArgs {
    /// Nom, préfixe d'UUID ou UUID de la fiche.
    reference: String,
    /// Markdown complet du récit.
    body: String,
    /// Révision lue. Une révision différente est refusée.
    revision: i64,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct KindNameArgs {
    /// Nom du type.
    name: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct KindIdArgs {
    /// Identifiant du type.
    id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct RenameKindArgs {
    /// Identifiant du type.
    id: String,
    /// Nouveau nom.
    name: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct DeleteKindArgs {
    /// Identifiant du type.
    id: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct SetKindsArgs {
    /// Nom, préfixe d'UUID ou UUID de la fiche.
    reference: String,
    /// Noms séparés par des virgules. Vide : le dossier ne porte plus de type.
    names: String,
    #[serde(default)]
    dry_run: bool,
}

fn split_kind_names(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(ToString::to_string)
        .collect()
}
