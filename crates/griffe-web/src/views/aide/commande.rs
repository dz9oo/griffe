//! Chaque feuille de `griffe --help`, une phrase et un exemple.
//!
//! Ajouter une commande au binaire : une ligne ici. Le test du module
//! compte les feuilles. Les exemples sont fictifs (Mairie de Valvert).

use maud::{Markup, html};

struct Cmd {
    verb: &'static str,
    said: &'static str,
    example: &'static str,
}

const COFFRE: &[Cmd] = &[
    Cmd {
        verb: r#"init"#,
        said: r#"Crée le coffre. S'il existe déjà, la commande échoue. Le terminal demande la passphrase deux fois, masquée."#,
        example: r#"griffe init"#,
    },
    Cmd {
        verb: r#"unlock"#,
        said: r#"Vérifie la passphrase et, avec --remember, garde la session dans le trousseau du système. Sans cela, griffe-mcp ne peut pas ouvrir le coffre. La session dure douze heures, ou la durée de --ttl."#,
        example: r#"griffe unlock --remember"#,
    },
    Cmd {
        verb: r#"lock"#,
        said: r#"Efface la session du trousseau. Le coffre reste. La prochaine commande redemandera la passphrase."#,
        example: r#"griffe lock"#,
    },
    Cmd {
        verb: r#"vault status"#,
        said: r#"Dit où est le coffre, s'il existe, et si une session est gardée. N'ouvre pas le coffre."#,
        example: r#"griffe vault status"#,
    },
    Cmd {
        verb: r#"passphrase change"#,
        said: r#"Change la passphrase et ré-chiffre le coffre. L'ancienne est demandée même si une session est ouverte. Une sauvegarde est écrite avant."#,
        example: r#"griffe passphrase change"#,
    },
    Cmd {
        verb: r#"setup status"#,
        said: r#"Ce qu'il reste à configurer au premier lancement : l'identité, le point de départ, un relevé. Et le prochain geste."#,
        example: r#"griffe setup status"#,
    },
    Cmd {
        verb: r#"setup new-company"#,
        said: r#"Déclare que la société vient d'être créée : pas de bilan antérieur à reprendre. --undo revient sur cette déclaration."#,
        example: r#"griffe setup new-company"#,
    },
    Cmd {
        verb: r#"pending list"#,
        said: r#"Liste les gestes qu'un agent a préparés et qui attendent une personne."#,
        example: r#"griffe pending list"#,
    },
    Cmd {
        verb: r#"confirm"#,
        said: r#"Applique un geste en attente. L'identifiant vient de pending list."#,
        example: r#"griffe confirm ID"#,
    },
    Cmd {
        verb: r#"audit verify-chain"#,
        said: r#"Revérifie que le journal n'a pas été altéré."#,
        example: r#"griffe audit verify-chain"#,
    },
    Cmd {
        verb: r#"backup create"#,
        said: r#"Écrit une sauvegarde tout de suite, sans attendre la suivante."#,
        example: r#"griffe backup create --out sauvegarde.db"#,
    },
    Cmd {
        verb: r#"backup restore"#,
        said: r#"Restaure vers un coffre neuf. Le coffre d'origine n'est pas touché. La console de la fenêtre refuse ce geste : il veut un terminal."#,
        example: r#"griffe backup restore --from sauvegarde.db --to coffre-neuf.db"#,
    },
];

