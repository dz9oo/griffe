//! Lettre d'aide. Deux rayons : l'atelier, rempli, et les formalités, vide.
//!
//! L'index n'affiche une section que si elle a des liens. Ajouter le second rayon
//! plus tard : des `Letter` dans `FORMALITES_LETTERS`, et des `Section` que
//! `sections` renvoie pour `Rayon::Formalites`. Les routes et les lettres de
//! l'atelier restent en place.

mod argent;
mod etat;
mod parler;
mod reperer;

use griffe_core::closing::GLOSSARY;
use griffe_core::fiscal::{FiscalDeadlineKind, VatFilingScheme};
use maud::{Markup, html};

use crate::views::copy::{deadline_fr, duty_href};

/// Où une lettre se range. Le second rayon n'a pas encore de page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rayon {
    Atelier,
    Formalites,
}

pub struct Letter {
    pub slug: &'static str,
    pub kicker: &'static str,
    pub title: &'static str,
    lede: &'static str,
    steps: &'static [&'static str],
    does_not: Option<&'static str>,
    href: Option<&'static str>,
    action: Option<&'static str>,
    rayon: Rayon,
}

struct Section {
    label: &'static str,
    links: &'static [(&'static str, &'static str)],
}

const ATELIER_LETTERS: &[&Letter] = &[
    &reperer::PIECES,
    &reperer::COFFRE,
    &parler::LENTILLE,
    &parler::CONVERSATION,
    &parler::RELANCER,
    &parler::COURRIER,
    &parler::DEVIS,
    &parler::TRAVAUX,
    &parler::PHRASES,
    &parler::GESTES,
    &argent::JOUR,
    &argent::RELEVE,
    &argent::FACTURE,
    &argent::PAYER,
    &etat::IMPOTS,
    &etat::CFE,
    &etat::CLORE,
    &etat::PAPIERS,
    &etat::LEXIQUE,
];

const FORMALITES_LETTERS: &[&Letter] = &[];

const ATELIER_SECTIONS: &[Section] = &[
    Section {
        label: "Se repérer",
        links: &[
            ("pieces", "Le jour, Les affaires, La société"),
            ("coffre", "passphrase, douze heures, version"),
        ],
    },
    Section {
        label: "Parler aux gens",
        links: &[
            ("lentille", "geste, livre, tas, sorties, arrêtés"),
            ("conversation", "un nom, une phrase, la fiche"),
            ("relancer", "cinq secondes pour retenir la lettre"),
            ("courrier", "la liaison, la formule, l'essai"),
            ("devis", "des lignes, pas un devis rédigé ici"),
            ("travaux", "le récit, gardé tout seul"),
            ("phrases", "la série, le genre"),
            ("gestes", "reporter, rencontre, arrêter"),
        ],
    },
    Section {
        label: "Tenir l'argent",
        links: &[
            ("jour", "la piste, les gestes, le tas replié"),
            ("releve", "dépense, dette, ou vous"),
            ("facture", "née ailleurs, posée sur le dossier"),
            ("payer", "sans casser la piste"),
        ],
    },
    Section {
        label: "Rendre des comptes",
        links: &[
            ("impots", "Le jour vous prévient"),
            ("cfe", "l'avis, pas le coffre"),
            ("clore", "le parcours, puis le greffe"),
            ("papiers", "tendre le dossier d'un contrôle"),
            ("lexique", "le lexique de la clôture"),
        ],
    },
];

fn sections(rayon: Rayon) -> &'static [Section] {
    match rayon {
        Rayon::Atelier => ATELIER_SECTIONS,
        Rayon::Formalites => &[],
    }
}

fn letters(rayon: Rayon) -> &'static [&'static Letter] {
    match rayon {
        Rayon::Atelier => ATELIER_LETTERS,
        Rayon::Formalites => FORMALITES_LETTERS,
    }
}

fn find(slug: &str) -> Option<&'static Letter> {
    letters(Rayon::Atelier)
        .iter()
        .chain(letters(Rayon::Formalites))
        .copied()
        .find(|letter| letter.slug == slug)
}

/// Lettres de l'atelier, pour la palette. Le rayon vide n'y entre pas.
pub fn atelier_letters() -> impl Iterator<Item = &'static Letter> {
    letters(Rayon::Atelier)
        .iter()
        .copied()
        .filter(|letter| letter.rayon == Rayon::Atelier)
}

fn chapter_link(href: &str, title: &str, sub: &str) -> Markup {
    html! {
        li {
            a href=(href) hx-get=(href) hx-target="#content" hx-push-url="true" {
                div {
                    strong { (title) }
                    span { (sub) }
                }
                span class="go" { "→" }
            }
        }
    }
}

fn back() -> Markup {
    html! {
        a class="back" href="/aide"
          hx-get="/aide" hx-target="#content" hx-push-url="true" hx-swap="innerHTML" {
            "← L'aide"
        }
    }
}

fn letter_shell(inner: Markup) -> Markup {
    html! {
        div class="letter" data-view="aide" {
            (inner)
        }
    }
}

fn section_markup(section: &Section) -> Markup {
    if section.links.is_empty() {
        return html! {};
    }
    html! {
        p class="section-label" { (section.label) }
        ul class="chapters" {
            @for (slug, sub) in section.links {
                @if let Some(letter) = find(slug) {
                    (chapter_link(&format!("/aide/{slug}"), letter.title, sub))
                }
            }
        }
    }
}

