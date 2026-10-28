//! Application du document. Une sauvegarde par dossier : l'échec de l'un n'emporte pas les autres.
//! Le contrôle à blanc rejoue la même fonction et annule la sauvegarde extérieure.

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::Date;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::app::{AppError, Command};
use crate::clients::{self, CreateContact, UpdateClient, UpdateContact};
use crate::domain::{
    self, Address, Civility, ClientId, FollowUpFact, FollowUpSubject, Money, OpportunityId,
    Probability, Siren, VatNumber, active_events, display_phone,
};
use crate::dossier_work::{self, SaveDossierWork};
use crate::follow_up::bind::phrase_genre_named;
use crate::follow_up::{
    ArrangeProspectPhrases, CreateProspectGenre, GenreWordDraft, MomentDraft, PrepareFollowUp,
    RetractLastFollowUp, RewriteGenreWords, SetDossierGenre, events_for, follow_up_sender,
    phrases_for_genre, prospect_genre_for, prospect_phrases,
};
use crate::prospection::{
    CreateOpportunity, CreateProspect, EstimationLineInput, OpportunityFilter, SetEstimation,
    UpdateOpportunity, estimation_lines, list_opportunities_with, opportunity_by_id,
};
use crate::reference::{self, RefMatch};
use crate::work_kinds::{self, ReviseWorkKind, SetDossierWorkKinds};

use super::document::{
    AddressFile, CadenceMoment, ContactFile, DossierFile, ExchangeDocument, LineFile, SeriesFile,
    SeriesMoment, TypeEntry, parse_montant,
};
use super::report::{
    CadenceOutcome, DossierOutcome, ExchangeReport, ExchangeStatus, SeriesOutcome,
};

const FICHE: &str = "fiche";
const PISTE: &str = "piste";

/// Applique un document déjà lu. `today` est le jour du coffre, pour le brouillon.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyExchange {
    pub document: ExchangeDocument,
    #[serde(with = "crate::domain::serde_date::date")]
    pub today: Date,
}

impl Command for ApplyExchange {
    type Output = ExchangeReport;
    const NAME: &'static str = "exchange.apply";

    fn requires_confirmation(&self) -> bool {
        true
    }

    fn apply(&self, conn: &Connection) -> Result<Self::Output, AppError> {
        apply_document(conn, &self.document, self.today)
    }
}

/// Même compte-rendu qu'une application, sans rien laisser dans le coffre.
///
/// # Errors
///
/// Version inconnue, ou lecture impossible.
pub fn preview(
    conn: &Connection,
    document: &ExchangeDocument,
    today: Date,
) -> Result<ExchangeReport, AppError> {
    conn.execute("SAVEPOINT echange_preview", [])?;
    let result = apply_document(conn, document, today);
    conn.execute("ROLLBACK TO echange_preview", [])?;
    conn.execute("RELEASE echange_preview", [])?;
    result
}

fn apply_document(
    conn: &Connection,
    document: &ExchangeDocument,
    today: Date,
) -> Result<ExchangeReport, AppError> {
    if document.version != 1 {
        let version = document.version;
        return Err(AppError::Domain(format!(
            "version d'échange inconnue : {version}"
        )));
    }
    let cadence = match &document.cadence {
        Some(moments) => Some(apply_cadence(conn, moments)?),
        None => None,
    };
    let mut series = Vec::with_capacity(document.series.len());
    for serie in &document.series {
        series.push(apply_series(conn, serie)?);
    }
    let mut dossiers = Vec::with_capacity(document.dossiers.len());
    for dossier in &document.dossiers {
        let outcome = apply_dossier(conn, dossier, today)?;
        let halt = document.stop && outcome.statut == ExchangeStatus::Incomplet;
        dossiers.push(outcome);
        if halt {
            break;
        }
    }
    Ok(ExchangeReport {
        dossiers,
        series,
        cadence,
    })
}

fn apply_cadence(conn: &Connection, moments: &[CadenceMoment]) -> Result<CadenceOutcome, AppError> {
    let current = prospect_phrases(conn)?;
    if let Some(raison) = cadence_refusal(&current, moments) {
        return Ok(CadenceOutcome {
            statut: ExchangeStatus::Incomplet,
            epingle: false,
            raison: Some(raison),
        });
    }
    if cadence_same(&current, moments) {
        return Ok(CadenceOutcome {
            statut: ExchangeStatus::Inchange,
            epingle: false,
            raison: None,
        });
    }
    let epingle = cadence_pins(&current, moments);
    let drafts = cadence_drafts(&current, moments)?;
    savepoint(conn, "echange_cadence", |conn| {
        ArrangeProspectPhrases { moments: drafts }.apply(conn)?;
        Ok(())
    })?;
    Ok(CadenceOutcome {
        statut: ExchangeStatus::Revise,
        epingle,
        raison: None,
    })
}

fn cadence_refusal(
    current: &[crate::follow_up::ProspectPhrase],
    moments: &[CadenceMoment],
) -> Option<String> {
    if moments.is_empty() {
        return Some("cadence vide".to_string());
    }
    if moments[0].ecart_ouvres != 0 {
        return Some("le premier écart est le jour même".to_string());
    }
    let mut seen = std::collections::HashSet::new();
    for moment in moments {
        let cle = moment.cle.trim();
        if cle.is_empty() || !seen.insert(cle.to_string()) {
            return Some(format!("moment illisible : {cle}"));
        }
        if !current.iter().any(|phrase| phrase.key == cle) {
            return Some(format!("moment inconnu : {cle}"));
        }
        if moment.libelle.trim().is_empty() {
            return Some(format!("moment sans libellé : {cle}"));
        }
    }
    None
}

