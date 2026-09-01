//! Écran `devis` (lot 21) — lecture et transitions. Un devis créé était jusqu'ici invisible
//! depuis la fenêtre (aucune query de lecture dans le cœur avant ce lot) ; cet écran le rend
//! lisible (liste, fiche avec lignes et totaux, lignée) et fait passer les transitions de son
//! cycle de vie (envoyer, décliner, accepter — cette dernière avec sa date de démarrage).
//!
//! Volontairement **pas** de création/révision ici : les lignes d'un devis sont polymorphes
//! (régie/forfait/récurrent, avec remise), et la CLI comme le serveur MCP les prennent déjà en
//! JSON — un vrai éditeur de lignes dans le panneau est un chantier d'UI à part entière, en
//! feuille de route, pas un formulaire de plus sur ce patron.

use freeflow_core::app::AppError;
use freeflow_core::domain::{LineKind, Money, Quote, QuoteId, QuoteStatus};
use freeflow_core::quotes::{QuoteReferences, list_quotes, priced_lines, quote_references};
use freeflow_core::store::Store;
use maud::{Markup, html};

use crate::layout::{ViewId, view_head};
use crate::views::{form, panel};

fn status_label(status: QuoteStatus) -> &'static str {
    match status {
        QuoteStatus::Draft => "brouillon",
        QuoteStatus::Sent => "envoyé",
        QuoteStatus::Accepted => "accepté",
        QuoteStatus::Declined => "décliné",
        QuoteStatus::Expired => "expiré",
    }
}

fn status_badge(status: QuoteStatus) -> Markup {
    let class = match status {
        QuoteStatus::Draft => "badge",
        QuoteStatus::Sent => "badge warn",
        QuoteStatus::Accepted => "badge ok",
        QuoteStatus::Declined | QuoteStatus::Expired => "badge danger",
    };
    html! { span class=(class) { (status_label(status)) } }
}

fn line_kind_label(kind: &LineKind) -> String {
    match kind {
        LineKind::Regie { daily_rate, days } => format!("régie — {days} j × {daily_rate}"),
        LineKind::Forfait { .. } => "forfait".to_string(),
        LineKind::Recurrent {
            monthly_amount,
            months,
        } => format!("récurrent — {months} mois × {monthly_amount}"),
    }
}

/// Total HT net de remise — toujours recalculé par `priced_lines`, la seule source de vérité de
/// l'arithmétique de devis, jamais réimplémenté par façade.
pub fn net_total(quote: &Quote) -> Money {
    priced_lines(&quote.lines, quote.discount)
        .iter()
        .map(|(gross, discount)| *gross - *discount)
        .sum()
}

fn client_name(store: &Store, id: freeflow_core::domain::ClientId) -> String {
    freeflow_core::clients::list_clients(store.connection())
        .ok()
        .and_then(|clients| clients.into_iter().find(|c| c.id == id))
        .map_or_else(|| "?".to_string(), |c| c.name)
}

