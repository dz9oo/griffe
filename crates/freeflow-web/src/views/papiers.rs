//! Chapitre Les papiers : checklist, dépôt, pack contrôle.

use freeflow_cli::ControlPackReport;
use freeflow_core::app::AppError;
use freeflow_core::closing::StepStatus;
use freeflow_core::domain::{PaperKind, format_date_fr};
use freeflow_core::fiscal_year::list_fiscal_years;
use freeflow_core::papers::{
    Paper, PapersCheckItem, PapersChecklist, missing_identity_papers, papers_checklist,
};
use freeflow_core::store::Store;
use maud::{Markup, html};
use time::Date;

use crate::layout::ViewId;
use crate::views::form;

fn kind_options() -> Vec<(&'static str, &'static str)> {
    PaperKind::sasu_kinds()
        .iter()
        .map(|k| (k.as_str(), k.label_fr()))
        .collect()
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
    let mut papers = freeflow_core::papers::list_papers(
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
    .filter(|p| matches!(p.kind, PaperKind::Statutes | PaperKind::Kbis));
    papers.extend(identity);
    let receipts = store.receipts_dir().display().to_string();
    let millesimes: Vec<i32> = list_fiscal_years(store.connection())?
        .into_iter()
        .map(|y| y.ends_on.year())
        .collect();
    Ok(chapter_markup(
        period,
        today,
        &checklist,
        &papers,
        banner,
        &receipts,
        &millesimes,
    ))
}

fn groups_of(
    items: &[PapersCheckItem],
    born_here: bool,
) -> Vec<(PaperKind, Vec<&PapersCheckItem>)> {
    let mut groups: Vec<(PaperKind, Vec<&PapersCheckItem>)> = Vec::new();
    for item in items {
        if item.kind.is_born_here() != born_here {
            continue;
        }
        if item.status == StepStatus::Later {
            continue;
        }
        if let Some((_, bucket)) = groups.iter_mut().find(|(k, _)| *k == item.kind) {
            bucket.push(item);
        } else {
            groups.push((item.kind, vec![item]));
        }
    }
    groups
}

fn group_status(items: &[&PapersCheckItem]) -> StepStatus {
    if items.iter().all(|i| i.status == StepStatus::Done) {
        return StepStatus::Done;
    }
    if items.iter().any(|i| i.status == StepStatus::Blocked) {
        return StepStatus::Blocked;
    }
    if items.iter().any(|i| i.status == StepStatus::Todo) {
        return StepStatus::Todo;
    }
    if items.iter().any(|i| i.status == StepStatus::Warning) {
        return StepStatus::Warning;
    }
    if items.iter().all(|i| i.status == StepStatus::Later) {
        return StepStatus::Later;
    }
    items.first().map_or(StepStatus::Info, |i| i.status)
}

fn fiche_class(status: StepStatus) -> &'static str {
    match status {
        StepStatus::Done => "paper-fiche is-done",
        StepStatus::Later | StepStatus::Info => "paper-fiche is-later",
        StepStatus::Todo | StepStatus::Blocked => "paper-fiche is-todo",
        StepStatus::Warning => "paper-fiche is-warn",
    }
}

fn papers_of<'a>(items: &[&PapersCheckItem], papers: &'a [Paper]) -> Vec<&'a Paper> {
    items
        .iter()
        .filter_map(|item| {
            let id = item.paper_id?;
            papers.iter().find(|p| p.id == id)
        })
        .collect()
}

fn original_names<'a>(items: &[&PapersCheckItem], papers: &'a [Paper]) -> Vec<&'a str> {
    papers_of(items, papers)
        .into_iter()
        .map(|p| p.original_name.as_str())
        .collect()
}

fn kicker_body(kind: PaperKind, items: &[&PapersCheckItem], papers: &[Paper]) -> String {
    let n = items.len();
    let done = items
        .iter()
        .filter(|i| i.status == StepStatus::Done)
        .count();
    let names = original_names(items, papers);
    if n > 1 {
        if done == n {
            return match names.first() {
                Some(name) => format!("{n} au coffre · {name}"),
                None => format!("{n} au coffre."),
            };
        }
        let missing = n - done;
        let miss = if missing == 1 {
            "1 manquante".to_string()
        } else {
            format!("{missing} manquantes")
        };
        return format!("{done} au coffre, {miss}.");
    }
    match group_status(items) {
        StepStatus::Done => match names.first() {
            Some(name) => format!("Au coffre · {name}"),
            None => "Au coffre.".into(),
        },
        StepStatus::Todo | StepStatus::Warning if kind.is_born_here() => "À produire ici.".into(),
        StepStatus::Todo | StepStatus::Warning => "À déposer.".into(),
        StepStatus::Later => "Plus tard, à la clôture.".into(),
        StepStatus::Blocked => "Bloqué.".into(),
        StepStatus::Info => "Pour information.".into(),
    }
}

