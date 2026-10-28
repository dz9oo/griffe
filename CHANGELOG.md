# Nouveautés

Ce qui change pour qui se sert de Griffe. Une section par version publiée.

## 0.3.20

- La formule du courrier se pose une fois. Une ligne qui n'est que `<signature>` disparaît à sa place. Une formule déjà écrite dans la lettre reste seule.
- Un brouillon dont les paragraphes se suivent sans blanc en reçoit un. Les lignes vides déjà là restent. Une série de phrases déjà écrite reste telle quelle.
- Le coffre garde la même forme.

## 0.3.19

- Un seul fichier installé ouvre la fenêtre, la commande et l'agent.
- `griffe` est la ligne de commande. `griffe desktop` ouvre la fenêtre. `griffe mcp` parle à un agent. Le menu lance `griffe desktop`. Un double-clic sur l'AppImage ouvre la fenêtre.
- Rejouer l'installation réécrit la fiche du menu. Une fiche ancienne ouvre l'aide : elle n'a pas le mot desktop.
- Le paquet pose `/usr/bin/griffe` et `/usr/bin/griffe-desktop` vers ce même fichier. Le menu lance `griffe-desktop`.
- Le coffre garde la même forme.

## 0.3.18

- Une lettre reçue s'ouvre avec sa mise en page. Les scripts n'y sont pas. Le texte brut, quand il est là, reste sous « Le texte ». Sans mise en page utilisable, ce texte est la lettre.
- Les images qui viennent d'un autre serveur ne se chargent pas. Dans Le courrier, la carte « Les images distantes » peut les afficher. L'expéditeur peut alors savoir que la lettre a été ouverte. Les images jointes à la lettre restent.
- Ouvrir un coffre déjà créé avance son schéma. La version précédente ne peut plus l'ouvrir.

## 0.3.17

- Courriel se souligne dans la barre. Tant qu'aucune lettre n'est là, le grand paraphe tient la page. Il s'efface dès qu'une lettre arrive.
- La page dit si la relève est en cours, l'heure de la dernière qui a abouti, ou qu'elle n'a pas encore eu lieu. Elle dit aussi la période. « Relever le courrier » en demande une. Sans adresse choisie, la phrase ouvre Le courrier.
- Dans Le courrier, on choisit ce qui est lu. Rien n'est lu tant que le choix n'est pas gardé. Quelques adresses, reçues et envoyées, ou tout le compte. Une lettre d'une autre adresse quitte le coffre à la relève. Elle n'est pas effacée sur le serveur.
- La relève se règle à une minute, trois, quinze ou trente. Trois minutes, si on ne choisit pas. Elle tourne pendant que le coffre est ouvert.
- Sur le dossier, les lettres reçues et les lettres parties sont dans l'histoire, une ligne qu'on ouvre.
- Les messages et les commandes portent le nom Griffe. Une session de douze heures déjà ouverte est reprise.
- Ouvrir un coffre déjà créé avance son schéma. La version précédente ne peut plus l'ouvrir.

## 0.3.16

- La boîte est dans le bandeau, entre les trois pièces et la date. Un compte apparaît quand une réponse de dossier n'a pas encore été ouverte, ou quand une adresse ne reçoit pas.
- Elle lit le compte déjà réglé dans Le courrier. À lire, Avec les dossiers, Envoyées, Tout, En échec. Ouvrir une lettre la marque lue. Ranger sur un dossier la pose sur la conversation.
- Sur le dossier, le fil tient le haut d'En cours. Quand la personne a répondu, la phrase le dit et la relance ne part pas. Laisser partir la relance la rend de nouveau.
- Répondre suit la lettre reçue. Elle part dans cinq secondes. Annuler la retient. Cette réponse ne relance pas la série.
- Quand une adresse ne reçoit pas, l'envoi vers elle est retenu. On la corrige, ou on confirme qu'elle est bonne.
- Le courrier reçu se lit tel qu'il est arrivé.
- Ouvrir un coffre déjà créé avance son schéma. La version précédente ne peut plus l'ouvrir.

## 0.3.15

