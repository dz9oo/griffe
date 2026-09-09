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
const CLOSING_GLOSSARY_URI: &str = "freeflow://closing-glossary";
const ASSETS_URI: &str = "freeflow://assets";
const FOLLOW_UPS_URI: &str = "freeflow://follow-ups";
const DAY_URI: &str = "freeflow://day";
const DAY_MONTH_PREFIX: &str = "freeflow://day/month/";
const PEOPLE_URI: &str = "freeflow://people";
const PEOPLE_DETAIL_PREFIX: &str = "freeflow://people/";
const SOCIETY_URI: &str = "freeflow://society";
const SOCIETY_DUTY_PREFIX: &str = "freeflow://society/duties/";
const PAPERS_URI: &str = "freeflow://papers";

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
        Resource::new(CLOSING_GLOSSARY_URI, "closing-glossary")
            .with_description(
                "Lexique de la clôture (lot 35) : les mots du parcours et des documents \
                 expliqués sans jargon, pour les reprendre tels quels auprès d'un utilisateur \
                 sans notion comptable — [{ term, meaning }].",
            )
            .with_mime_type("application/json"),
        Resource::new(OPENING_BALANCE_URI, "opening-balance")
            .with_description(
                "Bilan d'ouverture (reprise du dernier bilan tenu avant FreeFlow) : lignes, \
                 totaux, capitaux propres repris — ou null s'il n'est pas enregistré.",
            )
            .with_mime_type("application/json"),
        Resource::new(ASSETS_URI, "assets")
            .with_description(
                "Immobilisations déclarées, avec la dotation de l'exercice en cours — même vue \
                 que fiscal.assets.",
            )
            .with_mime_type("application/json"),
        Resource::new(FOLLOW_UPS_URI, "follow-ups")
            .with_description(
                "File de relances du jour (prospects et impayés) — même vue que follow_up.queue.",
            )
            .with_mime_type("application/json"),
        Resource::new(DAY_URI, "day")
            .with_description(
                "Le jour : mât et gestes (même vue que day.mast + day.gestures). today = horloge \
                 locale de l'adaptateur.",
            )
            .with_mime_type("application/json"),
        Resource::new(PEOPLE_URI, "people")
            .with_description(
                "Les affaires : trois chapitres (en conversation, en mission, fournisseurs) — même \
                 vue que people.list.",
            )
            .with_mime_type("application/json"),
        Resource::new(SOCIETY_URI, "society")
            .with_description(
                "La société : identité courte, paysage, conversations, chapitres — même vue que \
                 society.show.",
            )
            .with_mime_type("application/json"),
        Resource::new(PAPERS_URI, "papers")
            .with_description(
                "Les papiers : pièces actives du coffre (hors remplacées) — même vue que \
                 papers.list.",
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
        ResourceTemplate::new(format!("{DAY_MONTH_PREFIX}{{month}}"), "day-month")
            .with_description(
                "Événements et bandes de missions d'un mois civil (`AAAA-MM`) — même vue que \
                 day.month.",
            )
            .with_mime_type("application/json"),
        ResourceTemplate::new(format!("{PEOPLE_DETAIL_PREFIX}{{reference}}"), "person")
            .with_description(
                "Dossier d'une personne (nom, préfixe, UUID) : en cours, projet, papiers, \
                 histoire — même vue que people.show.",
            )
            .with_mime_type("application/json"),
        ResourceTemplate::new(format!("{SOCIETY_DUTY_PREFIX}{{kind}}"), "society-duty")
            .with_description(
                "Lettre d'une démarche hors de l'app (`is_acompte`, `ca3`, …) — même vue que \
                 society.duty.",
            )
            .with_mime_type("application/json"),
    ])
}