fn cadence_same(current: &[crate::follow_up::ProspectPhrase], moments: &[CadenceMoment]) -> bool {
    current.len() == moments.len()
        && current.iter().zip(moments).all(|(row, want)| {
            row.key == want.cle.trim()
                && row.label == want.libelle.trim()
                && row.offset_days == want.ecart_ouvres
                && want
                    .sujet
                    .as_deref()
                    .is_none_or(|sujet| domain::phrase_from_editor(sujet.trim()) == row.subject)
                && want
                    .corps
                    .as_deref()
                    .is_none_or(|corps| domain::phrase_from_editor(corps) == row.body)
        })
}

fn cadence_pins(current: &[crate::follow_up::ProspectPhrase], moments: &[CadenceMoment]) -> bool {
    current.len() != moments.len()
        || current
            .iter()
            .zip(moments)
            .any(|(row, want)| row.key != want.cle.trim() || row.offset_days != want.ecart_ouvres)
}

fn cadence_drafts(
    current: &[crate::follow_up::ProspectPhrase],
    moments: &[CadenceMoment],
) -> Result<Vec<MomentDraft>, AppError> {
    let mut drafts = Vec::with_capacity(moments.len());
    for want in moments {
        let cle = want.cle.trim();
        let Some(row) = current.iter().find(|phrase| phrase.key == cle) else {
            return Err(AppError::Domain(format!("moment inconnu : {cle}")));
        };
        drafts.push(MomentDraft {
            key: Some(row.key.clone()),
            label: want.libelle.trim().to_string(),
            offset_days: want.ecart_ouvres,
            subject: want.sujet.clone().unwrap_or_else(|| row.subject.clone()),
            body: want.corps.clone().unwrap_or_else(|| row.body.clone()),
            revision: Some(row.revision),
        });
    }
    Ok(drafts)
}

fn apply_series(conn: &Connection, serie: &SeriesFile) -> Result<SeriesOutcome, AppError> {
    let nom = serie.nom.trim();
    if nom.is_empty() {
        return Ok(series_incomplet(nom, "série sans nom"));
    }
    let phrases = prospect_phrases(conn)?;
    if let Some(raison) = series_refusal(&phrases, &serie.moments) {
        return Ok(series_incomplet(nom, raison));
    }
    let existing = phrase_genre_named(conn, nom)?;
    if existing.is_none() && serie.moments.is_empty() {
        return Ok(series_incomplet(nom, "série sans mots"));
    }
    if let Some(genre) = &existing
        && !serie.reprendre
        && !serie.moments.is_empty()
        && series_words_differ(conn, &genre.id, &serie.moments)?
    {
        return Ok(SeriesOutcome {
            nom: nom.to_string(),
            statut: ExchangeStatus::Inchange,
            raison: None,
            gardes: vec!["mots".to_string()],
        });
    }
    if let Some(genre) = &existing
        && !series_words_differ(conn, &genre.id, &serie.moments)?
    {
        return Ok(SeriesOutcome {
            nom: genre.name.clone(),
            statut: ExchangeStatus::Inchange,
            raison: None,
            gardes: Vec::new(),
        });
    }
    let created = existing.is_none();
    savepoint(conn, "echange_serie", |conn| {
        let genre = if let Some(genre) = phrase_genre_named(conn, nom)? {
            genre
        } else {
            CreateProspectGenre {
                name: nom.to_string(),
                copy_from: None,
            }
            .apply(conn)?
        };
        if !serie.moments.is_empty() {
            RewriteGenreWords {
                genre_id: genre.id,
                words: serie
                    .moments
                    .iter()
                    .map(|moment| GenreWordDraft {
                        key: moment.cle.trim().to_string(),
                        subject: moment.sujet.clone(),
                        body: moment.corps.clone(),
                    })
                    .collect(),
            }
            .apply(conn)?;
        }
        Ok(())
    })?;
    Ok(SeriesOutcome {
        nom: nom.to_string(),
        statut: if created {
            ExchangeStatus::Cree
        } else {
            ExchangeStatus::Revise
        },
        raison: None,
        gardes: Vec::new(),
    })
}

fn series_incomplet(nom: &str, raison: impl Into<String>) -> SeriesOutcome {
    SeriesOutcome {
        nom: nom.to_string(),
        statut: ExchangeStatus::Incomplet,
        raison: Some(raison.into()),
        gardes: Vec::new(),
    }
}

fn series_refusal(
    phrases: &[crate::follow_up::ProspectPhrase],
    moments: &[SeriesMoment],
) -> Option<String> {
    for moment in moments {
        let cle = moment.cle.trim();
        if cle.is_empty() || moment.sujet.trim().is_empty() || moment.corps.trim().is_empty() {
            return Some(format!("moment incomplet : {cle}"));
        }
        if !phrases.iter().any(|phrase| phrase.key == cle) {
            return Some(format!("moment inconnu : {cle}"));
        }
    }
    None
}