- Écrire s'ouvre sur la lettre telle qu'elle part. Éditer reprend le texte. Relire y revient. C'est parti n'est là que lorsqu'aucun serveur n'est configuré : il classe le double et n'envoie rien. En cours dit alors que cela suppose un envoi hors de Griffe, et ouvre Le courrier.
- Une lettre en partance quitte les gestes du matin.
- Sur la lettre, la série ne montre que son nom. Le nom ouvre les autres. Garder referme sans changer.
- L'écart entre deux moments d'une série compte les jours du lundi au vendredi. Un courrier parti un vendredi, à trois jours, se rappelle le mercredi. Une facture, un report, ou une date posée à la main, restent en jours du calendrier.
- Un constat vide disparaît de la lettre, et la ligne qui ne porte que lui aussi.
- La commande pose les lignes de l'estimation.
- L'aide prend le bureau. Elle documente la commande, l'agent, et chaque geste.
- Le bandeau ne trace plus un cadre bleu autour de la pièce ouverte quand on touche une touche. Au clavier, la pièce reprend son filet d'encre.
- Ouvrir un coffre déjà créé avance son schéma. La version précédente ne peut plus l'ouvrir.

## 0.3.14

- Les phrases se rangent en séries. On les nomme dans La société, Les phrases. La ligne montre ces noms, puis « Une autre ». La série ouverte est en encre, et la carte porte son nom. Sans série encore, la page dit « Aucune série. » Des mots déjà écrits sans nom demandent un nom : il les garde.
- En écrivant, on choisit la série. Ses mots remplissent la lettre. Une correction faite à la main reste. Un brouillon déjà prêt garde son texte. Une lettre déjà classée garde son texte. Le moment de la conversation reste le même.
- Le type du dossier et la série se règlent chacun de leur côté. Ouvrir cette version aligne les noms une fois : un type sans série en reçoit une, une série sans type en reçoit un. Un dossier qui avait un type et pas encore de série prend celle du premier type, par le nom.
- Une adresse suivie de ?utm_… se lit sans cette partie. Le lien la garde. Un navigateur peut la retirer en ouvrant la page.
- Sur Le courrier, le fichier `lettre.toml` apparaît à côté de Clair et de Sauge, marqué votre feuille. Lisible, on peut le choisir. Illisible, la ligne le dit, et la lettre reste sur Clair ou sur Sauge.
- Ouvrir un coffre déjà créé avance son schéma. La version précédente ne peut plus l'ouvrir.

## 0.3.13

- Le bandeau est le même sur Le jour, Les affaires et La société : le nom, les trois pièces, la date du jour à droite.
- La société s'ouvre sur quatre cartes. Ouvrir un chapitre replie ce menu. « Revenir au menu » le ramène. Le fil nomme la famille et le chapitre. Les voisins sont des pastilles.
- Te payer se lit en cartes : ce qu'on peut se verser ce mois-ci, le salaire, les dividendes, et la barre de l'année. Le compte garde le solde et ses trois définitions.
- L'État se lit en cartes : avant ce coffre, en retard, à venir. Une échéance ouvre sa lettre. Clore garde son parcours.
- Tenir se lit en cartes. La piste tient le paysage. Le relevé garde ses phrases, et chaque mouvement reste une ligne.
- Les réglages se lisent en cartes : La carte, Le courrier, Les phrases, Les papiers. Un formulaire reste un formulaire.
- Le jour pose la phrase du matin sur une carte. Les gestes restent dessous. Le mois suit : la grille, les bandes, l'agenda.
- Dans Les affaires, la phrase tient sur la même carte. Choisir un nom replie la liste, et le dossier prend la largeur. « Revenir au menu » ramène la liste, avec la vue et la recherche.
- Le courrier range ses cartes sur la largeur du bureau : la lettre, ce qui la ferme, d'où elles partent, l'envoi du jour.
- La lettre se peint en Clair, un ivoire, ou en Sauge, un vert pâle. Le choix est sous la lettre d'essai. Changer de feuille repeint la page qui part. Une troisième feuille peut venir d'un fichier sur la machine : `lettre.toml`, dans le dossier `griffe` de la configuration. Un fichier illisible laisse Clair. Les phrases ouvertes depuis une affaire restent dans le dossier.
- Le coffre garde la même forme.

## 0.3.12

- Sur La société, chaque ligne se lit en corps de texte, à côté de son montant. La piste, Le relevé, Clore et les autres ouvrent leur chapitre sous les quatre colonnes.
- Le courrier, Les papiers et Les phrases s'ouvrent dans ce chapitre. Les colonnes restent en place.
- Le coffre garde la même forme.

## 0.3.11

