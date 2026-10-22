# Atelier — orientation figée

Figé le 5 septembre 2026. Maquette de référence : [`mockups/atelier-app.html`](mockups/atelier-app.html).
Les deux pistes d’origine (`horizon.html`, `atelier.html`) restent en archive ; **Atelier-app
est la source**. Ce document est ce qu’on applique à la fenêtre lundi. Le cœur
(`Command` / `Query`) ne change de contrat que là où c’est dit plus bas.

À vivre encore : copy, densité du mois, le seuil où la piste « alarme ». Pas la structure.

---

## 1. Thèse

Un indépendant en SASU (puis EURL) n’ouvre pas une compta. Il se demande
**quoi faire maintenant**, **si l’argent tiendra**, **qui relancer**, **s’il peut se payer**.

Griffe est une **lettre du matin**, pas un dashboard. Trois pièces, pas dix onglets.
Les documents n’ont pas d’écran : les **personnes** oui. La société porte l’argent,
se payer, clore, l’identité.

## 2. Ce qu’on refuse

Tiime / Indy / Pennylane, et l’ancienne coque « Studio » (lot 9, encore sous le lot 46) :

- menu à gauche (ou bandeau) d’un item par module
- quatre tuiles de KPI (CA, charges, TVA, trésorerie)
- « Bienvenue 👋 », illustrations, blobs, Inter, teal SaaS
- sigles du quotidien (CA3, 3514, 2777, DSN) — ils existent dans le cœur, pas dans la lettre
- un écran « Factures », un écran « Devis », un écran « Missions » comme bases de documents
- un tableau de bord qui rassure en silence alors que la piste est courte ou le pipeline vide

La console, le journal d’audit, les slugs d’URL peuvent rester. Ils ne sont plus la chrome.

## 3. Trois pièces

| Pièce | Question | Ce qu’elle n’est pas |
|---|---|---|
| **Le jour** | Quoi faire, et où en est le mois ? | Un dashboard |
| **Les affaires** | Avec qui j’en suis ? | Un CRM + un module devis + un module missions |
| **La société** | Est-ce que ça tient, et que dois-je à l’État ? | « La maison ». Les réglages sont une des quatre cartes du plateau |

Chrome : `Griffe` (italique, serif) à gauche · les trois mots à droite.
La pièce active = un soulignement encre, pas un onglet teinté.

Pas d’autre item de nav. Console : `⌘K` / `Ctrl+K`, ou une route non liée.
L’aide n’est pas une pièce : `?` (pied **Aide**, palette) ouvre la lettre `/aide`.
Journal d’audit : replié, éventuellement depuis l’identité / le coffre.

### Correspondance avec l’app actuelle

| Aujourd’hui (`ViewId`) | Atelier |
|---|---|
| Dashboard, Relances | **Le jour** (lettre + mois) |
| Prospection, Clients, Devis, Missions, Facturation (fiche) | **Les affaires** (un dossier) |
| Dépenses, banque (rapprochement) | **Le relevé** (chapitre de La société, geste du Jour) |
| Société, Clôture, calendrier fiscal | **La société** |
| Console | hors nav |
| Facturation (émission) | hors fenêtre. Une facture née dans la PA se *pose* sur le dossier (lot 64). |

Les slugs `/view/…` peuvent rester un temps (htmx, tests HTTP). La chrome n’en montre que trois.
Nouveaux chemins souhaités, à poser dès que ça ne casse pas les tests :

- `/jour` (accueil après déverrouillage, à la place de `/view/dashboard`)
- `/affaires`, `/affaires/{réf}` (`/dossiers` et `/gens` restent des alias)
- `/societe`, `/societe/piste`, `/societe/payer`, `/societe/impots`, `/societe/cloture`, `/societe/releve`, `/societe/identite`

## 4. Langage

On parle comme on parlerait à voix haute.

| Ne pas écrire | Écrire |
|---|---|
| CA3 / 3514 / CA12 | TVA du mois / du trimestre, acompte de TVA, TVA de l’année |
| Impayé, aged balance | 6 200 € chez Atlas |
| Pipeline pondéré | Une seule conversation derrière Camille |
| Trésorerie prévisionnelle | Piste (en mois) |
| Rapprochement | Ranger le relevé |
| Écriture, OD, 401 | C’est le règlement d’une dette / une dépense / *C’est pour moi* / *C’est moi qui apporte* |
| Salaire vs dividendes (jargon) | Te payer — de l’argent sur ton compte, sans casser la société |

Un **geste** a un verbe : *Écrire à Camille*, *Ranger le relevé*, *Savoir pour la TVA*, *Ne plus attendre chez N.*.
Un **dossier** a un nom de personne, pas un type d’entité.
Une **alarme** est une phrase en italique couleur sceau, pas un badge.

## 5. Graphisme

Papier, encre, un sceau. Pas un thème sombre recyclé.

### Couleur

```
--paper:      #f1ebe0
--paper-2:    #e8e0d2
--ink:        #1c1814
--ink-2:      #6a635a
--rule:       #d0c6b4
--seal:       #9f3218
--seal-soft:  #f0d8cf
```

Le sceau n’est pas une « couleur d’accent SaaS ». Il marque : le geste, l’alarme,
aujourd’hui sur le calendrier, le bouton qui fait la chose. L’encre fait le reste.

Pas d’ombres portées. Des filets 1 px. Fond = papier, éventuellement un dégradé radial
très faible vers le haut (`#f7f1e6` → paper).

