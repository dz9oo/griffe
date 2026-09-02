//! Outil `fec.export` — miroir de `freeflow fec export` (CLI), lot 28. Le cœur construit et
//! rend le Fichier des Écritures Comptables ; l'écriture sur disque est de l'IO d'adaptateur et,
//! comme `invoice.render`/`fiscal.render_year`, refuse d'écraser un fichier existant.

use std::path::Path;

use freeflow_core::fec::build_fec;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::server::FreeflowServer;
use crate::support::{err_text, ok_json};
use crate::tools::fiscal::write_new_document;

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct FecExportArgs {
    /// Année civile de la clôture (ex. `2026`) — même désignation que `fiscal.year_show` ;
    /// l'exercice n'a pas besoin d'être clos.
    period: i32,
    /// Fichier à écrire (refusé s'il existe déjà), ou répertoire existant dans lequel écrire le
    /// fichier sous son nom réglementaire `<SIREN>FEC<AAAAMMJJ>.txt`.
    out: String,
}

#[tool_router(router = fec_router, vis = "pub(crate)")]
impl FreeflowServer {
    /// Écrit le Fichier des Écritures Comptables (art. A. 47 A-1 LPF) de l'exercice, dérivé des
    /// factures, avoirs, encaissements et dépenses (journaux VE/AC/BQ, plan de comptes PCG
    /// minimal, écritures équilibrées) — un export pour l'expert-comptable, pas une comptabilité
    /// définitive.
    #[tool(
        name = "fec.export",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn fec_export(&self, Parameters(args): Parameters<FecExportArgs>) -> CallToolResult {
        let store = self.store.lock().await;
        let fec = match build_fec(store.connection(), args.period) {
            Ok(fec) => fec,
            Err(e) => return err_text(e.to_string()),
        };
        let out = Path::new(&args.out);
        let path = if out.is_dir() {
            out.join(fec.file_name())
        } else {
            out.to_path_buf()
        };
        let Some(path) = path.to_str() else {
            return err_text("chemin de sortie non UTF-8");
        };
        match write_new_document(path, fec.render().as_bytes()) {
            Ok(msg) => ok_json(json!({ "written": msg, "path": path, "summary": fec.summary() })),
            Err(e) => err_text(e),
        }
    }
}
