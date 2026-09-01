//! Résolution de références lisibles par un humain vers les identifiants du domaine — un UUID
//! complet, un préfixe d'UUID (comme un hash git court), ou un nom (insensible à la casse et aux
//! accents, exact puis par préfixe). C'est ce qui évite à chaque façade d'inventer sa propre
//! notion de « désigner un client par son nom » : la CLI, le serveur MCP et la GUI appellent
//! toutes ce module plutôt que d'exiger un UUID complet pour chaque référence.
//!
//! Lot 16 : `resolve_client` a été réécrit au-dessus d'un cœur générique ([`resolve_among`]),
//! extrait quand la prospection et les missions en ont eu besoin à leur tour —
//! `resolve_opportunity`/`resolve_mission` en sont deux enveloppes de quelques lignes.

use rusqlite::Connection;

use crate::app::AppError;
use crate::domain::{ClientId, ExpenseId, MissionId, OpportunityId, QuoteId};
use crate::{clients, expenses, missions, prospection, quotes};

/// Résultat de la résolution d'une référence texte vers un identifiant typé. `label` (dans
/// `Ambiguous`) est le libellé lisible du candidat — le nom d'un client, par exemple — pour que
/// la façade appelante puisse lister les candidats sans requête supplémentaire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefMatch<T> {
    Unique(T),
    Ambiguous(Vec<(T, String)>),
    NotFound,
}

/// Longueur minimale d'un préfixe d'UUID pour être tenté comme tel plutôt que comme un nom —
/// en-dessous, un nom purement hexadécimal (peu probable, mais possible) resterait résolu par
/// nom plutôt que pris pour un fragment d'UUID.
const MIN_UUID_PREFIX_LEN: usize = 4;

/// Replie les diacritiques latins les plus courants en français sur leur lettre de base — pas
/// une translittération Unicode complète, seulement de quoi rendre `"Societe"` et `"Société"`
/// équivalents pour une recherche par nom.
fn fold_accents(c: char) -> char {
    match c {
        'à' | 'â' | 'ä' | 'á' | 'ã' => 'a',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'î' | 'ï' | 'í' | 'ì' => 'i',
        'ô' | 'ö' | 'ó' | 'ò' | 'õ' => 'o',
        'ù' | 'û' | 'ü' | 'ú' => 'u',
        'ç' => 'c',
        'ñ' => 'n',
        other => other,
    }
}

fn normalize(s: &str) -> String {
    s.to_lowercase().chars().map(fold_accents).collect()
}

fn as_ref_match<T>(mut candidates: Vec<(T, String)>) -> RefMatch<T> {
    match candidates.len() {
        0 => RefMatch::NotFound,
        1 => {
            let (id, _) = candidates.remove(0);
            RefMatch::Unique(id)
        }
        _ => RefMatch::Ambiguous(candidates),
    }
}

/// Résout `needle` contre une liste `(id, label)` déjà chargée — inclusive par construction :
/// c'est à l'appelant de fournir la bonne liste (toutes entités confondues, closes/archivées
/// comprises), ce module ne fait ici aucune requête.
///
/// Ordre de résolution : UUID complet (par appartenance à `candidates`) ; sinon, si `needle` est
/// un fragment hexadécimal d'au moins [`MIN_UUID_PREFIX_LEN`] caractères, préfixe d'UUID (à la
/// manière d'un hash git court) ; sinon libellé exact, insensible à la casse et aux accents ;
/// sinon préfixe de libellé, avec la même tolérance.
fn resolve_among<T>(needle: &str, candidates: &[(T, String)]) -> RefMatch<T>
where
    T: Copy + Eq + std::fmt::Display + std::str::FromStr,
{
    let trimmed = needle.trim();

    if let Ok(id) = trimmed.parse::<T>() {
        return if candidates.iter().any(|(cid, _)| *cid == id) {
            RefMatch::Unique(id)
        } else {
            RefMatch::NotFound
        };
    }

    if trimmed.len() >= MIN_UUID_PREFIX_LEN && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
        let prefix = trimmed.to_ascii_lowercase();
        let by_id_prefix: Vec<(T, String)> = candidates
            .iter()
            .filter(|(id, _)| id.to_string().starts_with(&prefix))
            .cloned()
            .collect();
        if !by_id_prefix.is_empty() {
            return as_ref_match(by_id_prefix);
        }
    }

    let needle_norm = normalize(trimmed);
    let by_exact_label: Vec<(T, String)> = candidates
        .iter()
        .filter(|(_, label)| normalize(label) == needle_norm)
        .cloned()
        .collect();
    if !by_exact_label.is_empty() {
        return as_ref_match(by_exact_label);
    }

    let by_label_prefix: Vec<(T, String)> = candidates
        .iter()
        .filter(|(_, label)| normalize(label).starts_with(&needle_norm))
        .cloned()
        .collect();
    as_ref_match(by_label_prefix)
}