Thème sombre : **hors v1 Atelier**. Le `html.dark` du lot 46 ne se traduit pas
en inversant ces variables.

### Type

Maquette : Newsreader (titres, chiffres du calendrier, italiques) + Source Sans 3 (UI).
**Pas de webfont distante.** Pas de Google Fonts dans la fenêtre.

Les woff2 (OFL) sont dans `crates/griffe-web/assets/fonts/`
et déclarés en `@font-face`. Replis : `Liberation Serif` / `Noto Serif` et
`Adwaita Sans` / `Noto Sans`.

- Titres : serif, weight 400, letter-spacing −0.03em, `clamp(36px, 6vw, 58px)`
- Lede : serif 20px, ink-2, max 52ch (voix éditoriale, plus courte que le corps). Sur un chapitre de La société, la phrase d’entrée est en sans 15 px, en encre.
- UI : sans 15–16px
- Dates kicker : sans 11–12px, uppercase, letter-spacing 0.2em
- Marque : serif italic 22px, « Griffe »

Chiffres d’argent en ligne : sans, tabular si disponible. Le grand fait d’une famille et le montant de Te payer sont en serif. Pas de mono type console sur Le jour.

### Chrome et page

- La chrome est la même sur Le jour, Les affaires, La société, l’aide et la console : le wordmark (nom et paraphe), les trois pièces au centre, la date du jour à droite. La date est `letter_date` (jour de semaine réel). Elle vit hors du fragment htmx, donc une navigation ne la redessine pas. Une pièce ne prend pas le cadre bleu du navigateur. Au clavier, elle reprend le même filet d’encre que la pièce ouverte.
- La date du jour n’est pas répétée en tête de lettre. Une date qui est le sujet (échéance, histoire, mois) reste dans la page.
- Chrome et pied prennent toute la largeur, avec le même retrait, quelle que soit la pièce.
- Le jour, Les affaires, La société et l’aide prennent toute la largeur. Le sommaire de l’aide et la lettre se partagent le bureau. Les autres lettres courtes restent à **760 px**.
- Padding des autres lettres ~40 px (20 px sous 700 px).
- Pas de rail d’audit visible par défaut.
- Bouton primaire : fond sceau, texte papier.
- Bouton secondaire : filet encre, pas de fond.
- Formulaires : pas de champs « settings ». Une ligne soulignée, un textarea bordé d’un filet.

### Densité

Le vide est un effet. Une journée claire se *compose* (« Rien n’est dû ce matin. ») ;
on ne remplit pas avec des graphiques. Inversement on ne cache pas un trou
(prospection, piste courte) : une phrase sceau suffit.

### Mesure

Le vide vit **autour de la feuille et entre les blocs**, pas à l’intérieur d’une
phrase. La colonne **est** la mesure.

- Le jour, Les affaires, La société et l’aide : toute la largeur. L’aide pose un sommaire (environ le quart) et la lettre dans le reste. Sous environ 900 px, le sommaire passe au-dessus et les deux colonnes de la lettre s’empilent. Les autres lettres courtes : **760 px**. Dans Avec un agent, l’introduction explique la commande et le MCP, puis La commande liste chaque geste avec un exemple, sous la lettre, sur toute la largeur de la feuille.
- Chrome et pied suivent la pièce : pleine largeur sur Le jour, Les affaires, La société et l’aide.
- La société est le plateau. L’accueil et chaque chapitre prennent toute la largeur, sur le bureau `#e6dfd2`. Quatre cartes : Tenir `#fbf7f0`, Te payer `#f4ead6`, L’État `#f6e4dc`, Les réglages `#efe6d6`. Les lignes sont du corps (sans, 15 px), des cartes crème posées sur la teinte. Le survol enfonce la ligne dans sa famille (`#f3ece0`, `#ead9bc`, `#f0d2c6`, `#e6d9c4`). Une URL de chapitre replie les cartes : « Revenir au menu », le fil, les pastilles des voisins, le chapitre dans `.work`. La piste est `/societe/piste`.
- Corps, listes, montants, chemins, boutons : 100 % de la colonne. Pas de
  `max-width` en `ch` sous un titre de bloc.
- Lede ≤ 52ch. Alarme (`.mast-note`) ≤ 46ch.
- `.hist` = quand + quoi (deux enfants). Inventaire sans date = `.steps`
  (marque en `::before`, phrase en `1fr`). Interdit : un seul enfant dans une
  grille à deux pistes — CSS Grid le calerait dans la piste date (~136 px,
  deux ou trois mots par ligne).

## 6. Le jour

Ordre de la page, toujours :

1. **Carte de statut** — la phrase du matin, puis l'alarme éventuelle. La date du jour reste dans la chrome.
2. **Gestes** — cinq lignes au plus, la langue de la lentille
3. **Le mois** — grille, bandes de missions, agenda

Le bureau a la largeur de Les affaires. La carte des affaires compte les noms de la vue.

### Mât

Toujours au-dessus des gestes, jamais en bas, jamais en tuiles.

Une phrase : **Ce matin, trois gestes — banque · piste en mois · à encaisser** (qui + montant).
S'il n'y a aucun geste : les trois faits, puis « Rien n'est dû ce matin. »
Si un fait est alarmant (piste sous le seuil, à encaisser trop vieux), il passe sceau.

Sous les faits, **une phrase** seulement, le premier de ces trous :

