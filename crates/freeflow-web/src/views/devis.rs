//! Écran `devis` — lecture et transitions (lot 21), puis création et révision (lot 23).
//!
//! L'éditeur de lignes polymorphes (régie/forfait/récurrent) est un `<textarea>`, une ligne de
//! devis par ligne de texte, parsée par `QuoteLine: FromStr` — le même parseur que `freeflow
//! quote create --line`, jamais réimplémenté par façade (thèse « Studio »). C'est la leçon du
//! lot 16 (jalons de mission) appliquée telle quelle : `serde_urlencoded` ne conserve que la
//! dernière valeur d'une clé répétée, et la CSP `script-src 'self'` de la fenêtre packagée
//! interdit tout « + ajouter une ligne » en JS inline — le textarea est la forme qui reste.
//!
//! Un devis n'a **pas** de révision optimiste : son contenu est immuable par trigger dès la
//! création (réviser = nouvelle version portant le même `root_id`), il n'y a donc pas d'édition
//! en place à protéger — pas de champ caché `revision` dans ces formulaires.

use freeflow_core::app::AppError;
use freeflow_core::domain::{Discount, LineKind, Money, Quote, QuoteId, QuoteLine, QuoteStatus};
use freeflow_core::quotes::{
    QuoteReferences, derive_mission, list_quotes, priced_lines, quote_references,
};
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

fn lines_to_text(lines: &[QuoteLine]) -> String {
    lines
        .iter()
        .map(std::string::ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Default, Clone)]
pub struct QuoteFormValues {
    pub client: String,
    pub opportunity: String,
    pub lines: String,
    pub discount_percent: String,
    pub discount_amount: String,
    pub terms: String,
    pub valid_until: String,
}

impl QuoteFormValues {
    /// Pré-remplit une révision depuis une version existante — client et opportunité n'y
    /// figurent pas : `ReviseQuote` les hérite de la racine, ils ne sont pas rééditables.
    pub fn from_quote(quote: &Quote) -> Self {
        let (discount_percent, discount_amount) = match quote.discount {
            Some(Discount::Percentage(bps)) => (bps.to_string(), String::new()),
            Some(Discount::FixedAmount(m)) => (String::new(), m.to_decimal_string()),
            None => (String::new(), String::new()),
        };
        Self {
            client: String::new(),
            opportunity: String::new(),
            lines: lines_to_text(&quote.lines),
            discount_percent,
            discount_amount,
            terms: quote.terms.clone().unwrap_or_default(),
            valid_until: freeflow_core::domain::format_date(quote.valid_until),
        }
    }
}

#[derive(Default)]
pub struct QuoteFormErrors {
    pub client: Option<String>,
    pub opportunity: Option<String>,
    pub lines: Option<String>,
    pub discount: Option<String>,
    pub valid_until: Option<String>,
    pub banner: Option<String>,
}

fn quote_form(
    action: &str,
    show_client: bool,
    submit_label: &str,
    values: &QuoteFormValues,
    errors: &QuoteFormErrors,
) -> Markup {
    html! {
        form hx-post=(action) hx-target="#panel" hx-swap="innerHTML" {
            @if let Some(msg) = &errors.banner {
                (form::error_banner(msg))
            }
            @if show_client {
                (form::text("client", "Client (nom ou référence)", &values.client, errors.client.as_deref()))
                (form::text("opportunity", "Opportunité liée (optionnel — nom ou référence)", &values.opportunity, errors.opportunity.as_deref()))
            }
            (form::textarea("lines", "Lignes (une par ligne)", &values.lines, 6, errors.lines.as_deref()))
            (form::field_help(
                "Syntaxe : description:type:montant[:taux] — ex. Développement:forfait:1350.00, \
                 Conseil:regie:650.00x10 (TJM×jours), TMA:recurrent:2000.00x12 (mensuel×mois). \
                 Taux : standard (défaut), intermediate, reduced, super_reduced, zero."
            ))
            (form::number("discount_percent", "Remise en % (dix-millièmes, 1000 = 10 %)", &values.discount_percent, "1", errors.discount.as_deref()))
            (form::number("discount_amount", "Remise en montant fixe (€ HT)", &values.discount_amount, "0.01", None))
            (form::field_help("Les deux remises sont exclusives — laisser les deux champs vides pour aucune remise."))
            (form::textarea("terms", "Conditions (optionnel)", &values.terms, 3, None))
            (form::date("valid_until", "Valide jusqu'au", &values.valid_until, errors.valid_until.as_deref()))
            (form::actions(submit_label))
        }
    }
}

pub fn new_panel(values: &QuoteFormValues, errors: &QuoteFormErrors) -> Markup {
    panel::sheet(
        "Nouveau devis",
        quote_form("/devis", true, "Créer le devis (v1)", values, errors),
    )
}

pub fn revise_panel(
    quote: &Quote,
    client: &str,
    values: &QuoteFormValues,
    errors: &QuoteFormErrors,
) -> Markup {
    let body = html! {
        div class="detail-note" {
            "La révision crée une nouvelle version en brouillon à partir de ces valeurs — les "
            "versions existantes de la lignée restent telles quelles (un devis est immuable)."
        }
        (quote_form(
            &format!("/devis/{}/revise", quote.id),
            false,
            "Créer la nouvelle version",
            values,
            errors,
        ))
    };
    panel::sheet(
        &format!("Réviser le devis — {client} (v{})", quote.version),
        body,
    )
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
        // La contrainte de `derive_mission` (un seul type de facturation, régie/récurrent sur
        // une seule ligne) ne bloque pas la création d'un devis — mais son acceptation, si. La
        // surfacer dès la fiche évite de la découvrir au moment de conclure.
        @if matches!(quote.status, QuoteStatus::Draft | QuoteStatus::Sent) {
            @if let Err(e) = derive_mission(quote, quote.valid_until) {
                div class="form-error" { (e.to_string()) " — l'acceptation échouera en l'état, révisez d'abord les lignes." }
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
            // Réviser reste possible quel que soit le statut : la révision repart de la racine
            // de la lignée et crée une version neuve en brouillon, elle ne touche pas celle-ci.
            button class="btn" hx-get=(format!("/devis/{}/revise", quote.id)) hx-target="#panel" hx-swap="innerHTML" { "réviser…" }
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
            div class="pipe-toolbar" {
                button class="btn primary" hx-get="/devis/new" hx-target="#panel" hx-swap="innerHTML" { "+ nouveau devis" }
            }
            @if quotes.is_empty() {
                div class="empty-state" { "aucun devis — cliquez sur « nouveau devis »" }
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
