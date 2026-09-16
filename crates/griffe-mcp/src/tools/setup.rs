//! `setup.status` (lot 39) : où en est la configuration du coffre — la même vue que
//! `freeflow setup status` et le bandeau de la fenêtre, calculée par `griffe_core::setup`.

use griffe_core::app::Executor;
use griffe_core::setup::{DeclareNewCompany, setup_json, setup_status};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::server::FreeflowServer;
use crate::support::{err_text, ok_json, outcome_json};

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct DeclareNewCompanyArgs {
    /// `true` : la société vient d'être créée, pas de bilan de cabinet à reprendre ; `false`
    /// annule ce choix.
    declared: bool,
    #[serde(default)]
    dry_run: bool,
}

#[tool_router(router = setup_router, vis = "pub(crate)")]
impl FreeflowServer {
    /// Premier lancement : pour chaque prérequis — profil de la société (avec les champs
    /// manquants nommés), point de départ (bilan d'ouverture repris ou société déclarée
    /// nouvelle), relevé bancaire importé — son état, et le prochain geste (`next_step` :
    /// profile, origin, bank, done) avec sa phrase. Lecture seule ; les gestes passent par
    /// company.set_profile, fiscal.set_opening_balance ou setup.declare_new_company, bank.import.
    #[tool(
        name = "setup.status",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn setup_status(&self) -> CallToolResult {
        let store = self.store.lock().await;
        match setup_status(store.connection()) {
            Ok(status) => ok_json(setup_json(&status)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Déclare que la société vient d'être créée (aucun bilan d'ouverture à reprendre) — ou
    /// annule ce choix.
    #[tool(
        name = "setup.declare_new_company",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true
        )
    )]
    async fn setup_declare_new_company(
        &self,
        Parameters(args): Parameters<DeclareNewCompanyArgs>,
    ) -> CallToolResult {
        let cmd = DeclareNewCompany {
            declared: args.declared,
        };
        let mut store = self.store.lock().await;
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }
}