- la piste sous deux mois
- aucune conversation ouverte
- une seule conversation ouverte
- une créance à encaisser depuis trente jours

Pas de phrase si tout va. Le calme se voit à l’absence d’italique sceau.

L’alarme de piste est sous **2 mois**. Le cœur compose les faits. La fenêtre rédige le français. Un mois vide de missions se dit sur le mois.

### Gestes

File dérivée de ce qui existe déjà, fusionnée, **un verbe par ligne** :

- relances dues (`follow_up_queue`)
- créance qu’on n’attend plus (à côté de la relance, jamais à sa place)
- relevé non lu (`unmatched_debits` + crédits / règlements)
- prochaine obligation d’État dans l’horizon court (TVA, pas « CA3 »)

Trois à cinq, pas vingt. Chaque ligne a quatre cellules : le verbe, le qui, le prochain pas, le montant. Le tas froid n'est pas nommé : un lien « et N autres » ouvre Les affaires. Une mission sans geste du matin reste sur sa bande. Un prestataire n'a une ligne que s'il a une dette d'ouverture ou un débit qui correspond. Le reste vit dans le mois. Déplier un geste (pas un panneau
latéral) : les boutons de la chose (ouvrir le dossier, le brouillon, plus tard).

Le brouillon `.eml` des relances reste. L’envoi part du dossier, ou depuis Le courrier.

### Le mois

Pas un widget de calendrier SaaS.

- Grille du mois civil, lundi → dimanche, typo serif
- Aujourd’hui : chiffre sceau, filet inférieur
- Jour porteur : une marque (point sceau = relance / jalon ; tiret encre = rencontre ;
  carré encre = obligation d’État / fin d’exercice)
- **Bandes** sous la grille : une par mission active, playhead = aujourd’hui,
  tick = jalon. On *voit* si l’activité avance, et le vide après la dernière fin.
- **Agenda** du mois à partir d’aujourd’hui : date, nature en français, phrase.
  Clic jour ↔ surbrillance agenda. Clic ligne → la pièce (dossier, impôts, clôture).
- Phrase sceau si, après la dernière mission du mois, plus rien n’est sur le papier.

Sources du mois (à agréger **dans le cœur**, une query) :

- `next_action_at` / file de relances
- `due_on` des factures non soldées
- jalons et `ended_on` des missions
- échéances fiscales (déjà dans `fiscal_calendar`)
- rencontres posées (interactions datées à venir, si on en saisit)

Pas de nouveau module « calendrier » dans la nav.

### Cartes de Le jour

La date du jour reste dans la chrome. Elle ne revient pas en tête de page.

La phrase du matin est la carte de statut déjà dessinée sur le plateau :
`article.plateau-status` et `p.plateau-sentence`, fond `#fbf7f0`. Le texte
est en encre. Un fait alarmant reste en sceau (`b.bad`, et `.mast-note`
dans la carte). Ce n'est pas une carte de chapitre. Ce ne sont pas les
teintes des quatre familles. Le bureau de la page est `#e6dfd2`, le même
que le plateau, pour que cette carte se lise comme elle est dessinée.
« Rien n'est dû ce matin. » reste le lede sous la carte quand aucun geste
n'est dû. Les notes du courrier restent sous la carte.

Les gestes restent sur la page, sous la carte. Quatre cellules, cinq lignes
au plus. Le dépli est `.unfold`, sur place : une action, pas un chapitre.
Il n'y a pas de second dépli, et pas de panneau latéral. Ouvrir la
destination d'un geste charge cette destination. Un lien vers La société
arrive sur le chapitre replié. Une lettre `/societe/impots/{kind}` reste
une lettre. Le lien « et N autres » ouvre Les affaires.

Le mois reste sous les gestes. La grille, les bandes et l'agenda tiennent
sur une surface `--paper`, rayon 16 px. L'encre, le filet et le sceau sont
les tokens communs. Le mois ne devient pas un tableau de bord : pas une
tuile par chiffre. Le défilement de la page reste permis.

Les queries `day_mast`, gestes et mois ne changent pas. Aucun montant
nouveau. Les boutons de mutation restent dans leur formulaire.

## 7. Les affaires

La lentille est le menu : ce qui demande un geste, le livre, le tas, les sorties,
les arrêtés, le type, la recherche, « Nouvelle conversation ». « Les types » ouvre
le catalogue. La phrase (« Montre … — N noms ») est la carte de statut
`article.plateau-status`, fond `#fbf7f0`, le même composant que Le jour. Le texte
est en encre. Les lignes sont des cartes sur le bureau `#e6dfd2`, fond `#fbf7f0`.
Pas de famille colorée. `#f4ead6`, `#f6e4dc`, `#efe6d6` et les teintes de lecture
`#f8f1e2`, `#f8ebe4`, `#f3ecdf` ne s'appliquent pas aux personnes. Les pastilles
de type restent celles du coffre.

Choisir un nom replie la liste. Le dossier prend la largeur. « Revenir au menu »
restaure la liste, avec la vue, la recherche et le type dans l'URL. Le volet
côte à côte n'est plus là.

Pastilles, dans l'ordre du dossier : En cours, Écrire, La fiche, L'estimation,
Les travaux, Noter une rencontre. Reporter, Arrêter et C'est un client restent
des actions dans En cours. Le projet, les papiers, les notes et l'histoire
restent le corps d'En cours. « Revenir au menu » rentre vers la liste. La
pastille En cours rentre au dossier. Le lien « ← nom » sous les pastilles
n'est plus là. Arrêter, C'est un client et Ils reviennent gardent le leur :
ces pages n'ont pas la barre.

