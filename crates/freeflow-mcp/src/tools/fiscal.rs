//! Outils `fiscal.*` — miroir de `freeflow fiscal ...` et `freeflow year ...` (CLI). Le
//! calendrier chiffré (lot 19) en lecture seule, puis la clôture d'exercice (lot 20) :
//! `fiscal.close_year`, `fiscal.approve_year` et `fiscal.delete_year` portent
//! `requires_confirmation()` côté cœur, donc un agent ne clôt, n'approuve ni ne supprime jamais
//! seul — il dépose une action en attente qu'un humain confirme au terminal ou dans la fenêtre.
//!
//! `fiscal.render_year` écrit un document de clôture sur disque : comme `invoice.render`,
//! l'appel `typst` et l'écriture fichier sont de l'IO d'adaptateur (le cœur ne touche que
//! `&Connection`), et l'outil refuse d'écraser un fichier existant — un agent ne doit pas
//! pouvoir remplacer un fichier arbitraire du poste.

use std::path::Path;

use freeflow_core::accounting::AccountingResult;
use freeflow_core::app::Executor;
use freeflow_core::company::{CompanyProfile, company_profile};
use freeflow_core::domain::{FiscalYearEnd, Money, format_date};
use freeflow_core::fiscal::{fiscal_calendar, upcoming_deadlines};
use freeflow_core::fiscal_year::{
    ApproveFiscalYear, CloseFiscalYear, DeleteFiscalYear, FiscalYearRecord,
    UpdateFiscalYearAppropriation, fiscal_year_ending_in, list_fiscal_years,
};
use freeflow_core::store::Store;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use time::Date;

use crate::server::FreeflowServer;
use crate::support::{err_text, ok_json, ok_or_return, outcome_json};

/// L'exercice clos dont la clôture tombe dans l'année civile `period` — même désignation que
/// `freeflow year show <period>` (un exercice n'a pas de nom, sa période dérive du profil).
fn require_year(store: &Store, period: i32) -> Result<FiscalYearRecord, String> {
    match fiscal_year_ending_in(store.connection(), period) {
        Ok(Some(record)) => Ok(record),
        Ok(None) => Err(format!(
            "aucun exercice clos ne se termine en {period} — voir fiscal.years"
        )),
        Err(e) => Err(e.to_string()),
    }
}

fn require_profile(store: &Store) -> Result<CompanyProfile, String> {
    match company_profile(store.connection()) {
        Ok(Some(p)) => Ok(p),
        Ok(None) => Err("aucun profil d'entreprise défini : company.set_profile".to_string()),
        Err(e) => Err(e.to_string()),
    }
}

