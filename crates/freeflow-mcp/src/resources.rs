//! Ressources MCP `freeflow://clients` (les clients actifs) et `freeflow://clients/{référence}`
//! (fiche d'un client, contacts et références comprises) — la doctrine annoncée dans
//! `CLAUDE.md` (« ressources = requêtes ») n'avait jusqu'ici aucune ressource réelle. Une
//! lecture de ressource coûte moins de jetons à un agent qu'un appel d'outil de liste, et ne
//! laisse aucune trace dans le journal d'audit (une lecture n'en est pas une, comme toute
//! `Query` — voir `freeflow_core::app`).

use freeflow_core::clients::{
    ClientFilter, client_by_id, client_references, list_clients_with, list_contacts,
};
use freeflow_core::missions::{self, MissionFilter};
use freeflow_core::prospection::{self, OpportunityFilter};
use freeflow_core::store::Store;
use rmcp::model::{
    ErrorData as McpError, ListResourceTemplatesResult, ListResourcesResult, ReadResourceResult,
    Resource, ResourceContents, ResourceTemplate,
};
use serde_json::json;

const CLIENTS_COLLECTION_URI: &str = "freeflow://clients";
const CLIENT_DETAIL_PREFIX: &str = "freeflow://clients/";
const OPPORTUNITIES_COLLECTION_URI: &str = "freeflow://opportunities";
const OPPORTUNITY_DETAIL_PREFIX: &str = "freeflow://opportunities/";
const MISSIONS_COLLECTION_URI: &str = "freeflow://missions";
const MISSION_DETAIL_PREFIX: &str = "freeflow://missions/";
const QUOTES_COLLECTION_URI: &str = "freeflow://quotes";
const QUOTE_DETAIL_PREFIX: &str = "freeflow://quotes/";
const EXPENSES_COLLECTION_URI: &str = "freeflow://expenses";
const EXPENSE_DETAIL_PREFIX: &str = "freeflow://expenses/";
const FISCAL_YEARS_COLLECTION_URI: &str = "freeflow://fiscal-years";
const COMPANY_URI: &str = "freeflow://company";
const OPENING_BALANCE_URI: &str = "freeflow://opening-balance";
const BALANCE_SHEET_PREFIX: &str = "freeflow://balance-sheet/";
const CLOSING_CHECKLIST_PREFIX: &str = "freeflow://closing-checklist/";

pub(crate) fn list() -> ListResourcesResult {
    ListResourcesResult::with_all_items(vec![
        Resource::new(CLIENTS_COLLECTION_URI, "clients")
            .with_description(
                "Clients actifs, en JSON — voir l'outil clients.list pour inclure les archivés.",
            )
            .with_mime_type("application/json"),
        Resource::new(OPPORTUNITIES_COLLECTION_URI, "opportunities")
            .with_description(
                "Pipeline : opportunités ouvertes et non archivées — voir prospect.list pour \
                 élargir aux closes/archivées.",
            )
            .with_mime_type("application/json"),
        Resource::new(MISSIONS_COLLECTION_URI, "missions")
            .with_description(
                "Missions en cours et non archivées — voir mission.list pour élargir aux \
                 clôturées/archivées.",
            )
            .with_mime_type("application/json"),
        Resource::new(QUOTES_COLLECTION_URI, "quotes")
            .with_description(
                "Devis, toutes versions confondues, les plus récents d'abord — lignes comprises.",
            )
            .with_mime_type("application/json"),
        Resource::new(EXPENSES_COLLECTION_URI, "expenses")
            .with_description("Dépenses professionnelles, les plus récentes d'abord.")
            .with_mime_type("application/json"),
        Resource::new(FISCAL_YEARS_COLLECTION_URI, "fiscal-years")
            .with_description(
                "Exercices clos (snapshot du résultat, affectation, approbation), du plus \
                 ancien au plus récent.",
            )
            .with_mime_type("application/json"),
        Resource::new(COMPANY_URI, "company")
            .with_description(
                "Identité légale de l'émetteur (mentions obligatoires des factures), ou null si \
                 aucun profil n'est défini — voir company.set_profile.",
            )
            .with_mime_type("application/json"),
        Resource::new(OPENING_BALANCE_URI, "opening-balance")
            .with_description(
                "Bilan d'ouverture (reprise du dernier bilan tenu avant FreeFlow) : lignes, \
                 totaux, capitaux propres repris — ou null s'il n'est pas enregistré.",
            )
            .with_mime_type("application/json"),
    ])
}