fn series_words_differ(
    conn: &Connection,
    genre_id: &str,
    moments: &[SeriesMoment],
) -> Result<bool, AppError> {
    if moments.is_empty() {
        return Ok(false);
    }
    let stored = phrases_for_genre(conn, genre_id)?;
    Ok(moments.iter().any(|moment| {
        stored
            .iter()
            .find(|phrase| phrase.key == moment.cle.trim())
            .is_none_or(|phrase| {
                domain::phrase_from_editor(moment.sujet.trim()) != phrase.subject
                    || domain::phrase_from_editor(&moment.corps) != phrase.body
            })
    }))
}

fn apply_dossier(
    conn: &Connection,
    dossier: &DossierFile,
    today: Date,
) -> Result<DossierOutcome, AppError> {
    let fiche_name = dossier.qui.trim();
    let piste_name = dossier.quoi.trim();
    let origine = dossier.origine.trim();
    if origine.is_empty() {
        return Ok(DossierOutcome::incomplet(
            origine,
            fiche_name,
            "origine vide",
        ));
    }
    if let Some(raison) = dossier_refusal(dossier, fiche_name, piste_name) {
        return Ok(DossierOutcome::incomplet(origine, fiche_name, raison));
    }
    if let Some(raison) = brouillon_refusal(conn, dossier)? {
        return Ok(DossierOutcome::incomplet(origine, fiche_name, raison));
    }
    let fingerprint = fingerprint(dossier)?;
    if let Some(link) = piste_link(conn, origine)?
        && let Some(opportunity) = opportunity_by_id(conn, link.piste)?
    {
        if closed(&opportunity) {
            return Ok(DossierOutcome::incomplet(
                origine,
                fiche_name,
                "piste déjà close",
            ));
        }
        if link.fingerprint == fingerprint {
            return Ok(DossierOutcome {
                origine: origine.to_string(),
                qui: fiche_name.to_string(),
                statut: ExchangeStatus::Inchange,
                raison: None,
                gardes: Vec::new(),
                fiche: Some(opportunity.client_id),
                piste: Some(opportunity.id),
            });
        }
    }
    if let Some(raison) = unknown_serie(conn, dossier)? {
        return Ok(DossierOutcome::incomplet(origine, fiche_name, raison));
    }
    match savepoint(conn, "echange_dossier", |conn| {
        write_dossier(conn, dossier, today, &fingerprint)
    }) {
        Ok(outcome) => Ok(outcome),
        Err(error) => Ok(DossierOutcome::incomplet(
            origine,
            fiche_name,
            error.to_string(),
        )),
    }
}

fn dossier_refusal(dossier: &DossierFile, fiche_name: &str, piste_name: &str) -> Option<String> {
    if fiche_name.is_empty() {
        return Some("Qui vide".to_string());
    }
    if piste_name.is_empty() {
        return Some("Quoi vide".to_string());
    }
    if let Some(probabilite) = dossier.probabilite
        && Probability::new(probabilite).is_err()
    {
        return Some("probabilité illisible".to_string());
    }
    if let Some(date) = dossier.prochaine_action.as_deref()
        && domain::parse_date(date.trim()).is_err()
    {
        return Some("date illisible".to_string());
    }
    if address_of(dossier.adresse.as_ref()).is_err() {
        return Some("adresse incomplète".to_string());
    }
    if let Some(lines) = &dossier.lignes
        && !lines.is_empty()
        && let Err(raison) = lines_of(lines)
    {
        return Some(raison);
    }
    if let Some(travaux) = &dossier.travaux
        && travaux.len() > dossier_work::MAX_BODY_BYTES
    {
        return Some("travaux trop longs".to_string());
    }
    if let Some(contact) = &dossier.contact
        && let Some(raison) = contact_refusal(contact)
    {
        return Some(raison);
    }
    if let Some(siren) = dossier.siren.as_deref()
        && Siren::parse(siren).is_err()
    {
        return Some("SIREN illisible".to_string());
    }
    if let Some(tva) = dossier.tva.as_deref()
        && VatNumber::parse(tva).is_err()
    {
        return Some("TVA illisible".to_string());
    }
    if let Some(types) = &dossier.types
        && let Some(raison) = types_refusal(types)
    {
        return Some(raison);
    }
    if let Some(serie) = dossier.serie.as_deref()
        && serie.trim().is_empty()
    {
        return Some("série sans nom".to_string());
    }
    None
}

fn unknown_serie(conn: &Connection, dossier: &DossierFile) -> Result<Option<String>, AppError> {
    let Some(nom) = cleaned(dossier.serie.as_deref()) else {
        return Ok(None);
    };
    if phrase_genre_named(conn, &nom)?.is_none() {
        return Ok(Some(format!("série inconnue : {nom}")));
    }
    Ok(None)
}

fn brouillon_refusal(conn: &Connection, dossier: &DossierFile) -> Result<Option<String>, AppError> {
    let Some(body) = dossier.brouillon.as_deref() else {
        return Ok(None);
    };
    if body.trim().is_empty() {
        return Ok(Some("brouillon vide".to_string()));
    }
    let courriel = dossier
        .courriel
        .as_deref()
        .map(str::trim)
        .filter(|email| !email.is_empty())
        .or_else(|| {
            dossier
                .contact
                .as_ref()
                .and_then(|contact| contact.courriel.as_deref())
                .map(str::trim)
                .filter(|email| !email.is_empty())
        });
    if courriel.is_none() {
        return Ok(Some("brouillon sans adresse".to_string()));
    }
    if follow_up_sender(conn)?.sender_email.is_none() {
        return Ok(Some("brouillon sans expéditeur".to_string()));
    }
    Ok(None)
}