/// Écrit un document rendu, en refusant d'écraser un fichier existant (`create_new`) — voir la
/// doc du module.
pub(crate) fn write_new_document(out: &str, bytes: &[u8]) -> Result<String, String> {
    use std::io::Write;
    let path = Path::new(out);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("écriture de {out} impossible (fichier déjà présent ?) : {e}"))?;
    file.write_all(bytes)
        .and_then(|()| file.flush())
        .map_err(|e| format!("écriture de {out} impossible : {e}"))?;
    Ok(format!("{out} écrit ({} octets)", bytes.len()))
}

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

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct FiscalDeadlinesArgs {
    /// Date du jour, au format `AAAA-MM-JJ`.
    today: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct YearRefArgs {
    /// Année civile de la clôture (ex. `2026`).
    period: i32,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct YearRefMutationArgs {
    /// Année civile de la clôture (ex. `2026`).
    period: i32,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct AmendYearArgs {
    /// Année civile de la clôture (ex. `2026`).
    period: i32,
    /// Dotation à la réserve légale, en centimes — absent : inchangée.
    legal_reserve_cents: Option<i64>,
    /// Dividendes distribués, en centimes — absent : inchangés.
    dividends_cents: Option<i64>,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ApproveYearArgs {
    /// Année civile de la clôture (ex. `2026`).
    period: i32,
    /// Date de l'AG d'approbation, au format `AAAA-MM-JJ`.
    approved_on: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct RenderYearArgs {
    /// Année civile de la clôture (ex. `2026`).
    period: i32,
    /// `minutes` (PV d'AG), `appropriation` (affectation), `synthesis` (compte de résultat
    /// simplifié) — PDF — ou `liasse` (cases 2065/2033 en JSON).
    doc: String,
    /// Chemin du fichier à écrire — refusé s'il existe déjà.
    out: String,
    /// Date du jour (`AAAA-MM-JJ`), requise pour un PV en projet (non approuvé), ignorée sinon.
    today: Option<String>,
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

    /// Prochaines échéances calendaires de base (CA3, acompte d'IS, CFE), sans montants — dates
    /// indicatives (année civile) ; voir `fiscal.calendar` pour le calendrier chiffré dérivé du
    /// profil.
    #[tool(
        name = "fiscal.deadlines",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn fiscal_deadlines(
        &self,
        Parameters(args): Parameters<FiscalDeadlinesArgs>,
    ) -> CallToolResult {
        let today = ok_or_return!("today", freeflow_core::domain::parse_date(&args.today));
        ok_json(
            upcoming_deadlines(today)
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
        )
    }

    /// Détail d'un exercice clos (snapshot du résultat, affectation, approbation), désigné par
    /// l'année civile de sa clôture.
    #[tool(
        name = "fiscal.year_show",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn fiscal_year_show(&self, Parameters(args): Parameters<YearRefArgs>) -> CallToolResult {
        let store = self.store.lock().await;
        match require_year(&store, args.period) {
            Ok(record) => ok_json(year_json(&record)),
            Err(e) => err_text(e),
        }
    }

    /// Révise l'affectation d'un exercice encore en projet (réserve légale, dividendes) — le
    /// snapshot du résultat reste figé. Seuls les champs fournis changent.
    #[tool(
        name = "fiscal.amend_year",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn fiscal_amend_year(
        &self,
        Parameters(args): Parameters<AmendYearArgs>,
    ) -> CallToolResult {
        let mut store = self.store.lock().await;
        let record = match require_year(&store, args.period) {
            Ok(r) => r,
            Err(e) => return err_text(e),
        };
        let cmd = UpdateFiscalYearAppropriation {
            id: record.id,
            revision: record.revision,
            legal_reserve: args
                .legal_reserve_cents
                .map_or(record.legal_reserve, Money::from_cents),
            dividends: args
                .dividends_cents
                .map_or(record.dividends, Money::from_cents),
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Approuve un exercice (date de l'AG) — il devient immuable. Acte à portée juridique :
    /// l'appel dépose une action en attente qu'un humain doit confirmer, jamais un effet direct.
    #[tool(
        name = "fiscal.approve_year",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn fiscal_approve_year(
        &self,
        Parameters(args): Parameters<ApproveYearArgs>,
    ) -> CallToolResult {
        let approved_on: Date = ok_or_return!(
            "approved_on",
            freeflow_core::domain::parse_date(&args.approved_on)
        );
        let mut store = self.store.lock().await;
        let record = match require_year(&store, args.period) {
            Ok(r) => r,
            Err(e) => return err_text(e),
        };
        let cmd = ApproveFiscalYear {
            id: record.id,
            revision: record.revision,
            approved_on,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Supprime un exercice encore en projet (clos par erreur) — action sensible : un agent la
    /// propose, seul un humain (`freeflow confirm`) peut l'appliquer.
    #[tool(
        name = "fiscal.delete_year",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false
        )
    )]
    async fn fiscal_delete_year(
        &self,
        Parameters(args): Parameters<YearRefMutationArgs>,
    ) -> CallToolResult {
        let mut store = self.store.lock().await;
        let record = match require_year(&store, args.period) {
            Ok(r) => r,
            Err(e) => return err_text(e),
        };
        let cmd = DeleteFiscalYear {
            id: record.id,
            revision: record.revision,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Rend un document de clôture (PDF, ou JSON pour la liasse) dans `out` — refuse d'écraser
    /// un fichier existant. Le document reflète le snapshot figé à la clôture, pas un recalcul
    /// vivant.
    #[tool(
        name = "fiscal.render_year",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn fiscal_render_year(
        &self,
        Parameters(args): Parameters<RenderYearArgs>,
    ) -> CallToolResult {
        let store = self.store.lock().await;
        let record = match require_year(&store, args.period) {
            Ok(r) => r,
            Err(e) => return err_text(e),
        };
        let profile = match require_profile(&store) {
            Ok(p) => p,
            Err(e) => return err_text(e),
        };
        let bytes = match args.doc.as_str() {
            "minutes" => {
                let today: Option<Date> = match &args.today {
                    Some(s) => Some(ok_or_return!("today", freeflow_core::domain::parse_date(s))),
                    None => None,
                };
                let Some(today) = record.approved_on.or(today) else {
                    return err_text(
                        "cet exercice n'est pas approuvé : précisez today pour dater le projet \
                         de PV",
                    );
                };
                ok_or_return!(
                    "minutes",
                    freeflow_docs::render_approval_minutes(&profile, &record, today)
                )
            }
            "appropriation" => ok_or_return!(
                "appropriation",
                freeflow_docs::render_appropriation_decision(&profile, &record)
            ),
            "synthesis" => {
                // Le document reflète le snapshot figé à la clôture, pas un recalcul vivant.
                let result = AccountingResult {
                    period: record.period(),
                    revenue_ht: record.revenue_ht,
                    expenses: record.expenses,
                    director_remuneration: record.director_remuneration,
                    result_before_tax: record.result_before_tax,
                    corporate_tax: record.corporate_tax,
                    net_result: record.net_result,
                };
                let years = ok_or_return!("years", list_fiscal_years(store.connection()));
                let prior = years
                    .iter()
                    .filter(|y| y.ends_on < record.starts_on)
                    .max_by_key(|y| y.ends_on);
                ok_or_return!(
                    "synthesis",
                    freeflow_docs::render_synthesis(&profile, &result, prior)
                )
            }
            "liasse" => {
                let export = freeflow_docs::liasse_export(&profile, &record);
                let mut bytes = ok_or_return!("liasse", serde_json::to_vec_pretty(&export));
                bytes.push(b'\n');
                bytes
            }
            other => {
                return err_text(format!(
                    "document inconnu : {other} (attendu : minutes, appropriation, synthesis, \
                     liasse)"
                ));
            }
        };
        match write_new_document(&args.out, &bytes) {
            Ok(msg) => ok_json(json!({ "written": msg })),
            Err(e) => err_text(e),
        }
    }
}
