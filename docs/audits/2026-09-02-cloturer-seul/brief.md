# Brief commun — audit « prêt à l'emploi » de FreeFlow

Dépôt : /path/to/griffe (lire CLAUDE.md et README.md en premier).
Binaires DÉJÀ construits (ne JAMAIS lancer cargo build/test/clippy/nix : machine ancienne, un seul
processus lourd à la fois, et les binaires sont à jour du dernier commit) :
  CLI      : /path/to/griffe/target/debug/freeflow
  Web dev  : /path/to/griffe/target/debug/freeflow-web-dev  (FREEFLOW_DB=<coffre> FREEFLOW_WEB_ADDR=127.0.0.1:<port>)
  MCP      : /path/to/griffe/target/debug/freeflow-mcp
Dossier de travail : /tmp/audit-scratch
  - passphrase : pass.txt (mode 0600) → `freeflow --db <ton-coffre>.db --passphrase-file pass.txt ...`
  - crée TON PROPRE coffre (ex. ec.db / freelance.db / rust.db), ne touche pas à ceux des autres.
  - n'utilise jamais --remember (trousseau OS réel de la machine).
  - outils utiles déjà installés : pdftotext (poppler) pour lire les PDF produits ; curl pour la GUI.

## Le scénario à rejouer (utilisateur cible)
Un indépendant (développeur) en SASU à l'IS installe FreeFlow pour la PREMIÈRE fois, coffre vide,
rien de configuré. Aujourd'hui : 2 septembre 2026.
- Il a son bilan comptable 2025 (produit par un cabinet, données venant d'un autre logiciel :
  Tiime, Indy…). Clôture d'exercice au 30 septembre. Exercice en cours : 01/10/2025 → 30/09/2026.
- Il n'a facturé AUCUN client depuis le dernier bilan.
- Il n'a payé que des factures courantes : frais de compte bancaire pro, honoraires du cabinet.
- Il veut saisir toutes les données nécessaires au bon fonctionnement de FreeFlow.
- Il veut produire son bilan au 30/09/2026 et TOUS les documents/démarches : PV d'AG, dépôt des
  comptes au greffe, liasse fiscale, déclaration(s) de TVA, IS, etc.
- Il n'a AUCUNE notion comptable ni fiscale (vrai débutant) : il doit être guidé DANS l'application,
  informé de ce dont FreeFlow a besoin, sans documentation externe.
- Il doit pouvoir reprendre les données/documents venant d'un autre logiciel (Tiime, Indy…).

## Ce qu'on attend de toi
Rejouer réellement le scénario (pas seulement lire le code), depuis le premier lancement, dans le
rôle qui t'est donné. Noter chaque friction, manque, erreur, bug, incohérence légale/fiscale.
Rapport FINAL en français, écrit dans <dossier de travail>/rapport-<rôle>.md, structure :
1. Verdict en 3 lignes (prêt / pas prêt pour ce scénario, et pourquoi).
2. Constats, chacun avec : sévérité (BLOQUANT / MAJEUR / MINEUR / SUGGESTION), titre, où (commande,
   écran, fichier:ligne), reproduction exacte, ce qui était attendu, preuve (sortie observée).
3. Ce qui fonctionne bien (court).
4. Recommandations priorisées (quoi construire en premier).
Sois concret et vérifiable : cite les sorties observées. Ne corrige RIEN dans le dépôt (audit en
lecture seule ; les seuls fichiers écrits sont dans le dossier de travail). Ta réponse finale doit
contenir le chemin du rapport et un résumé de 10 lignes.