const GENS: &[Cmd] = &[
    Cmd {
        verb: r#"day mast"#,
        said: r#"Le matin : la banque, la piste en mois, ce qu'il y a à encaisser."#,
        example: r#"griffe day mast"#,
    },
    Cmd {
        verb: r#"day gestures"#,
        said: r#"Les gestes du jour : les lettres dues, le relevé, une obligation."#,
        example: r#"griffe day gestures --json"#,
    },
    Cmd {
        verb: r#"day month"#,
        said: r#"La grille du mois. --month précise un autre mois, sous la forme AAAA-MM."#,
        example: r#"griffe day month --month 2026-09"#,
    },
    Cmd {
        verb: r#"people list"#,
        said: r#"Les affaires : en conversation, en mission, chez qui ça sort."#,
        example: r#"griffe people list"#,
    },
    Cmd {
        verb: r#"people show"#,
        said: r#"Le dossier d'une personne. Le nom suffit."#,
        example: r#"griffe people show "Mairie de Valvert""#,
    },
    Cmd {
        verb: r#"people travaux"#,
        said: r#"Le récit de la fiche. Sans --file, la commande l'affiche. Avec --file, elle le pose. Ce n'est pas une note ajoutée à l'histoire."#,
        example: r#"griffe people travaux "Mairie de Valvert" --file travaux.md"#,
    },
    Cmd {
        verb: r#"people types list"#,
        said: r#"Les types de travaux."#,
        example: r#"griffe people types list"#,
    },
    Cmd {
        verb: r#"people types create"#,
        said: r#"Crée un type."#,
        example: r#"griffe people types create Site"#,
    },
    Cmd {
        verb: r#"people types rename"#,
        said: r#"Renomme un type. Les dossiers gardent le lien. L'identifiant vient de la liste."#,
        example: r#"griffe people types rename ID Site"#,
    },
    Cmd {
        verb: r#"people types remove"#,
        said: r#"Retire le type et tous ses liens. Avec un agent, le geste attend une personne."#,
        example: r#"griffe people types remove ID"#,
    },
    Cmd {
        verb: r#"people types set"#,
        said: r#"Pose les types d'une fiche. Les noms sont séparés par des virgules. Une chaîne vide retire les liens."#,
        example: r#"griffe people types set "Mairie de Valvert" Site"#,
    },
    Cmd {
        verb: r#"people types dossiers"#,
        said: r#"Les dossiers qui portent ce type."#,
        example: r#"griffe people types dossiers ID"#,
    },
    Cmd {
        verb: r#"prospect create"#,
        said: r#"Ouvre une conversation, à l'étape qualification. --prospect crée la fiche, ou la retrouve par son nom exact. Le montant, la probabilité et la date sont demandés. Les lignes d'estimation viennent ensuite."#,
        example: r#"griffe prospect create --prospect "Mairie de Valvert" --name "Refonte du site" --amount 1200,00 --probability 40 --next-action 2026-09-12"#,
    },
    Cmd {
        verb: r#"prospect show"#,
        said: r#"Affiche une conversation."#,
        example: r#"griffe prospect show "Refonte du site""#,
    },
    Cmd {
        verb: r#"prospect list"#,
        said: r#"Les conversations ouvertes. --closed et --archived élargissent."#,
        example: r#"griffe prospect list"#,
    },
    Cmd {
        verb: r#"prospect edit"#,
        said: r#"Modifie une conversation ouverte. Seuls les champs donnés changent. L'étape se change avec advance, win ou lose."#,
        example: r#"griffe prospect edit "Refonte du site" --next-action 2026-09-20"#,
    },
    Cmd {
        verb: r#"prospect estimation set"#,
        said: r#"Pose les lignes de l'estimation. Chaque --line vaut Libellé=1200,00. Le dernier signe égal sépare le libellé du montant. Le total est la somme. Le Quoi ne change pas. La fenêtre et la commande le font. Le MCP n'a pas encore cet outil."#,
        example: r#"griffe prospect estimation set "Refonte du site" --line "Refonte du site=1200,00""#,
    },
    Cmd {
        verb: r#"prospect advance"#,
        said: r#"Avance vers une autre étape ouverte : discovery, proposal ou negotiation."#,
        example: r#"griffe prospect advance "Refonte du site" --to proposal --next-action 2026-09-20"#,
    },
    Cmd {
        verb: r#"prospect win"#,
        said: r#"La conversation est gagnée. Une mission au forfait est créée."#,
        example: r#"griffe prospect win "Refonte du site" --started-on 2026-09-05"#,
    },
    Cmd {
        verb: r#"prospect reopen"#,
        said: r#"Rouvre une conversation arrêtée, sur le même dossier."#,
        example: r#"griffe prospect reopen "Refonte du site" --next-action 2026-09-20"#,
    },
    Cmd {
        verb: r#"prospect lose"#,
        said: r#"Arrête la conversation, avec un motif : budget, timing, competitor, no-response, scope-mismatch, ou other:précision."#,
        example: r#"griffe prospect lose "Refonte du site" --reason timing"#,
    },
    Cmd {
        verb: r#"prospect archive"#,
        said: r#"Retire la conversation des listes, sans la supprimer et sans la marquer gagnée ou perdue."#,
        example: r#"griffe prospect archive "Refonte du site""#,
    },
    Cmd {
        verb: r#"prospect unarchive"#,
        said: r#"Ramène une conversation archivée dans les listes."#,
        example: r#"griffe prospect unarchive "Refonte du site""#,
    },
    Cmd {
        verb: r#"prospect rm"#,
        said: r#"Supprime la conversation. Refusé si un devis la référence : on l'archive. Avec un agent, le geste attend une personne."#,
        example: r#"griffe prospect rm "Refonte du site""#,
    },
    Cmd {
        verb: r#"prospect references"#,
        said: r#"Ce qui empêche de supprimer la conversation."#,
        example: r#"griffe prospect references "Refonte du site""#,
    },
    Cmd {
        verb: r#"prospect log-interaction"#,
        said: r#"Note un échange : call, email, meeting, visio ou note."#,
        example: r#"griffe prospect log-interaction "Refonte du site" --kind call --note "Les horaires du site manquent""#,
    },
    Cmd {
        verb: r#"prospect interaction list"#,
        said: r#"Les échanges déjà notés."#,
        example: r#"griffe prospect interaction list "Refonte du site""#,
    },
    Cmd {
        verb: r#"prospect interaction edit"#,
        said: r#"Modifie un échange. L'identifiant vient de la liste."#,
        example: r#"griffe prospect interaction edit ID --note "Les horaires d'ouverture manquent""#,
    },
    Cmd {
        verb: r#"prospect interaction rm"#,
        said: r#"Supprime un échange. Avec un agent, le geste attend une personne."#,
        example: r#"griffe prospect interaction rm ID"#,
    },
    Cmd {
        verb: r#"prospect late"#,
        said: r#"Les conversations ouvertes dont la prochaine date est passée."#,
        example: r#"griffe prospect late"#,
    },
    Cmd {
        verb: r#"prospect orphans"#,
        said: r#"Les conversations ouvertes sans prochaine date."#,
        example: r#"griffe prospect orphans"#,
    },
    Cmd {
        verb: r#"prospect pipeline"#,
        said: r#"Le pipeline pondéré, et la répartition par étape."#,
        example: r#"griffe prospect pipeline"#,
    },
    Cmd {
        verb: r#"follow-up queue"#,
        said: r#"La file du jour : en retard, dû aujourd'hui, brouillon ouvert."#,
        example: r#"griffe follow-up queue"#,
    },
    Cmd {
        verb: r#"follow-up board"#,
        said: r#"Toutes les relances actives."#,
        example: r#"griffe follow-up board"#,
    },
    Cmd {
        verb: r#"follow-up show"#,
        said: r#"La fiche d'une relance : la piste, l'historique, l'aperçu."#,
        example: r#"griffe follow-up show "Refonte du site""#,
    },
    Cmd {
        verb: r#"follow-up from"#,
        said: r#"Pose l'adresse qui signe les brouillons. On écrit l'adresse, pas dans cette page."#,
        example: r#"griffe follow-up from --email ADRESSE"#,
    },
    Cmd {
        verb: r#"follow-up draft"#,
        said: r#"Prépare un brouillon. --no-open l'écrit sans ouvrir le client. --body-file lit le texte. Rien ne part."#,
        example: r#"griffe follow-up draft "Refonte du site" --body-file lettre.txt --no-open"#,
    },
    Cmd {
        verb: r#"follow-up sent"#,
        said: r#"Dit que la lettre est partie, et avance la cadence. On ne le fait qu'une fois la lettre vraiment postée. Avec un agent, le geste attend une personne."#,
        example: r#"griffe follow-up sent "Refonte du site" --actor agent:matin"#,
    },
    Cmd {
        verb: r#"follow-up skip"#,
        said: r#"Saute l'étape courante, sans envoyer."#,
        example: r#"griffe follow-up skip "Refonte du site""#,
    },
    Cmd {
        verb: r#"follow-up snooze"#,
        said: r#"Reporte sans changer d'étape. --tomorrow, --days 3, ou --until 2026-09-20."#,
        example: r#"griffe follow-up snooze "Refonte du site" --until 2026-09-20"#,
    },
    Cmd {
        verb: r#"follow-up schedule"#,
        said: r#"Pose la prochaine date à la main. L'étape ne change pas."#,
        example: r#"griffe follow-up schedule "Refonte du site" --on 2026-09-20"#,
    },
    Cmd {
        verb: r#"follow-up phrases"#,
        said: r#"Les phrases de la série, dans l'ordre. --genre choisit la série."#,
        example: r#"griffe follow-up phrases --genre "Le premier contact""#,
    },
    Cmd {
        verb: r#"follow-up moments add"#,
        said: r#"Ajoute un moment à la fin de la série. --days est l'écart en jours ouvrés."#,
        example: r#"griffe follow-up moments add --label "Relance" --subject "Le site" --days 5"#,
    },
    Cmd {
        verb: r#"follow-up moments drop"#,
        said: r#"Retire un moment. Il en reste au moins un. Les clés livrées sont hello, bump, value, close."#,
        example: r#"griffe follow-up moments drop bump"#,
    },
    Cmd {
        verb: r#"follow-up moments earlier"#,
        said: r#"Monte le moment d'un cran."#,
        example: r#"griffe follow-up moments earlier bump"#,
    },
    Cmd {
        verb: r#"follow-up moments later"#,
        said: r#"Descend le moment d'un cran."#,
        example: r#"griffe follow-up moments later bump"#,
    },
    Cmd {
        verb: r#"follow-up moments gap"#,
        said: r#"Règle l'écart, en jours ouvrés après le moment précédent."#,
        example: r#"griffe follow-up moments gap bump --days 5"#,
    },
    Cmd {
        verb: r#"follow-up rewrite"#,
        said: r#"Réécrit le nom, le sujet et la lettre d'un moment, pour la série de base."#,
        example: r#"griffe follow-up rewrite hello --label "Premier message" --subject "Le site""#,
    },
    Cmd {
        verb: r#"follow-up genres"#,
        said: r#"Les séries. Les mots seulement : l'ordre et les écarts restent ceux du coffre."#,
        example: r#"griffe follow-up genres"#,
    },
    Cmd {
        verb: r#"follow-up genre add"#,
        said: r#"Crée une série en copiant les phrases par défaut, ou celles d'une autre avec --from."#,
        example: r#"griffe follow-up genre add "Le premier contact""#,
    },
    Cmd {
        verb: r#"follow-up genre drop"#,
        said: r#"Retire une série. Les fiches qui la portaient redeviennent sans série."#,
        example: r#"griffe follow-up genre drop ID"#,
    },
    Cmd {
        verb: r#"follow-up genre rewrite"#,
        said: r#"Réécrit le sujet et le corps d'un moment, pour cette série seulement."#,
        example: r#"griffe follow-up genre rewrite ID hello --subject "Le site de la mairie""#,
    },
    Cmd {
        verb: r#"follow-up genre assign"#,
        said: r#"Pose une série sur le dossier. Un nom inconnu crée la série. On ne le fait qu'exprès."#,
        example: r#"griffe follow-up genre assign "Refonte du site" "Le premier contact""#,
    },
    Cmd {
        verb: r#"follow-up genre clear"#,
        said: r#"Enlève la série du dossier."#,
        example: r#"griffe follow-up genre clear "Refonte du site""#,
    },
    Cmd {
        verb: r#"follow-up keep"#,
        said: r#"Garde le sujet et le corps pour cette série et ce moment. Cela réécrit la série, pour les suivantes. On ne l'appelle pas avec le texte d'une personne."#,
        example: r#"griffe follow-up keep "Refonte du site""#,
    },
    Cmd {
        verb: r#"follow-up retract"#,
        said: r#"Annule le dernier geste : envoi, saut, report ou brouillon."#,
        example: r#"griffe follow-up retract "Refonte du site""#,
    },
];

