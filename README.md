# FreeFlow

Application desktop de gestion pour un indépendant en SASU/EURL à l'IS : prospection, missions,
devis, facturation Factur-X, dépenses, échéances fiscales indicatives et prévisionnel de
trésorerie. **Local-only, chiffrée, sans serveur** — tes données ne quittent jamais ta machine
sauf si tu choisis explicitement de les synchroniser (Syncthing, iCloud Drive...).

## Pourquoi ce projet

La plupart des outils de facturation pour indépendants sont des SaaS : tes données de
prospection, tes marges, tes factures vivent chez un tiers. FreeFlow fait le pari inverse — un
seul fichier chiffré (SQLCipher) sur ton disque, aucune connexion sortante, et une architecture
« Studio » où la CLI, le serveur MCP (pour piloter l'app depuis un agent LLM) et la fenêtre
desktop sont trois façades strictement équivalentes au-dessus du même cœur applicatif. Tout ce
que fait la GUI, la CLI peut le faire à l'identique — et donc un agent aussi, avec les mêmes
garde-fous (audit, confirmation humaine avant tout effet sensible).

Le détail de l'architecture (pourquoi pas de serveur, comment le protocole `freeflow://` marche,
la doctrine de test) est dans [`CLAUDE.md`](./CLAUDE.md) — ce README est orienté usage, pas
implémentation.

## Fonctionnalités actuelles

### Clients
- Créer, consulter, **modifier**, **archiver** et **supprimer** un client, et gérer ses contacts
  (ajout, modification, suppression) — en CLI (`freeflow client …`), en MCP (`clients.*`) et
  depuis la fenêtre (écran `clients`, panneau latéral).
- Un client ne se supprime pour de bon que s'il n'est référencé par aucune opportunité, devis,
  mission ou facture ; sinon il s'**archive** (retiré des listes actives, ses références passées
  restent valides).
- Désignation par UUID, préfixe d'UUID, ou **nom** (insensible à la casse et aux accents) en CLI
  et en MCP — pas besoin de copier un identifiant complet pour agir sur un client.
- `freeflow client edit` ne change que les champs fournis ; `--clear-siren`, `--clear-vat-number`
  et `--clear-address` (et `--clear-email`/`--clear-phone`/`--clear-role` sur un contact)
  effacent un champ optionnel sans en fournir un nouveau.
- Garde-fou contre l'écriture concurrente : chaque modification porte la révision lue au
  préalable ; une modification concurrente est détectée plutôt qu'écrasée silencieusement.

### Prospection
- Opportunités avec étape, montant, probabilité, source d'acquisition, motif de perte structuré.
- Date de prochaine action **obligatoire** sur chaque opportunité ouverte — impossible d'en perdre
  une dans la nature.
- Pipeline pondéré (montant × probabilité), détection des relances en retard.
- Gain d'une opportunité → création automatique de la mission correspondante.
- Créer, consulter, **modifier**, **archiver** et **supprimer** une opportunité (refusé si un devis
  la référence), et journaliser/modifier/supprimer ses interactions (appels, emails, réunions,
  notes) — en CLI, en MCP et depuis la fenêtre. L'archivage est un axe distinct de gagnée/perdue :
  une opportunité archivée est simplement devenue sans objet. Même désignation par nom/UUID/préfixe
  et même garde-fou d'écriture concurrente que les clients.

### Missions
- Trois modes de facturation par mission : **régie** (TJM × jours saisis), **forfait** (jalons,
  facturable au fur et à mesure), **récurrent** (montant mensuel fixe).