- Les affaires nomment les arrêtés : les conversations qu'on a arrêtées. Ce qui demande un geste compte aussi un prochain pas déjà dû, une facture à régler, un débit qui correspond, un brouillon prêt.
- Les types se portent en pastilles. On leur choisit une teinte, et si on veut une note sur ce qu'on y range. Sur la fiche, seuls les types qu'elle porte sont là. Le « + » ouvre ceux du coffre. La liste en montre deux, puis le reste.
- La fiche sépare le standard et qui répond. Courriel et téléphone du haut sont ceux de l'accueil. Sous Qui répond : le prénom, le nom, Monsieur ou Madame, et leurs coordonnées. Quand les deux adresses sont là, la lettre se choisit : Le standard, ou Qui répond.
- Les phrases suivent le nom et le genre. Une formule écrite des deux côtés n'en garde qu'un quand le genre est connu. Sans genre, les deux restent visibles.
- Le jour ouvre sur une phrase : les gestes de ce matin, l'argent en banque, les mois de piste, ce qu'il reste à encaisser. Chaque geste se lit en colonnes : Geste, Qui, Prochain pas, Montant.
- La société s'ouvre sur quatre colonnes : Tenir, Te payer, L'État, Les réglages. La phrase du haut dit la banque, la piste, et la prochaine échéance. La ligne choisie ouvre son détail en dessous.
- Le nom affiché reste celui de l'expéditeur. Il n'ouvre plus la lettre.
- L'aide se lit en lettres. Se repérer, Parler aux gens, Tenir l'argent, Rendre des comptes. Chaque situation a la sienne, et la palette les retrouve. Premiers pas reste la porte d'entrée. Dans le lexique, le produit s'appelle Griffe.
- Ouvrir un coffre déjà créé avance son schéma. La version précédente ne peut plus l'ouvrir.

## 0.3.10

- Les affaires s'ouvrent sur ce qui demande un geste. On peut montrer le livre, le tas, les sorties, ou tout. Ouvrir ces mots en dit le sens. Le livre réunit les conversations en cours et les missions. Le tas compte les premiers messages et les premiers contacts, sans les étaler. Les sorties nomment ceux qu'une dépense porte.
- Un nom ouvre le dossier à côté de la liste. Pleine largeur le pose sur toute la pièce. Fermer le retire, Échap aussi.
- Chercher un nom ne garde que la vue affichée.
- Sur le dossier, Les travaux se lisent en rendu. Écrire ouvre le markdown, qui se garde seul. Relire revient au rendu. Plusieurs allers et retours laissent la page répondre : on peut revenir en arrière et changer de pièce.
- Une lettre part en page, et le texte seul reste pour les boîtes qui ne lisent pas la page. Une adresse suivie de ?utm_… reste cliquable en entier. La personne lit l'adresse sans cette partie.
- Dans Les affaires, Écrire et Relire se succèdent. Relire montre la lettre telle qu'elle part. La zone d'écriture garde le texte.
- Le courrier montre la lettre, et la formule à côté. D'où elles partent tient sur une ligne une fois le serveur connu. L'essai est cette lettre. L'envoi du jour reste à part.
- La formule ferme la lettre à la relecture et à l'envoi. Elle n'est pas recopiée dans la zone d'écriture. Un mail, un numéro ou une adresse, seuls sur leur ligne, restent cliquables, dans l'encre de la lettre, sans soulignement. Une lettre déjà classée garde son texte.
- Annuler, pendant les cinq secondes et jusqu'à l'envoi, retient la lettre. Passé l'envoi, la ligne dit qu'elle est en route.
- Ouvrir un coffre déjà créé avance son schéma. La version précédente ne peut plus l'ouvrir.

## 0.3.9

- Le courrier réunit le serveur d'envoi sur un seul panneau. L'adresse, l'identifiant, l'hôte et le port restent affichés. Enregistrer essaie la liaison : une coche si le serveur accepte l'identifiant, une croix s'il la refuse. Le mot de passe reste dans le coffre. Une lettre d'essai part vers l'adresse choisie.
- L'envoi compte les cinq secondes, une à une. Au bout, Annuler s'efface, la ligne dit Courrier envoyé, et le bouton d'envoi revient. Même geste pour l'essai et pour une lettre.
- Sur le dossier, cocher un type l'enregistre. Le bouton Enregistrer sous Types n'y est plus.
- Ouvrir un coffre déjà créé avance son schéma. La version précédente ne peut plus l'ouvrir.