fn identity_note(checklist: &PapersChecklist) -> Option<String> {
    let missing = missing_identity_papers(checklist);
    match missing.as_slice() {
        [] => None,
        [PaperKind::Statutes] => Some("Il manque les statuts.".into()),
        [PaperKind::Kbis] => Some("Il manque le Kbis.".into()),
        [PaperKind::Statutes, PaperKind::Kbis] | [PaperKind::Kbis, PaperKind::Statutes] => {
            Some("Il manque les statuts et le Kbis.".into())
        }
        [other] => Some(format!("Il manque {}.", other.label_fr())),
        rest => Some(format!("Il manque {} pièces à déposer.", rest.len())),
    }
}

fn default_kind(checklist: &PapersChecklist) -> &'static str {
    missing_identity_papers(checklist)
        .first()
        .map(|k| k.as_str())
        .unwrap_or("kbis")
}

fn missing_bring(kind: PaperKind, status: StepStatus) -> bool {
    !kind.is_born_here() && matches!(status, StepStatus::Todo | StepStatus::Warning)
}

fn later_born_here_line(items: &[PapersCheckItem]) -> Option<&'static str> {
    let later: Vec<PaperKind> = items
        .iter()
        .filter(|i| i.kind.is_born_here() && i.status == StepStatus::Later)
        .map(|i| i.kind)
        .collect();
    if later.is_empty() {
        return None;
    }
    let only_approval = later
        .iter()
        .all(|k| matches!(k, PaperKind::Minutes | PaperKind::Appropriation));
    Some(if only_approval {
        "Le PV et l'affectation se figent ici, à l'approbation."
    } else {
        "Le PV, le FEC, les factures se figent ici, à la clôture."
    })
}

fn pick_file(id: &str) -> Markup {
    html! {
        label class="pick-file" for=(id) {
            input class="sr-only" id=(id) name="file" type="file"
                  accept=".pdf,.png,.jpg,.jpeg,.txt";
            span class="pick-name" { "le fichier" }
        }
    }
}

fn deposit_form(period: i32, kind: &str, file_id: &str) -> Markup {
    html! {
        form class="deposit" hx-post="/societe/papiers"
             hx-encoding="multipart/form-data" hx-swap="none" {
            (form::hidden("kind", kind))
            (form::hidden("period", &period.to_string()))
            div class="deposit-act" {
                (pick_file(file_id))
                button class="seal" type="submit" { "Déposer" }
            }
        }
    }
}

fn paper_fiche(
    kind: PaperKind,
    items: &[&PapersCheckItem],
    papers: &[Paper],
    period: i32,
) -> Markup {
    let brief = kind.brief();
    let status = group_status(items);
    let required = items
        .iter()
        .any(|i| i.required && i.status != StepStatus::Done);
    let kick = kicker_body(kind, items, papers);
    let unfold_id = format!("paper-unfold-{}", kind.as_str());
    let open = missing_bring(kind, status);
    let expanded = if open { "true" } else { "false" };
    html! {
        li class={ @if open { "open" } } {
            button class=(fiche_class(status)) type="button"
                   aria-expanded=(expanded) aria-controls=(unfold_id) {
                span class="spot" {}
                div {
                    span class="nm" { (brief.title) }
                    p {
                        (kick)
                        @if required {
                            span class="need" { " Requis." }
                        }
                    }
                }
            }
            div class="unfold" id=(unfold_id) {
                @if open {
                    (deposit_form(period, kind.as_str(), &format!("file-{}", kind.as_str())))
                }
                @for paper in papers_of(items, papers) {
                    p class="prose" {
                        a href=(format!("/societe/papiers/{}", paper.id)) target="_blank" {
                            "Ouvrir " (paper.original_name)
                        }
                    }
                }
                p class="paper-official" { (brief.official) }
                p class="prose" { (brief.what) }
                p class="prose" { (brief.whence) }
                p class="paper-meta" { "Vit au coffre, chiffré." }
            }
        }
    }
}

fn paper_list(items: &[PapersCheckItem], papers: &[Paper], born_here: bool, period: i32) -> Markup {
    let groups = groups_of(items, born_here);
    html! {
        @if groups.is_empty() {
            @if !born_here {
                p class="prose" { "Rien à apporter pour cet exercice." }
            }
        } @else {
            ol class="papers" {
                @for (kind, group) in &groups {
                    (paper_fiche(*kind, group.as_slice(), papers, period))
                }
            }
        }
    }
}

fn year_control(period: i32, millesimes: &[i32]) -> Markup {
    if millesimes.len() < 2 {
        return html! {
            b id="period" { (period) }
        };
    }
    html! {
        nav class="millesimes" id="period" aria-label="Exercice" {
            @for year in [period - 1, period, period + 1] {
                @if year == period {
                    b { (year) }
                } @else {
                    a href=(format!("/societe/papiers?period={year}"))
                      hx-get=(format!("/societe/papiers?period={year}"))
                      hx-target="#content" hx-push-url="true" hx-swap="innerHTML" {
                        (year)
                    }
                }
            }
        }
    }
}

