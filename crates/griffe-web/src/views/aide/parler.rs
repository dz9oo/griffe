//! Parler aux gens : la lentille, le dossier, les lettres.

use super::{Letter, Rayon};

pub(super) const LENTILLE: Letter = Letter {
    slug: "lentille",
    kicker: "Parler aux gens",
    title: "Lire Les affaires.",
    lede: "Une phrase, un menu, une recherche. Le nom ouvre le dossier à droite.",
    steps: &[
        "Le menu nomme ce qui demande un geste, le livre, le tas, les sorties, les arrêtés, tout.",
        "Ce qui demande un geste : à écrire, à reprendre, ou à régler aujourd'hui.",
        "Le livre, ici, ce sont les conversations en cours et les missions. Le grand livre des comptes est un autre mot, dans Les mots.",
        "Le tas : les premiers messages et les premiers contacts. Les sorties : un nom, dès qu'une dépense le porte. Tout réunit ces vues.",
        "La recherche filtre la vue ouverte. Le filtre de type, à côté, ne garde que les fiches de ce type. Les types se règlent sur la page Les types.",
        "Fermer, ou Échap, quitte le dossier. La phrase compte les noms de la vue ouverte.",
    ],
    does_not: Some("Griffe ne classe pas les noms en colonnes, ni en étapes de pipeline."),
    href: Some("/affaires"),
    action: Some("Ouvrir Les affaires"),
    rayon: Rayon::Atelier,
};

pub(super) const CONVERSATION: Letter = Letter {
    slug: "conversation",
    kicker: "Parler aux gens",
    title: "Commencer une conversation.",
    lede: "Un nom, une phrase. La fiche se complète ensuite, sans estimation.",
    steps: &[
        "Les affaires, Nouvelle conversation. Qui, et ce dont il s'agit.",
        "Ce Qui nomme la fiche. Une nouvelle conversation au même nom retrouve la fiche déjà là. Enregistrer la fiche sous un Qui déjà pris est refusé.",
        "La fiche tient l'adresse, le courriel, le téléphone. Qui répond — prénom, nom, Monsieur ou Madame — reste sous le titre.",
        "Un numéro français s'affiche par paires. Le genre des phrases se choisit en mots : aucun, un mot déjà là, ou un autre.",
        "Les pastilles de type de cette fiche sont sous le titre. Le « + » en ajoute ou en retire. Les types du coffre se créent sur Les types.",
    ],
    does_not: Some("Griffe n'envoie rien au moment où la conversation s'ouvre."),
    href: Some("/affaires/nouvelle"),
    action: Some("Nouvelle conversation"),
    rayon: Rayon::Atelier,
};

pub(super) const RELANCER: Letter = Letter {
    slug: "relancer",
    kicker: "Parler aux gens",
    title: "Écrire à quelqu'un.",
    lede: "Une lettre est un geste du Jour, ou un bouton sur le dossier.",
    steps: &[
        "Sur Le jour, le geste est là, avec un verbe, quand quelqu'un est à écrire ou à relancer.",
        "Sur le dossier, Écrire ouvre la lettre. Relire la montre telle qu'elle part. La zone d'écriture reprend le texte.",
        "Envoyer la fait partir, avec cinq secondes pour la retenir. Annuler, pendant ces cinq secondes, la garde. Fermer la fenêtre pendant ce délai la retient aussi.",
        "C'est parti classe le double dans l'historique et ne contacte pas le serveur.",
        "La formule se règle dans Le courrier. Elle ferme la lettre. Elle n'est pas recopiée dans la zone d'écriture.",
    ],
    does_not: Some(
        "Griffe ne lit pas votre boîte. Tant que Le courrier n'est pas branché, le brouillon peut s'ouvrir dans votre client mail.",
    ),
    href: Some("/jour"),
    action: Some("Aller au jour"),
    rayon: Rayon::Atelier,
};