Écrire s'ouvre sur la lettre telle qu'elle part. « Éditer » ouvre le texte,
« Relire » y revient. « Envoyer » part quand Le courrier est branché.
« C'est parti » n'est là que sans serveur. Après ce geste, En cours dit que
la lettre a été envoyée hors de Griffe, et lie Le courrier.

`/affaires/types` et la nouvelle conversation utilisent la même barre. Les routes
`/affaires/{référence}/fiche`, `/estimation`, `/ecrire`, `/travaux`, `/rencontre`
restent. Les anciens `/view/prospection`, `/view/devis`, `/view/missions`,
`/view/facturation`, `/view/clients` restent joignables par ⌘K. On ne les redessine pas.

Une nouvelle conversation est un nom et une phrase. Pas de fiche obligatoire pour
commencer. L'estimation est le sujet et le montant gardés dans le coffre. Griffe
ne rédige pas le devis. On ouvre un nom. La résolution de référence est inchangée
(nom, préfixe, UUID).

« Aucun nom. » quand la recherche ne trouve personne. « Aucune conversation de ce
type. » quand le type filtre le livre.

## 8. La société

Page d’entrée = le plateau. Une phrase (banque, piste, prochaine échéance), quatre
cartes toujours visibles. L’alarme de piste est en sceau. Les destinations sont
des cartes crème posées sur la teinte de la famille.

| Famille | Grand fait | Lignes |
|---|---|---|
| Tenir | la banque | La piste, Le relevé. Sous le fait : les mois de piste. L’encaissement prévu (partie · montant) est une mention, pas un lien. |
| Te payer | le montant du mois | Ce mois-ci, Le compte, L’année |
| L’État | la date courte de la prochaine échéance (« 24 sept. ») | Les échéances, Clore. Le nom de l’échéance reste dans la phrase et dans le fait. |
| Les réglages | La maison | La carte, Le courrier, Les phrases, Les papiers |

La piste a sa propre adresse, `/societe/piste`. Elle montre le **paysage de
trésorerie** (encre sur papier, pointillés = si l’encaissement prévu arrive).

Une URL de chapitre replie le menu. « Revenir au menu » ramène à `/societe`.
Le fil nomme la famille et le titre. Les voisins sont des pastilles ; la pastille
courante est en encre. Échap, depuis `app.js`, ramène au menu. Le chapitre garde
son marquage dans `.work`. Te payer est peint en cartes. L’État peint Les
échéances et Clore. Tenir peint La piste et Le relevé. Les réglages peignent
La carte, Le courrier, Les phrases et Les papiers. Les lettres d’échéance
gardent le marquage. Les teintes de lecture restent `#fbf7f0`,
`#f8f1e2`, `#f8ebe4`, `#f3ecdf`.

Voisins : Tenir = La piste et Le relevé ; Te payer = Ce mois-ci, Le compte et
L’année ; L’État = Les échéances et Clore ; Les réglages = La carte, Le courrier,
Les phrases et Les papiers.

Une lettre `/societe/impots/{kind}` a pour parent « Les échéances » (lien vers
`/societe/impots`) et pour voisins « L’État » et « Clore ». « Revenir au menu »
rentre à `/societe`.

Les phrases portent la famille Les réglages et le soulignement La société,
y compris ouvertes depuis une lettre ou une fiche (`depuis=lettre`,
`depuis=fiche`, ou un `pour`).

### Cartes de Te payer

Le fil, « Revenir au menu » et les pastilles restent. La teinte de lecture des
cartes est `#f8f1e2`. Le texte est en encre. Les queries ne changent pas : aucun
montant nouveau, la porte dividende suit le coffre.

La grille remplit l’écran du chapitre. Au-dessus de 1 100 px, Ce mois-ci tient
en trois colonnes et Le compte en deux (le solde, puis les définitions
empilées). Sous 1 100 px, deux colonnes. Sous 700 px, une colonne. Le
défilement de `.work` reste permis : les cartes suivent leur texte, elles ne
sont pas coupées.

**Ce mois-ci** (`/societe/payer`). La phrase d’entrée reste au-dessus. Trois
cartes, puis la barre de l’année sur toute la largeur :

- le montant possible, avec la phrase déjà écrite (jusqu’où se verser en gardant
  la piste, ou rien ce mois-ci) ;
- « Comme un salaire », porte ouverte tant que les dividendes sont fermés ;
- « Plus tard, en dividendes », porte fermée avec la phrase déjà écrite
  (exercice non clos, ou sans bénéfice). Quand la query ouvre les dividendes,
  cette carte passe ouverte ;
- la barre « Cette année, pour vivre » : objectif, versé, mètre. Sans objectif,
  la barre n’est pas dessinée.

« Le prochain virement, depuis le relevé » reste un lien sous la grille. Le
solde du compte courant vit sur Le compte.

**Le compte** (`/societe/compte`). Carte du solde : la phrase déjà écrite (la
société te doit, tu dois, ou rien entre toi et la société). « Repris du cabinet »
reste sur cette carte quand l’ouverture n’est pas nulle. Trois cartes de
définition, phrases déjà écrites : « avancés de ta poche », « pris sur le compte
de la société », « mis sur le compte de la société ». Le montant de la
définition s’écrit quand il n’est pas nul. Les mouvements restent une liste
sous la grille (Au départ, Avancé, Pris, Apporté).

