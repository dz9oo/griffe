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

use freeflow_core::app::Executor;
use freeflow_core::closing::{checklist_json, closing_checklist};
use freeflow_core::company::{CompanyProfile, company_profile};
use freeflow_core::domain::{FiscalYearEnd, Money, format_date};
use freeflow_core::fiscal::{fiscal_calendar, upcoming_deadlines};
use freeflow_core::fiscal_year::{
    ApproveFiscalYear, CloseFiscalYear, DeleteFiscalYear, FiscalYearRecord,
    UpdateFiscalYearAppropriation, fiscal_year_ending_in, list_fiscal_years,
};
use freeflow_core::ledger::{balance_json, build_ledger, ledger_ending_in};
use freeflow_core::opening_balance::{
    DeleteOpeningBalance, OpeningBalanceRecord, RecordOpeningBalance, UpdateOpeningBalance,
    opening_balance,
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
    /// Option de report en arrière du déficit de l'exercice (art. 220 quinquies CGI) sur le
    /// bénéfice de l'exercice précédent clos dans l'application — la créance d'IS entre dans
    /// le résultat net. Refusé sans déficit, sans exercice précédent ou sans bénéfice
    /// d'imputation (défaut : faux).
    #[serde(default)]
    carry_back: bool,
    /// Charges non déductibles fiscalement (art. 39-4 CGI), en centimes, à réintégrer —
    /// reportées sur le PV (art. 223 quater) et la 2033-B (défaut : 0).
    #[serde(default)]
    non_deductible_expenses_cents: i64,
    /// Date du jour (`AAAA-MM-JJ`, défaut : aujourd'hui, heure locale) : la clôture est refusée
    /// tant que l'exercice n'est pas écoulé.
    today: Option<String>,
    /// N'écrit rien, montre ce qui serait fait (défaut : faux).
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct SetOpeningBalanceArgs {
    /// Premier jour de l'exercice qui s'ouvre sur ce bilan (`AAAA-MM-JJ`), lendemain de la
    /// clôture reprise.
    opens_on: String,
    /// Provenance, libre (ex. « bilan au 30/09/2025, cabinet X »).
    source: Option<String>,
    /// Lignes de la balance, une par compte, au format `compte:libellé:D|C:montant` — ex.
    /// `["101000:Capital social:C:1000.00", "512000:Banque:D:1000.00"]`. Comptes de bilan
    /// (classes 1 à 5) seulement ; total débit = total crédit.
    lines: Vec<String>,
    /// Déficits fiscaux antérieurs encore reportables, en centimes (case 870 du dernier
    /// 2033-D), hors bilan — imputés sur les bénéfices des exercices clos ici (défaut : 0).
    #[serde(default)]
    tax_losses_cents: i64,
    /// IS de l'exercice précédent, en centimes (base des acomptes d'IS) — absent = inconnu.
    prior_corporate_tax_cents: Option<i64>,
    /// TVA due au titre de l'exercice précédent, en centimes (base des acomptes 3514).
    prior_vat_due_cents: Option<i64>,
    /// N'écrit rien, montre ce qui serait fait (défaut : faux).
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ImportOpeningBalanceArgs {
    /// Premier jour de l'exercice qui s'ouvre sur ce bilan (`AAAA-MM-JJ`).
    opens_on: String,
    /// `balance` (CSV), `fec`, ou `2033a` (cases du formulaire, voir `boxes`).
    format: Option<String>,
    /// Cases du 2033-A `NNN=montant` en euros, ex. `["084=9540.00","120=1000.00"]`. Si
    /// renseigné, le fichier n'est pas lu (le PDF du cabinet n'a pas de numéros de compte).
    boxes: Option<Vec<String>>,
    /// Contenu du fichier en texte, ou `content_base64` (octets exacts), ou `path` (fichier
    /// local lu par le serveur).
    content: Option<String>,
    content_base64: Option<String>,
    path: Option<String>,
    /// Provenance, libre (défaut : « import »).
    source: Option<String>,
    #[serde(default)]
    tax_losses_cents: i64,
    prior_corporate_tax_cents: Option<i64>,
    prior_vat_due_cents: Option<i64>,
    /// `true` : renvoie seulement l'aperçu (lignes retenues, écartées, résultat dérivé,
    /// avertissements) sans rien écrire.
    #[serde(default)]
    dry_run: bool,
    /// Durée d'usage en mois appliquée à chaque immobilisation reprise (couple 2xx/28x) après
    /// un enregistrement réussi. Sans elle, les candidats sont seulement dans l'aperçu.
    duration_months: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct OpeningBalanceMutationArgs {
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
pub(crate) struct ChecklistArgs {
    /// Année civile de la clôture (ex. `2026`) — même désignation que `fiscal.year_show`.
    period: i32,
    /// Date du jour (`AAAA-MM-JJ`, défaut : aujourd'hui) — décide si l'exercice est écoulé et
    /// des échéances dépassées.
    #[serde(default)]
    today: Option<String>,
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
    /// Date de l'AG d'approbation, au format `AAAA-MM-JJ` — ni avant la clôture, ni dans le
    /// futur.
    approved_on: String,
    /// Date du jour (`AAAA-MM-JJ`, défaut : aujourd'hui, heure locale).
    today: Option<String>,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct RenderYearArgs {
    /// Année civile de la clôture (ex. `2026`).
    period: i32,
    /// `minutes` (PV d'AG), `appropriation` (affectation), `synthesis` (compte de résultat
    /// simplifié), `balance_sheet` (bilan 2033-A et balance), `inventory` (inventaire L227-9),
    /// `efi_notice` (notice de saisie EFI) — PDF — ou `liasse` (cases 2065/2033 en JSON).
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
        "depreciation_cents": r.depreciation.cents(),
        "result_before_tax_cents": r.result_before_tax.cents(),
        "losses_imputed_cents": r.losses_imputed.cents(),
        "taxable_result_cents": r.taxable_result().cents(),
        "corporate_tax_cents": r.corporate_tax.cents(),
        "carried_back_cents": r.carried_back.cents(),
        "carry_back_credit_cents": r.carry_back_credit.cents(),
        "net_result_cents": r.net_result.cents(),
        "legal_reserve_cents": r.legal_reserve.cents(),
        "dividends_cents": r.dividends.cents(),
        "retained_earnings_cents": r.retained_earnings.cents(),
        "losses_carried_forward_cents": r.losses_carried_forward.cents(),
        "approved_on": r.approved_on.map(format_date),
        "revision": r.revision,
    })
}

pub(crate) fn opening_json(r: &OpeningBalanceRecord) -> serde_json::Value {
    let equity = r.equity();
    json!({
        "opens_on": format_date(r.balance.opens_on),
        "source": r.balance.source,
        "lines": r.balance.lines.iter().map(|l| json!({
            "account": l.account.as_str(),
            "label": l.label,
            "side": l.side.as_str(),
            "amount_cents": l.amount.cents(),
        })).collect::<Vec<_>>(),
        "total_debit_cents": r.balance.total_debit().cents(),
        "total_credit_cents": r.balance.total_credit().cents(),
        "equity": {
            "share_capital_cents": equity.share_capital.cents(),
            "legal_reserve_cents": equity.legal_reserve.cents(),
            "retained_earnings_cents": equity.retained_earnings.cents(),
        },
        "tax_losses_cents": r.balance.tax_losses.cents(),
        "revision": r.revision,
    })
}

#[tool_router(router = fiscal_router, vis = "pub(crate)")]
impl FreeflowServer {
    /// Le bilan d'ouverture (reprise du dernier bilan tenu avant FreeFlow) : lignes, totaux,
    /// capitaux propres repris — ou `null` s'il n'est pas enregistré. Voir aussi la ressource
    /// `freeflow://opening-balance`.
    #[tool(
        name = "fiscal.opening_balance",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn fiscal_opening_balance(&self) -> CallToolResult {
        let store = self.store.lock().await;
        match opening_balance(store.connection()) {
            Ok(Some(record)) => ok_json(opening_json(&record)),
            Ok(None) => ok_json(serde_json::Value::Null),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Enregistre le bilan d'ouverture, ou le remplace en entier s'il existe déjà (état
    /// complet, jamais un patch). Refusé dès qu'un exercice est clos dans l'application. Fait
    /// comptable fondateur : l'appel dépose une action en attente qu'un humain doit confirmer
    /// (`freeflow confirm <id>`), jamais un effet direct.
    #[tool(
        name = "fiscal.set_opening_balance",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true
        )
    )]
    async fn fiscal_set_opening_balance(
        &self,
        Parameters(args): Parameters<SetOpeningBalanceArgs>,
    ) -> CallToolResult {
        let opens_on = ok_or_return!(
            "opens_on",
            freeflow_core::domain::parse_date(&args.opens_on)
        );
        let mut lines = Vec::with_capacity(args.lines.len());
        for spec in &args.lines {
            lines.push(ok_or_return!(
                "lines",
                spec.parse::<freeflow_core::domain::OpeningBalanceLine>()
            ));
        }
        let mut store = self.store.lock().await;
        let existing = match opening_balance(store.connection()) {
            Ok(existing) => existing,
            Err(e) => return err_text(e.to_string()),
        };
        let ctx = self.ctx(args.dry_run);
        let result = match existing {
            Some(existing) => Executor::new(&mut store)
                .execute(
                    &UpdateOpeningBalance {
                        revision: existing.revision,
                        opens_on,
                        source: args.source,
                        lines,
                        tax_losses: Money::from_cents(args.tax_losses_cents),
                        prior_corporate_tax: args.prior_corporate_tax_cents.map(Money::from_cents),
                        prior_vat_due: args.prior_vat_due_cents.map(Money::from_cents),
                    },
                    &ctx,
                )
                .map(|o| outcome_json(&o)),
            None => Executor::new(&mut store)
                .execute(
                    &RecordOpeningBalance {
                        opens_on,
                        source: args.source,
                        lines,
                        tax_losses: Money::from_cents(args.tax_losses_cents),
                        prior_corporate_tax: args.prior_corporate_tax_cents.map(Money::from_cents),
                        prior_vat_due: args.prior_vat_due_cents.map(Money::from_cents),
                    },
                    &ctx,
                )
                .map(|o| outcome_json(&o)),
        };
        match result {
            Ok(value) => ok_json(value),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Supprime le bilan d'ouverture — tant qu'aucun exercice n'est clos. Derrière confirmation
    /// humaine, comme les autres suppressions.
    /// Reprend le bilan d'ouverture depuis un fichier du cabinet : une balance générale (CSV :
    /// compte, libellé, débit, crédit ou soldes) ou le FEC de l'exercice précédent (export
    /// Tiime/Indy, `|` ou tabulation). Comptes de bilan repris tels quels, comptes 6/7 agrégés en
    /// résultat (120/129) sauf balance déjà affectée, reste écarté avec son motif. `dry_run`
    /// renvoie l'aperçu ; sinon l'appel dépose une action en attente (`RecordOpeningBalance`,
    /// ou remplacement) qu'un humain doit confirmer.
    #[tool(
        name = "fiscal.import_opening_balance",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true
        )
    )]
    async fn fiscal_import_opening_balance(
        &self,
        Parameters(args): Parameters<ImportOpeningBalanceArgs>,
    ) -> CallToolResult {
        use base64::Engine as _;
        let opens_on = ok_or_return!(
            "opens_on",
            freeflow_core::domain::parse_date(&args.opens_on)
        );
        let preview = if let Some(raw_boxes) = args.boxes.as_ref() {
            let mut mapped = freeflow_core::opening_balance::import::CerfaBoxes::new();
            for spec in raw_boxes {
                let Some((case, amount)) = spec.split_once('=') else {
                    return err_text(format!(
                        "case 2033-A invalide « {spec} » (attendu CASE=MONTANT, ex. 084=9540.00)"
                    ));
                };
                let amount = ok_or_return!("boxes", Money::parse_decimal(amount.trim()));
                mapped.insert(case.trim().to_string(), amount);
            }
            ok_or_return!(
                "boxes",
                freeflow_core::opening_balance::import::from_2033a(opens_on, &mapped)
            )
        } else {
            let bytes: Vec<u8> = match (&args.content, &args.content_base64, &args.path) {
                (Some(text), _, _) => text.clone().into_bytes(),
                (None, Some(b64), _) => ok_or_return!(
                    "content_base64",
                    base64::engine::general_purpose::STANDARD.decode(b64.trim())
                ),
                (None, None, Some(path)) => ok_or_return!("path", std::fs::read(path)),
                (None, None, None) => {
                    return err_text("fournissez boxes (2033-A), content, content_base64 ou path");
                }
            };
            let hint = match args.format.as_deref() {
                None => None,
                Some(raw) => Some(ok_or_return!(
                    "format",
                    raw.parse::<freeflow_core::opening_balance::import::OpeningImportFormat>()
                )),
            };
            ok_or_return!(
                "content",
                freeflow_core::opening_balance::import::import_opening_balance(
                    &bytes, opens_on, hint
                )
            )
        };
        if args.dry_run {
            return ok_json(freeflow_core::opening_balance::import::preview_json(
                &preview,
            ));
        }
        let mut store = self.store.lock().await;
        let existing = match opening_balance(store.connection()) {
            Ok(existing) => existing,
            Err(e) => return err_text(e.to_string()),
        };
        let ctx = self.ctx(false);
        let source = Some(args.source.unwrap_or_else(|| "import".to_string()));
        let result = match existing {
            Some(existing) => Executor::new(&mut store)
                .execute(
                    &UpdateOpeningBalance {
                        revision: existing.revision,
                        opens_on,
                        source,
                        lines: preview.lines.clone(),
                        tax_losses: Money::from_cents(args.tax_losses_cents),
                        prior_corporate_tax: args.prior_corporate_tax_cents.map(Money::from_cents),
                        prior_vat_due: args.prior_vat_due_cents.map(Money::from_cents),
                    },
                    &ctx,
                )
                .map(|o| outcome_json(&o)),
            None => Executor::new(&mut store)
                .execute(
                    &RecordOpeningBalance {
                        opens_on,
                        source,
                        lines: preview.lines.clone(),
                        tax_losses: Money::from_cents(args.tax_losses_cents),
                        prior_corporate_tax: args.prior_corporate_tax_cents.map(Money::from_cents),
                        prior_vat_due: args.prior_vat_due_cents.map(Money::from_cents),
                    },
                    &ctx,
                )
                .map(|o| outcome_json(&o)),
        };
        match result {
            Ok(mut body) => {
                body["preview"] = freeflow_core::opening_balance::import::preview_json(&preview);
                if body["status"] == "applied"
                    && let Some(months) = args.duration_months
                {
                    let candidates = freeflow_core::domain::fixed_asset_candidates(&preview.lines);
                    let mut declared = Vec::new();
                    for candidate in &candidates {
                        let cmd = freeflow_core::fixed_assets::AddFixedAsset::from_candidate(
                            candidate, months, opens_on,
                        );
                        match Executor::new(&mut store).execute(&cmd, &ctx) {
                            Ok(o) => declared.push(outcome_json(&o)),
                            Err(e) => {
                                body["asset_error"] = json!(e.to_string());
                                break;
                            }
                        }
                    }
                    if !declared.is_empty() {
                        body["assets"] = json!(declared);
                    }
                }
                ok_json(body)
            }
            Err(e) => err_text(e.to_string()),
        }
    }

    #[tool(
        name = "fiscal.delete_opening_balance",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false
        )
    )]
    async fn fiscal_delete_opening_balance(
        &self,
        Parameters(args): Parameters<OpeningBalanceMutationArgs>,
    ) -> CallToolResult {
        let mut store = self.store.lock().await;
        let existing = match opening_balance(store.connection()) {
            Ok(Some(existing)) => existing,
            Ok(None) => return err_text("aucun bilan d'ouverture enregistré"),
            Err(e) => return err_text(e.to_string()),
        };
        let cmd = DeleteOpeningBalance {
            revision: existing.revision,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

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
        let today = match args.today.as_deref() {
            None => freeflow_core::clock::today_local(),
            Some(raw) => ok_or_return!("today", freeflow_core::domain::parse_date(raw)),
        };
        let cmd = CloseFiscalYear {
            starts_on,
            ends_on,
            legal_reserve: Money::from_cents(args.legal_reserve_cents),
            dividends: Money::from_cents(args.dividends_cents),
            carry_back: args.carry_back,
            today: Some(today),
            non_deductible_expenses: Money::from_cents(args.non_deductible_expenses_cents),
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

    /// Approuve un exercice (date de l'AG, ni avant la clôture ni dans le futur) — il devient
    /// immuable ; une approbation au-delà des six mois légaux est acceptée mais rapportée
    /// (`late_by_days`). Acte à portée juridique : l'appel dépose une action en attente qu'un
    /// humain doit confirmer (`freeflow confirm <id>`, qui écrit d'abord une sauvegarde du
    /// coffre), jamais un effet direct.
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
        let today = match args.today.as_deref() {
            None => freeflow_core::clock::today_local(),
            Some(raw) => ok_or_return!("today", freeflow_core::domain::parse_date(raw)),
        };
        let cmd = ApproveFiscalYear {
            id: record.id,
            revision: record.revision,
            approved_on,
            today: Some(today),
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

    /// Parcours de clôture guidé de l'exercice clos dans `period` : où en est la clôture
    /// (`stage` : not_ended, blocked, ready, draft, approved), les étapes par phase (préparer,
    /// clore, affecter et approuver, déclarer et déposer) avec leur statut — `blocked` empêche
    /// la clôture, `warning` mérite attention, `todo` est le prochain geste, `later` n'est pas
    /// encore atteignable — le résultat figé ou prévisionnel, la dotation minimale à la réserve
    /// légale (art. L232-10) et le report en arrière possible. Lecture seule : les gestes
    /// passent par fiscal.close_year, fiscal.amend_year, fiscal.approve_year,
    /// fiscal.set_opening_balance, company.set_profile, expense.*, bank.*. Montants en centimes.
    /// Le vocabulaire des étapes est expliqué sans jargon par la ressource
    /// `freeflow://closing-glossary`, à reprendre auprès d'un utilisateur non comptable.
    #[tool(
        name = "fiscal.checklist",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn fiscal_checklist(
        &self,
        Parameters(args): Parameters<ChecklistArgs>,
    ) -> CallToolResult {
        let today = match args.today.as_deref() {
            None => freeflow_core::clock::today_local(),
            Some(raw) => ok_or_return!("today", freeflow_core::domain::parse_date(raw)),
        };
        let store = self.store.lock().await;
        match closing_checklist(store.connection(), args.period, today) {
            Ok(checklist) => ok_json(checklist_json(&checklist)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Balance des comptes et bilan simplifié (2033-A) dérivés du grand livre de l'exercice clos
    /// dans `period` — clos ou non : à-nouveaux (bilan d'ouverture, ou bilan de clôture dérivé
    /// de l'exercice clos précédent), ventes, achats, banque, opérations de clôture
    /// (rémunération du dirigeant, IS, affectation du résultat). Montants en centimes.
    #[tool(
        name = "fiscal.balance_sheet",
        annotations(read_only_hint = true, idempotent_hint = true)
    )]
    async fn fiscal_balance_sheet(
        &self,
        Parameters(args): Parameters<YearRefArgs>,
    ) -> CallToolResult {
        let store = self.store.lock().await;
        match ledger_ending_in(store.connection(), args.period) {
            Ok((_, ledger)) => ok_json(balance_json(
                &ledger.trial_balance(),
                &ledger.balance_sheet(),
            )),
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
        let mut store = self.store.lock().await;
        if args.doc == "balance_sheet" || args.doc == "inventory" {
            // Dérivé du grand livre : pas besoin d'un exercice clos, comme `fec.export`.
            let (profile, ledger) =
                ok_or_return!("ledger", ledger_ending_in(store.connection(), args.period));
            let bytes = if args.doc == "inventory" {
                ok_or_return!(
                    "inventory",
                    freeflow_docs::render_inventory(&profile, &ledger.trial_balance())
                )
            } else {
                ok_or_return!(
                    "balance_sheet",
                    freeflow_docs::render_balance_sheet(
                        &profile,
                        &ledger.balance_sheet(),
                        &ledger.trial_balance()
                    )
                )
            };
            let written = match write_new_document(&args.out, &bytes) {
                Ok(msg) => msg,
                Err(e) => return err_text(e),
            };
            let _ = freeflow_cli::capture_year(&mut store, &self.ctx(false), args.period);
            return ok_json(json!({ "written": written }));
        }
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
                let result = record.accounting_result();
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
                let ledger =
                    ok_or_return!("ledger", build_ledger(store.connection(), record.period()));
                let export = freeflow_docs::liasse_export(&profile, &record, Some(&ledger));
                let mut bytes = ok_or_return!("liasse", serde_json::to_vec_pretty(&export));
                bytes.push(b'\n');
                bytes
            }
            "efi_notice" => {
                let ledger =
                    ok_or_return!("ledger", build_ledger(store.connection(), record.period()));
                let export = freeflow_docs::liasse_export(&profile, &record, Some(&ledger));
                ok_or_return!("efi_notice", freeflow_docs::render_efi_notice(&export))
            }
            other => {
                return err_text(format!(
                    "document inconnu : {other} (attendu : minutes, appropriation, synthesis, \
                     balance_sheet, inventory, efi_notice, liasse)"
                ));
            }
        };
        let written = match write_new_document(&args.out, &bytes) {
            Ok(msg) => msg,
            Err(e) => return err_text(e),
        };
        let _ = freeflow_cli::capture_year(&mut store, &self.ctx(false), args.period);
        ok_json(json!({ "written": written }))
    }

    /// Immobilisations déclarées, avec la dotation, le cumul et la valeur nette de `period`
    /// (année civile de clôture, défaut : exercice contenant aujourd'hui).
    #[tool(
        name = "fiscal.assets",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn fiscal_assets(&self, Parameters(args): Parameters<AssetsListArgs>) -> CallToolResult {
        let store = self.store.lock().await;
        let fye = company_profile(store.connection())
            .ok()
            .flatten()
            .and_then(|p| p.fiscal_year_end)
            .unwrap_or(FiscalYearEnd::CALENDAR);
        let fy = match args.period {
            Some(year) => fye.containing(fye.end_in_year(year)),
            None => fye.containing(freeflow_core::clock::today_local()),
        };
        match freeflow_core::fixed_assets::list_fixed_assets(store.connection()) {
            Ok(assets) => ok_json(
                assets
                    .iter()
                    .map(|a| freeflow_core::fixed_assets::asset_json(a, fy))
                    .collect::<Vec<_>>(),
            ),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Déclare une immobilisation (reprise de bilan ou dépense matériel à immobiliser). `base`
    /// d'une dépense = son net (TTC − TVA déductible), exigé exactement.
    #[tool(
        name = "fiscal.add_asset",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    async fn fiscal_add_asset(&self, Parameters(args): Parameters<AddAssetArgs>) -> CallToolResult {
        let acquired_on = ok_or_return!(
            "acquired_on",
            freeflow_core::domain::parse_date(&args.acquired_on)
        );
        let account = ok_or_return!(
            "account",
            args.account.parse::<freeflow_core::domain::AccountCode>()
        );
        let mut store = self.store.lock().await;
        let expense_id = match args.expense.as_deref() {
            None => None,
            Some(reference) => Some(ok_or_return!(
                "expense",
                crate::support::resolve_expense(&store, reference)
            )),
        };
        let cmd = freeflow_core::fixed_assets::AddFixedAsset {
            label: args.label,
            account,
            acquired_on,
            base: Money::from_cents(args.base_cents),
            duration_months: args.duration_months.unwrap_or(36),
            prior_depreciation: Money::from_cents(args.prior_depreciation_cents.unwrap_or(0)),
            expense_id,
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }

    /// Supprime une immobilisation. Refusé dès qu'un exercice clos a porté une de ses
    /// dotations. Un agent dépose une action en attente.
    #[tool(
        name = "fiscal.delete_asset",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false
        )
    )]
    async fn fiscal_delete_asset(
        &self,
        Parameters(args): Parameters<DeleteAssetArgs>,
    ) -> CallToolResult {
        let mut store = self.store.lock().await;
        let id = ok_or_return!(
            "asset",
            crate::support::resolve_fixed_asset(&store, &args.asset)
        );
        let asset = match freeflow_core::fixed_assets::fixed_asset_by_id(store.connection(), id) {
            Ok(Some(a)) => a,
            Ok(None) => return err_text("immobilisation introuvable"),
            Err(e) => return err_text(e.to_string()),
        };
        let cmd = freeflow_core::fixed_assets::DeleteFixedAsset {
            id,
            revision: args.revision.unwrap_or(asset.revision),
        };
        match Executor::new(&mut store).execute(&cmd, &self.ctx(args.dry_run)) {
            Ok(outcome) => ok_json(outcome_json(&outcome)),
            Err(e) => err_text(e.to_string()),
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct AssetsListArgs {
    /// Année civile de clôture dont afficher la dotation.
    period: Option<i32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct AddAssetArgs {
    label: String,
    /// Compte d'immobilisation (2xx : `218300`, `205000`…).
    account: String,
    /// Mise en service (`AAAA-MM-JJ`).
    acquired_on: String,
    base_cents: i64,
    duration_months: Option<u32>,
    prior_depreciation_cents: Option<i64>,
    /// Référence de la dépense à immobiliser.
    expense: Option<String>,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct DeleteAssetArgs {
    /// Référence (UUID, préfixe ou libellé).
    asset: String,
    revision: Option<i64>,
    #[serde(default)]
    dry_run: bool,
}