## 0.3.8

- Le courrier, dans La société, règle le serveur qui poste les lettres. Le mot de passe reste dans le coffre.
- La formule ferme les lettres et les phrases. On l'écrit dans Le courrier. Vide, la lettre se termine par Bien à vous, le nom, la société. Une lettre d'essai reprend le premier message, pour un prénom fictif, vers l'adresse choisie, avec cinq secondes pour la retenir. Le serveur qui l'accepte ne promet pas qu'elle arrive hors des indésirables.
- Sur une lettre, Envoyer laisse cinq secondes. Le filet s'allonge, Annuler le retient. Passé ce délai, la lettre part. C'est parti classe toujours le double dans l'historique.
- L'envoi du jour est éteint. L'activer poste, pendant que Griffe est ouvert, les lettres dont la date prévue est aujourd'hui. Une lettre de la veille reste un geste à la main.
- Dans les affaires et sur le jour, la fiche porte le nom du Qui. Écrire à et Relancer suivent ce nom. Qui répond reste sur la fiche, et la recherche le trouve encore. Deux fiches du même interlocuteur restent deux noms. Quand un nom en désigne plusieurs, on choisit laquelle.
- Un numéro français s'affiche par paires, 03 27 44 44 44. Le saisir compact, avec des points, ou avec l'indicatif +33 ou 0033, donne la même forme. Un numéro d'un autre pays reste tel que saisi.
- La case des 12 h tient la fenêtre ouverte pendant ces 12 h. Rouvrir Griffe dans ce délai retrouve la passphrase. « Verrouiller » ferme tout de suite. La case vide laisse le quart d'heure d'inactivité.
- Sur le dossier, Reporter se déplie. On choisit 3 jours, 10 jours, ou un nombre de jours, de 1 à 366. Demain, sur le jour, ne change pas.
- Sur la fiche, le genre se choisit en mots. « aucun » le retire. « un autre » ouvre une ligne pour le nommer. Sans genre encore, le premier nom sert aux phrases.
- Sur le dossier, Les travaux gardent un récit en markdown, à côté de l'estimation. L'aperçu suit la frappe sans enregistrer. Garder pose le texte dans le coffre.
- Sur le dossier, les types de travaux se choisissent en mots. Les affaires se filtrent par type. La page Les types crée, renomme et retire. Retirer demande de confirmer.
- Ouvrir un coffre déjà créé avance son schéma. La version précédente ne peut plus l'ouvrir.

## 0.3.7

- Les écritures restent dans le coffre. Une correction en ajoute une qui inverse la première. L'originale reste.
- Une facture d'achat non payée n'entre pas. Le paiement par la banque écrit la charge au jour du relevé. Un paiement depuis un compte personnel l'écrit sur le compte de l'associé.
- Une facture de prestation écrit la créance, le produit et la TVA en attente. L'encaissement déplace la créance vers la banque, et la part de TVA vers la TVA devenue exigible.
- Un avoir non encaissé inverse la créance, le produit et la TVA en attente. Annuler un encaissement ramène cette TVA vers la TVA en attente.
- Depuis la société, « Écrire la TVA du mois » pose l'écriture une seule fois, le dernier jour du mois. La lettre et les cases à recopier lisent la même chose. Un mois déjà déposé, un mois non terminé, ou un exercice clos, refuse ce geste.
- Une prestation reçue d'un autre pays de l'Union européenne écrit, au paiement, la charge et une paire de TVA, due et déductible. Le mois solde cette paire au centime.
- Une rémunération n'entre dans le livre que lorsqu'elle est saisie. Un montant seulement déclaré au profil n'écrit rien. La fenêtre n'a pas encore ce formulaire. Il n'y a pas de bulletin, pas de fichier de déclaration sociale, pas de prélèvement à la source.
- La clôture lit le résultat sur les charges et les produits du livre. Une dépense non payée n'y entre pas. Une rémunération non saisie n'y entre pas. L'impôt sur les sociétés est écrit au moment de la clôture. Retirer la clôture inverse cette écriture d'impôt.
- Les cases à recopier se lisent sur le livre. Le montant à recopier est à l'euro. Le montant du livre, au centime, reste à côté. Deux feuillets, celui de la valeur ajoutée et celui des filiales, portent la mention néant.
- L'amortissement, la perte sur une créance que l'on n'attend plus, le report en arrière et les lignes d'affectation sont encore calculés à la lecture.
- À l'ouverture, les paiements de dépenses et les factures de prestation déjà enregistrés sont repris dans le livre. L'écriture de TVA du mois attend le geste qui l'écrit.
- Ouvrir un coffre déjà utilisé avec cette version le fait avancer. La version précédente ne peut plus l'ouvrir.
- L'AppImage, l'archive et le paquet pacman portent une signature. On la vérifie avant d'installer.