fn contact_refusal(contact: &ContactFile) -> Option<String> {
    if contact.prenom.trim().is_empty() || contact.nom.trim().is_empty() {
        return Some("contact incomplet".to_string());
    }
    if let Some(genre) = contact.genre.as_deref()
        && !genre.trim().is_empty()
        && Civility::parse(&genre.trim().to_lowercase()).is_none()
    {
        return Some("genre illisible".to_string());
    }
    None
}

fn types_refusal(types: &[TypeEntry]) -> Option<String> {
    for entry in types {
        if entry.nom().trim().is_empty() {
            return Some("type sans nom".to_string());
        }
        if let Some(detail) = entry.detail()
            && let Some(teinte) = detail.teinte.as_deref()
            && work_kinds::WorkKindTint::parse(teinte).is_err()
        {
            return Some("teinte inconnue".to_string());
        }
        if let Some(detail) = entry.detail()
            && detail
                .note
                .as_deref()
                .is_some_and(|note| note.chars().count() > 120)
        {
            return Some("note de type trop longue".to_string());
        }
    }
    None
}

fn address_of(raw: Option<&AddressFile>) -> Result<Option<Address>, ()> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let street = raw.rue.trim();
    let postal_code = raw.code_postal.trim();
    let city = raw.ville.trim();
    if street.is_empty() || postal_code.is_empty() || city.is_empty() {
        return Err(());
    }
    let country = raw
        .pays
        .as_deref()
        .map(str::trim)
        .filter(|pays| !pays.is_empty())
        .unwrap_or("FR");
    if country.len() != 2 || !country.chars().all(|c| c.is_ascii_alphabetic()) {
        return Err(());
    }
    Ok(Some(Address {
        street: street.to_string(),
        postal_code: postal_code.to_string(),
        city: city.to_string(),
        country: country.to_ascii_uppercase(),
    }))
}

fn lines_of(lines: &[LineFile]) -> Result<Vec<EstimationLineInput>, String> {
    let mut parsed = Vec::with_capacity(lines.len());
    for line in lines {
        let label = line.libelle.trim();
        if label.is_empty() {
            return Err("ligne d'estimation vide".to_string());
        }
        let amount = parse_montant(&line.montant)?;
        if amount.cents() <= 0 {
            return Err("ligne d'estimation vide".to_string());
        }
        parsed.push(EstimationLineInput {
            label: label.to_string(),
            amount,
        });
    }
    Ok(parsed)
}

struct DossierWrite<'a> {
    dossier: &'a DossierFile,
    today: Date,
    fingerprint: &'a str,
    fiche_name: &'a str,
    piste_name: &'a str,
    origine: &'a str,
    file_date: Option<Date>,
    created: bool,
}

fn write_dossier(
    conn: &Connection,
    dossier: &DossierFile,
    today: Date,
    fingerprint: &str,
) -> Result<DossierOutcome, AppError> {
    let fiche_name = dossier.qui.trim().to_string();
    let piste_name = dossier.quoi.trim().to_string();
    let origine = dossier.origine.trim().to_string();
    let date = dossier
        .prochaine_action
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(domain::parse_date)
        .transpose()
        .map_err(|_| AppError::Domain("date illisible".to_string()))?;
    let draft = DossierWrite {
        dossier,
        today,
        fingerprint,
        fiche_name: &fiche_name,
        piste_name: &piste_name,
        origine: &origine,
        file_date: date,
        created: false,
    };
    if let Some(link) = piste_link(conn, &origine)? {
        let Some(opportunity) = opportunity_by_id(conn, link.piste)? else {
            return Err(AppError::Domain("piste introuvable".to_string()));
        };
        if closed(&opportunity) {
            return Ok(DossierOutcome::incomplet(
                &origine,
                &fiche_name,
                "piste déjà close",
            ));
        }
        return revise(conn, &draft, opportunity);
    }
    let client_id = match resolve_fiche(conn, dossier, &fiche_name)? {
        FicheChoice::Ready(id) => Some(id),
        FicheChoice::New => None,
        FicheChoice::Refuse(raison) => {
            return Ok(DossierOutcome::incomplet(&origine, &fiche_name, raison));
        }
    };
    if let Some(client_id) = client_id {
        match named_pistes(conn, client_id, &piste_name)? {
            NamedPistes::None => {}
            NamedPistes::Open(opportunity) => {
                return revise(conn, &draft, opportunity);
            }
            NamedPistes::Closed => {
                return Ok(DossierOutcome::incomplet(
                    &origine,
                    &fiche_name,
                    "piste déjà close",
                ));
            }
            NamedPistes::Several => {
                return Ok(DossierOutcome::incomplet(
                    &origine,
                    &fiche_name,
                    "plusieurs pistes",
                ));
            }
        }
    }
    let Some(opened_on) = date else {
        return Ok(DossierOutcome::incomplet(
            &origine,
            &fiche_name,
            "date manquante",
        ));
    };
    let created = create_piste(
        conn,
        dossier,
        &fiche_name,
        &piste_name,
        opened_on,
        client_id,
    )?;
    revise(
        conn,
        &DossierWrite {
            file_date: Some(opened_on),
            created: true,
            ..draft
        },
        created,
    )
}