const COURRIER: &[Cmd] = &[
    Cmd {
        verb: r#"client create"#,
        said: r#"Crée un client. Le nom suffit pour commencer."#,
        example: r#"griffe client create --name "Mairie de Valvert""#,
    },
    Cmd {
        verb: r#"client show"#,
        said: r#"Affiche un client."#,
        example: r#"griffe client show "Mairie de Valvert""#,
    },
    Cmd {
        verb: r#"client list"#,
        said: r#"Les clients actifs. --archived inclut les archivés."#,
        example: r#"griffe client list"#,
    },
    Cmd {
        verb: r#"client edit"#,
        said: r#"Modifie un client. Seuls les champs donnés changent. --clear-siren, --clear-vat-number et --clear-address effacent un champ."#,
        example: r#"griffe client edit "Mairie de Valvert" --name "Mairie de Valvert""#,
    },
    Cmd {
        verb: r#"client archive"#,
        said: r#"Retire un client des listes, sans le supprimer. Les factures et les devis passés gardent leur lien."#,
        example: r#"griffe client archive "Mairie de Valvert""#,
    },
    Cmd {
        verb: r#"client unarchive"#,
        said: r#"Ramène un client archivé dans les listes."#,
        example: r#"griffe client unarchive "Mairie de Valvert""#,
    },
    Cmd {
        verb: r#"client rm"#,
        said: r#"Supprime un client. Refusé s'il est encore lié à une conversation, un devis, une mission ou une facture : on l'archive. Avec un agent, le geste attend une personne."#,
        example: r#"griffe client rm "Mairie de Valvert""#,
    },
    Cmd {
        verb: r#"client contact add"#,
        said: r#"Ajoute un contact. Le nom affiché ne tient pas lieu du standard de la fiche."#,
        example: r#"griffe client contact add --client "Mairie de Valvert" --name "Hélène Laurent""#,
    },
    Cmd {
        verb: r#"client contact list"#,
        said: r#"Les contacts d'un client."#,
        example: r#"griffe client contact list --client "Mairie de Valvert""#,
    },
    Cmd {
        verb: r#"client contact edit"#,
        said: r#"Modifie un contact. --clear-email, --clear-phone et --clear-role effacent un champ. L'identifiant vient de la liste."#,
        example: r#"griffe client contact edit ID --name "Hélène Laurent""#,
    },
    Cmd {
        verb: r#"client contact rm"#,
        said: r#"Supprime un contact. Avec un agent, le geste attend une personne."#,
        example: r#"griffe client contact rm ID"#,
    },
    Cmd {
        verb: r#"courrier show"#,
        said: r#"Le compte d'envoi, sans le mot de passe."#,
        example: r#"griffe courrier show"#,
    },
    Cmd {
        verb: r#"courrier save"#,
        said: r#"Enregistre le serveur. L'adresse qui signe, puis l'identifiant. Pour iCloud, l'hôte et le port se remplissent. Sinon, --host et --port (587 ou 465)."#,
        example: r#"griffe courrier save ADRESSE IDENTIFIANT"#,
    },
    Cmd {
        verb: r#"courrier secret"#,
        said: r#"Pose ou retire le mot de passe d'envoi. Invite masquée, ou --secret-file. Jamais en argument. --clear le retire."#,
        example: r#"griffe courrier secret --secret-file secret.txt"#,
    },
    Cmd {
        verb: r#"courrier auto"#,
        said: r#"Allume ou éteint l'envoi des lettres dont la date est aujourd'hui. Avec un agent, le geste attend une personne."#,
        example: r#"griffe courrier auto --off"#,
    },
    Cmd {
        verb: r#"courrier apparence"#,
        said: r#"La feuille de la lettre. Sans drapeau, montre l'allure. --theme, --metier et --site posent la feuille livrée."#,
        example: r#"griffe courrier apparence"#,
    },
    Cmd {
        verb: r#"courrier signature"#,
        said: r#"Montre la formule. --set la pose depuis l'entrée standard."#,
        example: r#"griffe courrier signature"#,
    },
    Cmd {
        verb: r#"courrier essayer"#,
        said: r#"Essaie la liaison, sans envoyer de lettre."#,
        example: r#"griffe courrier essayer"#,
    },
    Cmd {
        verb: r#"courrier essai"#,
        said: r#"Arme la lettre d'essai. flush la poste après cinq secondes. --to choisit le destinataire. Sans lui, c'est l'adresse qui signe. Avec un agent, le geste attend une personne."#,
        example: r#"griffe courrier essai"#,
    },
    Cmd {
        verb: r#"courrier envoyer"#,
        said: r#"Arme une lettre. Le texte vient de --body ou de --body-file. flush la poste. Armer n'envoie pas. Avec un agent, le geste attend une personne."#,
        example: r#"griffe courrier envoyer DESTINATAIRE "Le site" --body "Bonjour.""#,
    },
    Cmd {
        verb: r#"courrier annuler"#,
        said: r#"Retient une lettre encore armée. L'identifiant vient de la liste des lettres en attente. Avec un agent, le geste attend une personne."#,
        example: r#"griffe courrier annuler ID"#,
    },
    Cmd {
        verb: r#"courrier flush"#,
        said: r#"Poste les lettres échues préparées dans ce terminal. Celles que la fenêtre retient encore restent. C'est l'envoi."#,
        example: r#"griffe courrier flush"#,
    },
];

