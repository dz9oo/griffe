//! Les chiffres attendus sont posés à la main. Chaque test ouvre un vrai coffre.

use time::Date;
use time::macros::date;

use crate::app::{Actor, ExecutionContext, Executor, Outcome};
use crate::clients::{self, CreateClient};
use crate::domain::{
    Civility, FollowUpFact, FollowUpSubject, LossReason, Money, OpportunityStage, Probability,
    Siren, VatNumber, active_events,
};
use crate::dossier_work;
use crate::exchange::{
    AddressFile, ApplyExchange, CadenceMoment, ContactFile, DossierFile, ExchangeDocument,
    ExchangeReport, ExchangeStatus, LineFile, SeriesFile, SeriesMoment, preview,
};
use crate::follow_up::bind::phrase_genre_named;
use crate::follow_up::{
    SetFollowUpDate, SetFollowUpSender, events_for, phrases_for_genre, prospect_phrases,
};
use crate::prospection::{CreateProspect, LoseOpportunity, opportunity_by_id};
use crate::store::Store;
use crate::store::testing::test_store;

const TODAY: Date = date!(2026 - 10 - 28);

const fn human() -> ExecutionContext {
    ExecutionContext::new(Actor::Human, false)
}

fn apply(store: &mut Store, document: &ExchangeDocument) -> ExchangeReport {
    let command = ApplyExchange {
        document: document.clone(),
        today: TODAY,
    };
    match Executor::new(store).execute(&command, &human()).unwrap() {
        Outcome::Applied(report) => report,
        other => panic!("application attendue, reçu {other:?}"),
    }
}

fn rows(store: &Store, sql: &str) -> i64 {
    store
        .connection()
        .query_row(sql, [], |row| row.get(0))
        .unwrap()
}

fn document(dossiers: Vec<DossierFile>) -> ExchangeDocument {
    ExchangeDocument {
        version: 1,
        stop: false,
        dossiers,
        series: Vec::new(),
        cadence: None,
    }
}

fn valvert() -> DossierFile {
    DossierFile {
        origine: "opp-valvert".to_string(),
        origine_fiche: Some("org-valvert".to_string()),
        qui: "Mairie de Valvert".to_string(),
        quoi: "Refonte du site".to_string(),
        reprendre: false,
        adresse: Some(AddressFile {
            rue: "1 place de la Mairie".to_string(),
            code_postal: "59171".to_string(),
            ville: "Valvert".to_string(),
            pays: Some("FR".to_string()),
        }),
        courriel: Some("mairie@valvert.example".to_string()),
        telephone: Some("0327444444".to_string()),
        siren: Some("552100554".to_string()),
        tva: Some("FR96552100554".to_string()),
        types: None,
        travaux: Some("Le site n'est pas conforme.".to_string()),
        contact: Some(ContactFile {
            prenom: "Camille".to_string(),
            nom: "Rivière".to_string(),
            genre: Some("madame".to_string()),
            courriel: Some("camille@valvert.example".to_string()),
            telephone: None,
            role: None,
        }),
        probabilite: Some(40),
        prochaine_action: Some("2026-10-28".to_string()),
        source: Some("Autre".to_string()),
        lignes: Some(vec![
            LineFile {
                libelle: "Diagnostic".to_string(),
                montant: "1200,00".to_string(),
            },
            LineFile {
                libelle: "Atelier".to_string(),
                montant: "400,00".to_string(),
            },
        ]),
        serie: None,
        brouillon: None,
    }
}

fn sole(report: &ExchangeReport) -> &crate::exchange::DossierOutcome {
    assert_eq!(report.dossiers.len(), 1, "{report:?}");
    &report.dossiers[0]
}