pub(crate) fn list_templates() -> ListResourceTemplatesResult {
    ListResourceTemplatesResult::with_all_items(vec![
        ResourceTemplate::new(format!("{CLIENT_DETAIL_PREFIX}{{reference}}"), "client")
            .with_description(
                "Fiche d'un client (UUID, préfixe d'UUID, ou nom — voir freeflow_core::reference), \
                 avec ses contacts et ce qui le référence.",
            )
            .with_mime_type("application/json"),
        ResourceTemplate::new(
            format!("{OPPORTUNITY_DETAIL_PREFIX}{{reference}}"),
            "opportunity",
        )
        .with_description(
            "Fiche d'une opportunité (UUID, préfixe d'UUID, ou nom), avec ses interactions \
                 et ce qui la référence.",
        )
        .with_mime_type("application/json"),
        ResourceTemplate::new(format!("{MISSION_DETAIL_PREFIX}{{reference}}"), "mission")
            .with_description(
                "Fiche d'une mission (UUID, préfixe d'UUID, ou nom), avec ses saisies de temps, \
                 son échéancier de facturation et ce qui la référence.",
            )
            .with_mime_type("application/json"),
        ResourceTemplate::new(format!("{QUOTE_DETAIL_PREFIX}{{reference}}"), "quote")
            .with_description(
                "Fiche d'un devis (UUID, préfixe d'UUID, ou nom du client porteur), avec son \
                 total HT net de remise et sa lignée (mission issue, versions).",
            )
            .with_mime_type("application/json"),
        ResourceTemplate::new(format!("{EXPENSE_DETAIL_PREFIX}{{reference}}"), "expense")
            .with_description("Fiche d'une dépense (UUID, préfixe d'UUID, ou libellé).")
            .with_mime_type("application/json"),
        ResourceTemplate::new(format!("{BALANCE_SHEET_PREFIX}{{period}}"), "balance-sheet")
            .with_description(
                "Balance des comptes et bilan simplifié (2033-A) dérivés du grand livre de \
                 l'exercice clos dans cette année civile (clos ou non) — même vue que \
                 fiscal.balance_sheet.",
            )
            .with_mime_type("application/json"),
        ResourceTemplate::new(
            format!("{CLOSING_CHECKLIST_PREFIX}{{period}}"),
            "closing-checklist",
        )
        .with_description(
            "Parcours de clôture guidé de l'exercice clos dans cette année civile, vu du jour : \
             étapes par phase avec statut, résultat, réserve légale minimale — même vue que \
             fiscal.checklist.",
        )
        .with_mime_type("application/json"),
    ])
}

fn json_contents(uri: &str, value: impl serde::Serialize) -> Result<ReadResourceResult, McpError> {
    let text = serde_json::to_string_pretty(&value)
        .map_err(|e| McpError::resource_not_found(e.to_string(), None))?;
    Ok(ReadResourceResult::new(vec![
        ResourceContents::text(text, uri).with_mime_type("application/json"),
    ]))
}

