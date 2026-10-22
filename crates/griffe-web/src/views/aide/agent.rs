//! Avec un agent : trois portes, le branchement, le matin, puis chaque commande.
//!
//! Un fichier de plus, plus tard : une `Letter` dans `AGENT_LETTERS`, et une ligne
//! dans la section « Fichiers ». Une commande de plus : une ligne dans `commande`.

use maud::{Markup, html};

use super::{Letter, Rayon};

pub(super) const PORTES: Letter = Letter {
    slug: "portes",
    kicker: "Avec un agent",
    title: "Trois portes, un coffre.",
    lede: "La fenêtre est une porte. La commande en est une autre. L'agent, branché par le MCP, en est une troisième.",
    steps: &[
        "Une application en ligne vous laisse le plus souvent les écrans qu'elle a dessinés. Griffe en ouvre trois, sur le même coffre. Vous lisez le matin dans la fenêtre. Vous faites un geste en tapant une phrase. Un agent fait le même geste, parce qu'il est branché, pas parce qu'il clique à votre place.",
        "La commande est cette phrase. On la tape dans un terminal, la fenêtre où l'ordinateur attend une ligne. Le programme s'appelle griffe. griffe day gestures lit ce qu'il y a à faire. griffe --help montre les familles. Ce n'est pas un second logiciel.",
        "Un agent est un assistant qui lit, propose, et appelle des outils. Sans outil, il ne voit pas le coffre : il devine. Le MCP est le branchement. Le programme s'appelle griffe-mcp. Il n'ouvre aucun port. L'agent et Griffe se parlent par leur entrée et leur sortie. On appelle cela stdio.",
        "Avec --json, la commande répond pour un agent. Avec --dry-run, elle montre le geste sans l'écrire. L'acteur se nomme. Pour un agent, c'est agent, suivi du nom de la session. Une commande qui demande confirmation attend alors une personne.",
        "La passphrase ne s'écrit ni dans la conversation, ni en argument. Le terminal la demande, masquée. Ou bien un fichier, une commande, l'entrée standard.",
        "Brancher dit comment ouvrir la session, lancer griffe-mcp, et écrire la compétence que l'agent suit. La commande liste chaque geste, avec un exemple.",
    ],
    does_not: Some("Les trois portes ouvrent le même coffre. Aucune n'a sa règle à part."),
    href: Some("/aide/brancher"),
    action: Some("Brancher l'agent"),
    rayon: Rayon::Agent,
};

pub(super) const BRANCHER: Letter = Letter {
    slug: "brancher",
    kicker: "Avec un agent",
    title: "Brancher l'agent.",
    lede: "Quatre choses, dans l'ordre. Le coffre ouvert, la commande, le MCP, la compétence.",
    steps: &[
        "Ouvrez un terminal. Tapez griffe --help. Les familles s'affichent : le jour, les gens, les factures, la société. griffe day gestures lit le matin. Ajoutez --json quand c'est un agent qui lit.",
        "La passphrase ne passe pas dans la ligne. Pour qu'un agent travaille ensuite sans vous la redemander, gardez la session dans le trousseau : griffe unlock --remember. Elle dure douze heures, ou la durée donnée avec --ttl. griffe lock l'efface. griffe vault status dit si une session est là, sans ouvrir le coffre.",
        "Le MCP est un programme à côté de l'agent, pas un site. Son nom est griffe-mcp. Il reprend la session déjà gardée. S'il n'en trouve pas, il s'arrête et vous demande de relancer unlock --remember. Aucun port n'écoute. Le transport est stdio.",
        "Avec Grok, une ligne suffit : grok mcp add griffe -- griffe-mcp. Avec un autre agent, le fichier qu'il lit pour ses outils nomme la commande. Le coffre habituel est celui de la session. S'il est ailleurs, GRIFFE_DB donne le chemin. Pas de passphrase dans ce fichier.",
        "Une compétence, souvent appelée skill, est le mode d'emploi que vous écrivez. Griffe ne l'embarque pas. Elle dit quand agir, quel outil appeler, et ce qui reste votre geste. Sans elle, l'agent a les outils et pas la manière. Avec Grok, le fichier vit dans le dossier skills de votre maison, sous un nom, et il s'appelle SKILL.md.",
        "Les lectures portent une adresse griffe://. griffe://day est le matin. griffe://people est Les affaires. griffe://follow-ups est la file des lettres. Une lecture ne s'écrit pas dans le journal.",
        "Les outils écrivent. Le plus souvent le nom reprend la commande, avec un point : day.mast, prospect.create. Quelques gestes ont un nom à eux. Le brouillon est follow_up.prepare. Dire que la lettre est partie est follow_up.mark_sent. Les travaux sont people.save_work. Le courrier est la famille mail.",
        "L'agent agit toujours comme un agent. Un geste qui demande confirmation ne s'applique pas. Il attend. Vous le voyez dans la fenêtre, ou avec griffe pending list. Vous le confirmez avec griffe confirm, quand c'est vraiment fait.",
        "Dans la fenêtre, la console tape la même ligne sur le coffre déjà ouvert. Elle refuse d'ouvrir, de fermer, de changer la passphrase, et de restaurer.",
    ],
    does_not: Some("L'agent ne reçoit pas la passphrase, et il ne poste pas à votre place."),
    href: Some("/aide/matin"),
    action: Some("Le matin de Valvert"),
    rayon: Rayon::Agent,
};

