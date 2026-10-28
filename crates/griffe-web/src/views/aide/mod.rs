//! Lettre d'aide. Le bureau est un livre : sommaire persistant, lettre à côté.
//!
//! Trois rayons. L'atelier est rempli. Avec un agent aussi. Les formalités sont
//! vides : un livre n'apparaît que si `sections` lui donne des liens. Ajouter un
//! fichier, plus tard : une lettre dans la section « Fichiers » de l'agent. Ajouter
//! un livre : un `Rayon`, des `Letter`, des `Section`.

mod agent;
mod argent;
mod commande;
mod etat;
mod parler;
mod reperer;

use griffe_core::closing::GLOSSARY;
use griffe_core::fiscal::{FiscalDeadlineKind, VatFilingScheme};
use maud::{Markup, html};

use crate::views::copy::{deadline_fr, duty_href};

/// Où une lettre se range. Un rayon sans section ne s'affiche pas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rayon {
    Atelier,
    Agent,
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
    &parler::BOITE,
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

const AGENT_LETTERS: &[&Letter] = agent::LETTERS;

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
            ("courrier", "la liaison, les adresses, la relève"),
            ("boite", "lire, répondre, retenir la relance"),
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

const AGENT_SECTIONS: &[Section] = &[
    Section {
        label: "Avec un agent",
        links: &[
            ("portes", "la fenêtre, la commande, l'agent"),
            ("brancher", "la session, le MCP, la compétence"),
            ("matin", "Valvert, du matin à l'envoi"),
            ("console", "la même commande, ici"),
            ("demandes", "créer, estimer, brouillon"),
            ("trame", "la trame, le constat"),
        ],
    },
    Section {
        label: "La commande",
        links: &[
            ("cmd-coffre", "ouvrir, garder, confirmer"),
            ("cmd-gens", "le jour, les gens, les séries"),
            ("cmd-courrier", "les clients, le compte qui poste"),
            ("cmd-argent", "devis, facture, relevé"),
            ("cmd-societe", "l'État, la clôture, les papiers"),
        ],
    },
    Section {
        label: "Fichiers",
        links: &[
            ("fichier", "choisir, lire, appliquer"),
            ("dehors", "la file, l'envoi, un montant inventé"),
        ],
    },
];

struct Book {
    rayon: Rayon,
    label: &'static str,
    home: &'static str,
}

const BOOKS: &[Book] = &[
    Book {
        rayon: Rayon::Atelier,
        label: "L'atelier",
        home: "/aide",
    },
    Book {
        rayon: Rayon::Agent,
        label: "Avec un agent",
        home: "/aide/portes",
    },
    Book {
        rayon: Rayon::Formalites,
        label: "Les formalités",
        home: "/aide",
    },
];

fn sections(rayon: Rayon) -> &'static [Section] {
    match rayon {
        Rayon::Atelier => ATELIER_SECTIONS,
        Rayon::Agent => AGENT_SECTIONS,
        Rayon::Formalites => &[],
    }
}

fn letters(rayon: Rayon) -> &'static [&'static Letter] {
    match rayon {
        Rayon::Atelier => ATELIER_LETTERS,
        Rayon::Agent => AGENT_LETTERS,
        Rayon::Formalites => FORMALITES_LETTERS,
    }
}

fn find(slug: &str) -> Option<&'static Letter> {
    BOOKS
        .iter()
        .flat_map(|book| letters(book.rayon).iter().copied())
        .find(|letter| letter.slug == slug)
}

/// Lettres des livres visibles, pour la palette.
pub fn guide_letters() -> impl Iterator<Item = &'static Letter> {
    BOOKS
        .iter()
        .filter(|book| !sections(book.rayon).is_empty())
        .flat_map(|book| letters(book.rayon).iter().copied())
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

fn toc_link(href: &str, title: &str, current: bool) -> Markup {
    html! {
        li {
            a href=(href)
              aria-current=[current.then_some("true")]
              hx-get=(href)
              hx-target="#content"
              hx-push-url="true"
              hx-swap="innerHTML" {
                (title)
            }
        }
    }
}

fn sommaire(active: Rayon, current: Option<&str>) -> Markup {
    html! {
        nav class="aide-sommaire" aria-label="Sommaire" {
            div class="aide-livres" {
                @for book in BOOKS {
                    @if !sections(book.rayon).is_empty() {
                        a class="aide-livre"
                          href=(book.home)
                          aria-current=[(book.rayon == active).then_some("true")]
                          hx-get=(book.home)
                          hx-target="#content"
                          hx-push-url="true"
                          hx-swap="innerHTML" {
                            (book.label)
                        }
                    }
                }
            }
            @for book in BOOKS {
                @if !sections(book.rayon).is_empty() {
                    div class="aide-chapitres" hidden[book.rayon != active] {
                        @for section in sections(book.rayon) {
                            p class="section-label" { (section.label) }
                            ul class="aide-toc" {
                                @for (slug, _) in section.links {
                                    @if let Some(letter) = find(slug) {
                                        (toc_link(
                                            &format!("/aide/{slug}"),
                                            letter.title,
                                            current == Some(letter.slug),
                                        ))
                                    }
                                }
                            }
                        }
                        @if book.rayon == Rayon::Atelier {
                            p class="section-label" { "Portes" }
                            ul class="aide-toc" {
                                (toc_link("/premiers-pas", "Premiers pas", false))
                            }
                        }
                    }
                }
            }
        }
    }
}

