# Brief — relances (lot 47, à concevoir)

Handoff pour une session neuve. Décisions déjà prises ; le design n’est pas encore figé.
Date : 2026-09-05. Dernier commit utile : `265db56` (lot 46, coque fenêtre).

## Contexte produit

FreeFlow : app locale chiffrée (SASU/EURL à l’IS). Thèse Studio : toute règle métier dans
`griffe-core` ; CLI, MCP et fenêtre sont trois façades équivalentes. **Aucune connexion
sortante.** Les mails ne partent jamais de l’app.

L’utilisateur facture pour l’instant via **Tiime / Indy** (réforme facturation électronique :
réception via PA dès sept. 2026, émission obligatoire pour une SASU en sept. 2027). L’émission
de facture dans FreeFlow est **reportée**. Les relances, elles, se font ici.

## Périmètre demandé

Les **deux** familles, dans cet ordre :

1. **Priorité — opportunités / prospects.** C’est le quotidien actuel. Pas encore de clients
   à relancer pour impayé.
2. **Ensuite — factures impayées** (cadences après échéance). Même tableau « où j’en suis »,
   même geste mail. Les factures peuvent venir de FreeFlow plus tard ; le modèle ne doit pas
   supposer que Tiime est éternel.

Succès : savoir **où on en est** dans la séquence (prévue / brouillon / envoyée / suivante),
cadences et modèles, ouverture d’un brouillon dans le client mail, sans que FreeFlow envoie.

## Ce qui existe déjà

- Opportunité : `next_action_at` obligatoire, `late_actions(conn, today)`, `LogInteraction`.
- Dashboard lot 46 : bloc « Aujourd’hui » (relances d’opportunité en retard, impayés, échéances).
- README / CLAUDE.md parlent de brouillons **`.eml`** ouverts dans le client par défaut.
  **Ce n’est pas implémenté** (aucun `.eml` / `mailto` dans le code). L’item feuille de route
  « Relances de paiement configurables au-delà des brouillons `.eml` actuels » est donc à
  prendre au pied de la lettre : d’abord les brouillons, puis les cadences.

## Contraintes d’architecture (ne pas casser)

- Pas d’envoi SMTP/IMAP/API mail depuis `griffe-core` ni les façades.
- Handoff mail = fichier `.eml` (RFC 5322) + IO d’adaptateur (`xdg-open`, ou commande
  configurable). Agnostique du client (aerc aujourd’hui ; Omamail / GUI possibles plus tard).
- Commandes via `Executor` ; lectures = queries. Confirmation humaine si un agent propose
  un envoi sensible (le brouillon lui-même n’est pas un envoi).
- `today` vient de l’adaptateur (`clock::today_local`), jamais du cœur.
- Trois façades dans le même lot, ou le lot n’est pas fini.

## Client mail (hors lot)

L’utilisateur est sous **Omarchy / Hyprland**, utilise **aerc 0.22**, le trouve un peu daté,
hésite vers **Omamail** (plugin Omarchy, clavier, Gmail/IMAP) ou une GUI, **pas Thunderbird**.
Décision : **ne pas coupler FreeFlow à un client**. Brancher `xdg-open`. Tester Omamail à part.

## Comment concevoir

Nouveau sous-système → brief, approches, design en sections, spec si le modèle d’état le
mérite, puis plan. Ne pas commencer par le CSS.

Pistes à challenger, pas à copier :

- Séquence d’étapes (J+0, J+n…) configurable, pas un booléen « en retard ».
- « Envoyée » = fait marqué par l’humain après envoi dans le client mail (FreeFlow ne lit
  pas la boîte).
- Une relance d’opportunité n’est pas un impayé : pas la même garde, pas le même modèle.
- L’écran doit dire la prochaine étape et l’historique, pas seulement une liste rouge.

## Hors périmètre

Émission de facture GUI, devenir plateforme agréée, envoi réel, lecture IMAP, i18n, refonte
visuelle complète (lot 46 = coque ; refonte toujours en feuille de route).
