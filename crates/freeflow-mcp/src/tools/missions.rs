//! Outils `mission.*` — miroir de `freeflow mission ...` (CLI, lot 7).

use freeflow_core::app::Executor;
use freeflow_core::domain::{
    ClientId, Milestone, MissionId, MissionKind, Money, QuoteId, TimeCategory,
};
use freeflow_core::missions::{self, effective_daily_rate, monthly_capacity};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::server::FreeflowServer;
use crate::support::{err_text, ok_json, ok_or_return, outcome_json};

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct CreateMissionArgs {
    client_id: String,
    /// Devis d'origine, si la mission découle d'un devis accepté hors du flux `quote.accept`.
    quote_id: Option<String>,
    name: String,
    /// `regie`, `forfait`, ou `recurrent`.
    kind: String,
    /// TJM en centimes — obligatoire pour `kind = "regie"`.
    daily_rate_cents: Option<i64>,
    /// Budget total en centimes — obligatoire pour `kind = "forfait"`.
    budget_cents: Option<i64>,
    /// Montant mensuel en centimes — obligatoire pour `kind = "recurrent"`.
    monthly_amount_cents: Option<i64>,
    started_on: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct LogTimeArgs {
    mission_id: String,
    worked_on: String,
    days: f64,
    /// `billable`, `pre_sales`, `admin`, ou `training`.
    category: String,
    note: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct MissionIdArgs {
    id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct CapacityArgs {
    /// Année et mois, ex. `2026-09`.
    month: String,
}

fn build_mission_kind(args: &CreateMissionArgs) -> Result<MissionKind, String> {
    match args.kind.as_str() {
        "regie" => {
            let cents = args
                .daily_rate_cents
                .ok_or("daily_rate_cents est requis pour kind = \"regie\"")?;
            Ok(MissionKind::Regie {
                daily_rate: Money::from_cents(cents),
            })
        }
        "forfait" => {
            let cents = args
                .budget_cents
                .ok_or("budget_cents est requis pour kind = \"forfait\"")?;
            Ok(MissionKind::Forfait {
                budget: Money::from_cents(cents),
            })
        }
        "recurrent" => {
            let cents = args
                .monthly_amount_cents
                .ok_or("monthly_amount_cents est requis pour kind = \"recurrent\"")?;
            Ok(MissionKind::Recurrent {
                monthly_amount: Money::from_cents(cents),
            })
        }
        other => Err(format!(
            "kind invalide : {other} (attendu regie, forfait, ou recurrent)"
        )),
    }
}

fn parse_month(s: &str) -> Result<freeflow_core::domain::Month, String> {
    let (year_str, month_str) = s
        .split_once('-')
        .ok_or_else(|| format!("mois invalide : {s} (attendu AAAA-MM)"))?;
    let year: i32 = year_str
        .parse()
        .map_err(|_| format!("mois invalide : {s}"))?;
    let month: u8 = month_str
        .parse()
        .map_err(|_| format!("mois invalide : {s}"))?;
    freeflow_core::domain::Month::new(year, month).map_err(|e| e.to_string())
}

#[tool_router(router = missions_router, vis = "pub(crate)")]
impl FreeflowServer {
    /// Crée une mission directement (sans passer par le gain d'une opportunité).
    #[tool(
        name = "mission.create",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn mission_create(
        &self,
        Parameters(args): Parameters<CreateMissionArgs>,
    ) -> CallToolResult {
        let client_id: ClientId = ok_or_return!("client_id", args.client_id.parse());
        let quote_id: Option<QuoteId> = match &args.quote_id {
            Some(s) => Some(ok_or_return!("quote_id", s.parse())),
            None => None,
        };
        let started_on = ok_or_return!(
            "started_on",
            freeflow_core::domain::parse_date(&args.started_on)
        );
        let kind = ok_or_return!("kind", build_mission_kind(&args));
        let cmd = missions::CreateMission {
            client_id,
            quote_id,
            name: args.name,
            kind,
            milestones: Vec::<Milestone>::new(),
            started_on,
        };
        let mut store = self.store.lock().await;
        match Executor::new(&mut store).execute(&cmd, &self.ctx()) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Saisit du temps sur une mission.
    #[tool(
        name = "mission.log_time",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn mission_log_time(&self, Parameters(args): Parameters<LogTimeArgs>) -> CallToolResult {
        let mission_id: MissionId = ok_or_return!("mission_id", args.mission_id.parse());
        let worked_on = ok_or_return!(
            "worked_on",
            freeflow_core::domain::parse_date(&args.worked_on)
        );
        let category: TimeCategory = ok_or_return!("category", args.category.parse());
        let cmd = missions::LogTime {
            mission_id,
            worked_on,
            days: args.days,
            category,
            note: args.note,
        };
        let mut store = self.store.lock().await;
        match Executor::new(&mut store).execute(&cmd, &self.ctx()) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// TJM effectif d'une mission (CA facturé ÷ jours facturables consommés).
    #[tool(
        name = "mission.rate",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn mission_rate(&self, Parameters(args): Parameters<MissionIdArgs>) -> CallToolResult {
        let id: MissionId = ok_or_return!("id", args.id.parse());
        let store = self.store.lock().await;
        match effective_daily_rate(store.connection(), id) {
            Ok(rate) => ok_json(json!({"effective_daily_rate_cents": rate.map(Money::cents)})),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Capacité vendue pour un mois donné, tous clients confondus.
    #[tool(
        name = "mission.capacity",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn mission_capacity(&self, Parameters(args): Parameters<CapacityArgs>) -> CallToolResult {
        let month = ok_or_return!("month", parse_month(&args.month));
        let store = self.store.lock().await;
        match monthly_capacity(store.connection(), month) {
            Ok(capacity) => ok_json(json!({
                "month": month.to_string(),
                "available_business_days": capacity.available_business_days,
                "billable_days": capacity.billable_days,
                "utilization_percent": capacity.utilization_percent(),
            })),
            Err(e) => err_text(e.to_string()),
        }
    }
}