pub(super) const MATIN: Letter = Letter {
    slug: "matin",
    kicker: "Avec un agent",
    title: "Le matin de Valvert.",
    lede: "La même matinée, dite à l'agent, tapée en commande, ou lue dans la fenêtre.",
    steps: &[
        "Vous demandez : qu'est-ce que j'ai à faire ? L'agent lit griffe://day, ou appelle day.gestures. Dans un terminal : griffe day gestures. Dans la fenêtre, c'est Le jour.",
        "Une lettre est due. Le dossier est la Mairie de Valvert. La conversation s'appelle Refonte du site. L'agent ouvre la fiche : people.show, ou griffe people show \"Mairie de Valvert\".",
        "Il lit la série, sans la réécrire. follow_up.phrases, ou griffe follow-up phrases --genre \"Le premier contact\". Le constat remplace le jeton. Le reste de la trame reste.",
        "Il prépare le brouillon et ne l'ouvre pas. follow_up.prepare, ou griffe follow-up draft \"Refonte du site\" --body-file lettre.txt --no-open. Rien ne part. Dans la fenêtre, c'est Écrire, sur le dossier.",
        "L'estimation, si elle n'est pas encore posée, est une ligne par mission, le montant copié. griffe prospect estimation set \"Refonte du site\" --line \"Refonte du site=1200,00\". Cet outil n'est pas dans le MCP : la fenêtre et la commande le font.",
        "Dire que la lettre est partie attend que vous l'ayez vraiment postée. follow_up.mark_sent, ou griffe follow-up sent \"Refonte du site\" --actor agent:matin, dépose l'attente. griffe pending list la montre. griffe confirm l'applique.",
        "La mairie répond. La relance ne part pas. L'agent lit le courriel avec boite liste, ou sur la page Courriel. Il ne laisse pas repartir la relance. Ce geste est le vôtre : boite relance, ou Laisser partir la relance, sur le dossier.",
    ],
    does_not: Some("L'agent prépare. Vous décidez de l'envoi."),
    href: Some("/aide/demandes"),
    action: Some("Ce que l'agent demande"),
    rayon: Rayon::Agent,
};

pub(super) const CONSOLE: Letter = Letter {
    slug: "console",
    kicker: "Avec un agent",
    title: "La console.",
    lede: "La même commande, tapée dans la fenêtre, sur le coffre déjà ouvert.",
    steps: &[
        "La console exécute la ligne. Elle ne rouvre pas un second coffre.",
        "Une commande utile ici est la même que dans un terminal.",
        "Ouvrir le coffre, le fermer, changer la passphrase, restaurer une sauvegarde : la console les refuse. Ces gestes veulent la fenêtre, ou un vrai terminal.",
    ],
    does_not: Some("La console n'est pas un second logiciel."),
    href: Some("/view/console"),
    action: Some("Ouvrir la console"),
    rayon: Rayon::Agent,
};