fn parse_month_uri(s: &str) -> Result<freeflow_core::domain::Month, String> {
    let (year_str, month_str) = s
        .split_once('-')
        .ok_or_else(|| format!("mois invalide : {s} (attendu AAAA-MM)"))?;
    let year: i32 = year_str
        .parse()
        .map_err(|_| format!("mois invalide : {s} (attendu AAAA-MM)"))?;
    let month: u8 = month_str
        .parse()
        .map_err(|_| format!("mois invalide : {s} (attendu AAAA-MM)"))?;
    freeflow_core::domain::Month::new(year, month).map_err(|e| e.to_string())
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

    if uri == CLOSING_GLOSSARY_URI {
        return json_contents(uri, freeflow_core::closing::glossary_json());
    }

    if uri == FOLLOW_UPS_URI {
        let today = freeflow_core::clock::today_local();
        let cards = freeflow_core::follow_up::follow_up_queue(store.connection(), today)
            .map_err(|e| McpError::resource_not_found(e.to_string(), None))?;
        return json_contents(uri, cards);
    }

    if uri == DAY_URI {
        let today = freeflow_core::clock::today_local();
        let mast = freeflow_core::day::day_mast(store.connection(), today)
            .map_err(|e| McpError::resource_not_found(e.to_string(), None))?;
        let gestures = freeflow_core::day::day_gestures(store.connection(), today)
            .map_err(|e| McpError::resource_not_found(e.to_string(), None))?;
        return json_contents(uri, json!({ "mast": mast, "gestures": gestures }));
    }
    if uri == SOCIETY_URI {
        let today = freeflow_core::clock::today_local();
        let home = freeflow_core::society::society_home(store.connection(), today)
            .map_err(|e| McpError::resource_not_found(e.to_string(), None))?;
        return json_contents(uri, home);
    }
    if let Some(kind_raw) = uri.strip_prefix(SOCIETY_DUTY_PREFIX) {
        let kind = freeflow_core::fiscal::FiscalDeadlineKind::parse(kind_raw).ok_or_else(|| {
            McpError::resource_not_found(format!("démarche inconnue : {kind_raw}"), None)
        })?;
        let today = freeflow_core::clock::today_local();
        let briefing = freeflow_core::society::duty_briefing(store.connection(), kind, today, None)
            .map_err(|e| McpError::resource_not_found(e.to_string(), None))?;
        return json_contents(uri, briefing);
    }
    if uri == PEOPLE_URI {
        let today = freeflow_core::clock::today_local();
        let list = freeflow_core::people::people_list(store.connection(), today)
            .map_err(|e| McpError::resource_not_found(e.to_string(), None))?;
        return json_contents(uri, list);
    }
    if let Some(reference) = uri.strip_prefix(PEOPLE_DETAIL_PREFIX) {
        let today = freeflow_core::clock::today_local();
        let dossier = freeflow_core::people::person(store.connection(), reference, today)
            .map_err(|e| McpError::resource_not_found(e.to_string(), None))?;
        return json_contents(uri, dossier);
    }
    if let Some(raw) = uri.strip_prefix(DAY_MONTH_PREFIX) {
        let today = freeflow_core::clock::today_local();
        let month = parse_month_uri(raw).map_err(|e| McpError::resource_not_found(e, None))?;
        let view = freeflow_core::day::day_month(store.connection(), month, today)
            .map_err(|e| McpError::resource_not_found(e.to_string(), None))?;
        return json_contents(uri, view);
    }

    if uri == PAPERS_URI {
        let papers = freeflow_core::papers::list_papers(
            store.connection(),
            freeflow_core::papers::PaperFilter::ACTIVE,
        )
        .map_err(|e| McpError::resource_not_found(e.to_string(), None))?;
        return json_contents(uri, papers);
    }

    if uri == ASSETS_URI {
        let fye = freeflow_core::company::company_profile(store.connection())
            .ok()
            .flatten()
            .and_then(|p| p.fiscal_year_end)
            .unwrap_or(freeflow_core::domain::FiscalYearEnd::CALENDAR);
        let fy = fye.containing(freeflow_core::clock::today_local());
        let assets = freeflow_core::fixed_assets::list_fixed_assets(store.connection())
            .map_err(|e| McpError::resource_not_found(e.to_string(), None))?;
        return json_contents(
            uri,
            assets
                .iter()
                .map(|a| freeflow_core::fixed_assets::asset_json(a, fy))
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
        let today = freeflow_core::clock::today_local();
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
        let today = freeflow_core::clock::today_local();
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