pub(crate) fn read(store: &Store, uri: &str) -> Result<ReadResourceResult, McpError> {
    if uri == CLIENTS_COLLECTION_URI {
        let clients = list_clients_with(store.connection(), ClientFilter::ActiveOnly)
            .map_err(|e| McpError::resource_not_found(e.to_string(), None))?;
        return json_contents(uri, clients);
    }

    if let Some(reference) = uri.strip_prefix(CLIENT_DETAIL_PREFIX) {
        let id = crate::support::resolve_client(store, reference)
            .map_err(|e| McpError::resource_not_found(e, None))?;
        let client = client_by_id(store.connection(), id)
            .map_err(|e| McpError::resource_not_found(e.to_string(), None))?
            .ok_or_else(|| {
                McpError::resource_not_found(format!("client introuvable : {id}"), None)
            })?;
        let contacts = list_contacts(store.connection(), id)
            .map_err(|e| McpError::resource_not_found(e.to_string(), None))?;
        let references = client_references(store.connection(), id)
            .map_err(|e| McpError::resource_not_found(e.to_string(), None))?;
        return json_contents(
            uri,
            json!({ "client": client, "contacts": contacts, "references": references }),
        );
    }

    // Testé avant le `strip_prefix` du template, comme pour `clients` ci-dessus : une URI de
    // collection ne doit jamais être interprétée comme un préfixe de détail.
    if uri == OPPORTUNITIES_COLLECTION_URI {
        let opportunities =
            prospection::list_opportunities_with(store.connection(), OpportunityFilter::OPEN)
                .map_err(|e| McpError::resource_not_found(e.to_string(), None))?;
        return json_contents(uri, opportunities);
    }
    if let Some(reference) = uri.strip_prefix(OPPORTUNITY_DETAIL_PREFIX) {
        let id = crate::support::resolve_opportunity(store, reference)
            .map_err(|e| McpError::resource_not_found(e, None))?;
        let opportunity = prospection::opportunity_by_id(store.connection(), id)
            .map_err(|e| McpError::resource_not_found(e.to_string(), None))?
            .ok_or_else(|| {
                McpError::resource_not_found(format!("opportunité introuvable : {id}"), None)
            })?;
        let interactions = prospection::list_interactions(store.connection(), id)
            .map_err(|e| McpError::resource_not_found(e.to_string(), None))?;
        let references = prospection::opportunity_references(store.connection(), id)
            .map_err(|e| McpError::resource_not_found(e.to_string(), None))?;
        return json_contents(
            uri,
            json!({ "opportunity": opportunity, "interactions": interactions, "references": references }),
        );
    }

    if uri == MISSIONS_COLLECTION_URI {
        let missions = missions::list_missions_with(store.connection(), MissionFilter::ACTIVE)
            .map_err(|e| McpError::resource_not_found(e.to_string(), None))?;
        return json_contents(uri, missions);
    }
    if let Some(reference) = uri.strip_prefix(MISSION_DETAIL_PREFIX) {
        let id = crate::support::resolve_mission(store, reference)
            .map_err(|e| McpError::resource_not_found(e, None))?;
        let mission = missions::mission_by_id(store.connection(), id)
            .map_err(|e| McpError::resource_not_found(e.to_string(), None))?
            .ok_or_else(|| {
                McpError::resource_not_found(format!("mission introuvable : {id}"), None)
            })?;
        let time_entries = missions::list_time_entries(store.connection(), id)
            .map_err(|e| McpError::resource_not_found(e.to_string(), None))?;
        let schedule = missions::billing_schedule(&mission);
        let effective_daily_rate_cents = missions::effective_daily_rate(store.connection(), id)
            .map_err(|e| McpError::resource_not_found(e.to_string(), None))?
            .map(freeflow_core::domain::Money::cents);
        let references = missions::mission_references(store.connection(), id)
            .map_err(|e| McpError::resource_not_found(e.to_string(), None))?;
        return json_contents(
            uri,
            json!({
                "mission": mission,
                "time_entries": time_entries,
                "schedule": schedule_json(&schedule),
                "effective_daily_rate_cents": effective_daily_rate_cents,
                "references": references,
            }),
        );
    }

    if uri == QUOTES_COLLECTION_URI {
        let quotes = freeflow_core::quotes::list_quotes(store.connection())
            .map_err(|e| McpError::resource_not_found(e.to_string(), None))?;
        return json_contents(uri, quotes);
    }
    if let Some(reference) = uri.strip_prefix(QUOTE_DETAIL_PREFIX) {
        let id = crate::support::resolve_quote(store, reference)
            .map_err(|e| McpError::resource_not_found(e, None))?;
        let quote = freeflow_core::quotes::quote_by_id(store.connection(), id)
            .map_err(|e| McpError::resource_not_found(e.to_string(), None))?
            .ok_or_else(|| {
                McpError::resource_not_found(format!("devis introuvable : {id}"), None)
            })?;
        let references = freeflow_core::quotes::quote_references(store.connection(), id)
            .map_err(|e| McpError::resource_not_found(e.to_string(), None))?;
        let total_net_ht: freeflow_core::domain::Money =
            freeflow_core::quotes::priced_lines(&quote.lines, quote.discount)
                .iter()
                .map(|(gross, discount)| *gross - *discount)
                .sum();
        return json_contents(
            uri,
            json!({ "quote": quote, "total_net_ht": total_net_ht, "references": references }),
        );
    }

    if uri == EXPENSES_COLLECTION_URI {
        let expenses = freeflow_core::expenses::list_expenses(store.connection())
            .map_err(|e| McpError::resource_not_found(e.to_string(), None))?;
        return json_contents(uri, expenses);
    }
    if let Some(reference) = uri.strip_prefix(EXPENSE_DETAIL_PREFIX) {
        let id = crate::support::resolve_expense(store, reference)
            .map_err(|e| McpError::resource_not_found(e, None))?;
        let expense = freeflow_core::expenses::expense_detail(store.connection(), id)
            .map_err(|e| McpError::resource_not_found(e.to_string(), None))?
            .ok_or_else(|| {
                McpError::resource_not_found(format!("dépense introuvable : {id}"), None)
            })?;
        return json_contents(uri, json!({ "expense": expense }));
    }

    if uri == FISCAL_YEARS_COLLECTION_URI {
        let years = freeflow_core::fiscal_year::list_fiscal_years(store.connection())
            .map_err(|e| McpError::resource_not_found(e.to_string(), None))?;
        return json_contents(
            uri,
            years
                .iter()
                .map(crate::tools::fiscal::year_json)
                .collect::<Vec<_>>(),
        );
    }

    if uri == OPENING_BALANCE_URI {
        let opening = freeflow_core::opening_balance::opening_balance(store.connection())
            .map_err(|e| McpError::resource_not_found(e.to_string(), None))?;
        return json_contents(
            uri,
            opening
                .as_ref()
                .map_or(serde_json::Value::Null, crate::tools::fiscal::opening_json),
        );
    }

    if let Some(period) = uri.strip_prefix(CLOSING_CHECKLIST_PREFIX) {
        let period: i32 = period.parse().map_err(|_| {
            McpError::resource_not_found(
                format!("période invalide : {period} (attendu AAAA)"),
                None,
            )
        })?;
        let today = time::OffsetDateTime::now_utc().date();
        let checklist =
            freeflow_core::closing::closing_checklist(store.connection(), period, today)
                .map_err(|e| McpError::resource_not_found(e.to_string(), None))?;
        return json_contents(uri, freeflow_core::closing::checklist_json(&checklist));
    }

    if let Some(period) = uri.strip_prefix(BALANCE_SHEET_PREFIX) {
        let period: i32 = period.parse().map_err(|_| {
            McpError::resource_not_found(
                format!("période invalide : {period} (attendu AAAA)"),
                None,
            )
        })?;
        let (_, ledger) = freeflow_core::ledger::ledger_ending_in(store.connection(), period)
            .map_err(|e| McpError::resource_not_found(e.to_string(), None))?;
        return json_contents(
            uri,
            freeflow_core::ledger::balance_json(&ledger.trial_balance(), &ledger.balance_sheet()),
        );
    }
    if uri == COMPANY_URI {
        let today = time::OffsetDateTime::now_utc().date();
        let profile =
            freeflow_core::fiscal::company_profile_with_vat_filing(store.connection(), today)
                .map_err(|e| McpError::resource_not_found(e.to_string(), None))?;
        return json_contents(uri, profile);
    }

    Err(McpError::resource_not_found(
        format!("ressource inconnue : {uri}"),
        None,
    ))
}

fn schedule_json(schedule: &missions::BillingSchedule) -> serde_json::Value {
    match schedule {
        missions::BillingSchedule::Regie { daily_rate } => {
            json!({"kind": "regie", "daily_rate_cents": daily_rate.cents()})
        }
        missions::BillingSchedule::Recurrent { monthly_amount } => {
            json!({"kind": "recurrent", "monthly_amount_cents": monthly_amount.cents()})
        }
        missions::BillingSchedule::Forfait { installments } => json!({
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
