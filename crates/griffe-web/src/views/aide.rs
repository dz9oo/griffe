//! Lettre d'aide Atelier (lot 53) : l'index rassure et traduit les modules SaaS ;
//! chaque recette ouvre une pièce déjà là. Copy fixe, aucune query.

use griffe_core::closing::GLOSSARY;
use griffe_core::fiscal::{FiscalDeadlineKind, VatFilingScheme};
use maud::{Markup, html};

use crate::layout::ViewId;
use crate::views::copy::{deadline_fr, duty_href};

struct Recipe {
    slug: &'static str,
    kicker: &'static str,
    title: &'static str,
    lede: &'static str,
    steps: &'static [&'static str],
    does_not: Option<&'static str>,
    href: Option<&'static str>,
    action: Option<&'static str>,
}

const RELANCER: Recipe = Recipe {
    slug: "relancer",
    kicker: "Au quotidien",
    title: "Écrire à quelqu'un.",
    lede: "Une relance, ce n'est pas un module. C'est un geste du Jour, ou un bouton sur le dossier de la personne.",
    steps: &[
        "Ouvrez Le jour : s'il y a quelqu'un à relancer, le geste est là, avec un verbe.",
        "Ou ouvrez la personne dans Les affaires : Écrire.",
        "Un brouillon s'ouvre dans votre client mail. Griffe ne l'envoie pas.",
        "Quand c'est parti, vous le dites ici — « Envoyé » est un fait humain.",
    ],
    does_not: Some("Griffe n'envoie rien, et ne lit pas votre boîte."),
    href: Some("/jour"),
    action: Some("Aller au jour"),
};

const CONVERSATION: Recipe = Recipe {
    slug: "conversation",
    kicker: "Au quotidien",
    title: "Commencer une conversation.",
    lede: "Pas besoin d'une fiche client pour parler à quelqu'un.",
    steps: &[
        "Les affaires → Nouvelle conversation.",
        "Un nom, une phrase. C'est tout.",
        "La fiche — nom, adresse, courriel — se complète sur le dossier, sans estimation.",
    ],
    does_not: None,
    href: Some("/affaires/nouvelle"),
    action: Some("Nouvelle conversation"),
};

const DEVIS: Recipe = Recipe {
    slug: "devis",
    kicker: "Au quotidien",
    title: "L'estimation, sur le dossier.",
    lede: "On n'ouvre pas un écran Devis. On ouvre la personne.",
    steps: &[
        "Dans Les affaires, ouvrez le dossier.",
        "L'estimation se note là : le sujet, et autour de combien. Griffe ne rédige pas le devis.",
        "La mission, quand elle existe, se lit sur le même dossier.",
        "Une facture se lit là aussi. Celle que vous avez émise dans Tiime (ou votre plateforme), vous la posez sur le dossier. L'émission depuis cette lettre n'existe pas.",
    ],
    does_not: Some("Griffe ne compose pas le devis et ne l'envoie pas."),
    href: Some("/affaires"),
    action: Some("Ouvrir Les affaires"),
};

const RELEVE: Recipe = Recipe {
    slug: "releve",
    kicker: "Au quotidien",
    title: "Ranger le relevé.",
    lede: "Chaque ligne de la banque a une de trois lectures : une dépense, le règlement d'une dette, ou vous qui vous payez.",
    steps: &[
        "Importez l'export de votre banque tel quel (le relevé, dans La société).",
        "Pour un débit : c'est une dépense, ou c'est le règlement d'une dette reprise au bilan — pas les deux.",
        "Joignez le justificatif. Sans lui, la dépense ne tient pas en cas de contrôle.",
        "Un crédit, c'est souvent quelqu'un qui vous paie : on le rattache à la facture, depuis le dossier.",
    ],
    does_not: Some("Griffe n'interroge pas votre banque. C'est vous qui importez le fichier."),
    href: Some("/societe/releve"),
    action: Some("Ouvrir le relevé"),
};

const IMPOTS: Recipe = Recipe {
    slug: "impots",
    kicker: "L'État",
    title: "Ce que tu dois à l'État.",
    lede: "Le jour vous prévient. La société porte les lettres de démarche — montant, chemin sur le site.",
    steps: &[
        "Quand une échéance approche, un geste apparaît sur Le jour.",
        "La lettre dit le montant et le chemin sur le site. On prépare, on ne transmet pas.",
        "Si l'État vous doit de la TVA, la lettre dit si on peut la récupérer, ou s'il faut attendre la fin d'année.",
        "Une facture déjà déclarée était trop haute : on rend l'écart sur la déclaration d'à présent, le crédit baisse.",
    ],
    does_not: Some("Griffe ne télétransmet rien."),
    href: Some("/societe/impots"),
    action: Some("Voir les échéances"),
};

