# Nouveautés

Ce qui change pour qui se sert de Griffe. Une section par version publiée.

## 0.3.8

- La case des 12 h tient la fenêtre ouverte pendant ces 12 h. Rouvrir Griffe dans ce délai retrouve la passphrase. « Verrouiller » ferme tout de suite. La case vide laisse le quart d'heure d'inactivité.
- Un numéro français s'affiche par paires, 03 27 44 44 44. Le saisir compact, avec des points, ou avec l'indicatif +33 ou 0033, donne la même forme. Un numéro d'un autre pays reste tel que saisi.

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
