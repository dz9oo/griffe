//! Outils `prospect.*` — miroir de `freeflow prospect ...` (CLI, lot 7).

use freeflow_core::app::Executor;
use freeflow_core::domain::{
    ClientId, InteractionKind, OpportunityId, OpportunityStage, Probability,
};
use freeflow_core::prospection::{
    self, late_actions, pipeline_by_stage, weighted_pipeline, without_next_action,
};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::server::FreeflowServer;
use crate::support::{err_text, ok_json, ok_or_return, outcome_json, parse_loss_reason};

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct CreateOpportunityArgs {
    /// Identifiant du client (UUID).
    client_id: String,
    name: String,
    /// Montant estimé, en centimes d'euro.
    amount_cents: i64,
    /// Probabilité de gain, en pourcentage entier (0-100).
    probability_percent: u8,
    /// Date de prochaine action, au format `AAAA-MM-JJ`.
    next_action: String,
    source: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct AdvanceOpportunityArgs {
    id: String,
    /// Étape cible : `qualification`, `discovery`, `proposal`, `negotiation`, `won`, ou `lost`.
    to: String,
    next_action: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct WinOpportunityArgs {
    id: String,
    /// Date de début de la mission créée, au format `AAAA-MM-JJ`.
    started_on: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct LoseOpportunityArgs {
    id: String,
    /// `budget`, `timing`, `competitor`, `no-response`, `scope-mismatch`, ou `other:<détail>`.
    reason: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct LogInteractionArgs {
    id: String,
    /// `call`, `email`, `meeting`, ou `note`.
    kind: String,
    note: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct LateArgs {
    /// Date du jour, au format `AAAA-MM-JJ` — sert de référence pour détecter le retard.
    today: String,
}

#[tool_router(router = prospection_router, vis = "pub(crate)")]
impl FreeflowServer {
    /// Crée une nouvelle opportunité, en étape Qualification.
    #[tool(
        name = "prospect.create",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn prospect_create(
        &self,
        Parameters(args): Parameters<CreateOpportunityArgs>,
    ) -> CallToolResult {
        let client_id: ClientId = ok_or_return!("client_id", args.client_id.parse());
        let probability = ok_or_return!(
            "probability_percent",
            Probability::new(args.probability_percent)
        );
        let next_action_at = ok_or_return!(
            "next_action",
            freeflow_core::domain::parse_date(&args.next_action)
        );
        let cmd = prospection::CreateOpportunity {
            client_id,
            name: args.name,
            amount: freeflow_core::domain::Money::from_cents(args.amount_cents),
            probability,
            next_action_at,
            source: args.source,
        };
        let mut store = self.store.lock().await;
        match Executor::new(&mut store).execute(&cmd, &self.ctx(false)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Fait avancer une opportunité vers une autre étape ouverte.
    #[tool(
        name = "prospect.advance",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn prospect_advance(
        &self,
        Parameters(args): Parameters<AdvanceOpportunityArgs>,
    ) -> CallToolResult {
        let opportunity_id: OpportunityId = ok_or_return!("id", args.id.parse());
        let to: OpportunityStage = ok_or_return!("to", args.to.parse());
        let next_action_at = ok_or_return!(
            "next_action",
            freeflow_core::domain::parse_date(&args.next_action)
        );
        let cmd = prospection::AdvanceOpportunity {
            opportunity_id,
            to,
            next_action_at,
        };
        let mut store = self.store.lock().await;
        match Executor::new(&mut store).execute(&cmd, &self.ctx(false)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Gagne une opportunité : crée la mission forfait correspondante.
    #[tool(
        name = "prospect.win",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn prospect_win(
        &self,
        Parameters(args): Parameters<WinOpportunityArgs>,
    ) -> CallToolResult {
        let opportunity_id: OpportunityId = ok_or_return!("id", args.id.parse());
        let started_on = ok_or_return!(
            "started_on",
            freeflow_core::domain::parse_date(&args.started_on)
        );
        let cmd = prospection::WinOpportunity {
            opportunity_id,
            started_on,
        };
        let mut store = self.store.lock().await;
        match Executor::new(&mut store).execute(&cmd, &self.ctx(false)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Perd une opportunité, avec un motif structuré.
    #[tool(
        name = "prospect.lose",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn prospect_lose(
        &self,
        Parameters(args): Parameters<LoseOpportunityArgs>,
    ) -> CallToolResult {
        let opportunity_id: OpportunityId = ok_or_return!("id", args.id.parse());
        let reason = ok_or_return!("reason", parse_loss_reason(&args.reason));
        let cmd = prospection::LoseOpportunity {
            opportunity_id,
            reason,
        };
        let mut store = self.store.lock().await;
        match Executor::new(&mut store).execute(&cmd, &self.ctx(false)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Journalise une interaction (appel, email, réunion, note) sur une opportunité.
    #[tool(
        name = "prospect.log_interaction",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn prospect_log_interaction(
        &self,
        Parameters(args): Parameters<LogInteractionArgs>,
    ) -> CallToolResult {
        let opportunity_id: OpportunityId = ok_or_return!("id", args.id.parse());
        let kind: InteractionKind = ok_or_return!("kind", args.kind.parse());
        let cmd = prospection::LogInteraction {
            opportunity_id,
            kind,
            note: args.note,
        };
        let mut store = self.store.lock().await;
        match Executor::new(&mut store).execute(&cmd, &self.ctx(false)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Opportunités ouvertes dont la prochaine action est en retard par rapport à `today`.
    #[tool(
        name = "prospect.late",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn prospect_late(&self, Parameters(args): Parameters<LateArgs>) -> CallToolResult {
        let today = ok_or_return!("today", freeflow_core::domain::parse_date(&args.today));
        let store = self.store.lock().await;
        match late_actions(store.connection(), today) {
            Ok(opportunities) => ok_json(opportunities),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Opportunités ouvertes sans prochaine action (filet de sécurité).
    #[tool(
        name = "prospect.orphans",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn prospect_orphans(&self) -> CallToolResult {
        let store = self.store.lock().await;
        match without_next_action(store.connection()) {
            Ok(opportunities) => ok_json(opportunities),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Pipeline pondéré (montant × probabilité) et répartition par étape.
    #[tool(
        name = "prospect.pipeline",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn prospect_pipeline(&self) -> CallToolResult {
        let store = self.store.lock().await;
        let weighted = ok_or_return!("pipeline", weighted_pipeline(store.connection()));
        let by_stage = ok_or_return!("pipeline", pipeline_by_stage(store.connection()));
        ok_json(json!({
            "weighted_total_cents": weighted.cents(),
            "by_stage": by_stage.iter().map(|s| json!({
                "stage": s.stage.as_str(),
                "count": s.count,
                "weighted_amount_cents": s.weighted_amount.cents(),
            })).collect::<Vec<_>>(),
        }))
    }
}
