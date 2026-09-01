//! Outils `fiscal.*` — miroir de `freeflow fiscal ...` et `freeflow year ...` (CLI). Le
//! calendrier chiffré (lot 19) en lecture seule, puis la clôture d'exercice (lot 20) :
//! `fiscal.close_year` porte `requires_confirmation()` côté cœur, donc un agent ne clôt jamais
//! seul — il dépose une action en attente qu'un humain confirme au terminal ou dans la fenêtre.

use freeflow_core::app::Executor;
use freeflow_core::company::company_profile;
use freeflow_core::domain::{FiscalYearEnd, Money, format_date};
use freeflow_core::fiscal::fiscal_calendar;
use freeflow_core::fiscal_year::{CloseFiscalYear, FiscalYearRecord, list_fiscal_years};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use time::Date;

use crate::server::FreeflowServer;
use crate::support::{err_text, ok_json, ok_or_return, outcome_json};

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct FiscalCalendarArgs {
    /// Date du jour, au format `AAAA-MM-JJ` — le calendrier ne renvoie que les échéances à venir.
    today: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct CloseYearArgs {
    /// Année civile de la clôture (ex. `2026`) — la période dérive de la date de clôture
    /// d'exercice du profil (année civile à défaut). Sinon, fournissez `starts_on`/`ends_on`.
    period: Option<i32>,
    /// Premier jour de l'exercice (`AAAA-MM-JJ`), avec `ends_on`, si `period` est absent.
    starts_on: Option<String>,
    ends_on: Option<String>,
    /// Dotation à la réserve légale, en centimes (défaut : 0).
    #[serde(default)]
    legal_reserve_cents: i64,
    /// Dividendes distribués, en centimes (défaut : 0).
    #[serde(default)]
    dividends_cents: i64,
    /// N'écrit rien, montre ce qui serait fait (défaut : faux).
    #[serde(default)]
    dry_run: bool,
}

pub(crate) fn year_json(r: &FiscalYearRecord) -> serde_json::Value {
    json!({
        "id": r.id.to_string(),
        "starts_on": format_date(r.starts_on),
        "ends_on": format_date(r.ends_on),
        "revenue_ht_cents": r.revenue_ht.cents(),
        "expenses_cents": r.expenses.cents(),
        "director_remuneration_cents": r.director_remuneration.cents(),
        "result_before_tax_cents": r.result_before_tax.cents(),
        "corporate_tax_cents": r.corporate_tax.cents(),
        "net_result_cents": r.net_result.cents(),
        "legal_reserve_cents": r.legal_reserve.cents(),
        "dividends_cents": r.dividends.cents(),
        "retained_earnings_cents": r.retained_earnings.cents(),
        "approved_on": r.approved_on.map(format_date),
        "revision": r.revision,
    })
}

#[tool_router(router = fiscal_router, vis = "pub(crate)")]
impl FreeflowServer {
    /// Calendrier fiscal et social chiffré sur 12 mois (TVA, acomptes et solde d'IS, liasse, AG,
    /// dépôt au greffe, DSN) — dates et montants indicatifs, voir `freeflow_core::fiscal`.
    #[tool(
        name = "fiscal.calendar",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn fiscal_calendar(
        &self,
        Parameters(args): Parameters<FiscalCalendarArgs>,
    ) -> CallToolResult {
        let today = ok_or_return!("today", freeflow_core::domain::parse_date(&args.today));
        let store = self.store.lock().await;
        match fiscal_calendar(store.connection(), today) {
            Ok(calendar) => ok_json(
                calendar
                    .iter()
                    .map(|d| {
                        json!({
                            "kind": d.kind.as_str(),
                            "due_on": d.due_on.to_string(),
                            "amount_cents": d.amount.map(Money::cents),
                            "note": d.note,
                        })
                    })
                    .collect::<Vec<_>>(),
            ),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Liste les exercices clos (snapshot du résultat, affectation, approbation), du plus
    /// ancien au plus récent — voir aussi la ressource `freeflow://fiscal-years`.
    #[tool(
        name = "fiscal.years",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn fiscal_years(&self) -> CallToolResult {
        let store = self.store.lock().await;
        match list_fiscal_years(store.connection()) {
            Ok(years) => ok_json(years.iter().map(year_json).collect::<Vec<_>>()),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Clôt un exercice : fige le résultat calculé (CA, charges, IS) et enregistre
    /// l'affectation (réserve légale, dividendes) en projet. Acte à portée juridique : l'appel
    /// dépose une action en attente qu'un humain doit confirmer (`freeflow confirm <id>`),
    /// jamais un effet direct. L'approbation (date d'AG) et les documents de clôture se font
    /// ensuite au terminal (`freeflow year approve|render`) ou dans la fenêtre.
    #[tool(
        name = "fiscal.close_year",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn fiscal_close_year(
        &self,
        Parameters(args): Parameters<CloseYearArgs>,
    ) -> CallToolResult {
        let mut store = self.store.lock().await;
        let (starts_on, ends_on): (Date, Date) = match (args.period, &args.starts_on, &args.ends_on)
        {
            (Some(year), None, None) => {
                let profile = match company_profile(store.connection()) {
                    Ok(Some(p)) => p,
                    Ok(None) => {
                        return err_text(
                            "aucun profil d'entreprise défini : freeflow company set-profile",
                        );
                    }
                    Err(e) => return err_text(e.to_string()),
                };
                let fiscal_year_end = profile.fiscal_year_end.unwrap_or(FiscalYearEnd::CALENDAR);
                let fy = fiscal_year_end.containing(fiscal_year_end.end_in_year(year));
                (fy.start(), fy.end())
            }
            (None, Some(start), Some(end)) => (
                ok_or_return!("starts_on", freeflow_core::domain::parse_date(start)),
                ok_or_return!("ends_on", freeflow_core::domain::parse_date(end)),
            ),
            _ => {
                return err_text(
                    "précisez soit period, soit starts_on et ends_on — pas un mélange",
                );
            }
        };
        let cmd = CloseFiscalYear {
            starts_on,
            ends_on,
            legal_reserve: Money::from_cents(args.legal_reserve_cents),
            dividends: Money::from_cents(args.dividends_cents),
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }
}
