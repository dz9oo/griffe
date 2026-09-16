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
| **La société** | Est-ce que ça tient, et que dois-je à l’État ? | « La maison », ni un écran Paramètres |

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
| Facturation (émission) | reportée (Tiime / PA) — une facture se *voit* sur le dossier |

Les slugs `/view/…` peuvent rester un temps (htmx, tests HTTP). La chrome n’en montre que trois.
Nouveaux chemins souhaités, à poser dès que ça ne casse pas les tests :

- `/jour` (accueil après déverrouillage, à la place de `/view/dashboard`)
- `/affaires`, `/affaires/{réf}` (`/dossiers` et `/gens` restent des alias)
- `/societe`, `/societe/payer`, `/societe/impots`, `/societe/cloture`, `/societe/releve`, `/societe/identite`

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

Un **geste** a un verbe : *Écrire à Camille*, *Ranger le relevé*, *Savoir pour la TVA*.
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
**Invariant produit : aucune connexion sortante.** Pas de Google Fonts dans la fenêtre.

À faire : vendoriser les woff2 (OFL) dans `crates/griffe-web/assets/fonts/`
et les déclarer en `@font-face`. Replis : `Liberation Serif` / `Noto Serif` et
`Adwaita Sans` / `Noto Sans`.

- Titres : serif, weight 400, letter-spacing −0.03em, `clamp(36px, 6vw, 58px)`
- Lede : serif 20px, ink-2, max 52ch (voix éditoriale, plus courte que le corps)
- UI : sans 15–16px
- Dates kicker : sans 11–12px, uppercase, letter-spacing 0.2em
- Marque : serif italic 22px, « Griffe »

Chiffres d’argent : sans, tabular si disponible. Pas de mono type console sur Le jour.

### Chrome et page

- Largeur lettre : **760 px**. La société (paysage) : **960 px**.
- Padding page ~40 px (20 px sous 700 px).
- Pas de rail d’audit visible par défaut.
- Bouton primaire : fond sceau, texte papier.
- Bouton secondaire : filet encre, pas de fond.
- Formulaires : pas de champs « settings ». Une ligne soulignée, un textarea bordé d’un filet.

### Densité

Le vide est un effet. Une journée claire se *compose* (« Rien aujourd’hui. ») ;
on ne remplit pas avec des graphiques. Inversement on ne cache pas un trou
(prospection, piste courte) : une phrase sceau suffit.

### Mesure

Le vide vit **autour de la feuille et entre les blocs**, pas à l’intérieur d’une
phrase. La colonne **est** la mesure.

- Colonne Jour / Gens : **760 px**. Colonne Société (paysage et chapitres) : **960 px**.
- Chrome et pied restent à 960 — c’est le bureau, pas une deuxième feuille plus
  large que la lettre du Jour.
- Corps, listes, montants, chemins, boutons : 100 % de la colonne. Pas de
  `max-width` en `ch` sous un titre de bloc.
- Lede ≤ 52ch. Alarme (`.mast-note`) ≤ 46ch.
- `.hist` = quand + quoi (deux enfants). Inventaire sans date = `.steps`
  (marque en `::before`, phrase en `1fr`). Interdit : un seul enfant dans une
  grille à deux pistes — CSS Grid le calerait dans la piste date (~136 px,
  deux ou trois mots par ligne).

## 6. Le jour

Ordre de la page, toujours :

1. **Date** (jour de la semaine réel — la maquette a corrigé mercredi → samedi 5 sept. 2026)
2. **Mât** — faits + alarme éventuelle
3. **Lettre** — titre en serif (« Trois gestes. ») + lede + liste numérotée
4. **Le mois** — grille, bandes de missions, agenda

### Mât

Toujours au-dessus de la lettre, jamais en bas, jamais en tuiles.

Trois faits, une ligne : **banque** · **piste en mois** · **à encaisser** (qui + montant).
Si un fait est alarmant (piste sous le seuil, à encaisser trop vieux), il passe sceau.

Sous les faits, **une phrase** seulement s’il y a un trou :

- piste courte
- aucune conversation derrière la relance du jour
- octobre vide de missions

Pas de phrase si tout va. Le calme se voit à l’absence d’italique rouge.

Seuil de piste à caler en vivant la maquette (proposition de départ : alarme sous **2 mois**,
note dès **3 mois** si le pipeline derrière est vide). Cœur : une query compose le mât,
elle ne décide pas du texte français mot à mot — la fenêtre rédige à partir de faits typés.

### Gestes

File dérivée de ce qui existe déjà, fusionnée, **un verbe par ligne** :

- relances dues (`follow_up_queue`)
- relevé non lu (`unmatched_debits` + crédits / règlements)
- prochaine obligation d’État dans l’horizon court (TVA, pas « CA3 »)

Trois à cinq, pas vingt. Le reste vit dans le mois. Déplier un geste (pas un panneau
latéral) : les boutons de la chose (ouvrir le dossier, le brouillon, plus tard).

Le `.eml` ne change pas (lot 47). Griffe n’envoie pas.

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

## 7. Les affaires

Une liste, trois chapitres : **en conversation** (prospects), **en mission** (clients
avec projet), **fournisseurs**. Un bouton « Nouvelle conversation » = nom + une phrase.
Pas de fiche client obligatoire pour commencer (lot 45).

Le **dossier** est l’unité. Chapitres selon ce qui existe :

- En cours (la phrase du moment)
- Le projet (mission, jalons, jours)
- Les papiers (devis, factures)
- Histoire (interactions, relances)

Actions : Écrire, Le devis, Noter une rencontre, Reporter.
On n’« ouvre pas un devis » depuis un onglet Devis : on ouvre Camille.

Résolution de référence inchangée (nom, préfixe, UUID).

## 8. La société

Page d’entrée = identité courte + **paysage de trésorerie** (la ligne d’Horizon,
encre sur papier, pointillés = si l’encaissement prévu arrive) + **conversations
de l’année** (12 barres, le mois courant en sceau s’il est maigre) + chapitres :

| Chapitre | Contenu |
|---|---|
| Te payer | Salaire (fiche chez l’expert-paie) vs dividendes (après clôture). Un montant possible ce mois-ci **sans casser la piste**. Objectif annuel = une barre, pas un simulateur. |
| Ce que tu dois à l’État | Dates + montants. Chaque ligne ouvre une **lettre** (chemin sur le site, montant visible, ce que l’écran demandera) avant de partir. On prépare, on ne transmet pas. |
| Clore l’exercice | Le parcours (lot 34) en phrases, pas seize étapes techniques d’un coup. |
| Le relevé | Chaque mouvement = une phrase, trois lectures : dépense, règlement d’une dette, *c’est moi que je me paie*. |
| L’identité | Carte (nom, forme, siège, président, SIREN, clôture, régime de TVA), pas un formulaire de 20 champs. Le coffre se dit ici. |

« Te payer » et le paysage demandent des **chiffres déjà dans le domaine**
(prévisionnel, rémunération du dirigeant, affectation) présentés autrement.
La *recommandation* « 2 800 € ce mois-ci gardent 3 mois de piste » est une
query nouvelle, pure, à tests chiffrés à la main — pas un calcul dans la fenêtre.

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

1. Le jour : mât visible sans scroller, trois gestes, septembre sous la lettre.
2. Clic 16 → agenda TVA ; clic la ligne → impôts en français.
3. Les affaires → Camille → lettre `.eml` (le texte dit qu’on n’envoie pas).
4. La société → paysage, Te payer (dividende fermé), relevé Leroy = dette.

Si un futur changement casse ce parcours, ce n’est plus Atelier.