pub(super) const DEMANDES: Letter = Letter {
    slug: "demandes",
    kicker: "Avec un agent",
    title: "Ce que l'agent demande.",
    lede: "Créer, estimer, choisir une série, préparer le brouillon. La fenêtre le fait déjà.",
    steps: &[
        "Créer une conversation : dans Les affaires, ou par prospect create, ou par l'outil prospect.create. Un nom, une phrase. Un Qui déjà là est retrouvé, sans être réécrit.",
        "La fenêtre, à l'ouverture, pose un montant nul. La commande et l'outil demandent en plus un montant, une probabilité et une date. Les lignes viennent ensuite.",
        "Poser les lignes : sur le dossier, L'estimation. La commande prospect estimation set prend une ligne par travail, Libellé=1200,00. Le dernier signe égal sépare le libellé du montant. Le total est la somme. Le MCP n'a pas encore cet outil : la fenêtre et la commande le font.",
        "Lire les séries : Les phrases, follow-up phrases, ou l'outil follow_up.phrases. En assigner une qui est listée : follow-up genre assign, ou follow_up.set_genre. Un nom inconnu crée la série. On ne le fait qu'exprès.",
        "Une série nouvelle s'ajoute, puis ses mots s'écrivent pour elle. follow-up genre add et follow_up.add_genre, puis la réécriture de ses moments. On ne part pas du texte d'une personne.",
        "Préparer le brouillon : Écrire, sur le dossier. follow-up draft avec --no-open écrit le fichier sans l'ouvrir. L'outil follow_up.prepare renvoie la lettre et n'ouvre pas le client. Rien ne part.",
        "Les travaux se posent sur la fiche : la page Les travaux, people travaux, ou people.save_work. Ce récit n'est pas une note ajoutée à l'histoire.",
        "Dire que la lettre est partie, follow-up sent ou follow_up.mark_sent, attend une personne quand l'acteur est un agent.",
        "Chaque geste, famille par famille, est dans La commande, avec un exemple à recopier.",
    ],
    does_not: Some("L'agent n'a pas une règle à part. Il passe par les mêmes gestes."),
    href: Some("/affaires/nouvelle"),
    action: Some("Nouvelle conversation"),
    rayon: Rayon::Agent,
};

pub(super) const TRAME: Letter = Letter {
    slug: "trame",
    kicker: "Avec un agent",
    title: "Les phrases.",
    lede: "La série est la trame. L'agent la recopie. Un seul passage est à lui.",
    steps: &[
        "Vous écrivez les séries dans Les phrases. L'agent les lit. Il ne les réécrit pas pour une personne.",
        "Le premier message reprend la trame. Le passage <constat> dit ce qui a été vu chez cette personne. Le reste reste.",
        "<signature> reste la formule du courrier. L'agent ne la remplace pas.",
        "Un <constat> vide : la ligne qui ne porte que lui disparaît.",
        "Une forme <Monsieur le Maire/Madame la Maire> suit Monsieur ou Madame. Sans genre, les deux restent.",
        "Garder une lettre réécrit la série, pour les suivantes. follow-up keep et follow_up.keep_words font ce geste. On ne les appelle pas avec le texte d'une personne.",
    ],
    does_not: Some(
        "Réécrire la série depuis une lettre déjà personnalisée la change pour tout le monde.",
    ),
    href: Some("/aide/phrases"),
    action: Some("Les phrases de l'atelier"),
    rayon: Rayon::Agent,
};