## 0.3.6

- Les phrases se choisissent sur une frise. On ouvre un moment, la lettre se lit à côté. L'écart se compte en jours. On monte, on descend, on ajoute, on retire un moment.
- Le prénom se prend dans Qui répond. Une enseigne ne prête pas son premier mot. Sans prénom, la lettre dit Bonjour,.
- Dans les affaires, un nom qui a déjà un corps reste écrit : brouillon prêt, estimation posée, estimation envoyée, en échange, repris. Les premiers messages et les premiers contacts se replient, avec leur nombre, et se retrouvent par le nom.
- Le jour écrit ces noms-là. Le reste tient en une ligne, « et N autres » ou « N autres conversations », qui ouvre les affaires. Les gestes du jour et l'agenda du mois restent sur les conversations qui ont un corps.
- Le compte des noms, sur le jour comme dans les affaires, continue de les compter tous.
- Le coffre garde la même forme.

## 0.3.5

- Les phrases que tu répètes sont dans le coffre, depuis L'identité. Les réécrire change la prochaine lettre. Une lettre déjà classée, ou un brouillon déjà préparé, garde son texte.
- Le nombre de moments et les écarts se règlent. Une conversation déjà engagée finit sa série. Une conversation neuve ou reprise prend la série du moment.
- Le genre, sur la fiche, ne change que les mots. Deux dossiers du même rang, le même jour, n'ont pas la même lettre.
- Garder ces mots depuis Écrire remplace ce moment pour ce genre, et n'envoie rien.
- La cadence des factures n'a pas bougé.

## 0.3.4

- La fenêtre affiche son numéro, à côté de l'état du coffre, et sous la carte quand le coffre est verrouillé ou à créer.
- Au 1er octobre, la TVA de septembre reste à déclarer. Elle ne disparaît pas parce que l'exercice suivant a commencé.
- Tant que l'exercice fini n'est pas arrêté, le jour le rappelle et la société le dit. On arrête les comptes depuis ce parcours. Une fois l'exercice clos, le rappel disparaît.

## 0.3.3

- Le jour place les conversations et les gestes côte à côte quand la fenêtre est assez large, et les empile sinon.
- À côté des mois de piste, le mât indique le nombre de prospects et de clients.
- Le titre des affaires compte les prospects et les clients, pas les noms chez qui l'argent sort.
- Une conversation dit où elle en est : premier message, premier contact, en échange, estimation posée, brouillon, repris.
- On peut arrêter une conversation. Elle quitte le jour et reste dans Arrêtées, repliée, avec le nombre de dossiers visible. On peut la rouvrir : les lettres et l'estimation restent.
- Arrêter une conversation se distingue des autres gestes du dossier.
- L'estimation est une liste de travaux. Le total est la somme de ces lignes.
- Le relevé garde chaque mouvement et la lecture qui lui a été donnée.
- Chaque mouvement est marqué traité ou non traité. Le relevé dit combien restent en attente, sur le total.
- Un long historique se charge par pages. On cherche un mouvement par son libellé ou son montant, et on peut n'afficher que les non traités.
- La case du coffre promet le prochain lancement : ne pas redemander la passphrase pendant 12 h. La fenêtre ouverte se referme toujours après un quart d'heure sans s'en servir.

## 0.3.2

- Le dossier d'une personne garde la fiche, l'estimation, et la nature d'une rencontre : téléphone, visioconférence, e-mail, rencontre physique ou note.

## 0.3.1

- Griffe s'installe sur Linux : AppImage, paquet pacman `griffe-bin`, ou archive à déposer dans le dossier personnel.
- Le coffre est dans `~/.local/share/griffe/`. Un coffre déjà présent sous `freeflow` y est déplacé au premier lancement.

## 0.3.0

- Premier paquet pacman public : Griffe s'installe avec pacman.
