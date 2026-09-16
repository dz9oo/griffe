//! Outils `day.*` — mât, gestes, mois. Lectures pures, `today` d'adaptateur.

use griffe_core::clock::today_local;
use griffe_core::day::{day_gestures, day_mast, day_month};
use griffe_core::domain::Month;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::server::FreeflowServer;
use crate::support::{err_text, ok_json, ok_or_return};

fn today_or(today: Option<String>) -> Result<time::Date, String> {
    match today {
        Some(s) => griffe_core::domain::parse_date(&s).map_err(|e| e.to_string()),
        None => Ok(today_local()),
    }
}

fn parse_month(s: &str) -> Result<Month, String> {
    let (year_str, month_str) = s
        .split_once('-')
        .ok_or_else(|| format!("mois invalide : {s} (attendu AAAA-MM)"))?;
    let year: i32 = year_str
        .parse()
        .map_err(|_| format!("mois invalide : {s} (attendu AAAA-MM)"))?;
    let month: u8 = month_str
        .parse()
        .map_err(|_| format!("mois invalide : {s} (attendu AAAA-MM)"))?;
    Month::new(year, month).map_err(|e| e.to_string())
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct TodayArgs {
    /// Date `AAAA-MM-JJ`. Défaut : aujourd'hui (heure locale).
    today: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct MonthArgs {
    /// Mois `AAAA-MM`. Défaut : le mois de `today`.
    month: Option<String>,
    /// Date `AAAA-MM-JJ`. Défaut : aujourd'hui (heure locale).
    today: Option<String>,
}

#[tool_router(router = day_router, vis = "pub(crate)")]
impl FreeflowServer {
    /// Mât du jour : banque, piste en mois, à encaisser, signaux typés (piste courte,
    /// pipeline vide). La fenêtre rédige le français à partir de ces faits.
    #[tool(
        name = "day.mast",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn day_mast_tool(&self, Parameters(args): Parameters<TodayArgs>) -> CallToolResult {
        let today = match today_or(args.today) {
            Ok(d) => d,
            Err(e) => return err_text(e),
        };
        let store = self.store.lock().await;
        match day_mast(store.connection(), today) {
            Ok(mast) => ok_json(mast),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Gestes du jour : fusion relances dues, relevé non lu, prochaine obligation d'État.
    /// Trois à cinq, un verbe par ligne.
    #[tool(
        name = "day.gestures",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn day_gestures_tool(&self, Parameters(args): Parameters<TodayArgs>) -> CallToolResult {
        let today = match today_or(args.today) {
            Ok(d) => d,
            Err(e) => return err_text(e),
        };
        let store = self.store.lock().await;
        match day_gestures(store.connection(), today) {
            Ok(gestes) => ok_json(gestes),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Mois civil : événements (relances, factures, jalons, échéances, rencontres) et bandes
    /// de missions. `month` au format `AAAA-MM`.
    #[tool(
        name = "day.month",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn day_month_tool(&self, Parameters(args): Parameters<MonthArgs>) -> CallToolResult {
        let today = match today_or(args.today) {
            Ok(d) => d,
            Err(e) => return err_text(e),
        };
        let month = match args.month {
            Some(s) => ok_or_return!("month", parse_month(&s)),
            None => Month::new(today.year(), u8::from(today.month()))
                .expect("le mois courant est toujours valide"),
        };
        let store = self.store.lock().await;
        match day_month(store.connection(), month, today) {
            Ok(view) => ok_json(view),
            Err(e) => err_text(e.to_string()),
        }
    }
}