fn create_piste(
    conn: &Connection,
    dossier: &DossierFile,
    fiche_name: &str,
    piste_name: &str,
    date: Date,
    client_id: Option<ClientId>,
) -> Result<crate::domain::Opportunity, AppError> {
    let probability = Probability::new(dossier.probabilite.unwrap_or(0))
        .map_err(|_| AppError::Domain("probabilité illisible".to_string()))?;
    let source = cleaned(dossier.source.as_deref());
    let address = address_of(dossier.adresse.as_ref())
        .map_err(|()| AppError::Domain("adresse incomplète".to_string()))?;
    let id = if let Some(client_id) = client_id {
        CreateOpportunity {
            client_id,
            name: piste_name.to_string(),
            amount: Money::ZERO,
            probability,
            next_action_at: date,
            source,
        }
        .apply(conn)?
    } else {
        CreateProspect {
            prospect_name: fiche_name.to_string(),
            address,
            representative: None,
            email: cleaned(dossier.courriel.as_deref()),
            phone: cleaned(dossier.telephone.as_deref()),
            name: piste_name.to_string(),
            amount: Money::ZERO,
            probability,
            next_action_at: date,
            source,
        }
        .apply(conn)?
    };
    opportunity_by_id(conn, id)?.ok_or_else(|| AppError::Domain("piste introuvable".to_string()))
}

fn revise(
    conn: &Connection,
    draft: &DossierWrite<'_>,
    mut opportunity: crate::domain::Opportunity,
) -> Result<DossierOutcome, AppError> {
    let mut gardes = Vec::new();
    let mut wrote = draft.created;
    wrote |= write_fiche(conn, draft.dossier, draft.fiche_name, opportunity.client_id)?;
    wrote |= write_contact(conn, draft.dossier, opportunity.client_id, &mut gardes)?;
    wrote |= write_travaux(conn, draft.dossier, opportunity.client_id, &mut gardes)?;
    let previous = piste_link(conn, draft.origine)?;
    let (next, date_kept) = decided_date(
        draft.file_date,
        previous.as_ref(),
        opportunity.next_action_at,
    );
    if date_kept {
        gardes.push("prochaine action".to_string());
    }
    if write_piste(
        conn,
        draft.dossier,
        draft.piste_name,
        &mut opportunity,
        next,
    )? {
        wrote = true;
        opportunity = opportunity_by_id(conn, opportunity.id)?
            .ok_or_else(|| AppError::Domain("piste introuvable".to_string()))?;
    }
    wrote |= write_lines(conn, draft.dossier, &opportunity, draft.piste_name)?;
    wrote |= write_types(conn, draft.dossier, opportunity.client_id)?;
    wrote |= write_serie(conn, draft.dossier, opportunity.id, &mut gardes)?;
    wrote |= write_brouillon(
        conn,
        draft.dossier,
        opportunity.id,
        draft.today,
        &mut gardes,
    )?;
    let link_hash = if gardes.is_empty() || previous.is_none() {
        draft.fingerprint.to_string()
    } else {
        previous.as_ref().map_or_else(
            || draft.fingerprint.to_string(),
            |link| link.fingerprint.clone(),
        )
    };
    let stored_date = if date_kept {
        previous.and_then(|link| link.next_action_on)
    } else {
        next.map(domain::format_date)
    };
    remember(
        conn,
        draft.origine,
        PISTE,
        &opportunity.id.to_string(),
        &link_hash,
        stored_date.as_deref(),
    )?;
    if let Some(origin) = cleaned(draft.dossier.origine_fiche.as_deref()) {
        remember(
            conn,
            &origin,
            FICHE,
            &opportunity.client_id.to_string(),
            draft.fingerprint,
            None,
        )?;
    }
    let statut = if draft.created {
        ExchangeStatus::Cree
    } else if wrote || !gardes.is_empty() {
        ExchangeStatus::Revise
    } else {
        ExchangeStatus::Inchange
    };
    Ok(DossierOutcome {
        origine: draft.origine.to_string(),
        qui: draft.fiche_name.to_string(),
        statut,
        raison: None,
        gardes,
        fiche: Some(opportunity.client_id),
        piste: Some(opportunity.id),
    })
}

fn decided_date(
    file_date: Option<Date>,
    previous: Option<&PisteLink>,
    vault: Option<Date>,
) -> (Option<Date>, bool) {
    let Some(wanted) = file_date else {
        return (vault, false);
    };
    match (previous.and_then(|link| link.next_action.as_ref()), vault) {
        (Some(stored), Some(current)) if current != *stored && wanted != current => {
            (Some(current), true)
        }
        _ => (Some(wanted), false),
    }
}

