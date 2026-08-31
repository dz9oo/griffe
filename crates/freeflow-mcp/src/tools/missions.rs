//! Outils `mission.*` — miroir de `freeflow mission ...` (CLI, lot 7). Lot 16 : ajout de
//! `show`/`list`/`update`/`close`/`reopen`/`archive`/`unarchive`/`delete`/`references`/
//! `schedule` et de `time.list`/`time.update`/`time.delete` — `log_time` reste tel quel (même
//! asymétrie assumée que `prospect.log_interaction`, voir `tools/prospection.rs`).

use freeflow_core::app::Executor;
use freeflow_core::domain::{
    ClientId, Milestone, MilestoneParseError, MissionId, MissionKind, Money, QuoteId, TimeCategory,
    TimeEntryId,
};
use freeflow_core::missions::{
    self, BillingSchedule, MissionFilter, effective_daily_rate, list_missions_with,
    list_time_entries, mission_by_id, mission_references, monthly_capacity, time_entry_by_id,
};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::server::FreeflowServer;
use crate::support::{err_text, ok_json, ok_or_return, outcome_json, resolve_mission};

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct CreateMissionArgs {
    /// Référence du client : UUID, préfixe d'UUID, ou nom.
    client: String,
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
    /// Un jalon par entrée : `label:parts_bps[:AAAA-MM-JJ]` (ex. `Acompte:3000:2026-03-31`).
    #[serde(default)]
    milestones: Vec<String>,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct MissionRefArgs {
    /// Référence de la mission : UUID, préfixe d'UUID, ou nom.
    mission: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct MissionRefMutationArgs {
    mission: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ListMissionsArgs {
    /// Inclut les missions clôturées si vrai (défaut : faux).
    #[serde(default)]
    ended: bool,
    /// Inclut les missions archivées si vrai (défaut : faux).
    #[serde(default)]
    archived: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct UpdateMissionArgs {
    mission: String,
    name: Option<String>,
    /// `regie`, `forfait`, ou `recurrent` — si omis, le type actuel est conservé.
    kind: Option<String>,
    daily_rate_cents: Option<i64>,
    budget_cents: Option<i64>,
    monthly_amount_cents: Option<i64>,
    started_on: Option<String>,
    /// Si non vide, remplace toute la collection de jalons actuelle.
    #[serde(default)]
    milestones: Vec<String>,
    /// Vide la collection de jalons, sans en fournir une nouvelle.
    #[serde(default)]
    clear_milestones: bool,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct CloseMissionArgs {
    mission: String,
    ended_on: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct LogTimeArgs {
    mission: String,
    worked_on: String,
    days: f64,
    /// `billable`, `pre_sales`, `admin`, ou `training`.
    category: String,
    note: Option<String>,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct UpdateTimeEntryArgs {
    /// Identifiant de la saisie de temps (UUID) — voir `mission.time.list`.
    id: String,
    worked_on: Option<String>,
    days: Option<f64>,
    category: Option<String>,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    clear_note: bool,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct TimeEntryIdArgs {
    id: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct CapacityArgs {
    /// Année et mois, ex. `2026-09`.
    month: String,
}

fn build_mission_kind(
    kind: &str,
    daily_rate_cents: Option<i64>,
    budget_cents: Option<i64>,
    monthly_amount_cents: Option<i64>,
) -> Result<MissionKind, String> {
    match kind {
        "regie" => {
            let cents =
                daily_rate_cents.ok_or("daily_rate_cents est requis pour kind = \"regie\"")?;
            Ok(MissionKind::Regie {
                daily_rate: Money::from_cents(cents),
            })
        }
        "forfait" => {
            let cents = budget_cents.ok_or("budget_cents est requis pour kind = \"forfait\"")?;
            Ok(MissionKind::Forfait {
                budget: Money::from_cents(cents),
            })
        }
        "recurrent" => {
            let cents = monthly_amount_cents
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

fn parse_milestones(raw: &[String]) -> Result<Vec<Milestone>, MilestoneParseError> {
    raw.iter().map(|s| s.parse()).collect()
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

fn schedule_json(schedule: &BillingSchedule) -> serde_json::Value {
    match schedule {
        BillingSchedule::Regie { daily_rate } => {
            json!({"kind": "regie", "daily_rate_cents": daily_rate.cents()})
        }
        BillingSchedule::Recurrent { monthly_amount } => {
            json!({"kind": "recurrent", "monthly_amount_cents": monthly_amount.cents()})
        }
        BillingSchedule::Forfait { installments } => json!({
            "kind": "forfait",
            "installments": installments.iter().map(|(m, amount)| json!({
                "label": m.label,
                "share_bps": m.share_bps,
                "due_on": m.due_on.map(freeflow_core::domain::format_date),
                "amount_cents": amount.cents(),
            })).collect::<Vec<_>>(),
        }),
    }
}

fn mission_or_not_found(
    store: &freeflow_core::store::Store,
    id: MissionId,
) -> Result<freeflow_core::domain::Mission, CallToolResult> {
    match mission_by_id(store.connection(), id) {
        Ok(Some(m)) => Ok(m),
        Ok(None) => Err(err_text(format!("mission introuvable : {id}"))),
        Err(e) => Err(err_text(e.to_string())),
    }
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
        let mut store = self.store.lock().await;
        let client_id: ClientId = ok_or_return!(
            "client",
            crate::support::resolve_client(&store, &args.client)
        );
        let quote_id: Option<QuoteId> = match &args.quote_id {
            Some(s) => Some(ok_or_return!("quote_id", s.parse())),
            None => None,
        };
        let started_on = ok_or_return!(
            "started_on",
            freeflow_core::domain::parse_date(&args.started_on)
        );
        let kind = ok_or_return!(
            "kind",
            build_mission_kind(
                &args.kind,
                args.daily_rate_cents,
                args.budget_cents,
                args.monthly_amount_cents
            )
        );
        let milestones = ok_or_return!("milestones", parse_milestones(&args.milestones));
        let cmd = missions::CreateMission {
            client_id,
            quote_id,
            name: args.name,
            kind,
            milestones,
            started_on,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Affiche une mission.
    #[tool(
        name = "mission.show",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn mission_show(&self, Parameters(args): Parameters<MissionRefArgs>) -> CallToolResult {
        let store = self.store.lock().await;
        let id = ok_or_return!("mission", resolve_mission(&store, &args.mission));
        match mission_or_not_found(&store, id) {
            Ok(m) => ok_json(m),
            Err(result) => result,
        }
    }

    /// Liste les missions en cours et non archivées (`ended`/`archived: true` pour élargir).
    #[tool(
        name = "mission.list",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn mission_list(&self, Parameters(args): Parameters<ListMissionsArgs>) -> CallToolResult {
        let store = self.store.lock().await;
        let filter = MissionFilter {
            include_ended: args.ended,
            include_archived: args.archived,
        };
        match list_missions_with(store.connection(), filter) {
            Ok(missions) => ok_json(missions),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Ce qui référence encore une mission (factures, saisies de temps) — à consulter avant
    /// `mission.delete`.
    #[tool(
        name = "mission.references",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn mission_references(
        &self,
        Parameters(args): Parameters<MissionRefArgs>,
    ) -> CallToolResult {
        let store = self.store.lock().await;
        let id = ok_or_return!("mission", resolve_mission(&store, &args.mission));
        match mission_references(store.connection(), id) {
            Ok(refs) => ok_json(refs),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Échéancier de facturation dérivé du type de mission et de ses jalons.
    #[tool(
        name = "mission.schedule",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn mission_schedule(
        &self,
        Parameters(args): Parameters<MissionRefArgs>,
    ) -> CallToolResult {
        let store = self.store.lock().await;
        let id = ok_or_return!("mission", resolve_mission(&store, &args.mission));
        let mission = match mission_or_not_found(&store, id) {
            Ok(m) => m,
            Err(result) => return result,
        };
        ok_json(schedule_json(&missions::billing_schedule(&mission)))
    }

    /// Modifie une mission existante — seuls les champs fournis changent. Ne peut ni clore la
    /// mission (`mission.close`) ni changer son devis d'origine.
    #[tool(
        name = "mission.update",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn mission_update(
        &self,
        Parameters(args): Parameters<UpdateMissionArgs>,
    ) -> CallToolResult {
        let mut store = self.store.lock().await;
        let id = ok_or_return!("mission", resolve_mission(&store, &args.mission));
        let current = match mission_or_not_found(&store, id) {
            Ok(m) => m,
            Err(result) => return result,
        };
        let kind = match &args.kind {
            Some(k) => ok_or_return!(
                "kind",
                build_mission_kind(
                    k,
                    args.daily_rate_cents,
                    args.budget_cents,
                    args.monthly_amount_cents
                )
            ),
            None => current.kind.clone(),
        };
        let started_on = match &args.started_on {
            Some(s) => ok_or_return!("started_on", freeflow_core::domain::parse_date(s)),
            None => current.started_on,
        };
        let milestones = if args.clear_milestones {
            Vec::new()
        } else if args.milestones.is_empty() {
            current.milestones.clone()
        } else {
            ok_or_return!("milestones", parse_milestones(&args.milestones))
        };
        let cmd = missions::UpdateMission {
            id,
            revision: current.revision,
            name: args.name.unwrap_or(current.name),
            kind,
            milestones,
            started_on,
            quote_id: current.quote_id,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Clôt une mission à une date donnée — fait métier daté et réversible (`mission.reopen`),
    /// distinct de `mission.archive` (classement sans date).
    #[tool(
        name = "mission.close",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn mission_close(
        &self,
        Parameters(args): Parameters<CloseMissionArgs>,
    ) -> CallToolResult {
        let mut store = self.store.lock().await;
        let id = ok_or_return!("mission", resolve_mission(&store, &args.mission));
        let current = match mission_or_not_found(&store, id) {
            Ok(m) => m,
            Err(result) => return result,
        };
        let ended_on = ok_or_return!(
            "ended_on",
            freeflow_core::domain::parse_date(&args.ended_on)
        );
        let cmd = missions::CloseMission {
            id,
            revision: current.revision,
            ended_on,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Rouvre une mission clôturée.
    #[tool(
        name = "mission.reopen",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn mission_reopen(
        &self,
        Parameters(args): Parameters<MissionRefMutationArgs>,
    ) -> CallToolResult {
        let mut store = self.store.lock().await;
        let id = ok_or_return!("mission", resolve_mission(&store, &args.mission));
        let current = match mission_or_not_found(&store, id) {
            Ok(m) => m,
            Err(result) => return result,
        };
        let cmd = missions::ReopenMission {
            id,
            revision: current.revision,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Retire une mission des listes actives sans la clore ni la supprimer.
    #[tool(
        name = "mission.archive",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn mission_archive(
        &self,
        Parameters(args): Parameters<MissionRefMutationArgs>,
    ) -> CallToolResult {
        let mut store = self.store.lock().await;
        let id = ok_or_return!("mission", resolve_mission(&store, &args.mission));
        let current = match mission_or_not_found(&store, id) {
            Ok(m) => m,
            Err(result) => return result,
        };
        let cmd = missions::ArchiveMission {
            id,
            revision: current.revision,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Réintègre une mission archivée dans les listes actives.
    #[tool(
        name = "mission.unarchive",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn mission_unarchive(
        &self,
        Parameters(args): Parameters<MissionRefMutationArgs>,
    ) -> CallToolResult {
        let mut store = self.store.lock().await;
        let id = ok_or_return!("mission", resolve_mission(&store, &args.mission));
        let current = match mission_or_not_found(&store, id) {
            Ok(m) => m,
            Err(result) => return result,
        };
        let cmd = missions::UnarchiveMission {
            id,
            revision: current.revision,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Supprime une mission pour de bon — refusée si une facture ou une saisie de temps la
    /// référence (voir `mission.references`, ou archivez-la / videz ses saisies de temps à la
    /// place). Effet destructeur : déclenché par un agent, attend toujours une confirmation
    /// humaine.
    #[tool(
        name = "mission.delete",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false
        )
    )]
    async fn mission_delete(
        &self,
        Parameters(args): Parameters<MissionRefMutationArgs>,
    ) -> CallToolResult {
        let mut store = self.store.lock().await;
        let id = ok_or_return!("mission", resolve_mission(&store, &args.mission));
        let current = match mission_or_not_found(&store, id) {
            Ok(m) => m,
            Err(result) => return result,
        };
        let cmd = missions::DeleteMission {
            id,
            revision: current.revision,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
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
        let mut store = self.store.lock().await;
        let mission_id = ok_or_return!("mission", resolve_mission(&store, &args.mission));
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
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Liste les saisies de temps d'une mission.
    #[tool(
        name = "mission.time.list",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn mission_time_list(
        &self,
        Parameters(args): Parameters<MissionRefArgs>,
    ) -> CallToolResult {
        let store = self.store.lock().await;
        let id = ok_or_return!("mission", resolve_mission(&store, &args.mission));
        match list_time_entries(store.connection(), id) {
            Ok(entries) => ok_json(entries),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Modifie une saisie de temps existante.
    #[tool(
        name = "mission.time.update",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn mission_time_update(
        &self,
        Parameters(args): Parameters<UpdateTimeEntryArgs>,
    ) -> CallToolResult {
        let mut store = self.store.lock().await;
        let id: TimeEntryId = ok_or_return!("id", args.id.parse());
        let current = match time_entry_by_id(store.connection(), id) {
            Ok(Some(e)) => e,
            Ok(None) => return err_text(format!("saisie de temps introuvable : {id}")),
            Err(e) => return err_text(e.to_string()),
        };
        let worked_on = match &args.worked_on {
            Some(s) => ok_or_return!("worked_on", freeflow_core::domain::parse_date(s)),
            None => current.worked_on,
        };
        let category = match &args.category {
            Some(s) => ok_or_return!("category", s.parse()),
            None => current.category,
        };
        let note = if args.clear_note {
            None
        } else {
            args.note.or(current.note)
        };
        let cmd = missions::UpdateTimeEntry {
            id,
            revision: current.revision,
            worked_on,
            days: args.days.unwrap_or(current.days),
            category,
            note,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Supprime une saisie de temps.
    #[tool(
        name = "mission.time.delete",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false
        )
    )]
    async fn mission_time_delete(
        &self,
        Parameters(args): Parameters<TimeEntryIdArgs>,
    ) -> CallToolResult {
        let mut store = self.store.lock().await;
        let id: TimeEntryId = ok_or_return!("id", args.id.parse());
        let current = match time_entry_by_id(store.connection(), id) {
            Ok(Some(e)) => e,
            Ok(None) => return err_text(format!("saisie de temps introuvable : {id}")),
            Err(e) => return err_text(e.to_string()),
        };
        let cmd = missions::DeleteTimeEntry {
            id,
            revision: current.revision,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// TJM effectif d'une mission (CA facturé ÷ jours facturables consommés).
    #[tool(
        name = "mission.rate",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn mission_rate(&self, Parameters(args): Parameters<MissionRefArgs>) -> CallToolResult {
        let store = self.store.lock().await;
        let id = ok_or_return!("mission", resolve_mission(&store, &args.mission));
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