const ARGENT: &[Cmd] = &[
    Cmd {
        verb: r#"mission create"#,
        said: r#"Crée une mission sans passer par une conversation gagnée. regie demande --daily-rate, forfait demande --budget, recurrent demande --monthly-amount."#,
        example: r#"griffe mission create --client "Mairie de Valvert" --name "Refonte du site" --kind forfait --budget 1200,00 --started-on 2026-09-05"#,
    },
    Cmd {
        verb: r#"mission show"#,
        said: r#"Affiche une mission."#,
        example: r#"griffe mission show "Refonte du site""#,
    },
    Cmd {
        verb: r#"mission list"#,
        said: r#"Les missions en cours. --ended et --archived élargissent."#,
        example: r#"griffe mission list"#,
    },
    Cmd {
        verb: r#"mission edit"#,
        said: r#"Modifie une mission. Seuls les champs donnés changent. Clore se fait avec close."#,
        example: r#"griffe mission edit "Refonte du site" --name "Refonte du site""#,
    },
    Cmd {
        verb: r#"mission close"#,
        said: r#"Clôt la mission à une date. On peut la rouvrir. Ce n'est pas l'archivage."#,
        example: r#"griffe mission close "Refonte du site" --ended-on 2026-12-01"#,
    },
    Cmd {
        verb: r#"mission reopen"#,
        said: r#"Rouvre une mission clôturée."#,
        example: r#"griffe mission reopen "Refonte du site""#,
    },
    Cmd {
        verb: r#"mission archive"#,
        said: r#"Retire la mission des listes, sans la clore et sans la supprimer."#,
        example: r#"griffe mission archive "Refonte du site""#,
    },
    Cmd {
        verb: r#"mission unarchive"#,
        said: r#"Ramène une mission archivée."#,
        example: r#"griffe mission unarchive "Refonte du site""#,
    },
    Cmd {
        verb: r#"mission rm"#,
        said: r#"Supprime la mission. Refusé si une facture ou une saisie de temps la référence. Avec un agent, le geste attend une personne."#,
        example: r#"griffe mission rm "Refonte du site""#,
    },
    Cmd {
        verb: r#"mission references"#,
        said: r#"Ce qui empêche de supprimer la mission."#,
        example: r#"griffe mission references "Refonte du site""#,
    },
    Cmd {
        verb: r#"mission schedule"#,
        said: r#"L'échéancier de facturation, dérivé du type de mission."#,
        example: r#"griffe mission schedule "Refonte du site""#,
    },
    Cmd {
        verb: r#"mission log-time"#,
        said: r#"Saisit du temps. Les catégories : billable, pre_sales, admin, training."#,
        example: r#"griffe mission log-time "Refonte du site" --worked-on 2026-09-05 --days 1 --category billable"#,
    },
    Cmd {
        verb: r#"mission time list"#,
        said: r#"Les saisies de temps de la mission."#,
        example: r#"griffe mission time list "Refonte du site""#,
    },
    Cmd {
        verb: r#"mission time edit"#,
        said: r#"Modifie une saisie. L'identifiant vient de la liste."#,
        example: r#"griffe mission time edit ID --days 0.5"#,
    },
    Cmd {
        verb: r#"mission time rm"#,
        said: r#"Supprime une saisie de temps. Avec un agent, le geste attend une personne."#,
        example: r#"griffe mission time rm ID"#,
    },
    Cmd {
        verb: r#"mission rate"#,
        said: r#"Le taux journalier effectif : le chiffre d'affaires divisé par les jours facturables."#,
        example: r#"griffe mission rate "Refonte du site""#,
    },
    Cmd {
        verb: r#"mission capacity"#,
        said: r#"La capacité vendue pour un mois, tous clients confondus."#,
        example: r#"griffe mission capacity --month 2026-09"#,
    },
    Cmd {
        verb: r#"quote create"#,
        said: r#"Crée un devis, version 1. Une ligne : description:type:montant. forfait, regie (montant fois les jours), recurrent (montant fois les mois). Le taux, s'il est donné : standard, intermediate, reduced, super_reduced, zero."#,
        example: r#"griffe quote create --client "Mairie de Valvert" --valid-until 2026-10-12 --line "Refonte du site:forfait:1200,00""#,
    },
    Cmd {
        verb: r#"quote list"#,
        said: r#"Les devis, toutes versions, les plus récents d'abord."#,
        example: r#"griffe quote list"#,
    },
    Cmd {
        verb: r#"quote show"#,
        said: r#"Un devis : le contenu, les montants, la lignée. Le nom du client suffit s'il n'en a qu'un."#,
        example: r#"griffe quote show "Mairie de Valvert""#,
    },
    Cmd {
        verb: r#"quote revise"#,
        said: r#"Une nouvelle version. La référence est le nom du client, ou l'identifiant. N'importe quelle version de la lignée convient."#,
        example: r#"griffe quote revise "Mairie de Valvert" --valid-until 2026-11-12 --line "Refonte du site:forfait:1200,00""#,
    },
    Cmd {
        verb: r#"quote send"#,
        said: r#"Marque le devis comme envoyé."#,
        example: r#"griffe quote send "Mairie de Valvert""#,
    },
    Cmd {
        verb: r#"quote decline"#,
        said: r#"Décline un devis envoyé."#,
        example: r#"griffe quote decline "Mairie de Valvert""#,
    },
    Cmd {
        verb: r#"quote accept"#,
        said: r#"Accepte un devis envoyé. Crée la mission et l'échéancier. La référence est le nom du client, ou l'identifiant."#,
        example: r#"griffe quote accept "Mairie de Valvert" --started-on 2026-09-05"#,
    },
    Cmd {
        verb: r#"invoice emit"#,
        said: "Émet une facture. Le client se donne par son identifiant. griffe client show \"Mairie de Valvert\" --json le montre. Le prix unitaire du JSON est en centimes : 120000 pour 1\u{202f}200,00\u{00a0}€. Une facture émise ne se modifie pas : un avoir. Avec un agent, le geste attend une personne.",
        example: r#"griffe invoice emit --client ID --issued-on 2026-09-12 --lines '[{"description":"Refonte du site","quantity":1,"unit_price":120000,"vat_rate":"Standard"}]' --actor agent:matin --dry-run"#,
    },
    Cmd {
        verb: r#"invoice import"#,
        said: r#"Enregistre une facture née ailleurs. Le numéro est celui du document, pas un numéro alloué ici. Le client est son identifiant. --file archive le fichier. Avec un agent, le geste attend une personne."#,
        example: r#"griffe invoice import --file facture.pdf --client ID --number "EXT-14" --issued-on 2026-09-12 --lines '[{"description":"Refonte du site","quantity":1,"unit_price":120000,"vat_rate":"Standard"}]'"#,
    },
    Cmd {
        verb: r#"invoice credit-note"#,
        said: r#"Émet un avoir qui annule toute la facture. L'identifiant est celui de la facture, pas son numéro. Avec un agent, le geste attend une personne."#,
        example: r#"griffe invoice credit-note --id ID --issued-on 2026-09-20"#,
    },
    Cmd {
        verb: r#"invoice write-off"#,
        said: r#"Enregistre que cette facture ne sera pas encaissée. L'identifiant est celui de la facture. Avec un agent, le geste attend une personne."#,
        example: r#"griffe invoice write-off --id ID --on 2026-09-20"#,
    },
    Cmd {
        verb: r#"invoice retract-write-off"#,
        said: r#"Rétablit une créance qui avait été marquée non encaissée. L'identifiant est celui de cette écriture. Avec un agent, le geste attend une personne."#,
        example: r#"griffe invoice retract-write-off --id ID --on 2026-09-21"#,
    },
    Cmd {
        verb: r#"invoice verify-chain"#,
        said: r#"Revérifie la chaîne de hash des factures."#,
        example: r#"griffe invoice verify-chain"#,
    },
    Cmd {
        verb: r#"invoice aged-balance"#,
        said: r#"La balance âgée. --today fixe la date. Sans lui, c'est aujourd'hui."#,
        example: r#"griffe invoice aged-balance"#,
    },
    Cmd {
        verb: r#"invoice render"#,
        said: r#"Rend la facture en PDF, avec le XML embarqué. L'identifiant est celui de la facture. L'identité de l'émetteur doit être posée."#,
        example: r#"griffe invoice render --id ID --out facture.pdf"#,
    },
    Cmd {
        verb: r#"payment record"#,
        said: r#"Enregistre un encaissement. La facture se donne par son identifiant. Les modes : bank_transfer, check, card, other. Avec un agent, le geste attend une personne."#,
        example: r#"griffe payment record --invoice ID --amount 1200,00 --received-on 2026-09-30 --method bank_transfer"#,
    },
    Cmd {
        verb: r#"payment list"#,
        said: r#"Les encaissements, annulés compris, les plus récents d'abord."#,
        example: r#"griffe payment list"#,
    },
    Cmd {
        verb: r#"payment void"#,
        said: r#"Annule un encaissement saisi à tort. Il reste dans l'historique et sort des calculs. Avec un agent, le geste attend une personne."#,
        example: r#"griffe payment void --id ID"#,
    },
    Cmd {
        verb: r#"bank import"#,
        said: r#"Importe un relevé : csv, ofx ou xlsx. Le format est reconnu."#,
        example: r#"griffe bank import releve.csv"#,
    },
    Cmd {
        verb: r#"bank rm"#,
        said: r#"Supprime une transaction importée par erreur, si elle n'est pas rapprochée. Avec un agent, le geste attend une personne."#,
        example: r#"griffe bank rm ID"#,
    },
    Cmd {
        verb: r#"bank list"#,
        said: r#"Les transactions importées. --unmatched ne garde que celles qui restent à rapprocher."#,
        example: r#"griffe bank list --unmatched"#,
    },
    Cmd {
        verb: r#"bank reconcile"#,
        said: r#"Rapproche un crédit avec une facture, et crée l'encaissement. Les deux identifiants viennent du JSON. Un débit se rapproche d'une dépense. Avec un agent, le geste attend une personne."#,
        example: r#"griffe bank reconcile --transaction ID --invoice ID"#,
    },
    Cmd {
        verb: r#"bank unreconcile"#,
        said: r#"Défait un rapprochement. La transaction est libérée. L'encaissement qui en venait est annulé. Avec un agent, le geste attend une personne."#,
        example: r#"griffe bank unreconcile --transaction ID"#,
    },
    Cmd {
        verb: r#"bank settle"#,
        said: r#"Règle un compte de bilan depuis un mouvement du relevé. Ni une charge, ni un produit. Ici, le compte 401000. Avec un agent, le geste attend une personne."#,
        example: r#"griffe bank settle --account 401000 ID"#,
    },
    Cmd {
        verb: r#"bank unsettle"#,
        said: r#"Défait ce règlement. La transaction redevient à rapprocher. Avec un agent, le geste attend une personne."#,
        example: r#"griffe bank unsettle ID"#,
    },
    Cmd {
        verb: r#"expense record"#,
        said: r#"Enregistre une dépense. Le montant est TTC. Les catégories : software, equipment, travel, meals, office, professional, fees, bank_charges, taxes, other. Si elle est payée de votre poche (--paid-by me) ou rapprochée d'un débit, un agent attend."#,
        example: r#"griffe expense record --label "Hébergement" --category software --amount 120,00 --vat-rate standard --vat-deductible 20,00 --incurred-on 2026-09-01"#,
    },
    Cmd {
        verb: r#"expense attach"#,
        said: r#"Joint le justificatif, ou le remplace. La pièce ne change ni le montant ni la date. On peut le faire même dans un exercice déjà clos."#,
        example: r#"griffe expense attach ID justificatif.pdf"#,
    },
    Cmd {
        verb: r#"expense receipt"#,
        said: r#"Déchiffre le justificatif. --out choisit le fichier. Sans lui, un fichier temporaire s'ouvre."#,
        example: r#"griffe expense receipt ID --out justificatif.pdf"#,
    },
    Cmd {
        verb: r#"expense list"#,
        said: r#"Les dépenses, les plus récentes d'abord."#,
        example: r#"griffe expense list"#,
    },
    Cmd {
        verb: r#"expense show"#,
        said: r#"Affiche une dépense."#,
        example: r#"griffe expense show ID"#,
    },
    Cmd {
        verb: r#"expense edit"#,
        said: r#"Modifie une dépense. Seuls les champs donnés changent. Refusé si la date tombe dans un exercice clos."#,
        example: r#"griffe expense edit ID --label "Hébergement annuel""#,
    },
    Cmd {
        verb: r#"expense rm"#,
        said: r#"Supprime la dépense. Le justificatif archivé reste. Un débit rapproché est libéré. Avec un agent, le geste attend une personne."#,
        example: r#"griffe expense rm ID"#,
    },
    Cmd {
        verb: r#"expense reconcile"#,
        said: r#"Rapproche la dépense d'un débit du relevé, au montant exact. Défaire : bank unreconcile. Avec un agent, le geste attend une personne."#,
        example: r#"griffe expense reconcile ID --transaction ID"#,
    },
];