pub(super) const FICHIER: Letter = Letter {
    slug: "fichier",
    kicker: "Fichiers",
    title: "Un fichier à côté.",
    lede: "Notion est le premier fichier. La forme sert pour le suivant.",
    steps: &[
        "Vous tenez le barème : un nom de mission, un montant hors taxes. Vous tenez les séries. L'agent les lit. Il ne réécrit ni l'un ni les autres.",
        "Des bots, par exemple des Grok Bots, ou tout agent branché sur la commande ou le MCP, remplissent le fichier. Griffe n'ouvre pas Notion.",
        "Le Qui : le nom, et le standard — courriel et téléphone de l'organisation. Qui répond est le contact : prénom, nom, la ligne directe seulement. Monsieur ou Madame si une source le dit, sinon rien. Le nom affiché ne tient pas lieu de ce contact.",
        "L'adresse s'écrit en entier — rue, code, ville, pays — ou pas du tout.",
        "Le Quoi nomme la conversation. L'analyse va dans les travaux. Une ligne d'estimation par mission du barème, le montant copié. Une mission absente du barème est nommée à part, sans montant.",
        "Une série déjà là, ou une série proposée à part. Le premier message suit la trame. Le constat prend la place de <constat>.",
        "Vous seul marquez la file. L'agent n'écrit dans le coffre que les lignes marquées. L'import crée la fiche, le contact, les travaux, l'estimation, la série et le brouillon.",
        "Le brouillon ne part pas. L'histoire du dossier ne reçoit pas une note de cet import.",
        "La procédure que suit l'agent vit chez vous. Cette lettre est le contrat.",
        "Un autre fichier, plus tard, tient la même forme. L'agent remplit. Vous marquez. Griffe écrit la fiche, le contact, les travaux, l'estimation, la série, le brouillon. Il laisse la file, l'envoi, un montant absent du barème, et la passphrase.",
    ],
    does_not: Some(
        "Le fichier d'à côté n'est pas le coffre. Griffe n'y va pas chercher les lignes tout seul.",
    ),
    href: Some("/aide/trame"),
    action: Some("Les phrases"),
    rayon: Rayon::Agent,
};

pub(super) const DEHORS: Letter = Letter {
    slug: "dehors",
    kicker: "Fichiers",
    title: "Ce qui reste dehors.",
    lede: "Ce que l'agent prépare, et ce qui reste votre geste.",
    steps: &[
        "La file. Vous seul décidez qui entre.",
        "L'envoi. Le brouillon attend. Dire que la lettre est partie, quand l'acteur est un agent, attend une personne.",
        "Le texte d'une personne dans la série. Garder cette lettre réécrit la série.",
        "Un montant absent du barème. La mission est nommée. Le prix ne s'invente pas.",
        "La passphrase, et le mot de passe d'envoi. Le mot de passe se donne avec --secret-file, ou dans un terminal. Ni l'un ni l'autre n'entre dans la conversation, ni dans un argument.",
    ],
    does_not: Some("Ces cinq gestes ne se délèguent pas."),
    href: Some("/aide/fichier"),
    action: Some("Un fichier à côté"),
    rayon: Rayon::Agent,
};

pub(super) const CMD_COFFRE: Letter = Letter {
    slug: "cmd-coffre",
    kicker: "La commande",
    title: "Le coffre, en commande.",
    lede: "Ouvrir, garder la session, confirmer, sauvegarder.",
    steps: &[
        "Chaque ligne plus bas est un geste. Le nom, ce qu'il fait, un exemple à recopier.",
        "Les drapeaux communs : --json pour un agent, --dry-run pour voir sans écrire, --actor agent:NOM quand c'est un agent qui demande.",
        "Ouvrir, fermer, changer la passphrase, restaurer : le terminal les fait. La console de la fenêtre les refuse.",
    ],
    does_not: Some("La passphrase ne s'écrit pas dans la ligne."),
    href: Some("/aide/brancher"),
    action: Some("Brancher l'agent"),
    rayon: Rayon::Agent,
};

pub(super) const CMD_GENS: Letter = Letter {
    slug: "cmd-gens",
    kicker: "La commande",
    title: "Les gens, en commande.",
    lede: "Le matin, les dossiers, les conversations, les séries.",
    steps: &[
        "La référence est le nom : Mairie de Valvert, Refonte du site.",
        "L'exemple est fictif. C'est le même coffre que la fenêtre.",
        "L'outil reprend souvent le nom, avec un point. Les exceptions sont dans Brancher l'agent.",
    ],
    does_not: Some(
        "Garder une lettre réécrit la série. On ne le fait pas avec le texte d'une personne.",
    ),
    href: Some("/aide/matin"),
    action: Some("Le matin de Valvert"),
    rayon: Rayon::Agent,
};