**Cette année** (`/societe/annee`). Deux faits, versé et objectif, puis la même
barre. Sans objectif : « Aucun montant visé. », sans barre.

Aucun formulaire sur ces trois chapitres.

### Cartes de L’État

Le fil, « Revenir au menu » et les pastilles restent. La teinte de menu reste
`#f6e4dc`. La teinte de lecture des cartes est `#f8ebe4`. Le texte est en
encre. Les queries et les POST ne changent pas.

Le fil porte « Les échéances » et « Clore ». Le `h1` du chapitre sort. Le lien
« ← La société » reste, le pli le masque.

**Les échéances** (`/societe/impots`). La phrase d’entrée reste au-dessus :
« Pas des sigles. Des dates, des montants, et où le déposer. On prépare. On ne
transmet rien. » Une carte par groupe qui a des lignes :

- « Avant ce coffre » ;
- « En retard » ;
- « À venir ».

La liste du groupe reste une liste dans la carte : la date, la phrase déjà
écrite, le montant déjà écrit. Le formulaire « Tout était à jour à l’échéance »
reste un formulaire, le bouton dans le form, dans la carte « Avant ce coffre ».
« Déjà déposé » reste le repliable sous la grille. « Aucune échéance dans
l’horizon. » et « Rien à déposer pour l’instant. » restent des phrases.

La grille vise un écran. Autant de colonnes que de cartes présentes, trois au
plus. Une seule carte occupe une colonne, sans piste vide. Sous 1 100 px, deux
colonnes dès qu’il y en a deux. Sous 700 px, une. Les cartes suivent leur texte
(`minmax(min-content, …)`). Le défilement de `.work` est permis.

**Clore** (`/societe/cloture`). Une carte pour la phrase d’exercice déjà écrite.
Le parcours reste la liste `.hist` : fait, aujourd’hui, ensuite. « Arrêter les
comptes » reste le bouton qui ouvre `/cloture/new` dans `#panel`. « Ranger le
relevé » reste le lien sous la liste quand des mouvements ne sont pas rangés.

Une lettre `/societe/impots/{kind}` reste une lettre. Le parent est « Les
échéances », les voisins « L’État » et « Clore ». Les blocs, les formulaires et
les POST restent.

Classes `.etat-*`. Ces deux adresses restent sans CA3, 3514, 2777.

### Cartes de Tenir

Le fil, « Revenir au menu » et les pastilles restent. La teinte de menu reste
`#fbf7f0`. La teinte de lecture des cartes est `#fbf7f0`. Le texte est en
encre. Les cartes se posent sur le bureau `#e6dfd2`. Les queries et les POST
ne changent pas.

La piste tient sur une carte, pleine largeur. Le relevé tient sa carte
au-dessus de la liste, pleine largeur. Sous 1 100 px et sous 700 px, la carte
reste seule : pas de grille à deux ou trois colonnes. Les cartes suivent leur
texte. Le défilement de `.work` reste permis.

**La piste** (`/societe/piste`). Le volet n’a pas de `h1` et n’a pas le lien
« ← La société ». Le fil porte « La piste ». Une carte pour le paysage déjà
dessiné. `figure.landscape` reste une figure dans la carte : le SVG s’il y a
au moins deux points, la légende déjà écrite (mois abrégé, « aujourd’hui »,
« · clôture dans {n} jours » s’il y a un délai, « · si ça arrive » si un
encaissement est prévu). La phrase déjà écrite (`landscape_prose`) reste une
phrase, dans la carte. Sans figure et sans phrase, aucune carte vide. Pas une
carte par point de la courbe. Le pointillé reste dans le SVG. Le SVG garde
160 px de haut et 100 % de large. Aucun formulaire, aucun bouton.

**Le relevé** (`/societe/releve`). Le `h1` « Le relevé. » sort : le fil porte
« Le relevé ». Le lien « ← La société » reste, le pli le masque. La lettre
garde `letter` et `spread`. La racine qui se rafraîchit sur `griffe:saved`
reste cette lettre : `hx-get` vers elle-même,
`hx-trigger="griffe:saved from:body"`, `hx-swap="outerHTML"`,
`hx-disinherit="hx-swap"`. Le fragment HX-Request garde le retour en
`hx-swap="innerHTML"`, et il ne redessine pas « Revenir au menu ». Une carte
pour la phrase d’entrée déjà écrite : « Dernier mouvement au {date}. » quand
il y en a un, puis « Tu déposes l’export de la banque quand il y a du nouveau.
Griffe ne va pas le chercher. » « Déposer un export » reste le bouton
`type="button"` qui ouvre `/banque/import` dans `#panel`, dans cette carte.
La recherche reste le formulaire `form.releve-search` (GET vers
`#releve-body`, le champ et le délai déjà écrits), pas une carte. La phrase de
compteur reste une phrase (« Aucun mouvement dans le coffre. », « Tout est
traité. … », « {n} mouvements ne sont pas encore traités, sur {total}. »).
« Voir les non traités » et « Voir tout l’historique » restent des liens.
Chaque mouvement reste un `article.block` : pastille « non traité » ou
« traité », date, montant déjà écrit, phrase déjà écrite, lectures déjà
écrites. Pas une carte par mouvement. « Aucun mouvement ne correspond. » et
« Les mouvements plus anciens » restent où ils sont. Les lectures ne bougent
pas : formulaire dont le bouton est dans le form quand c’est un POST vers
`#panel` (« C’est le règlement d’une dette » si la dette est suggérée,
« C’est pour moi. » ou « C’est moi qui apporte. »), lien vers `#panel` pour
« C’est une dépense » et pour la dette quand c’est déjà un lien.

