//! L'assistant de premier lancement (lot 39) : quatre temps — *Ma société*, *D'où venez-vous ?*,
//! *Votre banque*, *Prochaine étape* — sur une seule page, chacun avec son état et, pour celui
//! qui est attendu, le geste à faire. L'état vient de `freeflow_core::setup::setup_status` :
//! la page ne devine rien, elle affiche.

use freeflow_core::app::AppError;
use freeflow_core::company::company_profile;
use freeflow_core::setup::{Prerequisite, SetupStep, setup_status};
use freeflow_core::store::Store;
use maud::{Markup, html};

use crate::layout::ViewId;
use crate::views::societe::{ProfileFormErrors, ProfileFormValues, profile_form};

fn glyph(p: &Prerequisite) -> Markup {
    match p {
        Prerequisite::Done => html! { span class="badge ok" { "fait" } },
        Prerequisite::Missing => html! { span class="badge warn" { "à faire" } },
        Prerequisite::Incomplete { .. } => html! { span class="badge warn" { "à compléter" } },
    }
}

/// La page de l'assistant, dans la coque (le coffre est ouvert).
pub fn render(store: &Store) -> Result<Markup, AppError> {
    let status = setup_status(store.connection())?;
    let profile = company_profile(store.connection())?;
    let values = profile
        .as_ref()
        .map_or_else(ProfileFormValues::blank, ProfileFormValues::from);
    let fiscal_year_end = profile
        .as_ref()
        .and_then(|p| p.fiscal_year_end)
        .unwrap_or(freeflow_core::domain::FiscalYearEnd::CALENDAR);
    let today = freeflow_core::clock::today_local();
    let last_ended = fiscal_year_end
        .previous(fiscal_year_end.current(today))
        .end()
        .year();
    Ok(html! {
        div class="view-head" {
            div {
                div class="view-title" { "Premiers pas" }
                div class="view-sub" { (status.next_step.text()) }
            }
        }

        div class="panel bordered" id="premiers-pas" {
            h3 { "1. Ma société " (glyph(&status.profile)) }
            @if let Prerequisite::Incomplete { fields } = &status.profile {
                div class="detail-note" { "À compléter : " (fields.join(", ")) "." }
            }
            (profile_form("/societe", &values, &ProfileFormErrors::default(), None, "Enregistrer ma société"))
        }

        div class="panel bordered" {
            h3 { "2. D'où venez-vous ? " (glyph(&status.origin)) }
            @if status.declared_new_company {
                div class="detail-note" { "Société nouvelle : pas de bilan à reprendre, les comptes partent de zéro. " button class="btn small" hx-post="/premiers-pas/nouvelle?undo=1" hx-target="#content" hx-swap="innerHTML" { "annuler ce choix" } }
            } @else if status.origin.is_done() {
                div class="detail-note" { "Bilan d'ouverture repris : le point de départ est posé. " button class="btn small" hx-get="/cloture/opening" hx-target="#panel" hx-swap="innerHTML" { "le revoir" } }
            } @else {
                div class="detail-note" {
                    strong { "La société existait déjà" } " (un cabinet tenait les comptes) : demandez-lui la "
                    em { "balance de clôture" } " ou le dernier bilan, et recopiez-le compte par compte — c'est \
                    le point de départ de tout (report à nouveau, réserve, trésorerie)."
                }
                div class="form-actions" {
                    button class="btn primary" hx-get="/cloture/opening" hx-target="#panel" hx-swap="innerHTML" { "recopier le bilan du cabinet" }
                }
                div class="detail-note" {
                    strong { "La société vient d'être créée" } " : rien à reprendre, les comptes partent de zéro."
                }
                div class="form-actions" {
                    button class="btn" hx-post="/premiers-pas/nouvelle" hx-target="#content" hx-swap="innerHTML" { "société nouvelle, rien à reprendre" }
                }
            }
        }

        div class="panel bordered" {
            h3 { "3. Votre banque " (glyph(&status.bank)) }
            @if status.bank.is_done() {
                div class="detail-note" { "Relevé importé : les dépenses se saisissent depuis ses lignes (écran dépenses)." }
            } @else {
                div class="detail-note" {
                    "Importez l'export de votre banque tel quel (CSV ou OFX) : c'est lui qui vous \
                     dira quoi saisir, ligne par ligne."
                }
                div class="form-actions" {
                    button class="btn primary" hx-get="/banque/import" hx-target="#panel" hx-swap="innerHTML" { "importer un relevé" }
                }
            }
        }

        div class="panel bordered" {
            h3 { "4. Prochaine étape " @if status.is_done() { span class="badge ok" { "prêt" } } }
            @match status.next_step {
                SetupStep::Done => {
                    div class="detail-note" {
                        "Tout est en place. Le " strong { "parcours de clôture" } " vous dit désormais où vous en êtes et quoi faire ensuite, exercice par exercice."
                    }
                    div class="form-actions" {
                        button class="btn primary" hx-get=(format!("/cloture/checklist?period={last_ended}")) hx-target="#panel" hx-swap="innerHTML" { "ouvrir le parcours de l'exercice " (last_ended) }
                        " "
                        a class="btn" href=(ViewId::Cloture.path()) hx-get=(ViewId::Cloture.path()) hx-target="#content" hx-push-url="true" { "écran clôture" }
                    }
                }
                _ => {
                    div class="detail-note" { (status.next_step.text()) }
                }
            }
        }
    })
}