fn mast(period: i32, today: Date, millesimes: &[i32]) -> Markup {
    html! {
        div class="mast" {
            div class="mast-facts" {
                span {
                    span class="dim" { "Exercice clos en" }
                    " "
                    (year_control(period, millesimes))
                }
                span {
                    span class="dim" { "vu le" }
                    " "
                    (format_date_fr(today))
                }
            }
        }
    }
}

fn places(period: i32, receipts_dir: &str) -> Markup {
    html! {
        h2 { "Deux endroits." }
        div class="places" {
            div class="place" {
                p class="kicker" { "chiffré" }
                h3 { "Au coffre" }
                p class="prose" {
                    "Tout ce qui est « au coffre » vit ici, à côté du fichier du coffre, "
                    "chiffré avec la même clé. Les sauvegardes l'emportent."
                }
                p class="path" title=(receipts_dir) { (receipts_dir) }
            }
            div class="places-rule" aria-hidden="true" {}
            div class="place" {
                p class="kicker" { "en clair" }
                h3 { "Pour un contrôle" }
                p class="prose" {
                    "Un dossier neuf, à tendre. "
                    a href="/aide/papiers"
                      hx-get="/aide/papiers"
                      hx-target="#content"
                      hx-push-url="true" {
                        "Qu'est-ce qu'un contrôle ?"
                    }
                    " Ces fichiers ne sont plus chiffrés : "
                    "ne les laissez pas à côté du coffre."
                }
                form class="place-act"
                     hx-post="/societe/papiers/export"
                     hx-target="#pack-result" hx-swap="innerHTML" {
                    (form::hidden("period", &period.to_string()))
                    button class="seal" type="submit" { "Préparer le dossier d'un contrôle" }
                }
                div id="pack-result" {}
            }
        }
    }
}

fn deposit_other(period: i32, selected: &str) -> Markup {
    let options = kind_options();
    html! {
        details class="deposit-other" {
            summary { "une autre pièce" }
            form class="deposit" hx-post="/societe/papiers"
                 hx-encoding="multipart/form-data" hx-swap="none" {
                (form::hidden("period", &period.to_string()))
                p class="deposit-line" {
                    "Je dépose "
                    select name="kind" id="kind" {
                        @for (value, text) in &options {
                            option value=(value) selected[*value == selected] { (*text) }
                        }
                    }
                    " de l'exercice "
                    (period)
                    "."
                }
                div class="deposit-act" {
                    (pick_file("file-other"))
                    button class="seal" type="submit" { "Déposer" }
                }
            }
        }
    }
}

fn chapter_markup(
    period: i32,
    today: Date,
    checklist: &PapersChecklist,
    papers: &[Paper],
    banner: Option<&str>,
    receipts_dir: &str,
    millesimes: &[i32],
) -> Markup {
    let href = format!("/societe/papiers?period={period}");
    let note = identity_note(checklist);
    let selected = default_kind(checklist);
    let later = later_born_here_line(&checklist.items);
    html! {
        div class="letter" id="papiers-letter" data-view=(ViewId::Societe.slug())
            hx-get=(href)
            hx-trigger="freeflow:saved from:body"
            hx-swap="outerHTML"
            hx-disinherit="hx-swap" {
            (back())
            (mast(period, today, millesimes))
            h1 { "Les papiers." }
            p class="lede" {
                "Les originaux nés ici restent ici. Ce que FreeFlow ne produit pas "
                "(Kbis, statuts, accusé de dépôt), vous le déposez."
            }
            @if let Some(msg) = banner {
                p class="prose" role="alert" { (msg) }
            }
            @if let Some(msg) = note {
                p class="mast-note" { (msg) }
            }
            h2 { "Ce que FreeFlow écrit." }
            @if let Some(line) = later {
                p class="prose later-line" {
                    (line)
                    " "
                    a href="/aide/papiers"
                      hx-get="/aide/papiers"
                      hx-target="#content"
                      hx-push-url="true" {
                        "Lesquelles ?"
                    }
                }
            }
            (paper_list(&checklist.items, papers, true, period))
            h2 { "Ce que vous apportez." }
            (paper_list(&checklist.items, papers, false, period))
            (deposit_other(period, selected))
            (places(period, receipts_dir))
        }
    }
}

/// Confirmation après `POST /societe/papiers/export` — le dossier est déjà sur le disque.
#[must_use]
pub fn export_done(report: &ControlPackReport) -> Markup {
    html! {
        div id="pack-result" data-pack-dir=(report.dest.as_str()) {
            p class="prose" { (report.files) " fichiers, dossier ouvert." }
            p class="path" { (report.dest) }
            p class="paper-meta" { (report.warning) }
        }
    }
}