Classes `.tenir-*`. Ces deux adresses restent sans CA3, 3514, 2777.

### Cartes des réglages

Le fil, « Revenir au menu » et les pastilles restent. La teinte de menu reste
`#efe6d6`. La teinte de lecture des cartes est `#f3ecdf`. Le texte est en
encre. Les cartes se posent sur le bureau `#e6dfd2`. Les queries et les POST
ne changent pas.

La carte, les phrases et les papiers posent leurs cartes l’une sous l’autre,
pleine largeur. Pas de grille à deux ou trois colonnes pour ces cartes. Sous
1 100 px et sous 700 px, les cartes empilées restent une colonne. Elles
suivent leur texte. Le défilement de `.work` reste permis. Le courrier garde
`courrier-desk` et prend la largeur du bureau. Les phrases gardent
`phrase-desk`. Les papiers gardent `letter` et `#papiers-letter`.

**La carte** (`/societe/identite`). Le fil porte « La carte ». Le `h1`
« L’identité. » sort : le lede dit déjà « La carte de la société, pas un
formulaire de paramètres. » Le lien « ← La société » reste, le pli le masque.
La racine reste `div.letter` dans le bureau. Deux cartes. La première porte
`ul.people`, une liste dans la carte : Nom, Forme avec le capital quand il
n’est pas nul, Siège s’il est là, Président s’il est là, SIREN s’il est là,
Clôture, TVA. Une ligne absente le reste. Pas une carte par ligne. La seconde
porte « Le coffre » : les deux phrases déjà écrites, et les liens déjà écrits
vers Le courrier et vers les phrases. « Modifier la carte » reste le lien
quiet vers `/view/societe`, sous les cartes. Pas un formulaire, pas une carte.
Aucun formulaire inventé.

**Le courrier** (`/societe/courrier`). Le `h1` « Le courrier. » sort. Le lien
« ← La société » reste, le pli le masque. La racine garde `letter` et
`courrier-desk`. Le lede déjà écrit reste une phrase au-dessus (pas encore
branché, prêt envoi du jour, ou prêt à la main). « Trop de lettres cette
heure. Celles qui restent attendent. » reste où elle est. Quatre cartes.
`courrier-board` les range : la lecture à gauche, dans 42 rem au plus, et les
trois autres à droite. Dès 1 280 px de bureau, la formule et l’envoi du jour
partagent une ligne, et « D’où elles partent » prend la largeur sous elles.
En dessous, la lecture reste à côté d’une colonne. Sous 880 px, une seule
colonne. La première est la lecture : l’amorce « Une lettre d’essai. Le
prénom est fictif. », `#letter-card`, la feuille, puis `form.courrier-send`.
`mail-pane` reste la figure de la lettre. L’iframe `.letter-frame` garde son
bloc vide. Les radios de la feuille restent sur l’input. `form.courrier-send`
reste le formulaire du champ « Vers » et de « Envoyer l’essai », le bouton
dans le form. La deuxième carte est `form.courrier-formula` : « Ce qui ferme
la lettre », l’aide déjà écrite, le textarea dont l’aperçu part en delay
600 ms vers `#letter-card` en innerHTML sans pousser l’URL, et le bouton
« Enregistrer » dans le form. La troisième est « D’où elles partent » : la
ligne de compte déjà écrite, `details.courrier-sheet`, `form.serveur` dont le
bouton « Enregistrer et essayer la liaison » est dans le form, et, quand un
mot de passe est au coffre, le formulaire « Retirer le mot de passe » dont le
bouton est dans le form. Le champ mot de passe reste `type="password"`. La
page ne le réécrit pas. La grille des champs du serveur reste la grille des
champs (une colonne, deux colonnes dès 640 px dans le conteneur courrier),
pas une grille de cartes. La quatrième est « L’envoi du jour » : les deux
phrases déjà écrites, et le formulaire dont le bouton est « Activer l’envoi
du jour » ou « Couper l’envoi du jour », dans le form.

