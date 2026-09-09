//! Chapitre Les papiers (lot 59) : checklist de conservation et dépôt d'une pièce extérieure.

use freeflow_core::app::AppError;
use freeflow_core::closing::StepStatus;
use freeflow_core::domain::{PaperKind, format_date_fr};
use freeflow_core::papers::{Paper, PapersChecklist, papers_checklist};
use freeflow_core::store::Store;
use maud::{Markup, html};
use time::Date;

use crate::layout::ViewId;
use crate::views::form;

pub fn kind_options() -> Vec<(&'static str, &'static str)> {
    PaperKind::sasu_kinds()
        .iter()
        .map(|k| (k.as_str(), k.label_fr()))
        .collect()
}

fn glyph(status: StepStatus) -> &'static str {
    match status {
        StepStatus::Done => "✓",
        StepStatus::Todo => "→",
        StepStatus::Warning => "!",
        StepStatus::Blocked => "✗",
        StepStatus::Info => "·",
        StepStatus::Later => "○",
    }
}

fn back() -> Markup {
    html! {
        a class="back" href="/societe"
          hx-get="/societe" hx-target="#content" hx-push-url="true" hx-swap="innerHTML" {
            "← La société"
        }
    }
}

/// Le chapitre : checklist de l'exercice, pièces déjà au coffre, formulaire de dépôt.
pub fn chapter(
    store: &Store,
    today: Date,
    period: i32,
    banner: Option<&str>,
) -> Result<Markup, AppError> {
    let checklist = papers_checklist(store.connection(), period, today)?;
    let papers = freeflow_core::papers::list_papers(
        store.connection(),
        freeflow_core::papers::PaperFilter {
            period: Some(period),
            kind: None,
            include_superseded: false,
        },
    )?;
    let identity = freeflow_core::papers::list_papers(
        store.connection(),
        freeflow_core::papers::PaperFilter::ACTIVE,
    )?
    .into_iter()
    .filter(|p| matches!(p.kind, PaperKind::Statutes | PaperKind::Kbis))
    .collect::<Vec<_>>();
    Ok(chapter_markup(
        period, today, &checklist, &papers, &identity, banner,
    ))
}

fn chapter_markup(
    period: i32,
    today: Date,
    checklist: &PapersChecklist,
    papers: &[Paper],
    identity: &[Paper],
    banner: Option<&str>,
) -> Markup {
    let href = format!("/societe/papiers?period={period}");
    let options = kind_options();
    html! {
        div class="letter" id="papiers-letter" data-view=(ViewId::Societe.slug())
            hx-get=(href)
            hx-trigger="freeflow:saved from:body"
            hx-swap="outerHTML"
            hx-disinherit="hx-swap" {
            (back())
            h1 { "Les papiers." }
            p class="lede" {
                "Les originaux nés ici restent ici. Ce que FreeFlow ne produit pas "
                "(Kbis, statuts, accusé de dépôt), vous le déposez."
            }
            @if let Some(msg) = banner {
                p class="prose" role="alert" { (msg) }
            }
            p class="prose" {
                "Exercice clos en " (period) ", vu le " (format_date_fr(today)) "."
            }
            form hx-get="/societe/papiers" hx-target="#content" hx-push-url="true"
                 style="display:inline-flex;gap:8px;align-items:center" {
                label { "exercice clos en " }
                input type="number" name="period" value=(period) min="2000" max="2100"
                      style="width:6em" {}
                button class="btn small" type="submit" { "voir" }
            }
            h2 { "À avoir au coffre" }
            @if checklist.items.is_empty() {
                p class="prose" { "Rien à signaler pour cet exercice." }
            }
            ul class="plain" {
                @for item in &checklist.items {
                    li {
                        (glyph(item.status)) " " (item.kind.label_fr())
                        @if item.required { " — requis" }
                    }
                }
            }
            h2 { "Déjà au coffre" }
            @if papers.is_empty() && identity.is_empty() {
                p class="prose" { "Aucune pièce indexée pour cet exercice." }
            }
            @for p in identity {
                p class="prose" { (p.kind.label_fr()) " · " (p.original_name) }
            }
            @for p in papers {
                @if !matches!(p.kind, PaperKind::Statutes | PaperKind::Kbis) {
                    p class="prose" { (p.kind.label_fr()) " · " (p.original_name) }
                }
            }
            h2 { "Déposer une pièce" }
            form hx-post="/societe/papiers" hx-encoding="multipart/form-data" hx-swap="none" {
                (form::select("kind", "Nature", &options, "kbis", None))
                (form::hidden("period", &period.to_string()))
                (form::file("file", "Fichier", ".pdf,.png,.jpg,.jpeg,.txt"))
                (form::actions("Déposer"))
            }
        }
    }
}
