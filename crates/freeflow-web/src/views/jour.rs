//! Le jour : la lettre du matin. Lot 48 assemble ce que les queries existantes savent déjà
//! (relances, relevé, prochaine échéance, assistant). Le mât et le mois arrivent au lot 49.

use freeflow_core::app::AppError;
use freeflow_core::billing::list_bank_transactions;
use freeflow_core::domain::{FollowUpKind, Money, format_date_fr};
use freeflow_core::fiscal::fiscal_calendar;
use freeflow_core::follow_up::{CardStatus, FollowUpCard, follow_up_queue};
use freeflow_core::setup::setup_status;
use freeflow_core::store::Store;
use maud::{Markup, html};
use time::Date;

use crate::layout::ViewId;
use crate::views::copy::{deadline_fr, gestes_title, is_vat, letter_date};
use crate::views::relances::card_href;

struct Geste {
    title: String,
    body: String,
    href: String,
}

pub fn render(store: &Store, today: Date) -> Result<Markup, AppError> {
    let conn = store.connection();
    let setup = setup_status(conn)?;
    let queue = follow_up_queue(conn, today)?;
    let unmatched: Vec<_> = list_bank_transactions(conn)?
        .into_iter()
        .filter(|t| !t.is_matched())
        .collect();
    let calendar = fiscal_calendar(conn, today)?;
    let next_deadline = calendar.first();

    let mut gestes = Vec::new();
    if !setup.is_done() {
        gestes.push(Geste {
            title: "Configurer ma société".to_string(),
            body: "Nom, SIREN, adresse, clôture — le formulaire est prêt.".to_string(),
            href: "/premiers-pas".to_string(),
        });
    }
    for card in queue.iter().take(4) {
        gestes.push(follow_up_geste(card));
        if gestes.len() >= 5 {
            break;
        }
    }
    if gestes.len() < 5 && !unmatched.is_empty() {
        let n = unmatched.len();
        gestes.push(Geste {
            title: "Ranger le relevé".to_string(),
            body: if n == 1 {
                "Un mouvement n'a pas encore de lecture.".to_string()
            } else {
                format!("{n} mouvements n'ont pas encore de lecture.")
            },
            href: ViewId::Depenses.path().to_string(),
        });
    }
    if gestes.len() < 5
        && let Some(d) = next_deadline
    {
        let label = deadline_fr(d.kind);
        let title = if is_vat(d.kind) {
            "Savoir pour la TVA".to_string()
        } else {
            format!("Savoir pour {label}")
        };
        let amount = d
            .amount
            .filter(|m| *m != Money::ZERO)
            .map(|m| format!("{m} · "))
            .unwrap_or_default();
        gestes.push(Geste {
            title,
            body: format!(
                "{amount}à déposer avant le {}. Les chiffres sont dans le coffre. Le dépôt se fait sur le site des impôts — pas ici.",
                format_date_fr(d.due_on)
            ),
            href: "/societe".to_string(),
        });
    }
    gestes.truncate(5);

    let title = gestes_title(gestes.len());
    let lede = if gestes.is_empty() {
        "Rien n'est dû ce matin."
    } else if !setup.is_done() {
        "On commence par dire qui vous êtes. Le reste attend."
    } else {
        "Rien d'autre n'est urgent."
    };

    Ok(html! {
        div class="letter" data-view=(ViewId::Jour.slug()) {
            div class="date" { (letter_date(today)) }
            @if !setup.is_done() {
                p class="mast-note" id="next-step" { (setup.next_step.text()) }
            }
            h1 { (title) }
            p class="lede" { (lede) }
            @if !gestes.is_empty() {
                ol class="gestes" {
                    @for (i, g) in gestes.iter().enumerate() {
                        li {
                            a class="geste" href=(g.href)
                              hx-get=(g.href) hx-target="#content" hx-push-url="true" {
                                span class="num" { (i + 1) }
                                div {
                                    h2 { (g.title) }
                                    p { (g.body) }
                                }
                            }
                        }
                    }
                }
            }
        }
    })
}

fn follow_up_geste(card: &FollowUpCard) -> Geste {
    let who = card.contact_name.as_deref().unwrap_or(card.party.as_str());
    let title = match card.kind {
        FollowUpKind::Prospect => format!("Écrire à {who}"),
        FollowUpKind::Invoice => format!("Relancer {who}"),
    };
    let mut body = card.title.clone();
    if let Some(at) = card.due_on {
        body.push_str(" — prévue le ");
        body.push_str(&format_date_fr(at));
    }
    match card.status {
        CardStatus::Drafted => {
            body.push_str(". Un brouillon est prêt — il s'ouvre dans ton client mail.")
        }
        CardStatus::Blocked => {
            if let Some(reason) = &card.block_reason {
                body.push_str(". ");
                body.push_str(reason);
            }
        }
        CardStatus::Overdue => body.push_str(". En retard."),
        _ => {}
    }
    Geste {
        title,
        body,
        href: card_href(card.subject),
    }
}