const SOCIETE: &[Cmd] = &[
    Cmd {
        verb: r#"company set-profile"#,
        said: r#"Pose l'identité qui figure sur les factures. Le numéro doit passer le contrôle. L'exemple ci-dessous est fictif."#,
        example: r#"griffe company set-profile --name "Atelier Valvert" --legal-form "Société" --siren 999000003 --street "1 place de la Mairie" --postal-code 12345 --city Valvert --country FR"#,
    },
    Cmd {
        verb: r#"company show"#,
        said: r#"L'identité enregistrée, et la règle de déclaration de TVA qui en découle."#,
        example: r#"griffe company show"#,
    },
    Cmd {
        verb: r#"society show"#,
        said: r#"La société : l'identité courte, le paysage, les conversations, les chapitres."#,
        example: r#"griffe society show"#,
    },
    Cmd {
        verb: r#"society pay"#,
        said: r#"Ce que l'on peut se verser ce mois-ci sans casser la piste."#,
        example: r#"griffe society pay"#,
    },
    Cmd {
        verb: r#"society current-account"#,
        said: r#"Entre vous et la société : ce que la société vous doit."#,
        example: r#"griffe society current-account"#,
    },
    Cmd {
        verb: r#"society duties"#,
        said: r#"Ce qui est dû à l'État : les dates, les montants, où déposer."#,
        example: r#"griffe society duties"#,
    },
    Cmd {
        verb: r#"society duty"#,
        said: r#"La lettre d'une démarche, hors de l'application. Ici, la cotisation foncière. --period précise la période."#,
        example: r#"griffe society duty cfe"#,
    },
    Cmd {
        verb: r#"society closing"#,
        said: r#"Clore l'exercice, en phrases."#,
        example: r#"griffe society closing"#,
    },
    Cmd {
        verb: r#"society statement"#,
        said: r#"Le relevé : chaque mouvement, une lecture."#,
        example: r#"griffe society statement"#,
    },
    Cmd {
        verb: r#"society identity"#,
        said: r#"La carte d'identité de la société."#,
        example: r#"griffe society identity"#,
    },
    Cmd {
        verb: r#"society filed"#,
        said: r#"Marque une démarche comme déposée. C'est votre geste, pas un accusé de l'administration. Avec un agent, le geste attend une personne."#,
        example: r#"griffe society filed cfe"#,
    },
    Cmd {
        verb: r#"society unfiled"#,
        said: r#"Retire le marquage déposé. Avec un agent, le geste attend une personne."#,
        example: r#"griffe society unfiled cfe"#,
    },
    Cmd {
        verb: r#"society vat-credit show"#,
        said: r#"Le crédit de TVA repris, s'il est enregistré."#,
        example: r#"griffe society vat-credit show"#,
    },
    Cmd {
        verb: r#"society vat-credit set"#,
        said: r#"Enregistre le crédit de TVA repris. --after est le mois de la déclaration déjà déposée, sous la forme AAAA-MM. On l'écrit une fois, puis il est figé. Avec un agent, le geste attend une personne."#,
        example: r#"griffe society vat-credit set --after 2026-08 --amount 120,00"#,
    },
    Cmd {
        verb: r#"society vat-credit rm"#,
        said: r#"Refusé : le crédit repris est figé."#,
        example: r#"griffe society vat-credit rm"#,
    },
    Cmd {
        verb: r#"society vat-refund request"#,
        said: r#"Demande le versement d'un crédit de TVA. Toute la somme, ou --amount pour une partie. La période est AAAA-MM. Avec un agent, le geste attend une personne."#,
        example: r#"griffe society vat-refund request 2026-08"#,
    },
    Cmd {
        verb: r#"society vat-refund retract"#,
        said: r#"Annule la demande, si la déclaration du mois n'est pas encore marquée déposée. Avec un agent, le geste attend une personne."#,
        example: r#"griffe society vat-refund retract 2026-08"#,
    },
    Cmd {
        verb: r#"society vat-reversal record"#,
        said: r#"Enregistre une TVA à rendre, trop déduite, sur la déclaration du mois. Avec un agent, le geste attend une personne."#,
        example: r#"griffe society vat-reversal record 2026-08 --amount 20,00"#,
    },
    Cmd {
        verb: r#"society vat-reversal retract"#,
        said: r#"Annule cette écriture, si la déclaration du mois n'est pas encore marquée déposée. Avec un agent, le geste attend une personne."#,
        example: r#"griffe society vat-reversal retract 2026-08"#,
    },
    Cmd {
        verb: r#"society liquidate"#,
        said: r#"Écrit la TVA du mois. La période est AAAA-MM. Avec un agent, le geste attend une personne."#,
        example: r#"griffe society liquidate 2026-08"#,
    },
    Cmd {
        verb: r#"society payroll"#,
        said: r#"Écrit une paie déjà chiffrée : le brut, la retenue, les cotisations. Griffe ne calcule pas la paie. Avec un agent, le geste attend une personne."#,
        example: r#"griffe society payroll --on 2026-09-30 --gross 3000,00 --withholding 200,00 --employer 900,00"#,
    },
    Cmd {
        verb: r#"fiscal deadlines"#,
        said: r#"Les prochaines échéances, sans les montants. Dates indicatives."#,
        example: r#"griffe fiscal deadlines"#,
    },
    Cmd {
        verb: r#"fiscal calendar"#,
        said: r#"Les douze prochains mois, avec les montants indicatifs. Ils se recopient. Ils ne se déposent pas d'ici."#,
        example: r#"griffe fiscal calendar"#,
    },
    Cmd {
        verb: r#"forecast show"#,
        said: r#"Le prévisionnel de trésorerie sur douze mois. --starting-cash est la trésorerie de départ."#,
        example: r#"griffe forecast show --starting-cash 1200,00"#,
    },
    Cmd {
        verb: r#"year close"#,
        said: r#"Clôt un exercice : fige le résultat et enregistre l'affectation, encore en projet. --period est l'année civile. --legal-reserve et --dividends sont en euros. Avec un agent, le geste attend une personne."#,
        example: r#"griffe year close --period 2026"#,
    },
    Cmd {
        verb: r#"year list"#,
        said: r#"Les exercices clos, du plus ancien au plus récent."#,
        example: r#"griffe year list"#,
    },
    Cmd {
        verb: r#"year show"#,
        said: r#"Le détail d'un exercice clos."#,
        example: r#"griffe year show 2026"#,
    },
    Cmd {
        verb: r#"year amend"#,
        said: r#"Révise l'affectation d'un exercice encore en projet. Le résultat figé ne change pas."#,
        example: r#"griffe year amend 2026 --dividends 0"#,
    },
    Cmd {
        verb: r#"year approve"#,
        said: r#"Approuve l'exercice, à la date de la décision. Il devient immuable. Une sauvegarde est écrite juste avant. Avec un agent, le geste attend une personne."#,
        example: r#"griffe year approve 2026 --approved-on 2026-06-30"#,
    },
    Cmd {
        verb: r#"year rm"#,
        said: r#"Supprime un exercice encore en projet, clos par erreur. Avec un agent, le geste attend une personne."#,
        example: r#"griffe year rm 2026"#,
    },
    Cmd {
        verb: r#"year checklist"#,
        said: r#"Où en est la clôture, ce qui bloque, ce qui reste après."#,
        example: r#"griffe year checklist 2026"#,
    },
    Cmd {
        verb: r#"year glossary"#,
        said: r#"Les mots de la clôture, en phrases."#,
        example: r#"griffe year glossary"#,
    },
    Cmd {
        verb: r#"year balance"#,
        said: r#"La balance et le bilan dérivés du grand livre. L'exercice n'a pas besoin d'être clos."#,
        example: r#"griffe year balance 2026"#,
    },
    Cmd {
        verb: r#"year render"#,
        said: r#"Rend un document dans --out. Les documents : minutes, appropriation, synthesis, liasse, balance-sheet, inventory, efi-notice."#,
        example: r#"griffe year render 2026 synthesis --out synthese.pdf"#,
    },
    Cmd {
        verb: r#"year opening show"#,
        said: r#"Le bilan d'ouverture : les lignes, les totaux, les capitaux repris."#,
        example: r#"griffe year opening show"#,
    },
    Cmd {
        verb: r#"year opening set"#,
        said: r#"Enregistre le bilan d'ouverture, ou le remplace en entier. Une ligne vaut compte:libellé:D|C:montant. Le débit et le crédit doivent s'équilibrer. Avec un agent, le geste attend une personne."#,
        example: r#"griffe year opening set --opens-on 2026-01-01 --line "512000:Banque:D:1200,00" --line "101000:Capital:C:1200,00""#,
    },
    Cmd {
        verb: r#"year opening import"#,
        said: r#"Reprend le bilan d'ouverture depuis une balance en csv, ou depuis le fichier d'écritures de l'exercice précédent. Avec un agent, le geste attend une personne."#,
        example: r#"griffe year opening import bilan.csv --opens-on 2026-01-01"#,
    },
    Cmd {
        verb: r#"year opening from2033a"#,
        said: r#"Reprend le bilan d'ouverture depuis les cases du bilan simplifié. --box se répète : CASE=montant. Avec un agent, le geste attend une personne."#,
        example: r#"griffe year opening from2033a --opens-on 2026-01-01 --box 084=1200,00"#,
    },
    Cmd {
        verb: r#"year opening rm"#,
        said: r#"Supprime le bilan d'ouverture, tant qu'aucun exercice n'est clos. Avec un agent, le geste attend une personne."#,
        example: r#"griffe year opening rm"#,
    },
    Cmd {
        verb: r#"papers list"#,
        said: r#"Les pièces du coffre, hors celles qui ont été remplacées."#,
        example: r#"griffe papers list"#,
    },
    Cmd {
        verb: r#"papers show"#,
        said: r#"Une pièce. Le nom du fichier d'origine suffit souvent."#,
        example: r#"griffe papers show statuts.pdf"#,
    },
    Cmd {
        verb: r#"papers add"#,
        said: r#"Dépose un fichier, chiffré. --kind : statutes, kbis, tax_notice, client_contract, et les autres natures. --period est l'année civile de l'exercice."#,
        example: r#"griffe papers add statuts.pdf --kind statutes"#,
    },
    Cmd {
        verb: r#"papers rm"#,
        said: r#"Purge une pièce dont le délai est échu. Les statuts et l'extrait d'immatriculation ne se purgent pas tant que la société existe. Avec un agent, le geste attend une personne."#,
        example: r#"griffe papers rm ID"#,
    },
    Cmd {
        verb: r#"papers checklist"#,
        said: r#"Pour un exercice : les originaux figés, les pièces qui manquent."#,
        example: r#"griffe papers checklist 2026"#,
    },
    Cmd {
        verb: r#"papers export"#,
        said: r#"Écrit le dossier d'un contrôle, en clair, dans un dossier neuf. On n'écrase jamais."#,
        example: r#"griffe papers export 2026 --out controle"#,
    },
    Cmd {
        verb: r#"fec export"#,
        said: r#"Écrit le fichier des écritures de l'exercice clos dans cette année civile."#,
        example: r#"griffe fec export 2026 --out ecritures.txt"#,
    },
    Cmd {
        verb: r#"fec check"#,
        said: r#"Contrôle la structure d'un fichier d'écritures. Un fichier se relit sans coffre. --period le dérive du coffre, sans l'écrire. Ce n'est pas une attestation."#,
        example: r#"griffe fec check ecritures.txt"#,
    },
    Cmd {
        verb: r#"asset list"#,
        said: r#"Les immobilisations, avec la dotation de l'exercice. --period choisit l'exercice."#,
        example: r#"griffe asset list"#,
    },
    Cmd {
        verb: r#"asset show"#,
        said: r#"Une immobilisation et son plan."#,
        example: r#"griffe asset show ID"#,
    },
    Cmd {
        verb: r#"asset add"#,
        said: r#"Déclare une immobilisation. Le compte est un compte d'immobilisation, par exemple 218300 pour du matériel. La base est hors taxes. La durée est en mois, 36 si on ne la donne pas."#,
        example: r#"griffe asset add --label "Ordinateur" --account 218300 --acquired-on 2026-09-01 --base 1200,00"#,
    },
    Cmd {
        verb: r#"asset rm"#,
        said: r#"Supprime l'immobilisation. Refusé si un exercice clos a déjà porté une de ses dotations. Avec un agent, le geste attend une personne."#,
        example: r#"griffe asset rm ID"#,
    },
];

