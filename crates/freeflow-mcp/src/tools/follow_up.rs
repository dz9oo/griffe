//! Relances : file, brouillon, attestation d'envoi. Jamais d'envoi SMTP.

use freeflow_core::app::Executor;
use freeflow_core::clock::today_local;
use freeflow_core::domain::{FollowUpSubject, SnoozePreset, parse_date, snooze_date};
use freeflow_core::follow_up::{
    MarkFollowUpSent, PrepareFollowUp, RetractLastFollowUp, SetFollowUpDate, SetFollowUpSender,
    SkipFollowUpStep, SnoozeFollowUp, card_for, follow_up_board, follow_up_queue,
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
        Some(s) => parse_date(&s).map_err(|e| e.to_string()),
        None => Ok(today_local()),
    }
}

fn resolve_subject(
    store: &freeflow_core::store::Store,
    reference: &str,
) -> Result<FollowUpSubject, String> {
    use freeflow_core::reference::{self, RefMatch};
    match reference::resolve_follow_up_subject(store.connection(), reference) {
        Ok(RefMatch::Unique(s)) => Ok(s),
        Ok(RefMatch::NotFound) => Err(format!("aucune relance ne correspond à « {reference} »")),
        Ok(RefMatch::Ambiguous(c)) => {
            let list = c
                .iter()
                .map(|(id, label)| format!("  {id} — {label}"))
                .collect::<Vec<_>>()
                .join("\n");
            Err(format!(
                "« {reference} » désigne plusieurs relances :\n{list}"
            ))
        }
        Err(e) => Err(e.to_string()),
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct TodayArgs {
    /// Date `AAAA-MM-JJ`. Défaut : aujourd'hui (heure locale).
    today: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct RefArgs {
    /// Opportunité ou facture (UUID, préfixe, nom ou numéro).
    reference: String,
    today: Option<String>,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct LetterArgs {
    /// Opportunité ou facture (UUID, préfixe, nom ou numéro).
    reference: String,
    today: Option<String>,
    /// Sujet. Absent : modèle de cadence (prepare) ou dernier brouillon (sent).
    subject_line: Option<String>,
    /// Corps. Absent : modèle de cadence (prepare) ou dernier brouillon (sent).
    body: Option<String>,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct SenderArgs {
    email: String,
    name: Option<String>,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct SnoozeArgs {
    reference: String,
    /// `AAAA-MM-JJ`. Ignoré si `preset` est fourni (`tomorrow`, `next_week`, `monday`).
    until: Option<String>,
    preset: Option<String>,
    today: Option<String>,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ScheduleArgs {
    reference: String,
    on: String,
    today: Option<String>,
    #[serde(default)]
    dry_run: bool,
}

#[tool_router(router = follow_up_router, vis = "pub(crate)")]
impl FreeflowServer {
    #[tool(
        name = "follow_up.queue",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn follow_up_queue_tool(
        &self,
        Parameters(args): Parameters<TodayArgs>,
    ) -> CallToolResult {
        let today = match today_or(args.today) {
            Ok(d) => d,
            Err(e) => return err_text(e),
        };
        let store = self.store.lock().await;
        match follow_up_queue(store.connection(), today) {
            Ok(cards) => ok_json(cards),
            Err(e) => err_text(e.to_string()),
        }
    }

    #[tool(
        name = "follow_up.board",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn follow_up_board_tool(
        &self,
        Parameters(args): Parameters<TodayArgs>,
    ) -> CallToolResult {
        let today = match today_or(args.today) {
            Ok(d) => d,
            Err(e) => return err_text(e),
        };
        let store = self.store.lock().await;
        match follow_up_board(store.connection(), today) {
            Ok(cards) => ok_json(cards),
            Err(e) => err_text(e.to_string()),
        }
    }

    #[tool(
        name = "follow_up.show",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn follow_up_show(&self, Parameters(args): Parameters<RefArgs>) -> CallToolResult {
        let today = match today_or(args.today) {
            Ok(d) => d,
            Err(e) => return err_text(e),
        };
        let store = self.store.lock().await;
        let subject = match resolve_subject(&store, &args.reference) {
            Ok(s) => s,
            Err(e) => return err_text(e),
        };
        match card_for(store.connection(), subject, today) {
            Ok(card) => ok_json(card),
            Err(e) => err_text(e.to_string()),
        }
    }

    #[tool(
        name = "follow_up.set_sender",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true
        )
    )]
    async fn follow_up_set_sender(
        &self,
        Parameters(args): Parameters<SenderArgs>,
    ) -> CallToolResult {
        let cmd = SetFollowUpSender {
            email: args.email,
            name: args.name,
        };
        let mut store = self.store.lock().await;
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Prépare un brouillon : renvoie le RFC 5322. N'ouvre pas le client mail (geste humain).
    #[tool(
        name = "follow_up.prepare",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn follow_up_prepare(&self, Parameters(args): Parameters<LetterArgs>) -> CallToolResult {
        let today = match today_or(args.today) {
            Ok(d) => d,
            Err(e) => return err_text(e),
        };
        let mut store = self.store.lock().await;
        let subject = match resolve_subject(&store, &args.reference) {
            Ok(s) => s,
            Err(e) => return err_text(e),
        };
        let cmd = PrepareFollowUp {
            subject,
            today,
            subject_line: args.subject_line,
            body: args.body,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Atteste que le mail est parti. Un agent dépose une action en attente.
    #[tool(
        name = "follow_up.mark_sent",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn follow_up_mark_sent(
        &self,
        Parameters(args): Parameters<LetterArgs>,
    ) -> CallToolResult {
        let today = match today_or(args.today) {
            Ok(d) => d,
            Err(e) => return err_text(e),
        };
        let mut store = self.store.lock().await;
        let subject = match resolve_subject(&store, &args.reference) {
            Ok(s) => s,
            Err(e) => return err_text(e),
        };
        let cmd = MarkFollowUpSent {
            subject,
            today,
            subject_line: args.subject_line,
            body: args.body,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    #[tool(
        name = "follow_up.skip",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn follow_up_skip(&self, Parameters(args): Parameters<RefArgs>) -> CallToolResult {
        let today = match today_or(args.today) {
            Ok(d) => d,
            Err(e) => return err_text(e),
        };
        let mut store = self.store.lock().await;
        let subject = match resolve_subject(&store, &args.reference) {
            Ok(s) => s,
            Err(e) => return err_text(e),
        };
        let cmd = SkipFollowUpStep { subject, today };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    #[tool(
        name = "follow_up.snooze",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn follow_up_snooze(&self, Parameters(args): Parameters<SnoozeArgs>) -> CallToolResult {
        let today = match today_or(args.today) {
            Ok(d) => d,
            Err(e) => return err_text(e),
        };
        let until = if let Some(preset) = args.preset.as_deref() {
            let p = match preset {
                "tomorrow" => SnoozePreset::Tomorrow,
                "next_week" => SnoozePreset::NextWeek,
                "monday" => SnoozePreset::NextMonday,
                other => return err_text(format!("preset inconnu : {other}")),
            };
            snooze_date(today, p)
        } else if let Some(s) = args.until {
            match parse_date(&s) {
                Ok(d) => d,
                Err(e) => return err_text(e.to_string()),
            }
        } else {
            return err_text("précisez until ou preset (tomorrow, next_week, monday)");
        };
        let mut store = self.store.lock().await;
        let subject = match resolve_subject(&store, &args.reference) {
            Ok(s) => s,
            Err(e) => return err_text(e),
        };
        let cmd = SnoozeFollowUp {
            subject,
            until,
            today,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    #[tool(
        name = "follow_up.schedule",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn follow_up_schedule(
        &self,
        Parameters(args): Parameters<ScheduleArgs>,
    ) -> CallToolResult {
        let today = match today_or(args.today) {
            Ok(d) => d,
            Err(e) => return err_text(e),
        };
        let on = match parse_date(&args.on) {
            Ok(d) => d,
            Err(e) => return err_text(e.to_string()),
        };
        let mut store = self.store.lock().await;
        let subject = match resolve_subject(&store, &args.reference) {
            Ok(s) => s,
            Err(e) => return err_text(e),
        };
        let cmd = SetFollowUpDate { subject, on, today };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    #[tool(
        name = "follow_up.retract",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false
        )
    )]
    async fn follow_up_retract(&self, Parameters(args): Parameters<RefArgs>) -> CallToolResult {
        let today = match today_or(args.today) {
            Ok(d) => d,
            Err(e) => return err_text(e),
        };
        let mut store = self.store.lock().await;
        let subject = match resolve_subject(&store, &args.reference) {
            Ok(s) => s,
            Err(e) => return err_text(e),
        };
        let cmd = RetractLastFollowUp { subject, today };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }
}