pub fn detail_panel(
    store: &Store,
    quote: &Quote,
    refs: QuoteReferences,
    error: Option<&str>,
) -> Markup {
    let client = client_name(store, quote.client_id);
    let priced = priced_lines(&quote.lines, quote.discount);
    let total = net_total(quote);
    let body = html! {
        @if let Some(msg) = error {
            (form::error_banner(msg))
        }
        div class="detail-head" {
            div class="detail-title" { (client) " — v" (quote.version) }
            (status_badge(quote.status))
        }
        dl class="detail-fields" {
            dt { "Total HT (net de remise)" } dd class="mono" { (total) }
            dt { "Valide jusqu'au" } dd { (freeflow_core::domain::format_date(quote.valid_until)) }
            @if let Some(terms) = &quote.terms {
                dt { "Conditions" } dd { (terms) }
            }
        }
        div class="detail-section" {
            div class="detail-section-head" { span { "Lignes" } }
            div class="panel bordered" style="padding:0" {
                table {
                    tr {
                        th style="padding-left:18px" { "description" }
                        th { "type" }
                        th { "brut ht" }
                        th style="padding-right:18px" { "remise" }
                    }
                    @for (line, (gross, discount)) in quote.lines.iter().zip(&priced) {
                        tr {
                            td style="padding-left:18px" { (line.description) }
                            td { (line_kind_label(&line.kind)) }
                            td class="mono" { (gross) }
                            td class="mono" style="padding-right:18px" {
                                @if discount.cents() > 0 { "−" (discount) } @else { "—" }
                            }
                        }
                    }
                }
            }
        }
        @if refs.missions > 0 || refs.versions > 1 {
            div class="detail-note" {
                @if refs.missions > 0 { "Son acceptation a produit " (refs.missions) " mission(s). " }
                @if refs.versions > 1 { (refs.versions) " versions partagent la même racine." }
            }
        }
        div class="detail-actions" {
            @match quote.status {
                QuoteStatus::Draft => {
                    button class="btn primary" hx-post=(format!("/devis/{}/send", quote.id)) hx-target="#panel" hx-swap="innerHTML" { "marquer envoyé" }
                }
                QuoteStatus::Sent => {
                    button class="btn primary" hx-get=(format!("/devis/{}/accept", quote.id)) hx-target="#panel" hx-swap="innerHTML" { "accepter…" }
                    button class="btn danger" hx-post=(format!("/devis/{}/decline", quote.id)) hx-target="#panel" hx-swap="innerHTML" { "décliner" }
                }
                _ => {}
            }
        }
        div class="detail-note" {
            "Créer ou réviser un devis se fait en CLI (" code { "freeflow quote create/revise" }
            ") ou via un agent MCP — les lignes polymorphes n'ont pas encore d'éditeur dans la fenêtre."
        }
    };
    panel::sheet(&format!("Devis — {client}"), body)
}

pub fn accept_panel(quote: &Quote, error: Option<&str>) -> Markup {
    let body = html! {
        @if let Some(msg) = error {
            (form::error_banner(msg))
        }
        div class="detail-note" {
            "Accepter ce devis crée la mission correspondante (avec son échéancier dérivé des lignes)."
        }
        form hx-post=(format!("/devis/{}/accept", quote.id)) hx-target="#panel" hx-swap="innerHTML" {
            (form::date("started_on", "Date de démarrage de la mission", "", None))
            (form::actions("Accepter le devis"))
        }
    };
    panel::sheet("Accepter le devis", body)
}

pub fn list_fragment(store: &Store) -> Result<Markup, AppError> {
    let quotes = list_quotes(store.connection())?;
    let clients = freeflow_core::clients::list_clients(store.connection())?;
    let name_of = |id: freeflow_core::domain::ClientId| {
        clients
            .iter()
            .find(|c| c.id == id)
            .map_or_else(|| "?".to_string(), |c| c.name.clone())
    };
    Ok(html! {
        div id="devis-list"
            hx-get="/devis/table"
            hx-trigger="freeflow:saved from:body"
            hx-target="this"
            hx-swap="outerHTML" {
            @if quotes.is_empty() {
                div class="empty-state" {
                    "aucun devis — créez-en un depuis la CLI : " code { "freeflow quote create" }
                }
            } @else {
                div class="panel bordered" style="padding:0" {
                    table {
                        tr {
                            th style="padding-left:18px" { "client" }
                            th { "version" }
                            th { "statut" }
                            th { "total ht" }
                            th style="padding-right:18px" { "valide jusqu'au" }
                        }
                        @for quote in &quotes {
                            tr class="row-clickable" hx-get=(format!("/devis/{}", quote.id)) hx-target="#panel" hx-swap="innerHTML" {
                                td style="padding-left:18px" { (name_of(quote.client_id)) }
                                td class="mono" { "v" (quote.version) }
                                td { (status_badge(quote.status)) }
                                td class="mono" { (net_total(quote)) }
                                td class="mono" style="padding-right:18px" { (freeflow_core::domain::format_date(quote.valid_until)) }
                            }
                        }
                    }
                }
            }
        }
    })
}

pub fn render(store: &Store) -> Result<Markup, AppError> {
    let count = list_quotes(store.connection())?.len();
    Ok(html! {
        (view_head(ViewId::Devis, &format!("{count} devis, toutes versions confondues")))
        (list_fragment(store)?)
    })
}

pub fn load(store: &Store, id: QuoteId) -> Result<Option<(Quote, QuoteReferences)>, AppError> {
    let Some(quote) = freeflow_core::quotes::quote_by_id(store.connection(), id)? else {
        return Ok(None);
    };
    let refs = quote_references(store.connection(), id)?;
    Ok(Some((quote, refs)))
}