- Saisie de temps facturable / non facturable, TJM effectif calculé, taux d'occupation.
- Créer, consulter, **modifier**, **clôturer**/**rouvrir**, **archiver** et **supprimer** une
  mission, et modifier/supprimer une saisie de temps — en CLI, en MCP et depuis la fenêtre. Clore
  (date de fin, réversible) et archiver (classement, pour une mission déjà facturée donc
  indélébile) sont deux gestes distincts. Une mission ne se supprime que si aucune facture ni
  saisie de temps ne la référence.

### Devis
- Versionnés et immuables une fois émis. Acceptation → génère la mission et son échéancier de
  facturation automatiquement — et la mission garde la **lignée** de l'opportunité dont le devis
  est issu (comme un gain direct).
- **Lisibles** (`quote list`/`quote show` : contenu, total HT net de remise, mission issue,
  nombre de versions) et désignables par **référence** (UUID, préfixe, ou nom du client porteur)
  dans tous les verbes — en CLI, en MCP (`quote.*`, ressources `freeflow://quotes`) et depuis la
  fenêtre (écran `devis` : liste, fiche avec lignes, envoyer/décliner/accepter).
- **Création et révision depuis la fenêtre** (panneaux « nouveau devis » / « réviser », palette
  ⌘K, raccourci `n`) comme en CLI : les lignes polymorphes s'écrivent dans une syntaxe texte
  partagée, une ligne par ligne — `description:type:montant[:taux]`, ex.
  `Développement:forfait:1350.00`, `Conseil:regie:650.00x10` (TJM×jours),
  `TMA:recurrent:2000.00x12` (mensuel×mois) — parsée par le domaine lui-même
  (`QuoteLine: FromStr`), jamais réimplémentée par façade. En CLI : `--line <SPEC>` répétable, à
  côté du `--lines <JSON>` historique (exclusifs). La fiche d'un devis avertit dès la saisie si
  ses lignes mélangent des types de facturation qui rendront l'acceptation impossible.

### Facturation & TVA
- Numérotation séquentielle **sans trou**, garantie même sous écriture concurrente entre
  processus (CLI + GUI + MCP ouverts en même temps).
- Facture émise **immuable** : ni modification ni suppression, seulement annulation par avoir.
- Journal d'audit **chaîné par hash** — `freeflow audit verify-chain` détecte toute altération
  directe de la base.
- Cinq taux de TVA (normal, intermédiaire, réduit, super-réduit, taux zéro / autoliquidation),
  arrondi au centime par taux.
- Import de relevés bancaires CSV et OFX, rapprochement, balance âgée — et depuis le lot 22 les
  transactions importées sont **listables** (`bank list --unmatched` : celles restant à
  rapprocher, d'une facture pour un crédit ou d'une dépense pour un débit — voir « Dépenses »),
  une transaction déjà rapprochée ne peut plus l'être une seconde fois (doublon d'encaissement
  silencieux, corrigé), et la lignée transaction → encaissement est persistée.
- **Corrections d'encaissement** : un paiement saisi à tort s'**annule** (`payment void
  --reason`, contre-écriture : il reste dans l'historique mais sort du statut payé, de la
  balance âgée et du prévisionnel — le motif est journalisé dans l'audit chaîné), et un
  rapprochement se **défait** (`bank unreconcile` : libère la transaction et annule
  l'encaissement qui en était issu). En CLI, en MCP (`payment.*`/`bank.*`, actions sensibles :
  un agent propose, un humain confirme) et depuis la fenêtre (fiche de facture : encaissements,
  annulation, statut payée/partielle enfin calculé). Sans effet fiscal : TVA et IS sont calculés
  sur les débits (factures émises), jamais sur les encaissements.
- **Factur-X** : génère un vrai PDF/A-3b (via Typst) avec le XML CII EN 16931 embarqué en pièce
  jointe conforme, validé contre le XSD officiel vendorisé dans le dépôt. Anticipe la réforme
  française (réception obligatoire au 1ᵉʳ septembre 2026, émission TPE/PME au 1ᵉʳ septembre 2027).

### Dépenses & obligations fiscales
- Dépenses catégorisées (logiciels, matériel, déplacement, repas, bureau, formation/cotisations,
  **honoraires**, **frais bancaires**, autre) avec TVA déductible et justificatif archivé par
  hash d'intégrité (SHA-256).
- Créer, consulter, **modifier** et **supprimer** une dépense — en CLI (`freeflow expense …`,
  avec `--receipt`/`--clear-receipt` pour remplacer ou détacher le justificatif), en MCP
  (`expense.*`, ressources `freeflow://expenses`) et depuis la fenêtre (écran `depenses`, avec
  un champ fichier pour joindre, remplacer ou détacher le justificatif — archivé exactement comme
  par la CLI, à côté du coffre, en `0600`). Même désignation par libellé/UUID/préfixe et même
  garde-fou d'écriture concurrente que les clients.
  **Une dépense datée dans un exercice déjà clôturé (voir la clôture d'exercice) ne se crée, ne
  se modifie et ne se supprime plus** : le résultat figé à la clôture a été calculé sur ces
  lignes-là — supprimez d'abord l'exercice s'il n'est qu'un projet (`freeflow year rm`).
- **Rapprochement bancaire des dépenses** : les débits du relevé importé (`bank list
  --unmatched`, qui ne montrait jusqu'ici que des lignes à jamais « à rapprocher » côté sorties)
  paient des dépenses. Créer la dépense depuis le débit — `freeflow expense record --transaction
  <id>` reprend le montant et la date du relevé s'ils sont omis, outil MCP `expense.record` avec
  `bank_transaction_id`, bouton « + dépense » du bloc « débits du relevé à rapprocher » de
  l'écran `depenses` (formulaire pré-rempli) — ou rapprocher une dépense existante au montant
  exact (`expense reconcile <réf> --transaction <id>`, `expense.reconcile`, bouton « rapprocher
  d'un débit » de la fiche, qui ne propose que les débits du même montant). Un crédit est refusé,
  un débit ne se rapproche qu'une fois, une dépense rapprochée garde le montant du relevé ;
  `bank unreconcile` libère le débit sans toucher à la dépense, supprimer la dépense libère son
  débit. Côté agent MCP, rapprocher est une action de rapprochement bancaire comme
  `bank.reconcile` : proposée, puis confirmée par un humain. `expense show` (et `expense.show`,
  `freeflow://expenses/{réf}`) porte le débit rapproché sous `bank_transaction`.
- Échéances indicatives CA3 (TVA), acomptes d'IS, CFE — **volontairement pas une source de vérité
  fiscale** : le module le documente explicitement, à vérifier sur impots.gouv.fr. La date limite
  de la CA3 suit toutefois la **grille officielle** (BOFIP BOI-TVA-DECLA-20-20-10-10) : zone du
  siège (Paris/92/93/94 ou autres départements), forme juridique (EI, société, SA/SAS) et deux
  premiers chiffres du SIREN, périodicité mensuelle ou trimestrielle selon le régime déclaré, et
  report au jour ouvré suivant quand la date tombe un week-end ou un jour férié. Au **réel
  simplifié**, le calendrier produit à la place les deux acomptes semestriels (formulaire 3514,
  juillet 55 % / décembre 40 % de la TVA de l'exercice précédent, dispense sous 1 000 €) et la
  CA12 annuelle (ou CA12 E pour un exercice décalé), en tenant compte de la suppression de ce
  régime pour les exercices ouverts à compter du 1er janvier 2027 (loi de finances pour 2025),
  après quoi il bascule en CA3 trimestrielle. `freeflow company show` affiche la règle de
  télédéclaration dérivée du profil (`vat_filing`).
- Prévisionnel de trésorerie sur 12 mois (factures émises non payées + missions signées non
  facturées + pipeline pondéré − charges connues).
- **Grand livre dérivé, balance et bilan** : FreeFlow ne tient pas de comptabilité, il *dérive*
  les écritures de ses faits — bilan d'ouverture (journal `AN`), factures et avoirs (`VE`),
  encaissements et annulations (`BQ`), dépenses (`AC` ; une dépense rapprochée d'un débit du
  relevé passe par 401 : la charge à sa date, le décaissement 401/512 à la date du relevé, en
  `BQ`, éventuellement dans l'exercice suivant — un 401 créditeur au bilan), puis les opérations
  de clôture (`OD`) :
  rémunération du dirigeant (641/645 contre 421/431, réputée due), IS (695 contre 444) et
  affectation du résultat de l'exercice précédent (120/129 vers 1061, 457, 110/119, datée de
  l'AG ou du premier jour de l'exercice tant que la décision est un projet). Les exercices
  s'enchaînent : ceux qui suivent un exercice clos dans l'application s'ouvrent sur son bilan de
  clôture dérivé. `freeflow year balance 2026` (ou `--json`), outil MCP `fiscal.balance_sheet`,
  ressource `freeflow://balance-sheet/{année}`, bouton « bilan » de l'écran `cloture` : la
  **balance des comptes** et le **bilan simplifié** dans la présentation du tableau
  **2033-A-SD** (actif brut / amortissements / net, passif par rubriques, cases 010 à 180),
  équilibré par construction — pour un exercice clos ou non, c'est ce qu'on regarde *avant* de
  clore. En PDF : `freeflow year render 2026 balance-sheet --out bilan.pdf`, `fiscal.render_year`
  avec `balance_sheet`, lien « bilan et balance (PDF) » de la fenêtre. La liasse JSON gagne les
  cases 2033-A.
- **Export FEC** (Fichier des Écritures Comptables, art. A. 47 A-1 LPF) d'un exercice, clos ou
  non, pour l'expert-comptable : `freeflow fec export 2026 --out <répertoire|fichier>`, outil MCP
  `fec.export`, bouton « FEC » de l'écran `cloture`. Le format DGFiP (18 colonnes, `|`,
  `AAAAMMJJ`, virgule décimale, nom `<SIREN>FEC<AAAAMMJJ>.txt`) du grand livre dérivé ci-dessus,
  journaux `AN`/`VE`/`AC`/`BQ`/`OD`, écritures équilibrées par construction.
- **Bilan d'ouverture** : la reprise, compte par compte, du dernier bilan tenu avant FreeFlow
  (typiquement par l'expert-comptable), à saisir **avant** toute clôture dans l'application.
  `freeflow year opening set --opens-on 2025-10-01 --line "101000:Capital social:C:1000.00"
  --line "512000:Banque:D:1000.00"` (ou `--lines-file`, une ligne par compte, même syntaxe
  `compte:libellé:D|C:montant`), `year opening show|rm` ; outils MCP `fiscal.opening_balance`/
  `fiscal.set_opening_balance`/`fiscal.delete_opening_balance` et ressource
  `freeflow://opening-balance` ; bouton « bilan d'ouverture » de l'écran `cloture`. Comptes de
  bilan seulement (classes 1 à 5), total débit = total crédit, une ligne par compte. Il fournit
  le report à nouveau (110/119, et un 120/129 réputé affecté en report) et la réserve légale
  (1061) dont hérite le premier exercice clos ici — qui doit commencer le jour même de la
  reprise — et les **à-nouveaux** (journal `AN`) du grand livre de ce premier exercice. Figé dès
  qu'un exercice est clos : corriger se fait en supprimant d'abord le projet de clôture. Il
  reprend aussi, hors bilan, les **déficits fiscaux antérieurs** encore reportables
  (`--tax-losses`, case 870 du dernier tableau 2033-D déposé).
- **Parcours de clôture guidé** : `freeflow year checklist 2026` (ou `--json`, `--today` pour
  se placer à une autre date), outil MCP `fiscal.checklist`, ressource
  `freeflow://closing-checklist/{année}`, bouton « parcours » de l'écran `cloture` (et depuis
  la fiche d'un exercice). Une lecture du coffre, qui n'écrit rien : où en est la clôture de
  l'exercice (*en cours*, *bloquée*, *prête*, *close en projet*, *approuvée*) et seize étapes en
  quatre temps — **préparer** (profil d'entreprise complet, exercice écoulé, bilan d'ouverture
  daté du bon jour, exercice précédent approuvé ou trou dans la chaîne, factures non
  encaissées, dépenses sans justificatif, mouvements du relevé non rapprochés, résultat et IS
  prévisionnels ou figés, bilan dérivé équilibré), **clore** (avec la **dotation minimale à la
  réserve légale** de l'art. L232-10 du Code de commerce — un vingtième du bénéfice diminué des
  pertes antérieures, jusqu'à 10 % du capital — et le report en arrière possible), **affecter et
  approuver** (dotation insuffisante signalée, décision de l'associé unique dans les six mois),
  **déclarer et déposer** (documents, solde d'IS, liasse, dépôt au greffe dans le mois suivant
  l'approbation, avec leurs échéances). Une étape *bloquante* est un refus que le cœur opposerait
  à la clôture (ou un résultat qui serait faux) ; une étape *attention* mérite un regard sans rien
  empêcher. Les étapes viennent du cœur ; chaque façade y branche ses propres gestes (la commande
  à taper, l'outil à appeler, le bouton du panneau — le formulaire de clôture de la fenêtre
  arrive pré-rempli avec la période et la dotation minimale).
- **Déficits fiscaux : report en avant et report en arrière.** Le résultat *fiscal* d'un
  exercice n'est pas son résultat comptable : les déficits des exercices antérieurs (bilan
  d'ouverture, puis chaque exercice déficitaire clos ici) **s'imputent sur le bénéfice avant
  IS** (art. 209 I CGI, dans la limite de 1 000 000 € + 50 % de l'excédent — BOFIP
  BOI-IS-DEF-10-30), et l'IS est calculé sur ce résultat fiscal. Le stock de déficits
  reportables n'est jamais saisi ni stocké : il se dérive de la chaîne des exercices clos
  (`losses_carried_forward` de `year show`, case 870 du 2033-D). À la clôture, **l'option de
  report en arrière** (`freeflow year close --carry-back`, `carry_back` de `fiscal.close_year`,
  case du formulaire de la fenêtre — art. 220 quinquies CGI, notice 2039-SD ligne 13) impute le
  déficit de l'exercice sur le bénéfice fiscal *non distribué* de l'exercice précédent clos
  ici, dans la limite de 1 000 000 €, en priorité sur la fraction taxée au taux normal puis sur
  celle au taux réduit (BOI-IS-DEF-20-10 § 100) ; la créance d'IS qui en naît est un produit
  (699 contre 444 dans le grand livre et le FEC, une créance à l'actif du 2033-A) qui entre dans
  le résultat net et donc dans le report à nouveau. Refusée sans déficit, sans exercice
  précédent clos dans l'application ou sans bénéfice d'imputation — jamais un report
  silencieusement nul. La liasse JSON gagne les cases de suivi des déficits (2033-B 356/360/372,
  2033-D 982/983/984/860/870) et le résultat fiscal en 2065. Conventions dites plutôt que
  devinées : les distributions sont réparties entre les deux fractions de taux au prorata (le
  2039-SD laisse ce choix à l'entreprise) ; l'option ne se change pas sur un projet (supprimer
  et clore à nouveau) ; l'utilisation de la créance (paiement de l'IS des cinq exercices
  suivants, remboursement au terme) n'est pas suivie.

### Sécurité & fiabilité
- Chiffrement SQLCipher par une **clé maître aléatoire** (modèle LUKS) : la passphrase ne sert
  qu'à dériver, par Argon2id, la clé qui enveloppe cette clé maître dans le sidecar `<db>.kdf`
  (scellement XChaCha20-Poly1305 — une mauvaise passphrase, comme un sidecar altéré, échoue au
  tag d'authentification avant de toucher la base). Les coffres créés avant le format v3 restent
  lisibles tels quels et migrent au premier changement de passphrase.
  Aucune variable d'environnement de passphrase
  n'existe : la passphrase est saisie au clavier (invite masquée), lue dans un fichier
  (`--passphrase-file`, permissions vérifiées), ou produite par une commande externe
  (`--passphrase-command`, ex. `pass show freeflow`) — jamais écrite en clair sur disque, jamais
  visible dans l'historique du shell ou `/proc/<pid>/environ`. La clé n'est mise en cache dans le
  trousseau OS (Keychain macOS / Secret Service Linux) que sur demande explicite
  (`--remember`/case « se souvenir »), toujours avec une expiration bornée (12 h par défaut).
  `freeflow passphrase change` permet d'en changer sans perdre les données du coffre (voir
  « Changer de passphrase » plus bas).
- Aucun port réseau ouvert, aucune connexion sortante. Les relances/emails sont générés en
  brouillons `.eml` ouverts dans ton client mail par défaut — rien n'est jamais envoyé par l'app
  elle-même.
- Sauvegarde chiffrée automatique et silencieuse à chaque commande (au plus une par semaine par
  défaut), avec restauration réellement testée (pas juste une copie de fichier supposée valide).
- Audité en continu : `cargo-deny` (licences, avisos RUSTSEC) et `cargo-audit` intégrés au gate de
  vérification.

### Trois façades, un seul cœur
- **CLI** (`freeflow`) — pensée pour un humain *et* pour un agent : `--json`, `--dry-run`,
  `--actor`, codes de sortie normalisés par famille d'erreur.
- **Serveur MCP** (`freeflow-mcp`) — expose les mêmes commandes/requêtes comme outils MCP en
  stdio, pour piloter l'app depuis Claude Code ou un autre client MCP, plus des **ressources**
  (`freeflow://clients`, `freeflow://clients/{référence}`) pour lire l'état sans appeler d'outil.
  Toute action sensible déclenchée par un agent (émission de facture, avoir, suppression d'un
  client) crée une action en attente (`PendingAction`) : rien ne s'applique sans confirmation
  humaine explicite, au terminal (`freeflow confirm <id>`) ou dans la fenêtre — il n'existe
  volontairement **aucun outil MCP pour confirmer** : un agent ne peut jamais valider sa propre
  proposition.
- **Desktop** (`freeflow-desktop`) — coque Tauri v2, dashboard/prospection/missions/facturation/
  clients/console avec journal d'audit en temps réel dans le rail latéral. Créer, modifier,
  archiver et supprimer des clients se fait depuis un panneau latéral, sans jamais passer par la
  console. La console intégrée exécute *littéralement* le même parseur que le terminal.

## Utilisation en production

### Prérequis
- [Nix](https://nixos.org/download) avec flakes activés, et idéalement [direnv](https://direnv.net/).
- macOS ou Linux (voir [limites connues](#limites-connues-de-cette-première-version) pour le détail plateforme).

### Installation

```bash
git clone <url-de-ton-fork-ou-dépôt> freeflow
cd freeflow
direnv allow          # ou : nix develop
just check            # fmt, lint, tests, cargo-deny/cargo-audit — doit être tout vert
```

### Premier lancement

**En CLI** — aucune variable d'environnement à exporter. Sans `--db`/`FREEFLOW_DB`, le coffre
vit à l'emplacement standard de ton système (`~/.local/share/freeflow/vault.db` sur Linux,
`~/Library/Application Support/FreeFlow/vault.db` sur macOS) :

```bash
freeflow init                        # crée le coffre, demande la passphrase deux fois au clavier
                                      # (invite masquée) — aucune récupération n'est possible si
                                      # tu la perds, note-le où tu notes déjà tes mots de passe
freeflow unlock --remember --ttl 12h # ouvre une session de 12h dans le trousseau OS
freeflow company set-profile --help  # renseigne SIREN, TVA intra, adresse...
```

Sans `--remember`, `freeflow unlock` vérifie seulement la passphrase — chaque commande suivante en
redemandera une, tant qu'aucune session n'est active. `freeflow vault status` affiche le chemin
résolu, si un coffre y existe, et jusqu'à quand une session est en cache. `freeflow lock` purge
cette session à tout moment.

Pour un usage non interactif (scripts, CI, `freeflow-mcp`) : `--passphrase-file <fichier>` (dont
les permissions doivent être 0600) ou `--passphrase-command "<commande>"` (ex.
`--passphrase-command "pass show freeflow"`) remplacent l'invite au clavier sur n'importe quelle
commande.

**En GUI** : `cargo run -p freeflow-desktop` ouvre une vraie fenêtre native — toujours, même si le
coffre n'existe pas encore ou est verrouillé : elle affiche alors l'écran de création ou de
déverrouillage plutôt que de disparaître. La case « rester déverrouillé 12h » y correspond à
`--remember`. La fenêtre se reverrouille elle-même après 15 minutes d'inactivité réelle (la
frappe et le clic comptent, le rafraîchissement automatique du journal d'audit non).

**Piloté par un agent** : lance d'abord `freeflow unlock --remember --ttl <durée>` dans un
terminal, puis `freeflow-mcp` (stdio) — ce serveur, sans terminal, ne peut jamais demander de
passphrase lui-même et dépend entièrement de cette session déjà en cache. Branche-le dans Claude
Code ou un autre client MCP compatible. Toute action à effet sensible proposée par l'agent attend
ta confirmation (`freeflow pending list`, `freeflow confirm <id>`).

> **Tu utilisais `FREEFLOW_PASSPHRASE` ?** Cette variable a été supprimée : plus aucune commande
> ne la lit. Si tu l'avais exportée dans un fichier de shell (`.bashrc`, `.envrc`...), retire-la
> et considère cette passphrase comme potentiellement compromise (elle est restée en clair dans
> ton historique de shell et dans l'environnement de chaque process que tu as lancé) —
> remplace-la avec `freeflow passphrase change` (voir ci-dessous), qui garde le même coffre et
> toutes ses données.

### Usage quotidien

- **En CLI** : `freeflow --help`, puis `freeflow <commande> --help` pour chaque sous-commande
  (`client`, `prospect`, `mission`, `quote`, `invoice`, `payment`, `bank`, `expense`, `fiscal`,
  `forecast`, `audit`, `backup`...). Ajoute `--json` pour scripter, `--dry-run` pour prévisualiser
  sans écrire.
- **En GUI** : voir « Premier lancement » ci-dessus. Le bouton « verrouiller » de la barre de
  commandes ferme la connexion et purge la session du trousseau OS, comme `freeflow lock`.
- **Piloté par un agent** : voir « Premier lancement » ci-dessus.

### Sauvegarde et restauration

Une sauvegarde silencieuse tourne déjà en arrière-plan (voir plus haut). Pour en forcer une ou
restaurer explicitement :

```bash
freeflow backup create --out ~/Sauvegardes/freeflow-$(date +%Y%m%d).db
freeflow backup restore --from ~/Sauvegardes/freeflow-20260101.db --to ~/nouveau-coffre.db
```

`backup restore` ne touche jamais ton coffre par défaut — il faut lui donner une destination
explicite, restaurée puis vérifiée avant de la mettre en usage.

### Changer de passphrase

```bash
freeflow passphrase change   # invite l'ancienne, puis deux fois la nouvelle (saisies masquées)
```

Sur un coffre au format v3 (tout coffre créé désormais), l'opération est **instantanée quelle
que soit la taille du coffre** : la clé maître ne change pas, seule son enveloppe (le sidecar
`.kdf`) est réécrite, atomiquement — la base n'est pas touchée. Un coffre plus ancien (v2) est
ré-chiffré intégralement une dernière fois et migre au format v3 à cette occasion ; les
changements suivants ne ré-chiffrent plus rien. L'ancienne passphrase est **toujours** exigée,
même si une session est déjà active dans le trousseau OS ou si la fenêtre est déjà
déverrouillée — ni l'une ni l'autre ne prouvent que c'est bien toi qui tapes la commande. Une
sauvegarde est écrite automatiquement avant toute modification (`backups/pre-passphrase-change-
*.db`) ; si l'opération échoue pour quelque raison que ce soit, rien n'a été touché. `--dry-run`
affiche ce qui serait fait (régime — ré-enveloppement seul ou re-chiffrement migrateur —, volume
concerné, emplacement de la sauvegarde) sans rien écrire. Chaque sauvegarde voyage avec son
propre `.kdf` : c'est lui qui porte la clé de cette copie-là — ne jamais séparer les deux
fichiers.
`--new-passphrase-file`/`--new-passphrase-command`/`--new-passphrase-stdin` existent en miroir des
options `--passphrase-*` pour un usage non interactif. **Cette sauvegarde préalable — comme toute
sauvegarde antérieure — reste chiffrée avec l'ANCIENNE passphrase** : ne t'en débarrasse pas sous
prétexte que tu viens d'en changer. Ce n'est volontairement pas exposé dans la GUI ni dans la
console de la fenêtre : lance-la depuis un terminal, fenêtre fermée (voir `CLAUDE.md`).

### Empaquetage natif

`nix build` produit les binaires (`freeflow`, `freeflow-desktop`, `freeflow-mcp`) pour ta
plateforme. Les bundles installables (`.dmg` macOS, `.AppImage` Linux) ne sont pas encore
distribués prêts à l'emploi — voir la checklist ci-dessous.

## Limites connues de cette première version

- **Statut fiscal** : pensé pour une SASU/EURL française à l'IS avec TVA au réel. D'autres statuts
  (micro-entreprise, société à l'IR...) ne sont pas couverts.
- **Devise** : euro uniquement — cohérent avec le statut fiscal visé, mais pas adapté à une
  activité facturée dans une autre devise.
- **Échéances fiscales indicatives, pas une source de vérité légale** — toujours vérifier sur
  impots.gouv.fr. La date de la CA3 suit la grille officielle et le régime réel simplifié est
  modélisé (acomptes 3514, CA12), mais la base des acomptes est la TVA nette de l'exercice
  précédent — le domaine ne distingue pas la TVA sur immobilisations, que la règle légale exclut.
- **Grand livre dérivé, pas une comptabilité tenue** : une dépense non rapprochée d'un débit du
  relevé est réputée payée à sa date (le 401 n'apparaît que pour les dépenses rapprochées),
  l'équipement passe en charge sans seuil d'immobilisation, la
  rémunération du dirigeant est réputée due et non décaissée (aucun fait de paie), la TVA n'est
  jamais liquidée (445660/445710 restent bruts au bilan), pas d'amortissement de l'exercice, de
  provision ni de régularisation ; lettrage et devise du FEC restent vides. Un exercice qui suit
  un exercice **non** clos dans l'application n'a pas d'à-nouveaux. L'expert-comptable reste
  maître des écritures définitives et du bilan déposé.
- **`.dmg` macOS** : pas encore construit/testé (nécessite une machine macOS réelle).
- **`.AppImage` Linux** : le bundling bute sur une incompatibilité d'environnement documentée dans
  `CLAUDE.md` (chemin `gdk-pixbuf` non-FHS sur certaines distributions type NixOS/Nix-sur-Arch).

## Feuille de route / améliorations futures

- [x] `freeflow passphrase change` : ré-chiffrement complet du coffre, sauvegarde préalable
      obligatoire (voir « Changer de passphrase » ci-dessus). N'émet pas `PRAGMA rekey` :
      `sqlite3_rekey_v2` de SQLCipher renvoie inconditionnellement succès même quand la
      transaction interne échoue (page illisible, coffre occupé, commit raté) — inutilisable pour
      l'opération la plus risquée du dépôt. À la place, une copie ré-chiffrée est écrite dans un
      fichier temporaire puis basculée en place par deux `rename()` (base, puis sidecar) ; un
      sidecar en attente rend la fenêtre entre les deux renames récupérable automatiquement au
      prochain déverrouillage, sans intervention.
- [x] Gestion des données (client + contact) : modifier, archiver, supprimer, en CLI, en MCP et
      depuis la fenêtre — voir « Clients » ci-dessus. Fondations posées pour les entités
      suivantes (révision optimiste, résolveur de référence par nom, panneau latéral de la GUI).
- [x] Gestion des données (opportunité + interaction, mission + saisie de temps) : modifier,
      archiver/désarchiver, supprimer, clore/rouvrir une mission — en CLI, en MCP et depuis la
      fenêtre. Voir « Prospection » et « Missions » ci-dessus.
- [x] Gestion des données des dépenses (`UpdateExpense`/`DeleteExpense`, avec garde « exercice
      clôturé »), lecture des devis (`quote list`/`show`, écran `devis`, ressources MCP) et
      colonne `missions.opportunity_id` traçant la lignée d'un gain d'opportunité (posée par
      `WinOpportunity`, héritée du devis par `AcceptQuote` ; une opportunité gagnée dont la
      mission existe encore n'est plus supprimable, seulement archivable) — voir « Devis » et
      « Dépenses » ci-dessus.
- [x] Éditeur de devis dans la fenêtre (création/révision) : lignes polymorphes en syntaxe texte
      partagée avec la CLI (`QuoteLine: FromStr`, `--line` répétable), remise, conditions,
      avertissement d'acceptabilité — voir « Devis » ci-dessus.
- [x] `VoidPayment`/`UnreconcileTransaction` : annulation d'encaissement (contre-écriture,
      motif journalisé) et rapprochement défaisable, avec lignée transaction → encaissement
      persistée (migration 0013) et garde contre le double rapprochement — voir « Facturation &
      TVA » ci-dessus.
- [x] Parité MCP sur `company`/`forecast`/`invoice render` — comblée pour `expense.*` (lot 21) et
      `fiscal.calendar`/`fiscal.years` (lots 19-20), puis achevée (lot 25) : `company.show`/
      `company.set_profile` (+ ressource `freeflow://company`), `forecast.show`,
      `fiscal.deadlines`, cycle de vie complet des exercices (`fiscal.year_show`/`amend_year`/
      `approve_year`/`delete_year` — approbation et suppression derrière confirmation humaine,
      comme la clôture) et rendu de documents (`invoice.render` Factur-X, `fiscal.render_year`
      PV/affectation/synthèse/liasse) qui refusent d'écraser un fichier existant. Hors périmètre
      MCP par conception, comme avant : session/coffre (`init`/`unlock`/`lock`/`passphrase`/
      `backup`) et `confirm`.
- [x] Sidecar v3 à clé maître enveloppée (modèle LUKS) : le coffre est chiffré par une clé
      aléatoire, elle-même scellée dans le sidecar (XChaCha20-Poly1305, en-tête en AAD) par la
      clé dérivée d'Argon2id. Un changement de passphrase est devenu la réécriture atomique d'un
      seul petit fichier — plus de ré-chiffrement de la base, plus de fenêtre de bascule. Les
      coffres v2 migrent au premier `passphrase change` (le seul moment où re-chiffrer est de
      toute façon inévitable) — voir « Sécurité » et « Changer de passphrase » ci-dessus.
- [ ] Bundle `.dmg` macOS signé et notarisé, construit et testé sur une vraie machine macOS.
- [ ] Bundle `.AppImage` Linux fonctionnel (résoudre l'incompatibilité `linuxdeploy-plugin-gtk` /
      chemin `gdk-pixbuf` non-FHS, ou bundler depuis une distribution Linux FHS conventionnelle).
- [ ] Validation Schematron EN 16931 complète (nécessite un moteur XPath 2.0 type Saxon/XSLT2 —
      `xmllint --schematron` ne peut exécuter que les règles XPath 1.0).
- [ ] Validation PDF/A-3b par veraPDF en plus des vérifications structurelles actuelles.
- [ ] Support d'autres statuts fiscaux français (micro-entreprise, société à l'IR).
- [ ] Support multi-devise.
- [x] Échéance CA3 exacte : la règle officielle ne dépend pas du dernier chiffre du SIREN (l'idée
      de départ de cet item) mais de la zone du siège, de la catégorie de redevable et des deux
      premiers chiffres du SIREN — voir « Dépenses & obligations fiscales » ci-dessus.
- [x] Régime réel simplifié de TVA : acomptes semestriels 3514 et CA12/CA12 E, avec la fin du
      régime pour les exercices ouverts à compter de 2027 ; règle de télédéclaration dérivée
      affichée par `company show`.
- [ ] Relances de paiement configurables (cadences, modèles de message) au-delà des brouillons
      `.eml` actuels.
- [x] Export comptable : FEC d'un exercice (CLI, MCP, fenêtre), dérivé des faits du domaine — voir
      « Dépenses & obligations fiscales » et les limites ci-dessus.
- [x] Bilan d'ouverture (reprise du bilan de l'expert-comptable) chaîné dans la clôture et le FEC —
      voir « Dépenses & obligations fiscales » ci-dessus.
- [x] Grand livre dérivé complet et **bilan de clôture** (actif/passif, tableau 2033-A) :
      à-nouveaux chaînés d'un exercice clos sur le suivant, opérations de clôture (rémunération,
      IS, affectation) en écritures `OD`, balance des comptes et bilan 2033-A en CLI/MCP/fenêtre
      et en PDF, cases 2033-A dans la liasse — voir « Dépenses & obligations fiscales ».
- [x] Déficit fiscal reportable (art. 209 I CGI) et option de report en arrière
      (art. 220 quinquies) : imputation plafonnée sur les bénéfices suivants, créance d'IS du
      report en arrière en produit, suivi des déficits dans la liasse — voir « Dépenses &
      obligations fiscales » ci-dessus. Non suivi : l'utilisation de la créance sur les cinq
      exercices suivants et son remboursement.
- [x] Catégories de dépense « honoraires » (622600) et « frais bancaires » (627000), et
      rapprochement bancaire des dépenses : un débit du relevé importé crée ou rapproche une
      dépense, au montant exact, dans les trois façades ; le grand livre date alors le
      décaissement du relevé (401 puis 512) — voir « Dépenses & obligations fiscales ».
- [x] Parcours de clôture guidé : `year checklist`, `fiscal.checklist`, panneau « parcours » —
      l'état de la clôture d'un exercice en seize étapes (préparer, clore, affecter et approuver,
      déclarer et déposer), avec la dotation minimale à la réserve légale (art. L232-10) et les
      échéances d'AG, de liasse, de solde d'IS et de dépôt au greffe — voir « Dépenses &
      obligations fiscales ».
- [ ] Tableau de bord de rentabilité par client sur la durée (au-delà de la mission en cours).
- [ ] Chiffrement additionnel des pièces jointes de justificatifs de dépenses sur disque (au-delà
      du hash d'intégrité SHA-256 déjà en place).
- [ ] Synchronisation multi-appareils documentée et testée de bout en bout (Syncthing/iCloud Drive
      — actuellement une possibilité architecturale, pas un flux accompagné).
- [ ] Thème clair/sombre et personnalisation de l'UI desktop.
- [ ] Internationalisation de l'interface (actuellement en français uniquement).

## Licence

MIT (voir `Cargo.toml`).
