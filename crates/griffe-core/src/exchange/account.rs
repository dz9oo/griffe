//! Le texte du compte-rendu. La commande et la fenêtre l'écrivent pareil.

use crate::domain::Money;

use super::document::{DossierFile, ExchangeDocument, parse_montant};
use super::report::{CadenceOutcome, DossierOutcome, ExchangeReport, ExchangeStatus};

/// Les groupes du compte-rendu, dans l'ordre de la commande.
#[must_use]
pub fn account(document: &ExchangeDocument, report: &ExchangeReport) -> String {
    let mut lines = Vec::new();
    push_group(
        &mut lines,
        "Créées",
        rows_for(document, report, ExchangeStatus::Cree),
    );
    push_group(
        &mut lines,
        "Révisées",
        rows_for(document, report, ExchangeStatus::Revise),
    );
    push_group(
        &mut lines,
        "Inchangées",
        rows_for(document, report, ExchangeStatus::Inchange),
    );
    push_group(
        &mut lines,
        "Incomplètes",
        rows_for(document, report, ExchangeStatus::Incomplet),
    );
    push_group(&mut lines, "Montants", montant_rows(document));
    push_group(&mut lines, "Séries à créer", series_to_create(report));
    push_group(&mut lines, "Reprendre écraserait", garder_rows(report));
    lines.join("\n")
}

fn push_group(out: &mut Vec<String>, title: &str, mut rows: Vec<String>) {
    out.push(title.to_string());
    if rows.is_empty() {
        out.push("aucune".to_string());
    } else {
        out.append(&mut rows);
    }
}

fn rows_for(
    document: &ExchangeDocument,
    report: &ExchangeReport,
    statut: ExchangeStatus,
) -> Vec<String> {
    let mut rows = Vec::new();
    for dossier in report.dossiers.iter().filter(|row| row.statut == statut) {
        rows.push(dossier_row(document, dossier));
    }
    for serie in report.series.iter().filter(|row| row.statut == statut) {
        rows.push(format!("série {}", serie.nom));
    }
    if let Some(cadence) = &report.cadence
        && cadence.statut == statut
    {
        rows.push(cadence_row(cadence));
    }
    rows
}

fn dossier_row(document: &ExchangeDocument, outcome: &DossierOutcome) -> String {
    let quoi = document
        .dossiers
        .iter()
        .find(|dossier| dossier.origine.trim() == outcome.origine)
        .map(|dossier| dossier.quoi.trim().to_string())
        .unwrap_or_default();
    let mut line = if quoi.is_empty() {
        outcome.qui.clone()
    } else {
        format!("{} — {quoi}", outcome.qui)
    };
    if let Some(raison) = &outcome.raison {
        line.push_str(" — ");
        line.push_str(raison);
    }
    line
}

fn cadence_row(cadence: &CadenceOutcome) -> String {
    let statut = statut_fr(cadence.statut);
    if cadence.epingle {
        format!("cadence — {statut} — les conversations ouvertes gardent la série d'avant")
    } else if let Some(raison) = &cadence.raison {
        format!("cadence — {statut} — {raison}")
    } else {
        format!("cadence — {statut}")
    }
}

const fn statut_fr(statut: ExchangeStatus) -> &'static str {
    match statut {
        ExchangeStatus::Cree => "créée",
        ExchangeStatus::Revise => "révisée",
        ExchangeStatus::Inchange => "inchangée",
        ExchangeStatus::Incomplet => "incomplète",
    }
}

fn montant_rows(document: &ExchangeDocument) -> Vec<String> {
    document
        .dossiers
        .iter()
        .filter_map(|dossier| {
            montant_of(dossier).map(|montant| {
                format!(
                    "{} — {} — {montant}",
                    dossier.qui.trim(),
                    dossier.quoi.trim()
                )
            })
        })
        .collect()
}

fn montant_of(dossier: &DossierFile) -> Option<String> {
    let lines = dossier.lignes.as_ref()?;
    if lines.is_empty() {
        return None;
    }
    let mut total = Money::ZERO;
    for line in lines {
        match parse_montant(&line.montant) {
            Ok(money) => total += money,
            Err(raison) => return Some(raison),
        }
    }
    Some(total.to_string())
}

fn series_to_create(report: &ExchangeReport) -> Vec<String> {
    report
        .series
        .iter()
        .filter(|serie| serie.statut == ExchangeStatus::Cree)
        .map(|serie| serie.nom.clone())
        .collect()
}

fn garder_rows(report: &ExchangeReport) -> Vec<String> {
    let mut rows = Vec::new();
    for dossier in &report.dossiers {
        if !dossier.gardes.is_empty() {
            rows.push(format!("{} — {}", dossier.qui, dossier.gardes.join(", ")));
        }
    }
    for serie in &report.series {
        if !serie.gardes.is_empty() {
            rows.push(format!("série {} — {}", serie.nom, serie.gardes.join(", ")));
        }
    }
    rows
}
