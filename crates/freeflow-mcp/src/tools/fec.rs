//! Outils `fec.export` / `fec.check` — miroir de `freeflow fec export` / `fec check`.
//!
//! Le cœur construit et rend le Fichier des Écritures Comptables ; l'écriture sur disque est de
//! l'IO d'adaptateur et, comme `invoice.render`/`fiscal.render_year`, refuse d'écraser un
//! fichier existant. `fec.check` est une requête : un fichier (`path` / `content` /
//! `content_base64`) ou l'exercice (`period`).

use std::path::Path;

use freeflow_core::fec::{build_fec, check_fec, check_fec_of};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::server::FreeflowServer;
use crate::support::{err_text, ok_json, ok_or_return};
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

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct FecCheckArgs {
    /// Contrôle le FEC de l'exercice clos dans cette année civile, sans l'écrire.
    period: Option<i32>,
    /// Contenu du fichier en texte (UTF-8). Pour un latin-1, préférez `content_base64` ou `path`.
    content: Option<String>,
    /// Octets exacts du fichier, encodés en base64.
    content_base64: Option<String>,
    /// Chemin local du FEC — lu par le serveur (IO d'adaptateur).
    path: Option<String>,
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

    /// Contrôle la structure d'un FEC (art. A. 47 A-1 LPF) : 18 colonnes, nom réglementaire,
    /// dates, montants à la virgule, équilibre des écritures. Fournir `period` (FEC dérivé du
    /// coffre) **ou** `content` / `content_base64` / `path`. Ce n'est pas une attestation DGFiP :
    /// la conformité structurelle ne présage pas de la régularité de la comptabilité.
    #[tool(
        name = "fec.check",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true
        )
    )]
    async fn fec_check(&self, Parameters(args): Parameters<FecCheckArgs>) -> CallToolResult {
        if let Some(period) = args.period {
            let store = self.store.lock().await;
            return match check_fec_of(store.connection(), period) {
                Ok(check) => ok_json(check),
                Err(e) => err_text(e.to_string()),
            };
        }
        let (bytes, name): (Vec<u8>, Option<String>) =
            match (&args.content, &args.content_base64, &args.path) {
                (Some(text), _, _) => (text.clone().into_bytes(), None),
                (None, Some(b64), _) => {
                    use base64::Engine as _;
                    (
                        ok_or_return!(
                            "content_base64",
                            base64::engine::general_purpose::STANDARD.decode(b64.trim())
                        ),
                        None,
                    )
                }
                (None, None, Some(path)) => (
                    ok_or_return!("path", std::fs::read(path)),
                    Path::new(path)
                        .file_name()
                        .and_then(|s| s.to_str())
                        .map(ToOwned::to_owned),
                ),
                (None, None, None) => {
                    return err_text("fournissez period, content, content_base64 ou path");
                }
            };
        ok_json(check_fec(&bytes, name.as_deref()))
    }
}