fn write_fiche(
    conn: &Connection,
    dossier: &DossierFile,
    qui: &str,
    client_id: ClientId,
) -> Result<bool, AppError> {
    let Some(mut client) = clients::client_by_id(conn, client_id)? else {
        return Err(AppError::Domain("fiche introuvable".to_string()));
    };
    let address = address_of(dossier.adresse.as_ref())
        .map_err(|()| AppError::Domain("adresse incomplète".to_string()))?;
    let email = match dossier.courriel.as_deref() {
        None => client.switchboard_email.clone(),
        Some(value) => cleaned(Some(value)),
    };
    let phone = match dossier.telephone.as_deref() {
        None => client.switchboard_phone.clone(),
        Some(value) => cleaned(Some(value)).map(|phone| display_phone(&phone)),
    };
    let phone = phone.filter(|value| !value.is_empty());
    let siren = match dossier.siren.as_deref() {
        None => client.siren,
        Some(value) => {
            Some(Siren::parse(value).map_err(|_| AppError::Domain("SIREN illisible".to_string()))?)
        }
    };
    let tva = match dossier.tva.as_deref() {
        None => client.vat_number.clone(),
        Some(value) => Some(
            VatNumber::parse(value).map_err(|_| AppError::Domain("TVA illisible".to_string()))?,
        ),
    };
    let name = qui.to_string();
    let address = address.or(client.address.clone());
    let same = client.name == name
        && client.siren == siren
        && client.vat_number == tva
        && client.address == address
        && client.switchboard_email == email
        && client.switchboard_phone == phone;
    if same {
        return Ok(false);
    }
    client.name.clone_from(&name);
    UpdateClient {
        id: client.id,
        revision: client.revision,
        name,
        siren,
        vat_number: tva,
        address,
        switchboard_email: email,
        switchboard_phone: phone,
    }
    .apply(conn)?;
    Ok(true)
}

fn write_contact(
    conn: &Connection,
    dossier: &DossierFile,
    client_id: ClientId,
    gardes: &mut Vec<String>,
) -> Result<bool, AppError> {
    let Some(contact) = &dossier.contact else {
        return Ok(false);
    };
    let prenom = contact.prenom.trim().to_string();
    let nom = contact.nom.trim().to_string();
    let civility = contact
        .genre
        .as_deref()
        .map(str::trim)
        .filter(|genre| !genre.is_empty())
        .map(|genre| {
            Civility::parse(&genre.to_lowercase())
                .ok_or_else(|| AppError::Domain("genre illisible".to_string()))
        })
        .transpose()?;
    let email = cleaned(contact.courriel.as_deref());
    let phone = cleaned(contact.telephone.as_deref());
    let role = cleaned(contact.role.as_deref());
    let Some(current) = clients::correspondent(conn, client_id)? else {
        CreateContact {
            client_id,
            name: format!("{prenom} {nom}"),
            email,
            phone,
            role,
            given_name: prenom,
            family_name: nom,
            civility,
            structured: true,
        }
        .apply(conn)?;
        return Ok(true);
    };
    let same_person = current.given_name.eq_ignore_ascii_case(&prenom)
        && current.family_name.eq_ignore_ascii_case(&nom);
    if !same_person && !dossier.reprendre {
        gardes.push("qui répond".to_string());
        return Ok(false);
    }
    let stored_phone = phone
        .as_deref()
        .map(display_phone)
        .filter(|value| !value.is_empty());
    let unchanged = current.given_name == prenom
        && current.family_name == nom
        && current.civility == civility
        && current.email == email
        && current.phone == stored_phone
        && current.role == role;
    if unchanged {
        return Ok(false);
    }
    UpdateContact {
        id: current.id,
        revision: current.revision,
        name: format!("{prenom} {nom}"),
        email,
        phone,
        role,
        given_name: prenom,
        family_name: nom,
        civility,
        structured: true,
    }
    .apply(conn)?;
    Ok(true)
}

fn write_travaux(
    conn: &Connection,
    dossier: &DossierFile,
    client_id: ClientId,
    gardes: &mut Vec<String>,
) -> Result<bool, AppError> {
    let Some(body) = dossier.travaux.clone() else {
        return Ok(false);
    };
    let current = dossier_work::dossier_work(conn, client_id)?;
    if body == current.body {
        return Ok(false);
    }
    if !current.body.is_empty() && !dossier.reprendre {
        gardes.push("travaux".to_string());
        return Ok(false);
    }
    SaveDossierWork {
        client: client_id,
        body,
        revision: current.revision,
    }
    .apply(conn)?;
    Ok(true)
}

fn write_piste(
    conn: &Connection,
    dossier: &DossierFile,
    quoi: &str,
    opportunity: &mut crate::domain::Opportunity,
    next: Option<Date>,
) -> Result<bool, AppError> {
    let probability = match dossier.probabilite {
        None => opportunity.probability,
        Some(value) => Probability::new(value)
            .map_err(|_| AppError::Domain("probabilité illisible".to_string()))?,
    };
    let source = match dossier.source.as_deref() {
        None => opportunity.source.clone(),
        Some(value) => cleaned(Some(value)),
    };
    let name = quoi.to_string();
    let same = opportunity.name == name
        && opportunity.probability == probability
        && opportunity.source == source
        && opportunity.next_action_at == next;
    if same {
        return Ok(false);
    }
    let revision = UpdateOpportunity {
        id: opportunity.id,
        revision: opportunity.revision,
        name,
        amount: opportunity.amount,
        probability,
        next_action_at: next,
        source,
    }
    .apply(conn)?;
    opportunity.revision = revision;
    Ok(true)
}