pub(super) const CMD_COURRIER: Letter = Letter {
    slug: "cmd-courrier",
    kicker: "La commande",
    title: "Le courrier, en commande.",
    lede: "Les clients, puis le compte qui poste.",
    steps: &[
        "Le mot de passe d'envoi ne s'écrit pas dans la ligne. --secret-file, ou le terminal, masqué.",
        "Armer une lettre ne la poste pas. flush poste.",
        "L'adresse qui signe s'écrit dans la commande. Elle n'est pas recopiée dans cette lettre.",
    ],
    does_not: Some("flush est l'envoi. On ne le laisse pas à l'agent."),
    href: Some("/aide/dehors"),
    action: Some("Ce qui reste dehors"),
    rayon: Rayon::Agent,
};

pub(super) const CMD_ARGENT: Letter = Letter {
    slug: "cmd-argent",
    kicker: "La commande",
    title: "L'argent, en commande.",
    lede: "Devis, mission, facture, encaissement, relevé, dépense.",
    steps: &[
        "Un devis et une mission se retrouvent par le nom. Une facture émise se retrouve par son identifiant.",
        "Sur une facture émise ici, le prix unitaire du JSON est en centimes.",
        "Émettre, encaisser, rapprocher, supprimer : avec un agent, le geste attend.",
    ],
    does_not: Some("Une facture émise ne se corrige pas. Un avoir."),
    href: Some("/aide/cmd-gens"),
    action: Some("Les gens, en commande"),
    rayon: Rayon::Agent,
};

pub(super) const CMD_SOCIETE: Letter = Letter {
    slug: "cmd-societe",
    kicker: "La commande",
    title: "La société, en commande.",
    lede: "L'identité, ce que l'État attend, la clôture, les papiers.",
    steps: &[
        "Les montants sont indicatifs. Déposer se fait dehors.",
        "Clore, approuver, écrire la TVA du mois : avec un agent, le geste attend.",
        "Le bilan d'ouverture équilibre le débit et le crédit.",
    ],
    does_not: Some("Griffe prépare les montants. Il ne les dépose pas."),
    href: Some("/aide/cmd-argent"),
    action: Some("L'argent, en commande"),
    rayon: Rayon::Agent,
};

pub(super) const LETTERS: &[&Letter] = &[
    &PORTES,
    &BRANCHER,
    &MATIN,
    &CONSOLE,
    &DEMANDES,
    &TRAME,
    &CMD_COFFRE,
    &CMD_GENS,
    &CMD_COURRIER,
    &CMD_ARGENT,
    &CMD_SOCIETE,
    &FICHIER,
    &DEHORS,
];

pub(super) fn regard(slug: &str) -> Markup {
    match slug {
        "portes" => portes_regard(),
        "brancher" => brancher_regard(),
        "matin" => matin_regard(),
        "console" => console_regard(),
        "demandes" => demandes_regard(),
        "trame" => trame_regard(),
        "cmd-coffre" => cmd_coffre_regard(),
        "cmd-gens" => cmd_gens_regard(),
        "cmd-courrier" => cmd_courrier_regard(),
        "cmd-argent" => cmd_argent_regard(),
        "cmd-societe" => cmd_societe_regard(),
        "fichier" => fichier_regard(),
        "dehors" => dehors_regard(),
        _ => html! {},
    }
}

fn portes_regard() -> Markup {
    html! {
        div class="block" {
            h3 { "Trois portes" }
            ol class="aide-lignes" {
                li { "La fenêtre. Vous lisez, vous confirmez." }
                li { code { "griffe" } " La même chose, une phrase à la fois." }
                li { code { "griffe-mcp" } " L'agent appelle les outils. stdio, aucun port." }
            }
        }
        div class="block" {
            h3 { "Ce qu'une seule interface ne fait pas" }
            p class="prose" {
                "Le matin se relit tout seul. Un brouillon se prépare sans ouvrir l'écran. Vous gardez l'envoi, la suppression, et la passphrase."
            }
        }
    }
}