/// Résout `needle` en identifiant de client.
///
/// # Errors
///
/// Retourne une erreur si la lecture en base échoue.
pub fn resolve_client(conn: &Connection, needle: &str) -> Result<RefMatch<ClientId>, AppError> {
    let candidates: Vec<(ClientId, String)> = clients::list_clients(conn)?
        .into_iter()
        .map(|c| (c.id, c.name))
        .collect();
    Ok(resolve_among(needle, &candidates))
}

/// Réécrit les libellés d'un résultat `Ambiguous` — laisse `Unique`/`NotFound` inchangés. Le
/// matching se fait sur le libellé nu (le nom de l'entité) ; l'enrichissement (client, étape ou
/// date) n'est calculé que pour les candidats effectivement retournés à l'utilisateur, jamais
/// pour l'espace de recherche entier.
fn qualify_ambiguous<T: Copy>(result: RefMatch<T>, qualify: impl Fn(T) -> String) -> RefMatch<T> {
    match result {
        RefMatch::Ambiguous(candidates) => RefMatch::Ambiguous(
            candidates
                .into_iter()
                .map(|(id, _)| (id, qualify(id)))
                .collect(),
        ),
        other => other,
    }
}

/// Résout `needle` en identifiant d'opportunité, par nom (le matching se fait sur le nom nu de
/// l'opportunité). En cas d'ambiguïté, le libellé de chaque candidat est enrichi du nom du
/// client et de l'étape — les noms d'opportunités sont bien moins uniques qu'une raison sociale,
/// et ce contexte n'est construit que dans le cas effectivement ambigu, où il coûte le moins et
/// sert le plus (ex. distinguer deux « Refonte site » dont l'une est déjà `won`).
///
/// # Errors
pub fn resolve_opportunity(
    conn: &Connection,
    needle: &str,
) -> Result<RefMatch<OpportunityId>, AppError> {
    let all_clients = clients::list_clients(conn)?;
    let opportunities = prospection::list_opportunities(conn)?;
    let client_name = |id: ClientId| {
        all_clients
            .iter()
            .find(|c| c.id == id)
            .map_or_else(|| "?".to_string(), |c| c.name.clone())
    };

    let candidates: Vec<(OpportunityId, String)> = opportunities
        .iter()
        .map(|o| (o.id, o.name.clone()))
        .collect();
    let result = resolve_among(needle, &candidates);
    Ok(qualify_ambiguous(result, |id| {
        opportunities
            .iter()
            .find(|o| o.id == id)
            .map_or_else(String::new, |o| {
                format!(
                    "{} — {} ({})",
                    client_name(o.client_id),
                    o.name,
                    o.stage.as_str()
                )
            })
    }))
}

/// Résout `needle` en identifiant de mission — même doctrine que
/// [`resolve_opportunity`], avec la date de démarrage plutôt que l'étape comme discriminant.
///
/// # Errors
pub fn resolve_mission(conn: &Connection, needle: &str) -> Result<RefMatch<MissionId>, AppError> {
    let all_clients = clients::list_clients(conn)?;
    let all_missions = missions::list_missions(conn)?;
    let client_name = |id: ClientId| {
        all_clients
            .iter()
            .find(|c| c.id == id)
            .map_or_else(|| "?".to_string(), |c| c.name.clone())
    };

    let candidates: Vec<(MissionId, String)> = all_missions
        .iter()
        .map(|m| (m.id, m.name.clone()))
        .collect();
    let result = resolve_among(needle, &candidates);
    Ok(qualify_ambiguous(result, |id| {
        all_missions
            .iter()
            .find(|m| m.id == id)
            .map_or_else(String::new, |m| {
                format!(
                    "{} — {} (depuis {})",
                    client_name(m.client_id),
                    m.name,
                    crate::domain::format_date(m.started_on)
                )
            })
    }))
}