pub(super) const COURRIER: Letter = Letter {
    slug: "courrier",
    kicker: "Parler aux gens",
    title: "Le courrier.",
    lede: "La société, Le courrier : d'où partent les lettres, ce qui les ferme, et l'essai.",
    steps: &[
        "D'où elles partent : le nom affiché, l'adresse qui signe, le serveur, le port. Enregistrer et essayer la liaison dit si le serveur accepte l'identifiant. Le mot de passe reste dans le coffre. La page ne le réécrit pas. Le laisser vide le garde.",
        "Deux ports, tous deux chiffrés : 587, ou 465. iCloud remplit le serveur. L'identifiant est alors l'adresse, et le mot de passe est un mot de passe d'application.",
        "La formule s'écrit une fois, à côté de la lettre d'essai. Enregistrer la range. Vide, la lettre se termine par Bien à vous, le nom, la société.",
        "Envoyer l'essai laisse cinq secondes. La fenêtre dit Courrier envoyé. Cela ne promet pas la boîte du destinataire.",
        "L'envoi du jour est éteint. L'activer poste, coffre ouvert, les lettres dont la date prévue est aujourd'hui. Une lettre de la veille reste à envoyer à la main.",
    ],
    does_not: Some("Griffe ne lit pas le courrier reçu."),
    href: Some("/societe/courrier"),
    action: Some("Ouvrir Le courrier"),
    rayon: Rayon::Atelier,
};

pub(super) const DEVIS: Letter = Letter {
    slug: "devis",
    kicker: "Parler aux gens",
    title: "L'estimation, sur le dossier.",
    lede: "On n'ouvre pas un écran Devis. On ouvre la personne.",
    steps: &[
        "Dans Les affaires, ouvrez le dossier, puis L'estimation.",
        "Chaque ligne dit un travail et un montant hors taxes. Le total est la somme. Ce dont il s'agit nomme l'ensemble.",
        "Le devis, vous le rédigez ailleurs. Griffe ne le compose pas et ne l'envoie pas.",
        "La mission, quand elle existe, se lit sur le même dossier. Une facture aussi : celle émise sur votre plateforme se pose sur le dossier.",
    ],
    does_not: Some("Griffe ne compose pas le devis et ne l'envoie pas."),
    href: Some("/affaires"),
    action: Some("Ouvrir Les affaires"),
    rayon: Rayon::Atelier,
};

pub(super) const TRAVAUX: Letter = Letter {
    slug: "travaux",
    kicker: "Parler aux gens",
    title: "Les travaux.",
    lede: "Le récit du dossier. L'estimation, à côté, dit ce que chaque ligne vaut.",
    steps: &[
        "Sur le dossier, Les travaux ouvre la page. Elle se lit en prose, même vide.",
        "Écrire ouvre le texte. Il se garde tout seul, au fil de la frappe, et en quittant la page. La ligne dit Gardé.",
        "Relire revient à la prose. Ce n'est pas un second enregistrement à part.",
    ],
    does_not: Some("Griffe ne montre pas le texte et la prose en même temps."),
    href: Some("/affaires"),
    action: Some("Ouvrir Les affaires"),
    rayon: Rayon::Atelier,
};

pub(super) const PHRASES: Letter = Letter {
    slug: "phrases",
    kicker: "Parler aux gens",
    title: "Les phrases.",
    lede: "Les mots des lettres de prospection. La société, Les phrases, ou le genre sur la fiche.",
    steps: &[
        "Une frise règle les moments, leur nombre et l'écart entre eux. Enregistrer écrit la série. Rien ne part.",
        "Le genre de la fiche choisit les mots. Changer le genre ne change pas le moment où l'on en est.",
        "Réécrire un moment change la prochaine lettre. Une lettre déjà classée, ou un brouillon déjà prêt, garde son texte.",
        "Les conversations déjà engagées finissent leur série.",
        "Les relances d'une facture gardent leur cadence. Cette série est celle des conversations.",
    ],
    does_not: Some("Enregistrer ces mots n'envoie pas la lettre."),
    href: Some("/affaires/phrases"),
    action: Some("Ouvrir Les phrases"),
    rayon: Rayon::Atelier,
};

pub(super) const GESTES: Letter = Letter {
    slug: "gestes",
    kicker: "Parler aux gens",
    title: "Les gestes du dossier.",
    lede: "Reporter, noter, arrêter, revenir, ou dire que c'est un client.",
    steps: &[
        "Reporter se déplie : 3 jours, 10 jours, ou un nombre de jours.",
        "Noter une rencontre demande une nature : téléphone, visioconférence, e-mail, rencontre physique, ou note.",
        "Cette conversation s'arrête retire le dossier de la liste du jour. Les lettres, les rencontres et l'estimation restent.",
        "Ils reviennent rouvre le même dossier.",
        "C'est un client, quand un montant est posé, ouvre la mission. Une mission déjà là ne se rouvre pas par ce geste.",
    ],
    does_not: None,
    href: Some("/affaires"),
    action: Some("Ouvrir Les affaires"),
    rayon: Rayon::Atelier,
};