fn brancher_regard() -> Markup {
    html! {
        div class="block" {
            h3 { "Avec Grok" }
            pre class="aide-file" { "grok mcp add griffe -- griffe-mcp" }
        }
        div class="block" {
            h3 { "Le fichier de l'agent" }
            pre class="aide-file" {
                "{\n  \"mcpServers\": {\n    \"griffe\": {\n      \"command\": \"griffe-mcp\"\n    }\n  }\n}"
            }
        }
        div class="block" {
            h3 { "La compétence" }
            p class="prose" {
                "Avec Grok, ce fichier est SKILL.md, dans le dossier matin-griffe, sous skills, dans votre maison."
            }
            pre class="aide-file" {
                "---\nname: matin-griffe\ndescription: Lit le matin dans Griffe et prépare les brouillons. Ne poste rien.\n---\n\nQuand on demande le matin :\n\n1. Lis griffe://day.\n2. Pour une lettre due, lis la série avec follow_up.phrases, puis prépare le brouillon avec follow_up.prepare. N'ouvre pas le courrier.\n3. N'appelle pas follow_up.mark_sent. Dis ce qui attend une personne.\n"
            }
        }
        div class="block" {
            h3 { "Les lectures" }
            ul class="aide-jetons" {
                @for uri in [
                    "griffe://day",
                    "griffe://people",
                    "griffe://follow-ups",
                    "griffe://opportunities",
                    "griffe://clients",
                    "griffe://missions",
                    "griffe://quotes",
                    "griffe://expenses",
                    "griffe://company",
                    "griffe://society",
                    "griffe://papers",
                ] {
                    li { code { (uri) } }
                }
            }
        }
    }
}

fn matin_regard() -> Markup {
    html! {
        div class="block" {
            h3 { "La même matinée" }
            pre class="aide-file" {
                "griffe day gestures --json\n"
                "griffe people show \"Mairie de Valvert\"\n"
                "griffe follow-up phrases --genre \"Le premier contact\"\n"
                "griffe follow-up draft \"Refonte du site\" --body-file lettre.txt --no-open\n"
                "griffe pending list\n"
            }
        }
        div class="block" {
            h3 { "Les outils" }
            p class="prose" {
                code { "day.gestures" } ", "
                code { "people.show" } ", "
                code { "follow_up.phrases" } ", "
                code { "follow_up.prepare" } ". "
                "L'envoi attend."
            }
        }
    }
}

fn cmd_coffre_regard() -> Markup {
    html! {
        div class="block" {
            h3 { "Avant l'agent" }
            pre class="aide-file" { "griffe unlock --remember\ngriffe-mcp" }
        }
    }
}

fn cmd_gens_regard() -> Markup {
    html! {
        div class="block" {
            h3 { "Valvert, en cinq lignes" }
            pre class="aide-file" {
                "griffe prospect create --prospect \"Mairie de Valvert\" --name \"Refonte du site\" --amount 1200,00 --probability 40 --next-action 2026-09-12\n"
                "griffe prospect estimation set \"Refonte du site\" --line \"Refonte du site=1200,00\"\n"
                "griffe follow-up genre assign \"Refonte du site\" \"Le premier contact\"\n"
                "griffe follow-up draft \"Refonte du site\" --body-file lettre.txt --no-open\n"
            }
        }
    }
}

fn cmd_courrier_regard() -> Markup {
    html! {
        div class="block" {
            h3 { "Le mot de passe" }
            p class="prose" {
                code { "--secret-file" } " ou le terminal. Jamais dans la ligne, ni dans la conversation."
            }
        }
    }
}

fn cmd_argent_regard() -> Markup {
    html! {
        div class="block" {
            h3 { "Les centimes" }
            p class="prose" {
                "120000 dans le JSON d'une facture, c'est 1\u{202f}200,00\u{00a0}€. Un devis, lui, s'écrit 1200,00."
            }
        }
    }
}