fn write_lines(
    conn: &Connection,
    dossier: &DossierFile,
    opportunity: &crate::domain::Opportunity,
    quoi: &str,
) -> Result<bool, AppError> {
    let Some(raw) = &dossier.lignes else {
        return Ok(false);
    };
    if raw.is_empty() {
        return Ok(false);
    }
    let lines = lines_of(raw).map_err(AppError::Domain)?;
    let current = estimation_lines(conn, opportunity.id)?;
    let same = current.len() == lines.len()
        && current
            .iter()
            .zip(&lines)
            .all(|(have, want)| have.0 == want.label && have.1 == want.amount);
    if same {
        return Ok(false);
    }
    let fresh = opportunity_by_id(conn, opportunity.id)?
        .ok_or_else(|| AppError::Domain("piste introuvable".to_string()))?;
    SetEstimation {
        id: fresh.id,
        revision: fresh.revision,
        name: quoi.to_string(),
        lines,
    }
    .apply(conn)?;
    Ok(true)
}

fn write_types(
    conn: &Connection,
    dossier: &DossierFile,
    client_id: ClientId,
) -> Result<bool, AppError> {
    let Some(entries) = &dossier.types else {
        return Ok(false);
    };
    let mut current = work_kinds::dossier_work_kinds(conn, client_id)?;
    let mut names: Vec<String> = current.iter().map(|kind| kind.name.clone()).collect();
    let mut added = false;
    for entry in entries {
        let nom = entry.nom().trim();
        if !names.iter().any(|name| name.eq_ignore_ascii_case(nom)) {
            names.push(nom.to_string());
            added = true;
        }
    }
    if added {
        current = SetDossierWorkKinds {
            client: client_id,
            names,
        }
        .apply(conn)?;
    }
    let mut revised = false;
    for entry in entries {
        let Some(detail) = entry.detail() else {
            continue;
        };
        if detail.teinte.is_none() && detail.note.is_none() {
            continue;
        }
        let Some(kind) = current
            .iter()
            .find(|kind| kind.name.eq_ignore_ascii_case(detail.nom.trim()))
        else {
            continue;
        };
        let note = detail.note.clone().unwrap_or_else(|| kind.note.clone());
        let tint = detail
            .teinte
            .clone()
            .unwrap_or_else(|| kind.tint.slug().to_string());
        ReviseWorkKind {
            id: kind.id.clone(),
            name: kind.name.clone(),
            note,
            tint,
        }
        .apply(conn)?;
        revised = true;
    }
    Ok(added || revised)
}

fn write_serie(
    conn: &Connection,
    dossier: &DossierFile,
    opportunity_id: OpportunityId,
    gardes: &mut Vec<String>,
) -> Result<bool, AppError> {
    let Some(nom) = cleaned(dossier.serie.as_deref()) else {
        return Ok(false);
    };
    let Some(genre) = phrase_genre_named(conn, &nom)? else {
        return Err(AppError::Domain(format!("série inconnue : {nom}")));
    };
    if prospect_genre_for(conn, opportunity_id)?
        .is_some_and(|current| current.name.eq_ignore_ascii_case(&genre.name))
    {
        return Ok(false);
    }
    if prospect_genre_for(conn, opportunity_id)?.is_some() && !dossier.reprendre {
        gardes.push("série".to_string());
        return Ok(false);
    }
    SetDossierGenre {
        subject: FollowUpSubject::Opportunity(opportunity_id),
        name: genre.name,
    }
    .apply(conn)?;
    Ok(true)
}

fn write_brouillon(
    conn: &Connection,
    dossier: &DossierFile,
    opportunity_id: OpportunityId,
    today: Date,
    gardes: &mut Vec<String>,
) -> Result<bool, AppError> {
    let Some(body) = dossier.brouillon.clone() else {
        return Ok(false);
    };
    let subject = FollowUpSubject::Opportunity(opportunity_id);
    let events = events_for(conn, subject)?;
    let active = active_events(&events);
    let sent = active
        .iter()
        .any(|event| event.fact == FollowUpFact::MarkedSent);
    let drafted = active
        .iter()
        .any(|event| event.fact == FollowUpFact::DraftPrepared);
    if sent {
        gardes.push("lettre classée".to_string());
        return Ok(false);
    }
    if drafted && !dossier.reprendre {
        gardes.push("brouillon".to_string());
        return Ok(false);
    }
    if drafted
        && active
            .last()
            .is_some_and(|event| event.fact != FollowUpFact::DraftPrepared)
    {
        gardes.push("brouillon".to_string());
        return Ok(false);
    }
    if drafted {
        RetractLastFollowUp { subject, today }.apply(conn)?;
    }
    PrepareFollowUp {
        subject,
        today,
        subject_line: None,
        body: Some(body),
        recipient: None,
    }
    .apply(conn)?;
    Ok(true)
}

enum FicheChoice {
    New,
    Ready(ClientId),
    Refuse(String),
}