const CFE: Recipe = Recipe {
    slug: "cfe",
    kicker: "L'État",
    title: "La cotisation foncière.",
    lede: "L'avis arrive par la poste et dans l'espace professionnel. Le montant n'est pas dans le coffre.",
    steps: &[
        "Le jour la fera apparaître le moment venu.",
        "Ouvrez la lettre : elle dit où aller, pas combien — ça, c'est l'avis.",
        "Si tu l'as payée depuis un compte perso, note-la comme une dépense payée par toi.",
    ],
    does_not: Some("Griffe ne connaît pas le montant de l'avis."),
    href: Some("/societe/impots/cfe"),
    action: Some("Ouvrir la lettre"),
};

const CLORE: Recipe = Recipe {
    slug: "clore",
    kicker: "L'État",
    title: "Clore, approuver, déposer.",
    lede: "Le parcours dans La société dit où on en est, exercice par exercice.",
    steps: &[
        "On clôt l'exercice (le résultat se fige, l'affectation est un projet).",
        "On approuve — une sauvegarde se fait avant. Ensuite, plus rien ne se modifie.",
        "On recopie la liasse sur le site des impôts, on dépose les comptes au guichet.",
    ],
    does_not: Some("Griffe ne dépose pas à votre place."),
    href: Some("/societe/cloture"),
    action: Some("Ouvrir la clôture"),
};

const PAPIERS: Recipe = Recipe {
    slug: "papiers",
    kicker: "L'État",
    title: "Garder les originaux, tendre le dossier d'un contrôle.",
    lede: "Les originaux nés ici restent ici. Ce que Griffe ne produit pas, vous le déposez. Le jour d'un contrôle, un dossier en clair.",
    steps: &[
        "Les papiers, dans La société : la liste dit ce qui manque pour l'exercice.",
        "Déposez ce que Griffe ne produit pas : Kbis, statuts, accusé de dépôt.",
        "Quand il faut tendre une liasse : Préparer le dossier d'un contrôle. Un dossier s'ouvre, en clair.",
        "Ces fichiers ne sont plus chiffrés. Ne les laissez pas à côté du coffre.",
    ],
    does_not: Some(
        "Ce n'est pas un coffre-fort certifié. Un scan déposé est une copie de travail ; le papier reste l'original.",
    ),
    href: Some("/societe/papiers"),
    action: Some("Ouvrir Les papiers"),
};

const RECIPES: &[Recipe] = &[
    RELANCER,
    CONVERSATION,
    DEVIS,
    RELEVE,
    IMPOTS,
    CFE,
    CLORE,
    PAPIERS,
];

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
        div class="letter" data-view=(ViewId::Aide.slug()) {
            (inner)
        }
    }
}

/// L'index : démarche Atelier, traduction des modules, sommaire des recettes.
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
        p class="section-label" { "Au quotidien" }
        ul class="chapters" {
            (chapter_link("/aide/relancer", "Écrire à quelqu'un.", "un geste du Jour, un brouillon, on n'envoie pas"))
            (chapter_link("/aide/conversation", "Commencer une conversation.", "un nom, une phrase"))
            (chapter_link("/aide/devis", "L'estimation, sur le dossier.", "le sujet et le montant, pas un devis rédigé ici"))
            (chapter_link("/aide/releve", "Ranger le relevé.", "dépense, dette, ou vous"))
        }
        p class="section-label" { "L'État, une fois l'an" }
        ul class="chapters" {
            (chapter_link("/aide/impots", "Ce que tu dois à l'État.", "Le jour vous prévient"))
            (chapter_link("/aide/cfe", "La cotisation foncière.", "l'avis, pas le coffre"))
            (chapter_link("/aide/clore", "Clore, approuver, déposer.", "le parcours, puis le greffe"))
            (chapter_link("/aide/papiers", "Garder les originaux.", "tendre le dossier d'un contrôle"))
        }
        p class="section-label" { "Portes" }
        ul class="chapters" {
            (chapter_link("/premiers-pas", "Premiers pas", "coffre neuf, profil, banque"))
            (chapter_link("/societe/payer", "Te payer", "sans casser la piste"))
            (chapter_link("/aide/lexique", "Les mots", "le lexique de la clôture"))
        }
    })
}

/// Une recette, le lexique, ou la lettre d'absence — toujours 200.
#[must_use]
pub fn page(slug: &str) -> Markup {
    if slug == "lexique" {
        return lexique();
    }
    if let Some(recipe) = RECIPES.iter().find(|r| r.slug == slug) {
        return recipe_letter(recipe);
    }
    absent()
}

fn recipe_letter(recipe: &Recipe) -> Markup {
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
        div class="date" { "Les mots" }
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