fn cmd_societe_regard() -> Markup {
    html! {
        div class="block" {
            h3 { "Indicatif" }
            p class="prose" {
                "Les échéances et le prévisionnel se lisent. Le dépôt, l'approbation et l'envoi restent des gestes à vous."
            }
        }
    }
}

fn console_regard() -> Markup {
    html! {
        div class="block" {
            h3 { "La console refuse" }
            ol class="aide-lignes" {
                li { code { "init" } ", " code { "unlock" } ", " code { "lock" } }
                li { code { "passphrase change" } }
                li { code { "backup restore" } }
            }
        }
    }
}

fn demandes_regard() -> Markup {
    html! {
        div class="block" {
            h3 { "Le geste" }
            pre class="aide-file" {
                "prospect estimation set \"Refonte du site\" --line \"Refonte du site=1200,00\"\n"
                "follow-up phrases --genre \"Le premier contact\"\n"
                "follow-up draft \"Refonte du site\" --body-file lettre.txt --no-open\n"
                "people travaux \"Mairie de Valvert\" --file travaux.md"
            }
        }
        div class="block" {
            h3 { "Les outils" }
            p class="prose" {
                code { "prospect.create" } ", "
                code { "follow_up.phrases" } ", "
                code { "follow_up.set_genre" } ", "
                code { "follow_up.prepare" } ", "
                code { "people.save_work" } ". "
                "Les lignes d'estimation : la fenêtre et la commande."
            }
        }
    }
}

fn trame_regard() -> Markup {
    html! {
        div class="block" {
            h3 { "La trame" }
            pre class="aide-file" {
                "Bonjour <genre> <nom>,\n\n<constat>\n\nLa refonte du site, <montant>.\n\n<signature>"
            }
        }
        div class="block" {
            h3 { "Mairie de Valvert" }
            pre class="aide-file" {
                "Bonjour Madame Laurent,\n\nLe site de la mairie ne déclare pas ses horaires d'ouverture.\n\nLa refonte du site, 1\u{202f}200,00\u{00a0}€.\n\n<signature>"
            }
        }
        (jetons())
    }
}

fn fichier_regard() -> Markup {
    html! {
        div class="block" {
            h3 { "Mairie de Valvert" }
            dl class="aide-exemple" {
                dt { "Qui" }
                dd { "Mairie de Valvert" }
                dt { "Standard" }
                dd { "Le secrétariat, le téléphone de la mairie." }
                dt { "Qui répond" }
                dd { "Hélène Laurent. Madame, si une source le dit. Sa ligne directe." }
                dt { "Quoi" }
                dd { "Refonte du site" }
                dt { "Estimation" }
                dd { "Refonte du site — 1\u{202f}200,00\u{00a0}€" }
                dt { "Hors barème" }
                dd { "Une mission nommée à part, sans montant." }
                dt { "Série" }
                dd { "La trame. Le constat à la place de <constat>." }
            }
        }
        (jetons())
        div class="block" {
            h3 { "Le fichier suivant" }
            p class="prose" {
                "L'agent remplit. Vous marquez. Griffe écrit la fiche, le contact, les travaux, l'estimation, la série, le brouillon. Il laisse la file, l'envoi, le montant absent, et la passphrase."
            }
        }
    }
}

fn dehors_regard() -> Markup {
    html! {
        div class="block" {
            h3 { "Dehors" }
            ol class="aide-lignes" {
                li { "La file." }
                li { "L'envoi." }
                li { "Le texte d'une personne dans la série." }
                li { "Un montant absent du barème." }
                li { "La passphrase, le mot de passe d'envoi." }
            }
        }
    }
}

fn jetons() -> Markup {
    html! {
        div class="block" {
            h3 { "Les jetons" }
            ul class="aide-jetons" {
                @for token in [
                    "<prénom>",
                    "<nom>",
                    "<genre>",
                    "<contact>",
                    "<client>",
                    "<sujet>",
                    "<montant>",
                    "<constat>",
                    "<moi>",
                    "<société>",
                    "<signature>",
                ] {
                    li { code { (token) } }
                }
            }
        }
    }
}
