//! Outils `society.*` — paysage, se payer, relevé, chapitres. Lectures pures, `today` d'adaptateur.

use freeflow_core::app::Executor;
use freeflow_core::clock::today_local;
use freeflow_core::fiscal::FiscalDeadlineKind;
use freeflow_core::society::{
    MarkDutyFiled, RetractDutyFiled, closing_story, duty_briefing, pay_yourself, society_duties,
    society_home, society_identity, statement_moves,
};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::server::FreeflowServer;
use crate::support::{err_text, ok_json, outcome_json};

fn today_or(today: Option<String>) -> Result<time::Date, String> {
    match today {
        Some(s) => freeflow_core::domain::parse_date(&s).map_err(|e| e.to_string()),
        None => Ok(today_local()),
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct TodayArgs {
    /// Date `AAAA-MM-JJ`. Défaut : aujourd'hui (heure locale).
    today: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct DutyArgs {
    /// Nature (`is_acompte`, `ca3`, `cfe`, `is_solde`, `liasse`, …).
    kind: String,
    /// Période (`AAAA-MM` ou `AAAA-MM-JJ`). Défaut : la prochaine non déposée.
    period: Option<String>,
    /// Date `AAAA-MM-JJ`. Défaut : aujourd'hui (heure locale).
    today: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct DutyFiledArgs {
    /// Nature (`is_acompte`, `ca3`, …).
    kind: String,
    /// Période (`AAAA-MM` ou `AAAA-MM-JJ`). Défaut : l'occurrence courante.
    period: Option<String>,
    /// Date `AAAA-MM-JJ`. Défaut : aujourd'hui (heure locale).
    today: Option<String>,
    /// `true` : montre ce qui serait fait, n'écrit rien.
    #[serde(default)]
    dry_run: bool,
}

#[tool_router(router = society_router, vis = "pub(crate)")]
impl FreeflowServer {
    /// La société : identité courte, paysage, conversations de l'année, chapitres.
    #[tool(
        name = "society.show",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn society_show_tool(&self, Parameters(args): Parameters<TodayArgs>) -> CallToolResult {
        let today = match today_or(args.today) {
            Ok(d) => d,
            Err(e) => return err_text(e),
        };
        let store = self.store.lock().await;
        match society_home(store.connection(), today) {
            Ok(home) => ok_json(home),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Se payer : montant possible ce mois-ci sans casser la piste, portes salaire / dividende.
    #[tool(
        name = "society.pay",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn society_pay_tool(&self, Parameters(args): Parameters<TodayArgs>) -> CallToolResult {
        let today = match today_or(args.today) {
            Ok(d) => d,
            Err(e) => return err_text(e),
        };
        let store = self.store.lock().await;
        match pay_yourself(store.connection(), today) {
            Ok(pay) => ok_json(pay),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Lettre d'une démarche hors de l'app : montant, chemin sur le site, ce que l'écran demandera.
    #[tool(
        name = "society.duty",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn society_duty_tool(&self, Parameters(args): Parameters<DutyArgs>) -> CallToolResult {
        let today = match today_or(args.today) {
            Ok(d) => d,
            Err(e) => return err_text(e),
        };
        let Some(kind) = FiscalDeadlineKind::parse(&args.kind) else {
            return err_text(format!("démarche inconnue : {}", args.kind));
        };
        let store = self.store.lock().await;
        match duty_briefing(store.connection(), kind, today, args.period.as_deref()) {
            Ok(briefing) => ok_json(briefing),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Ce que tu dois à l'État : dates, montants, où déposer. Faits typés, sans sigle.
    #[tool(
        name = "society.duties",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn society_duties_tool(&self, Parameters(args): Parameters<TodayArgs>) -> CallToolResult {
        let today = match today_or(args.today) {
            Ok(d) => d,
            Err(e) => return err_text(e),
        };
        let store = self.store.lock().await;
        match society_duties(store.connection(), today) {
            Ok(duties) => ok_json(duties),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Clore l'exercice, en quelques phrases (pas seize étapes techniques).
    #[tool(
        name = "society.closing",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn society_closing_tool(
        &self,
        Parameters(args): Parameters<TodayArgs>,
    ) -> CallToolResult {
        let today = match today_or(args.today) {
            Ok(d) => d,
            Err(e) => return err_text(e),
        };
        let store = self.store.lock().await;
        match closing_story(store.connection(), today) {
            Ok(story) => ok_json(story),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Le relevé : chaque mouvement sans lecture, avec une lecture suggérée (dépense, dette, te payer).
    #[tool(
        name = "society.statement",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn society_statement_tool(
        &self,
        Parameters(args): Parameters<TodayArgs>,
    ) -> CallToolResult {
        let today = match today_or(args.today) {
            Ok(d) => d,
            Err(e) => return err_text(e),
        };
        let store = self.store.lock().await;
        match statement_moves(store.connection(), today) {
            Ok(moves) => ok_json(moves),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// La carte d'identité de la société.
    #[tool(
        name = "society.identity",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn society_identity_tool(&self) -> CallToolResult {
        let store = self.store.lock().await;
        match society_identity(store.connection()) {
            Ok(card) => ok_json(card),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Marquer une démarche d'État comme déposée. Fait humain, pas une preuve DGFiP.
    #[tool(
        name = "society.mark_duty_filed",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true
        )
    )]
    async fn society_mark_duty_filed(
        &self,
        Parameters(args): Parameters<DutyFiledArgs>,
    ) -> CallToolResult {
        let today = match today_or(args.today) {
            Ok(d) => d,
            Err(e) => return err_text(e),
        };
        let Some(kind) = FiscalDeadlineKind::parse(&args.kind) else {
            return err_text(format!("démarche inconnue : {}", args.kind));
        };
        let mut store = self.store.lock().await;
        let briefing = match duty_briefing(store.connection(), kind, today, args.period.as_deref())
        {
            Ok(b) => b,
            Err(e) => return err_text(e.to_string()),
        };
        let cmd = MarkDutyFiled {
            kind,
            period_key: briefing.period_key,
            due_on: briefing.due_on,
            filed_on: today,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Retirer le marquage « déposé » d'une démarche.
    #[tool(
        name = "society.retract_duty_filed",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn society_retract_duty_filed(
        &self,
        Parameters(args): Parameters<DutyFiledArgs>,
    ) -> CallToolResult {
        let today = match today_or(args.today) {
            Ok(d) => d,
            Err(e) => return err_text(e),
        };
        let Some(kind) = FiscalDeadlineKind::parse(&args.kind) else {
            return err_text(format!("démarche inconnue : {}", args.kind));
        };
        let mut store = self.store.lock().await;
        let briefing = match duty_briefing(store.connection(), kind, today, args.period.as_deref())
        {
            Ok(b) => b,
            Err(e) => return err_text(e.to_string()),
        };
        let cmd = RetractDutyFiled {
            kind,
            period_key: briefing.period_key,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }
}
