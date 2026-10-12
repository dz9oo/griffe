//! Tenir l'argent : le matin, le relevé, une facture, se payer.

use super::{Letter, Rayon};

pub(super) const JOUR: Letter = Letter {
    slug: "jour",
    kicker: "Tenir l'argent",
    title: "La lettre du matin.",
    lede: "Le jour dit ce qu'il y a à faire, et où en est l'argent.",
    steps: &[
        "La phrase du matin dit ce qu'il y a en banque, combien de mois de piste, et ce qu'il reste à encaisser.",
        "Les gestes sont des verbes : écrire, relancer, ranger un mouvement, savoir pour une échéance.",
        "Les premiers messages et les premiers contacts ne sont pas écrits un par un. Ils tiennent sur une ligne : « et N autres », ou « N autres conversations ». Le nom se retrouve dans Les affaires.",
        "Le mois, sous la lettre, pose les échéances et la clôture quand elles viennent.",
    ],
    does_not: Some("Le jour n'est pas un tableau de chiffres."),
    href: Some("/jour"),
    action: Some("Aller au jour"),
    rayon: Rayon::Atelier,
};

pub(super) const RELEVE: Letter = Letter {
    slug: "releve",
    kicker: "Tenir l'argent",
    title: "Ranger le relevé.",
    lede: "Chaque ligne de la banque a une lecture. Griffe n'interroge pas votre banque.",
    steps: &[
        "La société, Le relevé. Déposer un export de la banque, tel quel.",
        "Chaque ligne est traitée, ou non traitée.",
        "Un débit : une dépense, le règlement d'une dette reprise au bilan, ou vous qui vous payez. La dépense et le règlement d'une dette ne sont pas la même lecture.",
        "Un crédit est souvent un encaissement : on le rattache à la facture, depuis le dossier. Ou c'est vous qui apportez.",
        "Joignez le justificatif. Sans lui, la dépense ne tient pas en cas de contrôle.",
        "La recherche filtre le libellé, ou un montant exact.",
    ],
    does_not: Some("Griffe n'interroge pas votre banque. C'est vous qui déposez le fichier."),
    href: Some("/societe/releve"),
    action: Some("Ouvrir le relevé"),
    rayon: Rayon::Atelier,
};

pub(super) const FACTURE: Letter = Letter {
    slug: "facture",
    kicker: "Tenir l'argent",
    title: "Poser une facture.",
    lede: "L'émission ne se fait pas dans cette lettre. La facture née ailleurs se pose sur le dossier.",
    steps: &[
        "Ouvrez la personne dans Les affaires. Le dossier d'un client porte Poser une facture née ailleurs.",
        "Le numéro est celui de votre plateforme. Griffe n'en invente pas un second. Le PDF se joint.",
        "Un avoir porte une quantité négative, et désigne la facture qu'il annule.",
        "Un crédit du relevé se rattache à cette facture, depuis le dossier.",
    ],
    does_not: Some("Griffe n'émet pas la facture depuis cette lettre."),
    href: Some("/affaires"),
    action: Some("Ouvrir Les affaires"),
    rayon: Rayon::Atelier,
};

pub(super) const PAYER: Letter = Letter {
    slug: "payer",
    kicker: "Tenir l'argent",
    title: "Te payer.",
    lede: "De l'argent sur votre compte, sans casser la société. La société ouvre trois chapitres.",
    steps: &[
        "Ce mois-ci dit jusqu'où vous pouvez vous verser en gardant des mois de piste.",
        "Entre toi et la société : ce que la société vous doit quand vous avez payé pour elle, ou l'inverse. Un virement du relevé se lit C'est pour moi, ou C'est moi qui apporte.",
        "Cette année suit ce que vous vous êtes versé, face à ce que vous aviez prévu.",
        "Les dividendes n'apparaissent qu'une fois l'exercice clos, et seulement s'il est bénéficiaire.",
    ],
    does_not: Some("Griffe ne fait pas de bulletin de paie, ni de déclaration sociale."),
    href: Some("/societe/payer"),
    action: Some("Ouvrir Te payer"),
    rayon: Rayon::Atelier,
};
