//! Import d'un relevé bancaire depuis la fenêtre (lot 38) : un panneau avec un sélecteur de
//! fichier, un aperçu de ce que la détection a compris (dialecte, colonnes, lignes retenues,
//! doublons, lignes sautées), puis l'import. L'aperçu porte le fichier lui-même (base64, champ
//! caché) : rien n'est gardé côté serveur entre les deux étapes — la fenêtre reste sans état,
//! comme tous ses écrans.

use freeflow_core::billing::{ParsedStatement, StatementFormat};
use freeflow_core::domain::{Money, format_date};
use maud::{Markup, html};

use crate::views::{form, panel};

/// Où trouver l'export dans les banques courantes — une ligne par banque, pour que l'utilisateur
/// n'ait pas à chercher.
const WHERE_TO_EXPORT: [(&str, &str); 7] = [
    ("Qonto", "Transactions → Exporter → CSV (toutes colonnes)"),
    ("Shine", "Transactions → Exporter → CSV"),
    ("Boursorama", "Mes comptes → Mouvements → Télécharger (CSV)"),
    (
        "Crédit Agricole",
        "Comptes → Télécharger les opérations → CSV ou OFX",
    ),
    ("BNP Paribas", "Comptes → Télécharger → CSV ou OFX"),
    ("LCL", "Comptes → Exporter les opérations → CSV"),
    (
        "La Banque Postale",
        "Comptes → Télécharger les opérations → CSV",
    ),
];

/// Le bouton « importer un relevé », partagé par les écrans `depenses` et `facturation`.
pub fn import_button() -> Markup {
    html! {
        button class="btn" hx-get="/banque/import" hx-target="#panel" hx-swap="innerHTML" title="L'export CSV ou OFX de votre banque, tel quel" { "importer un relevé" }
    }
}

pub fn import_panel(error: Option<&str>) -> Markup {
    let body = html! {
        @if let Some(msg) = error {
            (form::error_banner(msg))
        }
        form hx-post="/banque/import/preview" hx-target="#panel" hx-swap="innerHTML" hx-encoding="multipart/form-data" {
            (form::file("statement", "Fichier de relevé (CSV ou OFX)", ".csv,.txt,.ofx,.qfx,text/csv,application/x-ofx"))
            (form::field_help(
                "L'export de votre banque tel quel : encodage, séparateur, décimale, format de \
                 date et colonnes sont reconnus automatiquement (Qonto, Shine, Boursorama, Crédit \
                 Agricole, BNP, LCL, La Banque Postale, OFX). Vous verrez d'abord ce qui a été \
                 compris, puis vous confirmerez l'import. Réimporter un relevé ne crée pas de \
                 doublon."
            ))
            (form::actions("Analyser le fichier"))
        }
        details {
            summary { "Où trouver l'export dans ma banque ?" }
            ul {
                @for (bank, path) in WHERE_TO_EXPORT {
                    li { strong { (bank) } " : " (path) }
                }
            }
        }
    };
    panel::sheet("Importer un relevé bancaire", body)
}

/// L'aperçu : dialecte, colonnes, nombre de nouvelles lignes et de doublons, les cinq
/// premières, les lignes sautées — et le bouton d'import, qui renvoie le fichier (base64).
pub fn preview_panel(
    parsed: &ParsedStatement,
    fresh: &[bool],
    payload_base64: &str,
    filename: &str,
) -> Markup {
    let d = &parsed.dialect;
    let duplicates = fresh.iter().filter(|f| !**f).count();
    let new = parsed.transactions.len() - duplicates;
    let dialect = format!(
        "{} en {}{}{}{}{}",
        match d.format {
            StatementFormat::Csv => "CSV",
            StatementFormat::Ofx => "OFX",
        },
        d.encoding,
        d.separator.map_or(String::new(), |s| format!(
            ", séparateur « {} »",
            if s == '\t' {
                "tabulation".to_string()
            } else {
                s.to_string()
            }
        )),
        d.decimal
            .map_or(String::new(), |c| format!(", décimale « {c} »")),
        d.date_format
            .as_deref()
            .map_or(String::new(), |f| format!(", dates {f}")),
        d.header_line
            .map_or(String::new(), |l| format!(", en-tête ligne {l}")),
    );
    let body = html! {
        div class="detail-note" {
            strong { (filename) } " lu comme " (dialect) ". Colonnes : " (d.columns) "."
        }
        div class="detail-note" {
            (parsed.transactions.len()) " mouvement(s) lu(s) : "
            strong { (new) " nouveau(x)" }
            @if duplicates > 0 { ", " (duplicates) " déjà importé(s) (ignorés)" }
            "."
        }
        @if !parsed.skipped.is_empty() {
            div class="detail-note" {
                (parsed.skipped.len()) " ligne(s) sautée(s) : "
                @for (i, (line, reason)) in parsed.skipped.iter().enumerate() {
                    @if i > 0 { " ; " }
                    "ligne " (line) " (" (reason) ")"
                }
            }
        }
        div class="panel bordered" style="padding:0;margin-bottom:12px" {
            table {
                tr { th style="padding-left:18px" { "date" } th { "montant" } th { "libellé" } th style="padding-right:18px" { "état" } }
                @for (tx, is_new) in parsed.transactions.iter().zip(fresh).take(8) {
                    tr {
                        td style="padding-left:18px" class="mono" { (format_date(tx.occurred_on)) }
                        td class="mono" { (Money::from_cents(tx.amount_cents)) }
                        td { (tx.description) }
                        td style="padding-right:18px" {
                            @if *is_new { span class="badge ok" { "nouveau" } } @else { span class="badge warn" { "déjà importé" } }
                        }
                    }
                }
            }
            @if parsed.transactions.len() > 8 {
                div class="detail-note" { "… et " (parsed.transactions.len() - 8) " autre(s)" }
            }
        }
        @if new > 0 {
            form hx-post="/banque/import" hx-target="#panel" hx-swap="innerHTML" {
                (form::hidden("payload", payload_base64))
                (form::hidden("filename", filename))
                (form::actions(&format!("Importer {new} mouvement(s)")))
            }
        } @else {
            div class="empty-state" { "rien de nouveau à importer" }
        }
        div class="form-actions" {
            button class="btn" hx-get="/banque/import" hx-target="#panel" hx-swap="innerHTML" { "choisir un autre fichier" }
        }
    };
    panel::sheet("Aperçu du relevé", body)
}
