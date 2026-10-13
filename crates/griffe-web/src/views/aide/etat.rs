//! Rendre des comptes : échéances, clôture, papiers, mots.

use super::{Letter, Rayon};

pub(super) const IMPOTS: Letter = Letter {
    slug: "impots",
    kicker: "Rendre des comptes",
    title: "Ce que vous devez à l'État.",
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
    rayon: Rayon::Atelier,
};

pub(super) const CFE: Letter = Letter {
    slug: "cfe",
    kicker: "Rendre des comptes",
    title: "La cotisation foncière.",
    lede: "L'avis arrive par la poste et dans l'espace professionnel. Le montant n'est pas dans le coffre.",
    steps: &[
        "Le jour la fera apparaître le moment venu.",
        "Ouvrez la lettre : elle dit où aller, pas combien — ça, c'est l'avis.",
        "Si vous l'avez payée depuis un compte personnel, notez-la comme une dépense payée par vous.",
    ],
    does_not: Some("Griffe ne connaît pas le montant de l'avis."),
    href: Some("/societe/impots/cfe"),
    action: Some("Ouvrir la lettre"),
    rayon: Rayon::Atelier,
};

pub(super) const CLORE: Letter = Letter {
    slug: "clore",
    kicker: "Rendre des comptes",
    title: "Clore, approuver, déposer.",
    lede: "Le parcours dans La société dit où on en est, exercice par exercice.",
    steps: &[
        "On clôt l'exercice (le résultat se fige, l'affectation est un projet).",
        "On approuve — une sauvegarde se fait avant. Ensuite, plus rien ne se modifie.",
        "On recopie la liasse sur le site des impôts, on dépose les comptes au greffe.",
    ],
    does_not: Some("Griffe ne dépose pas à votre place."),
    href: Some("/societe/cloture"),
    action: Some("Ouvrir la clôture"),
    rayon: Rayon::Atelier,
};

pub(super) const PAPIERS: Letter = Letter {
    slug: "papiers",
    kicker: "Rendre des comptes",
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
    rayon: Rayon::Atelier,
};

pub(super) const LEXIQUE: Letter = Letter {
    slug: "lexique",
    kicker: "Rendre des comptes",
    title: "Les mots.",
    lede: "Les mots de la comptabilité, dans l'ordre où on les rencontre.",
    steps: &[],
    does_not: None,
    href: None,
    action: None,
    rayon: Rayon::Atelier,
};