**Les phrases** (`/affaires/phrases?depuis=societe`). L’habillage du bureau
reste : `frame_phrases`, famille Les réglages, soulignement La société.
`?depuis=lettre`, `?depuis=fiche` et un `pour` non vide ouvrent ce même pli.
Le retour vers la lettre est `/{dossier}/ecrire`, et `depuis` comme `pour`
restent sur les liens. Le fil porte « Les phrases ». Le `h1` « Les phrases. »
sort. Le mot est « série ». La ligne, sous la légende « Série », ne nomme que
les séries, puis « Une autre ». La série ouverte est en encre, avec le même
filet qu’un mot coché, et les autres ne sont pas soulignés. Sans série, la
carte dit « Aucune série. » et n’affiche pas les mots de départ. Si ces mots
ont déjà été réécrits, une ligne demande un nom. Le titre dans la carte est
le nom, « Services publics. ». Le lede : « Les mots de cette série. »
Une carte pour l’entrée déjà écrite : le lede, la chronique, « Les
conversations déjà engagées finissent leur série. » Un seul formulaire
ensuite, pas une carte par moment. La frise et la feuille gardent la grille
`phrase-stage` (frise | feuille, une colonne sous 800 px). Les boutons
Monter, Descendre, Retirer, Ajouter un moment, Enregistrer les phrases et
Retirer cette série restent dans ce form. Les jetons restent des boutons
`type="button"`. L’aperçu reste `#phrase-sheet`, remplacé en innerHTML,
delay 600 ms. La feuille reste une figure dans le formulaire. La page
« Une autre série. » pose le titre et le lede dans la carte quand le pli est
là, et le formulaire en dehors. Sur Écrire, pour une conversation, la ligne tient sur une rangée : Série,
puis le nom posé. Le nom ouvre le même menu que Montre, sous le nom, sans
pousser la lettre. Garder referme le menu et laisse la série. Sans série
posée, et s’il en existe, la ligne dit Choisir une série. Fermer referme
sans en poser. Sans série dans le coffre, la ligne dit Aucune série. et
Une autre ouvre la création. Une facture n’a pas ce choix. Au retour d’une
lettre, la série créée est posée sur le dossier.

**Les papiers** (`/societe/papiers`). Le `h1` « Les papiers. » sort. Le lien
« ← La société » reste, le pli le masque. La racine qui se rafraîchit sur
`griffe:saved` reste `#papiers-letter`. Le fragment dont la cible est
`papiers-letter` ne redessine pas le pli et ne contient pas « Revenir au
menu ». Le mât (exercice, « vu le ») reste le mât, au-dessus des cartes. La
phrase d’entrée déjà écrite reste au-dessus. Deux cartes de listes. « Ce que
Griffe écrit. » : la phrase déjà écrite (« Le PV et l’affectation… » ou « Le
PV, le FEC, les factures… ») et le lien « Lesquelles ? », puis `ol.papers`.
« Ce que vous apportez. » : `ol.papers`, ou « Rien à apporter pour cet
exercice. », puis `details` « une autre pièce » dont le formulaire (le
select, « Déposer ») reste dans le `details`. Chaque pièce reste un `li` : le
bouton `type="button"` qui déplie, le formulaire de dépôt dans le déplié
quand la pièce est à apporter, les liens « Ouvrir … », les phrases déjà
écrites. Pas une carte par pièce. « Deux endroits. » garde la grille
`.places` (deux colonnes, le filet, une colonne sous 700 px). « Au coffre »
et « Pour un contrôle » sont les deux cartes de cette grille. Le chemin du
coffre reste le chemin. Le formulaire « Préparer le dossier d’un contrôle »
reste dans « Pour un contrôle », le bouton dans le form, le résultat dans
`#pack-result`.

Ce qui reste une figure : `mail-pane` dans la carte de lecture du courrier,
la feuille `#phrase-sheet` dans le formulaire des phrases. Ce qui reste une
liste : `ul.people` dans La carte, `ol.papers` dans chaque carte de liste,
chaque pièce un `li`. Ce qui reste un formulaire : l’envoi d’essai, la
formule, le serveur, le retrait du mot de passe, l’envoi du jour, le
formulaire unique des phrases, le dépôt d’une pièce, le dépôt « une autre
pièce », la préparation du dossier d’un contrôle. La carte n’a pas de
formulaire.

Classes `.reglages-*`. Ces quatre adresses restent sans CA3, 3514, 2777. Le
SIREN déjà écrit reste.

| Chapitre | Contenu |
|---|---|
| Te payer | Salaire (fiche chez l’expert-paie) vs dividendes (après clôture). Un montant possible ce mois-ci **sans casser la piste**. Objectif annuel = une barre, pas un simulateur. |
| Ce que tu dois à l’État | Dates + montants. Chaque ligne ouvre une **lettre** (chemin sur le site, montant visible, ce que l’écran demandera) avant de partir. On prépare, on ne transmet pas. |
| Clore l’exercice | Le parcours (lot 34) en phrases, pas seize étapes techniques d’un coup. |
| Le relevé | Chaque mouvement = une phrase, trois lectures : dépense, règlement d’une dette, *c’est moi que je me paie*. |
| La carte | Carte (nom, forme, siège, président, SIREN, clôture, régime de TVA). Le coffre se dit ici. |

« Te payer » et le paysage montrent des **chiffres déjà dans le domaine**
(prévisionnel, rémunération du dirigeant, affectation).
Le montant qui garde 3 mois de piste vient de la query du cœur,
tenue par le test chiffré « 2 800 € ». La fenêtre affiche ce résultat.

## 9. Cœur vs fenêtre

Thèse Studio : **aucune règle métier dans `griffe-web`.**

Queries nouvelles (cœur), consommées par les trois façades ensuite :

1. **Mât** — banque, piste en mois, à encaisser (liste courte), signaux
   (pipeline derrière vide, piste sous seuil).
2. **Gestes du jour** — fusion relances + relevé + prochaine obligation d’État.
3. **Mois** — événements et bandes de missions pour un `Month`.
4. **Se payer** — montant possible sous contrainte de piste, portes salaire /
   dividende ouvertes ou fermées (exercice non clos → dividende fermé).

CLI / MCP : les mêmes lectures, texte ou JSON. Pas seulement la fenêtre.

IO inchangée : `.eml` et justificatifs = adaptateur ; `today` = adaptateur.

