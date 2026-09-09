//! `papers.list` / `papers.show` / `papers.add` / `papers.purge` / `papers.checklist` /
//! `papers.export`.

use freeflow_core::app::Executor;
use freeflow_core::clock::today_local;
use freeflow_core::domain::{PaperKind, PaperOrigin, parse_date};
use freeflow_core::expenses::hash_receipt;
use freeflow_core::papers::{
    NewPaper, PaperFilter, PurgePaper, archive_paper, list_papers, mime_from_name, paper_by_id,
    papers_checklist as load_papers_checklist,
};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::server::FreeflowServer;
use crate::support::{
    err_text, ok_json, ok_or_return, outcome_json, resolve_client, resolve_paper,
};

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct PapersListArgs {
    /// Année civile de clôture de l'exercice portant la pièce.
    period: Option<i32>,
    /// Nature (`statutes`, `kbis`, `issued_invoice`, …).
    kind: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct PapersShowArgs {
    /// UUID, préfixe, ou nom de fichier d'origine.
    paper: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct PapersAddArgs {
    /// Chemin local du fichier — lu par le serveur (IO d'adaptateur).
    path: Option<String>,
    /// Octets exacts, encodés en base64. Fournir `original_name` pour le nom au coffre.
    content_base64: Option<String>,
    /// Nom d'origine (requis avec `content_base64` ; sinon le nom du fichier).
    original_name: Option<String>,
    /// Nature : `statutes`, `kbis`, `tax_notice`, `filing_ack`, `client_contract`, …
    kind: String,
    period: Option<i32>,
    /// Client concerné (référence : nom, préfixe, UUID).
    client: Option<String>,
    note: Option<String>,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct PapersPurgeArgs {
    /// UUID, préfixe, ou nom de fichier d'origine.
    paper: String,
    /// Date du jour `AAAA-MM-JJ`. Défaut : aujourd'hui (heure locale).
    today: Option<String>,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct PapersChecklistArgs {
    /// Année civile de la clôture (ex. `2026`) — même désignation que `fiscal.year_show`.
    period: i32,
    /// Date du jour `AAAA-MM-JJ`. Défaut : aujourd'hui (heure locale).
    today: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct PapersExportArgs {
    /// Année civile de la clôture (ex. `2026`) — même désignation que `fiscal.year_show`.
    period: i32,
    /// Répertoire **neuf** à créer. Refusé s'il existe déjà (un agent n'écrase pas).
    out: String,
    /// Date du jour `AAAA-MM-JJ`. Défaut : aujourd'hui (heure locale).
    today: Option<String>,
}

#[tool_router(router = papers_router, vis = "pub(crate)")]
impl FreeflowServer {
    /// Liste les pièces du coffre (hors remplacées). Lecture seule.
    #[tool(
        name = "papers.list",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn papers_list(&self, Parameters(args): Parameters<PapersListArgs>) -> CallToolResult {
        let kind = match args.kind {
            Some(raw) => Some(ok_or_return!("kind", raw.parse::<PaperKind>())),
            None => None,
        };
        let store = self.store.lock().await;
        match list_papers(
            store.connection(),
            PaperFilter {
                period: args.period,
                kind,
                include_superseded: false,
            },
        ) {
            Ok(papers) => ok_json(papers),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Fiche d'une pièce (UUID, préfixe, ou nom d'origine).
    #[tool(
        name = "papers.show",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn papers_show(&self, Parameters(args): Parameters<PapersShowArgs>) -> CallToolResult {
        let store = self.store.lock().await;
        let id = ok_or_return!("paper", resolve_paper(&store, &args.paper));
        match paper_by_id(store.connection(), id) {
            Ok(Some(paper)) => ok_json(paper),
            Ok(None) => err_text(format!("pièce introuvable : {id}")),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Dépose un fichier au coffre (statuts, Kbis, avis…). Fournir `path` ou `content_base64`.
    /// Pas de confirmation : un dépôt n'est pas une destruction. `dry_run` n'écrit rien.
    #[tool(
        name = "papers.add",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true
        )
    )]
    async fn papers_add(&self, Parameters(args): Parameters<PapersAddArgs>) -> CallToolResult {
        let kind = ok_or_return!("kind", args.kind.parse::<PaperKind>());
        let (bytes, original_name): (Vec<u8>, String) = match (&args.path, &args.content_base64) {
            (Some(path), _) => {
                let bytes = ok_or_return!("path", std::fs::read(path));
                let name = args.original_name.clone().unwrap_or_else(|| {
                    std::path::Path::new(path)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "document".to_string())
                });
                (bytes, name)
            }
            (None, Some(b64)) => {
                use base64::Engine as _;
                let bytes = ok_or_return!(
                    "content_base64",
                    base64::engine::general_purpose::STANDARD.decode(b64.trim())
                );
                let name = args
                    .original_name
                    .clone()
                    .unwrap_or_else(|| "document".to_string());
                (bytes, name)
            }
            (None, None) => {
                return err_text("fournissez path ou content_base64");
            }
        };
        let hash = hash_receipt(&bytes);
        let mut store = self.store.lock().await;
        let client_id = match args.client.as_deref() {
            Some(r) => Some(ok_or_return!("client", resolve_client(&store, r))),
            None => None,
        };
        let spec = NewPaper {
            kind,
            origin: PaperOrigin::Uploaded,
            mime: mime_from_name(&original_name),
            original_name,
            period: args.period,
            issued_on: None,
            client_id,
            invoice_id: None,
            expense_id: None,
            fiscal_year_id: None,
            note: args.note,
            idempotency_key: Some(format!("papers:upload:{hash}:{kind}")),
        };
        match archive_paper(&mut store, spec, &bytes, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Purge une pièce dont le délai de conservation est échu. Un agent dépose une action en
    /// attente ; un humain confirme au terminal. Les statuts et le Kbis ne se purgent pas tant
    /// que la société existe.
    #[tool(
        name = "papers.purge",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false
        )
    )]
    async fn papers_purge(&self, Parameters(args): Parameters<PapersPurgeArgs>) -> CallToolResult {
        let today = match args.today {
            Some(s) => ok_or_return!("today", parse_date(&s)),
            None => today_local(),
        };
        let mut store = self.store.lock().await;
        let id = ok_or_return!("paper", resolve_paper(&store, &args.paper));
        match Executor::new(&mut store).execute(&PurgePaper { id, today }, &self.ctx(args.dry_run))
        {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Checklist de conservation d'un exercice : originaux que FreeFlow doit avoir figés
    /// (`required`), pièces extérieures en avertissement (statuts, Kbis, relevé, accusé).
    /// Lecture seule. `today` facultatif. Voir aussi la ressource `freeflow://papers/{period}`.
    #[tool(
        name = "papers.checklist",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn papers_checklist(
        &self,
        Parameters(args): Parameters<PapersChecklistArgs>,
    ) -> CallToolResult {
        let today = match args.today {
            Some(s) => ok_or_return!("today", parse_date(&s)),
            None => today_local(),
        };
        let store = self.store.lock().await;
        match load_papers_checklist(store.connection(), args.period, today) {
            Ok(checklist) => ok_json(checklist),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Écrit le dossier d'un contrôle en clair (inventaire + originaux déchiffrés). `out` est un
    /// répertoire **neuf** — refusé s'il existe. Ces fichiers ne sont plus chiffrés.
    #[tool(
        name = "papers.export",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn papers_export(
        &self,
        Parameters(args): Parameters<PapersExportArgs>,
    ) -> CallToolResult {
        let today = match args.today {
            Some(s) => ok_or_return!("today", parse_date(&s)),
            None => today_local(),
        };
        let store = self.store.lock().await;
        match freeflow_cli::write_control_pack(
            &store,
            args.period,
            std::path::Path::new(&args.out),
            today,
        ) {
            Ok(report) => ok_json(report),
            Err(e) => err_text(e.to_string()),
        }
    }
}