/// Résout `needle` en identifiant de dépense, par libellé — même doctrine que
/// [`resolve_opportunity`] : les libellés de dépense sont tout sauf uniques (« Abonnement »,
/// « Restaurant »), donc l'ambiguïté est qualifiée par la date et le montant, les deux
/// discriminants naturels d'une ligne de frais.
///
/// # Errors
pub fn resolve_expense(conn: &Connection, needle: &str) -> Result<RefMatch<ExpenseId>, AppError> {
    let all_expenses = expenses::list_expenses(conn)?;
    let candidates: Vec<(ExpenseId, String)> = all_expenses
        .iter()
        .map(|e| (e.id, e.label.clone()))
        .collect();
    let result = resolve_among(needle, &candidates);
    Ok(qualify_ambiguous(result, |id| {
        all_expenses
            .iter()
            .find(|e| e.id == id)
            .map_or_else(String::new, |e| {
                format!(
                    "{} — {} ({})",
                    e.label,
                    crate::domain::format_date(e.incurred_on),
                    e.amount
                )
            })
    }))
}

/// Résout `needle` en identifiant de devis. Un devis n'a pas de nom propre : le matching par
/// libellé se fait sur le **nom du client** (un UUID ou son préfixe restent prioritaires, comme
/// partout) — « `freeflow quote show acme` » suffit tant qu'Acme n'a qu'un devis, et l'ambiguïté
/// est qualifiée par la version, la date et le statut pour choisir parmi plusieurs.
///
/// # Errors
pub fn resolve_quote(conn: &Connection, needle: &str) -> Result<RefMatch<QuoteId>, AppError> {
    let all_clients = clients::list_clients(conn)?;
    let all_quotes = quotes::list_quotes(conn)?;
    let client_name = |id: ClientId| {
        all_clients
            .iter()
            .find(|c| c.id == id)
            .map_or_else(|| "?".to_string(), |c| c.name.clone())
    };

    let candidates: Vec<(QuoteId, String)> = all_quotes
        .iter()
        .map(|q| (q.id, client_name(q.client_id)))
        .collect();
    let result = resolve_among(needle, &candidates);
    Ok(qualify_ambiguous(result, |id| {
        all_quotes
            .iter()
            .find(|q| q.id == id)
            .map_or_else(String::new, |q| {
                format!(
                    "{} — v{}, valide jusqu'au {} ({})",
                    client_name(q.client_id),
                    q.version,
                    crate::domain::format_date(q.valid_until),
                    q.status.as_str()
                )
            })
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Actor, ExecutionContext, Executor};
    use crate::clients::CreateClient;
    use crate::store::{Passphrase, Store};

    fn test_store(label: &str) -> Store {
        let dir = std::env::temp_dir().join(format!(
            "freeflow-reference-test-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ));
        Store::create(&dir.join("vault.db"), &Passphrase::from("s3cret")).unwrap()
    }

    fn create_client(store: &mut Store, name: &str) -> ClientId {
        let cmd = CreateClient {
            name: name.to_string(),
            siren: None,
            vat_number: None,
            address: None,
        };
        let crate::app::Outcome::Applied(id) = Executor::new(store)
            .execute(&cmd, &ExecutionContext::new(Actor::Human, false))
            .unwrap()
        else {
            panic!("expected Applied")
        };
        id
    }

    #[test]
    fn resolves_a_full_uuid() {
        let mut store = test_store("full-uuid");
        let id = create_client(&mut store, "Acme");
        assert_eq!(
            resolve_client(store.connection(), &id.to_string()).unwrap(),
            RefMatch::Unique(id)
        );
    }

    #[test]
    fn a_well_formed_but_unknown_uuid_is_not_found() {
        let store = test_store("unknown-uuid");
        let unknown = ClientId::new();
        assert_eq!(
            resolve_client(store.connection(), &unknown.to_string()).unwrap(),
            RefMatch::NotFound
        );
    }

    #[test]
    fn resolves_a_short_uuid_prefix() {
        let mut store = test_store("uuid-prefix");
        let id = create_client(&mut store, "Acme");
        let prefix = &id.to_string()[..8];
        assert_eq!(
            resolve_client(store.connection(), prefix).unwrap(),
            RefMatch::Unique(id)
        );
    }

    #[test]
    fn resolves_by_exact_name_ignoring_case_and_accents() {
        let mut store = test_store("exact-name");
        let id = create_client(&mut store, "Société Générale");
        assert_eq!(
            resolve_client(store.connection(), "societe generale").unwrap(),
            RefMatch::Unique(id)
        );
    }

    #[test]
    fn resolves_by_name_prefix() {
        let mut store = test_store("name-prefix");
        let id = create_client(&mut store, "Argon Digital");
        assert_eq!(
            resolve_client(store.connection(), "argon").unwrap(),
            RefMatch::Unique(id)
        );
    }

    #[test]
    fn an_ambiguous_name_prefix_lists_every_candidate() {
        let mut store = test_store("ambiguous");
        let a = create_client(&mut store, "Argon Digital");
        let b = create_client(&mut store, "Argon Studio");
        let result = resolve_client(store.connection(), "argon").unwrap();
        let RefMatch::Ambiguous(mut candidates) = result else {
            panic!("expected Ambiguous, got a unique or absent match")
        };
        candidates.sort_by_key(|(id, _)| *id);
        let mut expected = vec![
            (a, "Argon Digital".to_string()),
            (b, "Argon Studio".to_string()),
        ];
        expected.sort_by_key(|(id, _)| *id);
        assert_eq!(candidates, expected);
    }

    #[test]
    fn an_unmatched_name_is_not_found() {
        let mut store = test_store("not-found");
        create_client(&mut store, "Acme");
        assert_eq!(
            resolve_client(store.connection(), "totally-unknown").unwrap(),
            RefMatch::NotFound
        );
    }

    fn create_opportunity(store: &mut Store, client_id: ClientId, name: &str) -> OpportunityId {
        let cmd = crate::prospection::CreateOpportunity {
            client_id,
            name: name.to_string(),
            amount: crate::domain::Money::from_cents(1_000_000),
            probability: crate::domain::Probability::new(50).unwrap(),
            next_action_at: time::Date::from_calendar_date(2026, time::Month::September, 1)
                .unwrap(),
            source: None,
        };
        let crate::app::Outcome::Applied(id) = Executor::new(store)
            .execute(&cmd, &ExecutionContext::new(Actor::Human, false))
            .unwrap()
        else {
            panic!("expected Applied")
        };
        id
    }

    #[test]
    fn resolves_an_opportunity_by_name() {
        let mut store = test_store("opportunity-by-name");
        let client_id = create_client(&mut store, "Argon Digital");
        let id = create_opportunity(&mut store, client_id, "Refonte plateforme");
        assert_eq!(
            resolve_opportunity(store.connection(), "refonte plateforme").unwrap(),
            RefMatch::Unique(id)
        );
    }

    #[test]
    fn an_ambiguous_opportunity_name_lists_candidates_qualified_by_client_and_stage() {
        let mut store = test_store("opportunity-ambiguous");
        let client_id = create_client(&mut store, "Argon Digital");
        let a = create_opportunity(&mut store, client_id, "Refonte");
        let b = create_opportunity(&mut store, client_id, "Refonte");
        let result = resolve_opportunity(store.connection(), "refonte").unwrap();
        let RefMatch::Ambiguous(candidates) = result else {
            panic!("expected Ambiguous")
        };
        let ids: Vec<OpportunityId> = candidates.iter().map(|(id, _)| *id).collect();
        assert!(ids.contains(&a) && ids.contains(&b));
        assert!(
            candidates.iter().all(
                |(_, label)| label.contains("Argon Digital") && label.contains("qualification")
            )
        );
    }

    fn create_mission(store: &mut Store, client_id: ClientId, name: &str) -> MissionId {
        let cmd = crate::missions::CreateMission {
            client_id,
            quote_id: None,
            name: name.to_string(),
            kind: crate::domain::MissionKind::Forfait {
                budget: crate::domain::Money::from_cents(1_000_000),
            },
            milestones: Vec::new(),
            started_on: time::Date::from_calendar_date(2026, time::Month::September, 1).unwrap(),
        };
        let crate::app::Outcome::Applied(id) = Executor::new(store)
            .execute(&cmd, &ExecutionContext::new(Actor::Human, false))
            .unwrap()
        else {
            panic!("expected Applied")
        };
        id
    }

    #[test]
    fn resolves_a_mission_by_name() {
        let mut store = test_store("mission-by-name");
        let client_id = create_client(&mut store, "Argon Digital");
        let id = create_mission(&mut store, client_id, "Refonte dashboard");
        assert_eq!(
            resolve_mission(store.connection(), "refonte dashboard").unwrap(),
            RefMatch::Unique(id)
        );
    }

    fn record_expense(store: &mut Store, label: &str, day: u8) -> ExpenseId {
        let cmd = crate::expenses::RecordExpense {
            label: label.to_string(),
            category: crate::domain::ExpenseCategory::Software,
            amount: crate::domain::Money::from_cents(12_000),
            vat_rate: crate::domain::VatRate::Standard,
            vat_deductible: crate::domain::Money::from_cents(2_000),
            incurred_on: time::Date::from_calendar_date(2026, time::Month::September, day).unwrap(),
            receipt_hash: None,
            receipt_filename: None,
        };
        let crate::app::Outcome::Applied(id) = Executor::new(store)
            .execute(&cmd, &ExecutionContext::new(Actor::Human, false))
            .unwrap()
        else {
            panic!("expected Applied")
        };
        id
    }

    #[test]
    fn resolves_an_expense_by_label() {
        let mut store = test_store("expense-by-label");
        let id = record_expense(&mut store, "Abonnement hébergement", 5);
        assert_eq!(
            resolve_expense(store.connection(), "abonnement").unwrap(),
            RefMatch::Unique(id)
        );
    }

    #[test]
    fn an_ambiguous_expense_label_is_qualified_by_date_and_amount() {
        let mut store = test_store("expense-ambiguous");
        let a = record_expense(&mut store, "Restaurant", 5);
        let b = record_expense(&mut store, "Restaurant", 12);
        let result = resolve_expense(store.connection(), "restaurant").unwrap();
        let RefMatch::Ambiguous(candidates) = result else {
            panic!("expected Ambiguous")
        };
        let ids: Vec<ExpenseId> = candidates.iter().map(|(id, _)| *id).collect();
        assert!(ids.contains(&a) && ids.contains(&b));
        assert!(
            candidates
                .iter()
                .any(|(_, label)| label.contains("2026-09-05"))
        );
    }

    fn create_quote(store: &mut Store, client_id: ClientId) -> crate::domain::QuoteId {
        let cmd = crate::quotes::CreateQuote {
            client_id,
            opportunity_id: None,
            lines: vec![crate::domain::QuoteLine {
                description: "Prestation".to_string(),
                kind: crate::domain::LineKind::Forfait {
                    amount: crate::domain::Money::from_cents(1_000_000),
                },
                vat_rate: crate::domain::VatRate::Standard,
            }],
            discount: None,
            terms: None,
            valid_until: time::Date::from_calendar_date(2026, time::Month::October, 31).unwrap(),
        };
        let crate::app::Outcome::Applied(id) = Executor::new(store)
            .execute(&cmd, &ExecutionContext::new(Actor::Human, false))
            .unwrap()
        else {
            panic!("expected Applied")
        };
        id
    }

    #[test]
    fn resolves_a_quote_by_client_name_and_by_uuid_prefix() {
        let mut store = test_store("quote-by-client");
        let client_id = create_client(&mut store, "Argon Digital");
        let id = create_quote(&mut store, client_id);
        assert_eq!(
            resolve_quote(store.connection(), "argon").unwrap(),
            RefMatch::Unique(id)
        );
        assert_eq!(
            resolve_quote(store.connection(), &id.to_string()[..8]).unwrap(),
            RefMatch::Unique(id)
        );
    }

    #[test]
    fn two_quotes_for_the_same_client_are_qualified_by_version_and_status() {
        let mut store = test_store("quote-ambiguous");
        let client_id = create_client(&mut store, "Argon Digital");
        let a = create_quote(&mut store, client_id);
        let b = create_quote(&mut store, client_id);
        let result = resolve_quote(store.connection(), "argon").unwrap();
        let RefMatch::Ambiguous(candidates) = result else {
            panic!("expected Ambiguous")
        };
        let ids: Vec<crate::domain::QuoteId> = candidates.iter().map(|(id, _)| *id).collect();
        assert!(ids.contains(&a) && ids.contains(&b));
        assert!(candidates.iter().all(|(_, label)| label.contains("draft")));
    }
}
