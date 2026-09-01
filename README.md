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
  fenêtre (écran `devis` : liste, fiche avec lignes, envoyer/décliner/accepter). La création et
  la révision restent CLI/MCP (lignes polymorphes en JSON, pas encore d'éditeur graphique).

### Facturation & TVA
- Numérotation séquentielle **sans trou**, garantie même sous écriture concurrente entre
  processus (CLI + GUI + MCP ouverts en même temps).
- Facture émise **immuable** : ni modification ni suppression, seulement annulation par avoir.
- Journal d'audit **chaîné par hash** — `freeflow audit verify-chain` détecte toute altération
  directe de la base.
- Cinq taux de TVA (normal, intermédiaire, réduit, super-réduit, taux zéro / autoliquidation),
  arrondi au centime par taux.
- Import de relevés bancaires CSV et OFX, rapprochement, balance âgée.
- **Factur-X** : génère un vrai PDF/A-3b (via Typst) avec le XML CII EN 16931 embarqué en pièce
  jointe conforme, validé contre le XSD officiel vendorisé dans le dépôt. Anticipe la réforme
  française (réception obligatoire au 1ᵉʳ septembre 2026, émission TPE/PME au 1ᵉʳ septembre 2027).

### Dépenses & obligations fiscales
- Dépenses catégorisées (logiciels, matériel, déplacement, repas, bureau, formation/cotisations,
  autre) avec TVA déductible et justificatif archivé par hash d'intégrité (SHA-256).
- Créer, consulter, **modifier** et **supprimer** une dépense — en CLI (`freeflow expense …`,
  avec `--receipt`/`--clear-receipt` pour remplacer ou détacher le justificatif), en MCP
  (`expense.*`, ressources `freeflow://expenses`) et depuis la fenêtre (écran `depenses`). Même
  désignation par libellé/UUID/préfixe et même garde-fou d'écriture concurrente que les clients.
  **Une dépense datée dans un exercice déjà clôturé (voir la clôture d'exercice) ne se crée, ne
  se modifie et ne se supprime plus** : le résultat figé à la clôture a été calculé sur ces
  lignes-là — supprimez d'abord l'exercice s'il n'est qu'un projet (`freeflow year rm`).
- Échéances indicatives CA3 (TVA), acomptes d'IS, CFE — **volontairement pas une source de vérité
  fiscale** : le module le documente explicitement, à vérifier sur impots.gouv.fr.
- Prévisionnel de trésorerie sur 12 mois (factures émises non payées + missions signées non
  facturées + pipeline pondéré − charges connues).

### Sécurité & fiabilité
- Chiffrement SQLCipher, clé dérivée par Argon2id. Aucune variable d'environnement de passphrase
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

Ré-chiffre l'intégralité du coffre sous une clé neuve. L'ancienne passphrase est **toujours**
exigée, même si une session est déjà active dans le trousseau OS ou si la fenêtre est déjà
déverrouillée — ni l'une ni l'autre ne prouvent que c'est bien toi qui tapes la commande. Une
sauvegarde est écrite automatiquement avant toute modification (`backups/pre-passphrase-change-
*.db`) ; si l'opération échoue pour quelque raison que ce soit, rien n'a été touché. `--dry-run`
affiche ce qui serait fait (volume à ré-chiffrer, emplacement de la sauvegarde) sans rien écrire.
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
  impots.gouv.fr, en particulier la date exacte de télédéclaration CA3 (dépend du dernier chiffre
  du SIREN, non modélisé).
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
- [ ] Éditeur graphique de devis (création/révision des lignes polymorphes depuis la fenêtre) —
      aujourd'hui CLI/MCP seulement.
- [ ] `VoidPayment`/`UnreconcileTransaction` : un encaissement ou un rapprochement saisi à tort
      n'est aujourd'hui ni corrigible ni annulable.
- [ ] Parité MCP sur `company`/`forecast`/`invoice render` — comblée pour `expense.*` (lot 21) et
      `fiscal.calendar`/`fiscal.years` (lots 19-20), le reste du serveur MCP demeure un
      sous-ensemble strict de la CLI.
- [ ] Sidecar v3 à clé maître enveloppée (modèle LUKS) : le coffre serait chiffré par une clé
      aléatoire, elle-même enveloppée dans le sidecar par la clé dérivée d'Argon2id. Un changement
      de passphrase deviendrait la réécriture atomique d'un seul petit fichier — plus de
      ré-chiffrement de la base, plus de fenêtre de bascule à gérer. Demande sa propre migration
      des coffres v2 existants et un algorithme d'enveloppement (AEAD), hors périmètre du lot qui
      a introduit `passphrase change`.
- [ ] Bundle `.dmg` macOS signé et notarisé, construit et testé sur une vraie machine macOS.
- [ ] Bundle `.AppImage` Linux fonctionnel (résoudre l'incompatibilité `linuxdeploy-plugin-gtk` /
      chemin `gdk-pixbuf` non-FHS, ou bundler depuis une distribution Linux FHS conventionnelle).
- [ ] Validation Schematron EN 16931 complète (nécessite un moteur XPath 2.0 type Saxon/XSLT2 —
      `xmllint --schematron` ne peut exécuter que les règles XPath 1.0).
- [ ] Validation PDF/A-3b par veraPDF en plus des vérifications structurelles actuelles.
- [ ] Support d'autres statuts fiscaux français (micro-entreprise, société à l'IR).
- [ ] Support multi-devise.
- [ ] Échéance CA3 exacte tenant compte du régime de TVA déclaré et du dernier chiffre du SIREN.
- [ ] Relances de paiement configurables (cadences, modèles de message) au-delà des brouillons
      `.eml` actuels.
- [ ] Export comptable (FEC ou format équivalent) pour transmission à un expert-comptable.
- [ ] Tableau de bord de rentabilité par client sur la durée (au-delà de la mission en cours).
- [ ] Chiffrement additionnel des pièces jointes de justificatifs de dépenses sur disque (au-delà
      du hash d'intégrité SHA-256 déjà en place).
- [ ] Synchronisation multi-appareils documentée et testée de bout en bout (Syncthing/iCloud Drive
      — actuellement une possibilité architecturale, pas un flux accompagné).
- [ ] Thème clair/sombre et personnalisation de l'UI desktop.
- [ ] Internationalisation de l'interface (actuellement en français uniquement).

## Licence

MIT (voir `Cargo.toml`).
