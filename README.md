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

### Prospection
- Opportunités avec étape, montant, probabilité, source d'acquisition, motif de perte structuré.
- Date de prochaine action **obligatoire** sur chaque opportunité ouverte — impossible d'en perdre
  une dans la nature.
- Pipeline pondéré (montant × probabilité), détection des relances en retard.
- Gain d'une opportunité → création automatique de la mission correspondante.

### Missions
- Trois modes de facturation par mission : **régie** (TJM × jours saisis), **forfait** (jalons,
  facturable au fur et à mesure), **récurrent** (montant mensuel fixe).
- Saisie de temps facturable / non facturable, TJM effectif calculé, taux d'occupation.

### Devis
- Versionnés et immuables une fois émis. Acceptation → génère la mission et son échéancier de
  facturation automatiquement.

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
  stdio, pour piloter l'app depuis Claude Code ou un autre client MCP. Toute action sensible
  déclenchée par un agent crée une action en attente (`PendingAction`) : rien ne s'applique sans
  confirmation humaine explicite (`freeflow confirm <id>`).
- **Desktop** (`freeflow-desktop`) — coque Tauri v2, dashboard/prospection/missions/facturation/
  console avec journal d'audit en temps réel dans le rail latéral. La console intégrée exécute
  *littéralement* le même parseur que le terminal.

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
> remplace-la par une nouvelle avec `freeflow init` sur un nouveau coffre, ou par la commande
> `passphrase change` une fois disponible (voir feuille de route).

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

- [ ] `freeflow passphrase change` : re-dérivation de la clé et `PRAGMA rekey` en place. Volontairement
      hors périmètre du chantier de déverrouillage initial — c'est l'opération la plus risquée du
      dépôt (réécriture de toutes les pages chiffrées) et mérite son propre lot, avec sauvegarde
      préalable obligatoire.
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