/// L'index : démarche, traduction des modules, sommaire.
#[must_use]
pub fn index() -> Markup {
    letter_shell(html! {
        div class="date" { "L'atelier" }
        h1 { "Laissez-vous guider." }
        p class="lede" {
            "Ce qu'il y a à faire aujourd'hui est sur Le jour — pas un tableau de bord. \
             Cette lettre dit pourquoi l'app ne ressemble pas à un logiciel de compta, \
             et comment faire les gestes courants. Vous pouvez vous laisser guider."
        }
        div class="block" {
            h3 { "Si vous venez d'ailleurs" }
            p class="prose" {
                "Ce n'est pas Tiime, Indy ou Pennylane. Pas de menu Factures, Devis, Banque, \
                 ni de tuiles de chiffre d'affaires. Le tableau de bord, c'est Le jour. \
                 Un client, un devis, une facture : on ouvre une personne, dans Les affaires. \
                 La banque et les dépenses : le relevé, dans La société. Les impôts, \
                 la clôture, l'identité : La société aussi."
            }
        }
        @for section in sections(Rayon::Atelier) {
            (section_markup(section))
        }
        @for section in sections(Rayon::Formalites) {
            (section_markup(section))
        }
        p class="section-label" { "Portes" }
        ul class="chapters" {
            (chapter_link("/premiers-pas", "Premiers pas", "coffre neuf, profil, banque"))
        }
    })
}

/// Une lettre, le lexique, ou la lettre d'absence. Toujours 200.
#[must_use]
pub fn page(slug: &str) -> Markup {
    if slug == "lexique" {
        return lexique();
    }
    find(slug).map_or_else(absent, recipe_letter)
}

fn recipe_letter(recipe: &Letter) -> Markup {
    let extra = match recipe.slug {
        "impots" => duty_chapters(&[
            FiscalDeadlineKind::Ca3,
            FiscalDeadlineKind::VatInstalment,
            FiscalDeadlineKind::Ca12,
            FiscalDeadlineKind::IsAcompte,
            FiscalDeadlineKind::IsSolde,
            FiscalDeadlineKind::Das2,
            FiscalDeadlineKind::Dividends2777,
        ]),
        "clore" => html! {
            ul class="chapters" {
                (chapter_link("/societe/impots/liasse", "Liasse fiscale", "recopier, ne pas transmettre d'ici"))
                (chapter_link("/societe/impots/accounts-filing", "Dépôt des comptes", "guichet unique, confidentialité possible"))
                (chapter_link("/aide/papiers", "Les papiers.", "ce qui se fige ici, ce que vous apportez"))
            }
        },
        "papiers" => html! {
            div class="block" {
                h3 { "Ce que Griffe écrit." }
                p class="prose" {
                    "À la clôture, le FEC, les factures, le bilan 2033-A, la liasse et la synthèse se figent ici, tout seuls. À l'approbation, le PV et l'affectation. Vous n'allez nulle part les chercher."
                }
            }
            div class="block" {
                h3 { "Ce que vous apportez." }
                p class="prose" {
                    "Le Kbis, les statuts, les accusés de dépôt : Griffe ne les produit pas. Vous les déposez, dans Les papiers."
                }
            }
            div class="block" {
                h3 { "Qu'est-ce qu'un contrôle ?" }
                p class="prose" {
                    "Un inspecteur demande les originaux de l'exercice. Griffe prépare un dossier en clair à tendre. Il ne transmet rien."
                }
            }
        },
        _ => html! {},
    };
    letter_shell(html! {
        (back())
        div class="date" { (recipe.kicker) }
        h1 { (recipe.title) }
        p class="lede" { (recipe.lede) }
        ol {
            @for step in recipe.steps {
                li { (step) }
            }
        }
        (extra)
        @if let Some(does_not) = recipe.does_not {
            p class="prose" style="color:var(--ink-2);font-size:14px" { (does_not) }
        }
        @if let (Some(href), Some(action)) = (recipe.href, recipe.action) {
            div class="row-actions" {
                a class="seal" href=(href)
                  hx-get=(href) hx-target="#content" hx-push-url="true" {
                    (action)
                }
            }
        }
    })
}

fn duty_chapters(kinds: &[FiscalDeadlineKind]) -> Markup {
    html! {
        ul class="chapters" {
            @for kind in kinds {
                (chapter_link(
                    &duty_href(*kind, None),
                    deadline_fr(*kind, VatFilingScheme::Ca3Monthly),
                    "la lettre de la démarche",
                ))
            }
        }
    }
}

fn lexique() -> Markup {
    letter_shell(html! {
        (back())
        div class="date" { "Rendre des comptes" }
        h1 { "Les mots." }
        p class="lede" { "Les mots de la comptabilité, dans l'ordre où on les rencontre." }
        dl class="detail-list" {
            @for entry in GLOSSARY {
                dt { (entry.term) }
                dd { (entry.meaning) }
            }
        }
    })
}

fn absent() -> Markup {
    letter_shell(html! {
        (back())
        div class="date" { "L'atelier" }
        h1 { "Cette recette n'est pas ici." }
        p class="lede" { "Le sommaire de l'aide liste ce qui existe." }
        div class="row-actions" {
            a class="seal" href="/aide"
              hx-get="/aide" hx-target="#content" hx-push-url="true" {
                "L'aide"
            }
        }
    })
}