const GROUPS: &[(&str, &[Cmd])] = &[
    ("coffre", COFFRE),
    ("gens", GENS),
    ("courrier", COURRIER),
    ("argent", ARGENT),
    ("societe", SOCIETE),
];

pub(super) fn catalog(slug: &str) -> Markup {
    let Some(cmds) = GROUPS
        .iter()
        .find(|(name, _)| *name == slug.strip_prefix("cmd-").unwrap_or(""))
        .map(|(_, cmds)| *cmds)
    else {
        return html! {};
    };
    html! {
        div class="block aide-catalogue" {
            h3 { "Les gestes" }
            dl {
                @for cmd in cmds {
                    dt { code { "griffe " (cmd.verb) } }
                    dd {
                        (cmd.said)
                        code class="aide-exemple-ligne" { (cmd.example) }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalog_lists_each_leaf_once() {
        let mut seen = std::collections::BTreeSet::new();
        for &(_, cmds) in GROUPS {
            for cmd in cmds {
                assert!(seen.insert(cmd.verb), "doublon {}", cmd.verb);
                assert!(cmd.example.starts_with("griffe "), "{}", cmd.verb);
                assert!(
                    cmd.example.contains(cmd.verb),
                    "{} : {}",
                    cmd.verb,
                    cmd.example
                );
                let blob = format!("{} {}", cmd.said, cmd.example);
                let low = blob.to_lowercase();
                assert!(!low.contains("freeflow"), "{}", cmd.verb);
                assert!(!low.contains("tiime"), "{}", cmd.verb);
                assert!(!low.contains("nicolas"), "{}", cmd.verb);
                assert!(!low.contains("collier"), "{}", cmd.verb);
                assert!(!low.contains("ca3"), "{}", cmd.verb);
                assert!(!blob.contains("3514"), "{}", cmd.verb);
                assert!(!blob.contains("2777"), "{}", cmd.verb);
                assert!(!blob.contains("SASU"), "{}", cmd.verb);
                assert!(!blob.contains('@'), "{}", cmd.verb);
                assert!(!blob.contains("collection://"), "{}", cmd.verb);
            }
        }
        assert_eq!(seen.len(), 191);
    }
}