#[test]
fn a_cent_amount_is_the_sum_and_a_second_apply_does_not_rewrite() {
    let mut store = test_store("echange-cents");
    let doc = document(vec![valvert()]);
    let first = apply(&mut store, &doc);
    let outcome = sole(&first);
    assert_eq!(outcome.statut, ExchangeStatus::Cree);
    let piste = outcome.piste.expect("piste créée");
    let fiche = outcome.fiche.expect("fiche créée");
    let opportunity = opportunity_by_id(store.connection(), piste)
        .unwrap()
        .unwrap();
    assert_eq!(opportunity.amount, Money::from_cents(160_000));
    assert_eq!(opportunity.probability, Probability::new(40).unwrap());
    assert_eq!(opportunity.revision, 2);
    let client = clients::client_by_id(store.connection(), fiche)
        .unwrap()
        .unwrap();
    assert_eq!(client.siren, Some(Siren::parse("552100554").unwrap()));
    assert_eq!(
        client.vat_number,
        Some(VatNumber::parse("FR96552100554").unwrap())
    );
    assert_eq!(client.switchboard_phone.as_deref(), Some("03 27 44 44 44"));
    let person = clients::correspondent(store.connection(), fiche)
        .unwrap()
        .unwrap();
    assert_eq!(person.given_name, "Camille");
    assert_eq!(person.family_name, "Rivière");
    assert_eq!(person.civility, Some(Civility::Madame));
    let revision = opportunity.revision;

    let second = apply(&mut store, &doc);
    assert_eq!(sole(&second).statut, ExchangeStatus::Inchange);
    assert!(sole(&second).gardes.is_empty());
    let reread = opportunity_by_id(store.connection(), piste)
        .unwrap()
        .unwrap();
    assert_eq!(reread.revision, revision);
    assert_eq!(reread.amount, Money::from_cents(160_000));

    let mut emptied = valvert();
    emptied.lignes = Some(Vec::new());
    apply(&mut store, &document(vec![emptied]));
    assert_eq!(
        crate::prospection::estimation_lines(store.connection(), piste).unwrap(),
        vec![
            ("Diagnostic".to_string(), Money::from_cents(120_000)),
            ("Atelier".to_string(), Money::from_cents(40_000)),
        ]
    );
    assert_eq!(
        opportunity_by_id(store.connection(), piste)
            .unwrap()
            .unwrap()
            .amount,
        Money::from_cents(160_000)
    );
}

#[test]
fn an_unreadable_amount_or_a_partial_address_writes_nothing() {
    let mut store = test_store("echange-refus");
    let mut cents = valvert();
    cents.lignes = Some(vec![LineFile {
        libelle: "Diagnostic".to_string(),
        montant: "12,345".to_string(),
    }]);
    let bad_cents = apply(&mut store, &document(vec![cents]));
    assert_eq!(sole(&bad_cents).statut, ExchangeStatus::Incomplet);
    assert_eq!(
        sole(&bad_cents).raison.as_deref(),
        Some("montant illisible : 12,345")
    );
    assert_eq!(rows(&store, "SELECT COUNT(*) FROM clients"), 0);

    let mut whole = valvert();
    whole.lignes = Some(vec![LineFile {
        libelle: "Diagnostic".to_string(),
        montant: "1200".to_string(),
    }]);
    let whole_euros = apply(&mut store, &document(vec![whole]));
    assert_eq!(
        sole(&whole_euros).raison.as_deref(),
        Some("montant illisible : 1200")
    );

    let mut address = valvert();
    address.adresse = Some(AddressFile {
        rue: "1 place de la Mairie".to_string(),
        code_postal: "59171".to_string(),
        ville: String::new(),
        pays: None,
    });
    let partial = apply(&mut store, &document(vec![address]));
    assert_eq!(sole(&partial).raison.as_deref(), Some("adresse incomplète"));
    assert_eq!(rows(&store, "SELECT COUNT(*) FROM clients"), 0);
}