## 10. Lots pour l’appliquer

Chaque lot : `just check` + `nix flake check`, entrée dans `CLAUDE.md`.
Ne pas tout fondre lundi.

**Lot 48 — Coque Atelier.** Tokens papier, polices vendorisées, chrome à trois
pièces, accueil = Le jour (même si le contenu est encore un assemblage des
écrans actuels). Tests HTTP : titres, slugs, pas de `freeflow ` dans les vues.
Le thème sombre lot 46 se retire de la chrome (préférence locale ignorée, ou
conservée sans CSS). Critère : on reconnaît le papier en ouvrant la fenêtre,
sans avoir encore le mois ni Les affaires.

**Lot 49 — Le jour.** Mât + gestes (queries 1–2) + mois (query 3). Calendrier
civil correct. Alarmes rédigées. Relances actuelles deviennent des gestes,
pas un écran à part.

**Lot 50 — Les affaires** (alors « Les gens »). Liste unique + dossier (opportunité, mission, devis,
factures, histoire). « Nouvelle conversation ». Les anciens onglets
Prospection / Devis / Missions / Clients / Facturation sortent de la nav ;
leurs routes redirigent ou rendent le dossier.

**Lot 51 — La société.** Paysage (série du prévisionnel), chapitres, impôts en
français, relevé à trois lectures, identité, clôture racontée. Query 4 si
« Te payer » est mûr ; sinon le chapitre pointe le profil dirigeant existant
et dit clairement ce qui n’est pas encore calculé.

Hors de ces lots, volontairement : émission de facture dans la fenêtre,
thème sombre Atelier, i18n, console en nav, EDI.

## 11. Fichiers (quand on code)

Fenêtre, pas une refonte du domaine d’abord :

- `crates/griffe-web/assets/app.css` — tokens, chrome, lettre, mois
- `crates/griffe-web/assets/fonts/` — woff2 vendorisés
- `crates/griffe-web/src/layout.rs` — trois pièces, plus PRIMARY/MORE à 10 onglets
- `crates/griffe-web/src/views/` — `jour`, `gens`, `societe` ; les vues actuelles
  se vident ou redirigent au fil des lots 49–51
- `crates/griffe-web/tests/http_routes.rs` — titres français, mât, trois liens de nav

Cœur, lots 49–51 :

- queries nouvelles à côté de `follow_up`, `forecast`, `fiscal`, `closing`
- pas de nouvelle `Command` tant qu’on ne fait que relire et ranger (les commandes
  de relance, rapprochement, clôture existent)

## 12. Recette de la maquette

Ouvrir `docs/mockups/atelier-app.html`. Parcours minimum avant de coder un lot :

1. Le jour (`docs/mockups/jour-pupitre.html`) : phrase du matin, quatre cellules, septembre sous les gestes.
2. Clic 16 → agenda TVA ; clic la ligne → impôts en français.
3. Les affaires → Camille → lettre `.eml` (le texte dit qu’on n’envoie pas).
4. La société → paysage, Te payer (dividende fermé), relevé Leroy = dette.

Si un futur changement casse ce parcours, ce n’est plus Atelier.

## 13. La boîte

La boîte n’est pas une quatrième pièce. Les trois questions restent. Dans la
chrome, entre les pièces et la date, le mot **Boîte**. Un compte en sceau
n’apparaît que si une réponse de dossier, ou une adresse qui ne reçoit pas,
demande un regard. Rien quand la boîte est calme. Le courrier, chapitre de
La société, reste le réglage du compte. On ne le renomme pas.

La page prend le bureau (`#e6dfd2`). Trois colonnes dès que la fenêtre le
permet : les vues (À lire, Avec les dossiers, Envoyées, Tout, En échec), les
lettres en cartes `#fbf7f0`, puis la lettre. Les dossiers du serveur
(INBOX, Sent, Brouillons, Archive) restent fermés. Une phrase de statut ouvre
la page, même carte que le matin. Pendant la lecture, cette phrase le dit.
Le grand paraphe ne paraît que si la boîte est encore vide. Une relève
suivante ne recouvre pas la lettre ouverte. L'échec, et le compte qui ne
se lit pas, s'écrivent. Au repos, le sondage s'arrête.

Sur le dossier, pas de sixième pastille. Le fil est le haut d’En cours, avant
le projet et l’histoire. Les pastilles restent : En cours, Écrire, La fiche,
L’estimation, Les travaux, Noter une rencontre. Répondre suit la lettre reçue.
Écrire prépare encore la lettre de la série. Les deux ne se confondent pas.

Une lettre partie par Griffe, et la réponse qu’on écrit, reprennent la feuille
et la formule de Le courrier (Clair, Sauge, ou le fichier lisible). Le courrier
reçu n’est pas repeint. Ces réglages ne se refont pas dans la boîte.

Quand la personne répond, le fil le montre et la prochaine relance automatique
de ce dossier reste là : « Camille a répondu. La relance ne part pas. »
« Laisser partir la relance » la rend de nouveau. Répondre soi-même est un
envoi, avec les cinq secondes, et ne réarme pas la série.

Un avis de non-remise certain dit que l’adresse ne reçoit pas. Griffe n’efface
pas l’adresse. On la corrige, ou on confirme « Elle est bonne ». Un indice
seulement par l’objet reste probable, jusqu’à ce geste. Si plusieurs fiches
portent la même adresse, Griffe propose le choix.