fn resolve_fiche(
    conn: &Connection,
    dossier: &DossierFile,
    qui: &str,
) -> Result<FicheChoice, AppError> {
    if let Some(origin) = cleaned(dossier.origine_fiche.as_deref())
        && let Some(link) = entity_link(conn, &origin, FICHE)?
    {
        let id = link
            .parse::<ClientId>()
            .map_err(|_| AppError::Domain("origine de fiche illisible".to_string()))?;
        return Ok(FicheChoice::Ready(id));
    }
    match reference::resolve_client_exact(conn, qui)? {
        RefMatch::Ambiguous(_) => Ok(FicheChoice::Refuse("plusieurs fiches".to_string())),
        RefMatch::NotFound => Ok(FicheChoice::New),
        RefMatch::Unique(id) => {
            if let Some(origin) = cleaned(dossier.origine_fiche.as_deref())
                && let Some(held) = origin_of(conn, FICHE, &id.to_string())?
                && held != origin
            {
                return Ok(FicheChoice::Refuse(
                    "origine de fiche déjà liée".to_string(),
                ));
            }
            Ok(FicheChoice::Ready(id))
        }
    }
}

enum NamedPistes {
    None,
    Open(crate::domain::Opportunity),
    Closed,
    Several,
}

fn named_pistes(
    conn: &Connection,
    client_id: ClientId,
    quoi: &str,
) -> Result<NamedPistes, AppError> {
    let found: Vec<_> = list_opportunities_with(conn, OpportunityFilter::ALL)?
        .into_iter()
        .filter(|opportunity| opportunity.client_id == client_id && opportunity.name == quoi)
        .filter(|opportunity| opportunity.archived_at.is_none())
        .collect();
    let open: Vec<_> = found
        .iter()
        .filter(|opportunity| !closed(opportunity))
        .cloned()
        .collect();
    if open.len() > 1 {
        return Ok(NamedPistes::Several);
    }
    if let Some(opportunity) = open.into_iter().next() {
        return Ok(NamedPistes::Open(opportunity));
    }
    if found.is_empty() {
        Ok(NamedPistes::None)
    } else {
        Ok(NamedPistes::Closed)
    }
}

fn closed(opportunity: &crate::domain::Opportunity) -> bool {
    opportunity.stage.is_closed() || opportunity.archived_at.is_some()
}

struct PisteLink {
    piste: OpportunityId,
    fingerprint: String,
    next_action: Option<Date>,
    next_action_on: Option<String>,
}

fn piste_link(conn: &Connection, origin: &str) -> Result<Option<PisteLink>, AppError> {
    conn.query_row(
        "SELECT entity_id, fingerprint, next_action_on FROM exchange_links
          WHERE origin = ?1 AND kind = ?2",
        params![origin, PISTE],
        |row| {
            let entity_id: String = row.get(0)?;
            let next_action_on: Option<String> = row.get(2)?;
            Ok((entity_id, row.get(1)?, next_action_on))
        },
    )
    .optional()?
    .map(|(entity_id, fingerprint, next_action_on)| {
        let piste = entity_id
            .parse::<OpportunityId>()
            .map_err(|_| AppError::Domain("origine de piste illisible".to_string()))?;
        let next_action = next_action_on
            .as_deref()
            .map(domain::parse_date)
            .transpose()
            .map_err(|_| AppError::Domain("date illisible".to_string()))?;
        Ok(PisteLink {
            piste,
            fingerprint,
            next_action,
            next_action_on,
        })
    })
    .transpose()
}

fn entity_link(conn: &Connection, origin: &str, kind: &str) -> Result<Option<String>, AppError> {
    conn.query_row(
        "SELECT entity_id FROM exchange_links WHERE origin = ?1 AND kind = ?2",
        params![origin, kind],
        |row| row.get(0),
    )
    .optional()
    .map_err(AppError::from)
}

fn origin_of(conn: &Connection, kind: &str, entity_id: &str) -> Result<Option<String>, AppError> {
    conn.query_row(
        "SELECT origin FROM exchange_links WHERE kind = ?1 AND entity_id = ?2",
        params![kind, entity_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(AppError::from)
}

fn remember(
    conn: &Connection,
    origin: &str,
    kind: &str,
    entity_id: &str,
    fingerprint: &str,
    next_action_on: Option<&str>,
) -> Result<(), AppError> {
    let applied_at = OffsetDateTime::now_utc().format(&Rfc3339)?;
    conn.execute(
        "INSERT INTO exchange_links (origin, kind, entity_id, applied_at, fingerprint, next_action_on)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(origin, kind) DO UPDATE SET
            entity_id = excluded.entity_id,
            applied_at = excluded.applied_at,
            fingerprint = excluded.fingerprint,
            next_action_on = excluded.next_action_on",
        params![origin, kind, entity_id, applied_at, fingerprint, next_action_on],
    )?;
    Ok(())
}

fn fingerprint(dossier: &DossierFile) -> Result<String, AppError> {
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(dossier)?)))
}

fn cleaned(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(ToOwned::to_owned)
}

fn savepoint<T>(
    conn: &Connection,
    name: &str,
    body: impl FnOnce(&Connection) -> Result<T, AppError>,
) -> Result<T, AppError> {
    conn.execute(&format!("SAVEPOINT {name}"), [])?;
    match body(conn) {
        Ok(value) => {
            conn.execute(&format!("RELEASE {name}"), [])?;
            Ok(value)
        }
        Err(error) => {
            let _ = conn.execute(&format!("ROLLBACK TO {name}"), []);
            let _ = conn.execute(&format!("RELEASE {name}"), []);
            Err(error)
        }
    }
}