#[test]
fn an_existing_qui_is_attached_and_not_duplicated() {
    let mut store = test_store("echange-qui");
    let created = match Executor::new(&mut store)
        .execute(
            &CreateClient {
                name: "Mairie de Valvert".to_string(),
                siren: None,
                vat_number: None,
                address: None,
            },
            &human(),
        )
        .unwrap()
    {
        Outcome::Applied(id) => id,
        other => panic!("fiche attendue, reçu {other:?}"),
    };
    let report = apply(&mut store, &document(vec![valvert()]));
    assert_eq!(sole(&report).statut, ExchangeStatus::Cree);
    assert_eq!(sole(&report).fiche, Some(created));
    assert_eq!(rows(&store, "SELECT COUNT(*) FROM clients"), 1);
    assert_eq!(rows(&store, "SELECT COUNT(*) FROM opportunities"), 1);
    let origin: String = store
        .connection()
        .query_row(
            "SELECT origin FROM exchange_links WHERE kind = 'fiche' AND entity_id = ?1",
            [created.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(origin, "org-valvert");
}

#[test]
fn travaux_already_written_stay_until_reprendre() {
    let mut store = test_store("echange-travaux");
    let doc = document(vec![valvert()]);
    let first = apply(&mut store, &doc);
    let fiche = sole(&first).fiche.unwrap();
    let mut changed = valvert();
    changed.travaux = Some("Un autre constat.".to_string());
    let kept = apply(&mut store, &document(vec![changed.clone()]));
    assert_eq!(sole(&kept).statut, ExchangeStatus::Revise);
    assert_eq!(sole(&kept).gardes, vec!["travaux".to_string()]);
    assert_eq!(
        dossier_work::dossier_work(store.connection(), fiche)
            .unwrap()
            .body,
        "Le site n'est pas conforme."
    );
    changed.reprendre = true;
    let replaced = apply(&mut store, &document(vec![changed]));
    assert_eq!(sole(&replaced).statut, ExchangeStatus::Revise);
    assert!(sole(&replaced).gardes.is_empty());
    assert_eq!(
        dossier_work::dossier_work(store.connection(), fiche)
            .unwrap()
            .body,
        "Un autre constat."
    );
}

#[test]
fn a_date_moved_in_griffe_is_not_replaced_by_the_old_file_date() {
    let mut store = test_store("echange-date");
    let doc = document(vec![valvert()]);
    let first = apply(&mut store, &doc);
    let piste = sole(&first).piste.unwrap();
    let fiche = sole(&first).fiche.unwrap();
    Executor::new(&mut store)
        .execute(
            &SetFollowUpDate {
                subject: FollowUpSubject::Opportunity(piste),
                on: date!(2026 - 11 - 16),
                today: TODAY,
            },
            &human(),
        )
        .unwrap();
    let mut later = valvert();
    later.telephone = Some("0327555555".to_string());
    let report = apply(&mut store, &document(vec![later]));
    assert_eq!(sole(&report).statut, ExchangeStatus::Revise);
    assert_eq!(sole(&report).gardes, vec!["prochaine action".to_string()]);
    let opportunity = opportunity_by_id(store.connection(), piste)
        .unwrap()
        .unwrap();
    assert_eq!(opportunity.next_action_at, Some(date!(2026 - 11 - 16)));
    let client = clients::client_by_id(store.connection(), fiche)
        .unwrap()
        .unwrap();
    assert_eq!(client.switchboard_phone.as_deref(), Some("03 27 55 55 55"));
}

#[test]
fn series_words_are_written_once_and_kept_without_reprendre() {
    let mut store = test_store("echange-serie");
    let serie = SeriesFile {
        nom: "Mairies".to_string(),
        reprendre: false,
        moments: vec![SeriesMoment {
            cle: "hello".to_string(),
            sujet: "Bonjour {{prenom}}".to_string(),
            corps: "Un mot.\n\n{{signature}}\n".to_string(),
        }],
    };
    let mut doc = document(Vec::new());
    doc.series = vec![serie.clone()];
    let created = apply(&mut store, &doc);
    assert_eq!(created.series[0].statut, ExchangeStatus::Cree);
    let genre = phrase_genre_named(store.connection(), "Mairies")
        .unwrap()
        .unwrap();
    let hello = phrases_for_genre(store.connection(), &genre.id)
        .unwrap()
        .into_iter()
        .find(|phrase| phrase.key == "hello")
        .unwrap();
    assert_eq!(hello.subject, "Bonjour {{prenom}}");
    assert_eq!(hello.body, "Un mot.\n\n{{signature}}\n");

    let mut rewritten = serie;
    rewritten.moments[0].corps = "Un autre.\n\n{{signature}}\n".to_string();
    doc.series = vec![rewritten];
    let kept = apply(&mut store, &doc);
    assert_eq!(kept.series[0].statut, ExchangeStatus::Inchange);
    assert_eq!(kept.series[0].gardes, vec!["mots".to_string()]);
    let kept_hello = phrases_for_genre(store.connection(), &genre.id)
        .unwrap()
        .into_iter()
        .find(|phrase| phrase.key == "hello")
        .unwrap();
    assert_eq!(kept_hello.body, "Un mot.\n\n{{signature}}\n");
}

#[test]
fn a_draft_is_prepared_and_a_second_apply_does_not_add_another() {
    let mut store = test_store("echange-brouillon");
    Executor::new(&mut store)
        .execute(
            &SetFollowUpSender {
                email: "nicolas@valvert.example".to_string(),
                name: None,
            },
            &human(),
        )
        .unwrap();
    let mut dossier = valvert();
    dossier.brouillon = Some("Bonjour,\n\nUn constat.\n\n<signature>".to_string());
    let doc = document(vec![dossier.clone()]);
    let first = apply(&mut store, &doc);
    let piste = sole(&first).piste.unwrap();
    assert_eq!(drafts(&store, piste), 1);

    dossier.source = Some("Site".to_string());
    let second = apply(&mut store, &document(vec![dossier]));
    assert_eq!(sole(&second).statut, ExchangeStatus::Revise);
    assert!(
        sole(&second)
            .gardes
            .iter()
            .any(|garde| garde == "brouillon")
    );
    assert_eq!(drafts(&store, piste), 1);
}

fn drafts(store: &Store, piste: crate::domain::OpportunityId) -> usize {
    active_events(&events_for(store.connection(), FollowUpSubject::Opportunity(piste)).unwrap())
        .iter()
        .filter(|event| event.fact == FollowUpFact::DraftPrepared)
        .count()
}

#[test]
fn a_lost_piste_stays_lost_when_the_same_file_returns() {
    let mut store = test_store("echange-close");
    let doc = document(vec![valvert()]);
    let first = apply(&mut store, &doc);
    let piste = sole(&first).piste.unwrap();
    Executor::new(&mut store)
        .execute(
            &LoseOpportunity {
                opportunity_id: piste,
                reason: LossReason::Budget,
            },
            &human(),
        )
        .unwrap();
    let again = apply(&mut store, &doc);
    assert_eq!(sole(&again).statut, ExchangeStatus::Incomplet);
    assert_eq!(sole(&again).raison.as_deref(), Some("piste déjà close"));
    let opportunity = opportunity_by_id(store.connection(), piste)
        .unwrap()
        .unwrap();
    assert_eq!(opportunity.stage, OpportunityStage::Lost);
    assert_eq!(rows(&store, "SELECT COUNT(*) FROM opportunities"), 1);
}

#[test]
fn a_closed_piste_with_the_same_quoi_is_not_opened_again() {
    let mut store = test_store("echange-close-nom");
    let piste = match Executor::new(&mut store)
        .execute(
            &CreateProspect {
                prospect_name: "Mairie de Valvert".to_string(),
                address: None,
                representative: None,
                email: None,
                phone: None,
                name: "Refonte du site".to_string(),
                amount: Money::ZERO,
                probability: Probability::new(0).unwrap(),
                next_action_at: TODAY,
                source: None,
            },
            &human(),
        )
        .unwrap()
    {
        Outcome::Applied(id) => id,
        other => panic!("piste attendue, reçu {other:?}"),
    };
    Executor::new(&mut store)
        .execute(
            &LoseOpportunity {
                opportunity_id: piste,
                reason: LossReason::Budget,
            },
            &human(),
        )
        .unwrap();
    let report = apply(&mut store, &document(vec![valvert()]));
    assert_eq!(sole(&report).statut, ExchangeStatus::Incomplet);
    assert_eq!(sole(&report).raison.as_deref(), Some("piste déjà close"));
    assert_eq!(rows(&store, "SELECT COUNT(*) FROM opportunities"), 1);
    assert_eq!(
        opportunity_by_id(store.connection(), piste)
            .unwrap()
            .unwrap()
            .stage,
        OpportunityStage::Lost
    );
}

#[test]
fn preview_reports_the_creation_and_leaves_the_vault_empty() {
    let mut store = test_store("echange-apercu");
    let doc = document(vec![valvert()]);
    let report = preview(store.connection(), &doc, TODAY).unwrap();
    assert_eq!(sole(&report).statut, ExchangeStatus::Cree);
    assert_eq!(rows(&store, "SELECT COUNT(*) FROM clients"), 0);
    assert_eq!(rows(&store, "SELECT COUNT(*) FROM exchange_links"), 0);
    let applied = apply(&mut store, &doc);
    assert_eq!(sole(&applied).statut, ExchangeStatus::Cree);
    assert_eq!(rows(&store, "SELECT COUNT(*) FROM clients"), 1);
}

#[test]
fn the_same_cadence_is_unchanged_and_an_offset_change_pins() {
    let mut store = test_store("echange-cadence");
    let current = live_cadence(&store);
    let mut same = document(Vec::new());
    same.cadence = Some(current.clone());
    let repeated = apply(&mut store, &same);
    let repeated_cadence = repeated.cadence.expect("cadence");
    assert_eq!(repeated_cadence.statut, ExchangeStatus::Inchange);
    assert!(!repeated_cadence.epingle);

    let mut shifted = current;
    let moment = shifted
        .iter_mut()
        .find(|moment| moment.cle == "bump")
        .unwrap();
    moment.ecart_ouvres = 5;
    same.cadence = Some(shifted);
    let changed = apply(&mut store, &same);
    let changed_cadence = changed.cadence.expect("cadence");
    assert_eq!(changed_cadence.statut, ExchangeStatus::Revise);
    assert!(changed_cadence.epingle);
    let stored = prospect_phrases(store.connection()).unwrap();
    let bump = stored.iter().find(|phrase| phrase.key == "bump").unwrap();
    assert_eq!(bump.offset_days, 5);
}

#[test]
fn an_unknown_cadence_key_is_refused_and_the_vault_stays() {
    let mut store = test_store("echange-cadence-cle");
    let mut moments = live_cadence(&store);
    moments.push(CadenceMoment {
        cle: "relance".to_string(),
        libelle: "Encore".to_string(),
        ecart_ouvres: 21,
        sujet: None,
        corps: None,
    });
    let mut doc = document(Vec::new());
    doc.cadence = Some(moments);
    let report = apply(&mut store, &doc);
    let cadence = report.cadence.expect("cadence");
    assert_eq!(cadence.statut, ExchangeStatus::Incomplet);
    assert_eq!(cadence.raison.as_deref(), Some("moment inconnu : relance"));
    assert!(!cadence.epingle);
    let stored = prospect_phrases(store.connection()).unwrap();
    let bump = stored.iter().find(|phrase| phrase.key == "bump").unwrap();
    assert_eq!(bump.offset_days, 3);
}

fn live_cadence(store: &Store) -> Vec<CadenceMoment> {
    prospect_phrases(store.connection())
        .unwrap()
        .into_iter()
        .map(|phrase| CadenceMoment {
            cle: phrase.key,
            libelle: phrase.label,
            ecart_ouvres: phrase.offset_days,
            sujet: None,
            corps: None,
        })
        .collect()
}