fn open_book(rayon: Rayon, current: Option<&str>, feuille: Markup) -> Markup {
    html! {
        div class="letter spread letter-desk aide-desk" data-view="aide" {
            div class="aide-book" {
                (sommaire(rayon, current))
                article class="aide-feuille" { (feuille) }
            }
        }
    }
}

/// L'index : démarche, traduction des modules. Le sommaire range les lettres.
#[must_use]
pub fn index() -> Markup {
    open_book(
        Rayon::Atelier,
        None,
        html! {
            div class="date" { "L'atelier" }
            h1 { "Laissez-vous guider." }
            p class="lede" {
                "Ce qu'il y a à faire aujourd'hui est sur Le jour — pas un tableau de bord. \
                 Cette lettre est le mode d'emploi : pourquoi l'app ne ressemble pas à un logiciel de compta, \
                 et comment faire les gestes courants. Vous pouvez vous laisser guider. \
                 Le sommaire range les lettres. Avec un agent dit la commande, le MCP, \
                 et le fichier d'à côté. La commande liste chaque geste, avec un exemple."
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
        },
    )
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
        "courrier" => html! {
            div class="block" {
                h3 { "Votre feuille." }
                p class="prose" {
                    "Le fichier se nomme lettre.toml, dans le dossier griffe de la configuration. Si XDG_CONFIG_HOME est posé, c'est là. Sinon, ~/.config. Cinq lignes, pas davantage. L'encre et le lien restent lisibles sur le papier. Sur Le courrier, cette feuille apparaît à côté de Clair et de Sauge, marquée votre feuille. Lisible, on peut la choisir. Illisible, la ligne le dit et la lettre reste sur une feuille livrée."
                }
                pre class="aide-file" {
                    "nom = Atelier\nmarge = #f3f1ec\npapier = #fffcf8\nencre = #2a2622\nlien = #3d5c66"
                }
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
    open_book(
        recipe.rayon,
        Some(recipe.slug),
        html! {
            div class="date" { (recipe.kicker) }
            h1 { (recipe.title) }
            p class="lede" { (recipe.lede) }
            div class="aide-columns" {
                div class="aide-geste" {
                    ol {
                        @for step in recipe.steps {
                            li { (step) }
                        }
                    }
                    (bridge(recipe.slug))
                }
                aside class="aide-regard" {
                    (agent::regard(recipe.slug))
                    (extra)
                    @if let Some(does_not) = recipe.does_not {
                        p class="prose aide-limite" { (does_not) }
                    }
                    @if let (Some(href), Some(action)) = (recipe.href, recipe.action) {
                        div class="row-actions" {
                            a class="seal" href=(href)
                              hx-get=(href) hx-target="#content" hx-push-url="true" {
                                (action)
                            }
                        }
                    }
                }
            }
            (commande::catalog(recipe.slug))
        },
    )
}

fn bridge(slug: &str) -> Markup {
    match slug {
        "conversation" => html! {
            p class="prose" {
                "Un agent ouvre la même conversation, par la commande ou par le MCP. "
                a href="/aide/demandes"
                  hx-get="/aide/demandes" hx-target="#content" hx-push-url="true" {
                    "Ce que l'agent demande."
                }
            }
        },
        "relancer" => html! {
            p class="prose" {
                "Le brouillon se prépare aussi par la commande, sans partir. "
                a href="/aide/demandes"
                  hx-get="/aide/demandes" hx-target="#content" hx-push-url="true" {
                    "Ce que l'agent demande."
                }
            }
        },
        "boite" => html! {
            p class="prose" {
                "Le même courriel se lit en commande. "
                a href="/aide/cmd-courrier"
                  hx-get="/aide/cmd-courrier" hx-target="#content" hx-push-url="true" {
                    "Le courrier, en commande."
                }
            }
        },
        "phrases" => html! {
            p class="prose" {
                "L'agent recopie la série. Seul le constat change. Un constat vide, la ligne qui ne porte que lui disparaît. "
                a href="/aide/trame"
                  hx-get="/aide/trame" hx-target="#content" hx-push-url="true" {
                    "Les phrases, avec un agent."
                }
            }
        },
        _ => html! {},
    }
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
    open_book(
        Rayon::Atelier,
        Some("lexique"),
        html! {
            div class="date" { "Rendre des comptes" }
            h1 { "Les mots." }
            p class="lede" { "Les mots de la comptabilité, dans l'ordre où on les rencontre." }
            dl class="detail-list" {
                @for entry in GLOSSARY {
                    dt { (entry.term) }
                    dd { (entry.meaning) }
                }
            }
        },
    )
}

fn absent() -> Markup {
    open_book(
        Rayon::Atelier,
        None,
        html! {
            div class="date" { "L'atelier" }
            h1 { "Cette recette n'est pas ici." }
            p class="lede" { "Le sommaire de l'aide liste ce qui existe." }
            div class="row-actions" {
                a class="seal" href="/aide"
                  hx-get="/aide" hx-target="#content" hx-push-url="true" {
                    "L'aide"
                }
            }
        },
    )
}
