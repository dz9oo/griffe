# Plan de travail — répondre au scénario « reprendre son bilan et clôturer seul »

Établi le 2 septembre 2026 à partir de la synthèse d'audit (`synthese.html`, constats numérotés
1 à 21 + mineurs) et des trois rapports (`rapport-*.md`). Commit de référence : `e58409f`
(lot 35). Ce plan prolonge la numérotation des lots du `CLAUDE.md` : **lots 36 à 42**.

## Le problème à résoudre, en une phrase

Un indépendant en SASU sans notion comptable installe FreeFlow, reprend le bilan que son cabinet
lui a remis (souvent exporté de Tiime ou Indy), importe le relevé de sa banque, saisit ses
quelques dépenses, et clôture au 30 septembre 2026 avec tous les documents et démarches — **guidé
dans l'application, sans README**. L'audit conclut que ce n'est pas possible aujourd'hui, pour
deux raisons de fond (un bilan faux dès qu'une dette reprise est payée ; une clôture prématurée
irréversible) et trois raisons d'accès (profil hors de la fenêtre, relevé réel inimportable,
aucune reprise de données).

## Critère de sortie global

Le plan est terminé quand un **scénario de preuve de bout en bout** (successeur de
`crates/freeflow-cli/tests/closing_scenario.rs`, complété d'un scénario fenêtre dans
`crates/freeflow-web/tests/`) rejoue le cas de l'audit tel quel :

- bilan d'ouverture **importé** depuis une balance CSV de cabinet, avec 401 (honoraires à payer),
  444 (solde d'IS), 455 (compte courant), 445670 (crédit de TVA), 218/281 (un ordinateur amorti) ;
- relevé **Qonto réel** (virgule, en-tête anglais) importé depuis la fenêtre, plus un relevé
  latin-1 à colonnes débit/crédit ;
- le paiement des honoraires 2025 et du solde d'IS **règle les comptes repris** (401 et 444 à
  zéro au 30/09/2026, banque égale au relevé) ;
- CFE en « impôts et taxes » (635, case 244) ; TVA déductible bornée par le taux ;
- clôture **refusée** le 2 septembre, acceptée le 1er octobre ; approbation refusée à une date
  future ; sauvegarde forcée avant l'approbation ;
- PV nominatif avec les mentions obligatoires ; liasse en 218/244/306 avec 2033-F ; FEC avec
  pièces uniques ; étape TVA et DAS2 dans le parcours ;
- **zéro sortie `Debug`**, dates ISO partout, et aucun message de la fenêtre ne renvoie à une
  commande CLI.

Chaque lot est validé comme les précédents : `just check` puis `nix flake check` en vert, avec
`git add -A` avant Nix (voir la mémoire de session et `CLAUDE.md`), et une entrée dans
l'historique des lots de `CLAUDE.md`.

## Principes qui s'appliquent à tous les lots

1. **Thèse « Studio »** : toute règle nouvelle vit dans `freeflow-core` ; CLI, MCP et fenêtre
   n'y ajoutent que des gestes. Un constat « la fenêtre laisse faire X » se corrige dans le cœur.
2. **`today` vient de l'adaptateur**, jamais du cœur (règle posée au lot 27), et il est pris en
   **heure locale** (mineur « UTC »), avec repli UTC si l'offset local n'est pas déterminable.
3. **Cœur d'abord, façades ensuite, dans le même lot** : un lot n'est pas fini tant que les trois
   façades n'exposent pas le geste (sauf exception documentée, comme la session).
4. **Chaque correction de calcul est d'abord un chiffre posé à la main** dans le doc-comment du
   test, comme au lot 35.
5. **Rupture de contrat JSON annoncée** : les dates passent en ISO 8601 (lot 36) — les snapshots
   `insta` sont mis à jour et le changement est dit dans le README.
6. **Pas de nouvelle dépendance sans nécessité** ; les deux envisagées ici (`encoding_rs` au
   lot 38, rien d'autre) sont justifiées dans leur lot.

---

## Lot 36 — Base saine : garde-fous de clôture, `init`, sorties lisibles, dates ISO

**Constats couverts** : 2, 6, 7, 8, 20, 21 et les mineurs « `--today` obligatoire », « UTC »,
« `confirm --id` », « dépense hors exercice invisible », « débordement `Money` », « approbation
hors délai non signalée », « `year render … liasse --out x.pdf` ».
**Taille** : M. **Pourquoi en premier** : ce lot supprime le seul cas *irréversible* de l'audit
(constat 2) et rend lisibles les commandes que les lots suivants vont solliciter.

### Cœur

- `CloseFiscalYear` gagne `today: Option<Date>` (`serde(default)` : les actions en attente et
  entrées d'audit antérieures restent lisibles). Avec `Some(today)`, la commande **refuse**
  `ends_on > today` (`FiscalYearError::PeriodNotEnded { ends_on }`) et une période de plus de
  24 mois (`PeriodTooLong`, art. L123-12 : un exercice de plus de 12 mois est exceptionnel,
  24 mois est déjà une tolérance). `None` = pas de garde, réservé aux rejouages internes ; les
  trois façades passent toujours `Some`.
- `ApproveFiscalYear` gagne `today: Option<Date>` de la même façon et refuse `approved_on >
  today` (`ApprovalInFuture`). Une approbation au-delà de six mois après la clôture n'est **pas**
  refusée (la décision tardive existe, elle est simplement en retard) mais la sortie de la
  commande porte `late_by_days: Option<u32>` que chaque façade affiche en avertissement.
- **Pas de `UnapproveFiscalYear`** : l'immuabilité d'un exercice approuvé est un invariant par
  trigger (lot 20) et une décision d'AG ne se « dé-prend » pas. Le filet est la **sauvegarde
  forcée** : avant `ApproveFiscalYear`, chaque adaptateur appelle `Store::backup_to(
  backups/pre-approve-<période>-<horodatage>.db)` (IO d'adaptateur, comme la sauvegarde
  préalable de `passphrase change`) et refuse d'approuver si elle échoue. `year approve` dit
  dans sa sortie où est la sauvegarde.
- `RecordExpense`/`UpdateExpense` refusent une date **antérieure à `opens_on`** du bilan
  d'ouverture (`ExpensesError::BeforeOpeningBalance`) : un fait daté avant le bilan repris le
  contredit, et l'audit a montré qu'il disparaissait silencieusement.
- `closing.rs` : `period_ended_step` ne tient plus compte de `facts.record.is_some()` (un
  exercice clos prématurément reste « non écoulé » jusqu'à sa date) ; `previous_year_step` lit
  `chain.reserve` (réserve cumulée) au lieu de la dotation du dernier exercice ; l'échéance
  d'approbation affichée reste la date légale même quand l'approbation est postérieure.
- `Money::parse_decimal` borne l'entrée à `Money::MAX_INPUT` (10^13 centimes, cent milliards
  d'euros) ; les sommes de `Ledger`/`TrialBalance` passent en arithmétique vérifiée et remontent
  `LedgerError::Overflow` au lieu de paniquer.
- `domain::serde_date` : module `time::serde::format_description!` ISO 8601 appliqué à **tous**
  les `Date`/`OffsetDateTime` sérialisés : `BankTransaction`, `Expense`, `Payment`,
  `PendingAction`, les champs de `CloseFiscalYear`, `liasse_export`. Test : un `grep` des
  snapshots ne trouve plus `[20`.
- `store/kdf.rs::sync_dir` : `Some("")` → `"."` ; `Store::init` devient atomique : le sidecar
  est écrit **après** la création réussie du fichier de base, et un échec supprime ce qui a été
  écrit (test : `init` sur un nom nu réussit ; `init` avec un répertoire non inscriptible ne
  laisse rien).

### CLI

- `output.rs` : suppression du repli `{value:?}`. `format_value` exige un trait
  `HumanRender` (rendu texte) ; `Outcome::Applied(v)` affiche `v` via `Display` (identifiants) ou
  un texte dédié (`✓ profil enregistré`, `✓ bilan d'ouverture enregistré (révision 2)`). Rendus
  tabulaires ajoutés pour `company show`, `fiscal calendar`, `fiscal deadlines`, `audit
  verify-chain`, `pending list`, `expense show`, `bank unreconcile`, `expense reconcile`. Le
  compilateur interdit désormais qu'un nouveau type sorte en `Debug`.
- `--today` devient optionnel partout (`fiscal calendar`, `fiscal deadlines`, `forecast show`),
  défaut = date locale.
- `freeflow confirm <ID>` positionnel (`--id` conservé comme alias caché un lot, puis retiré).
- `year close`/`year approve` passent `today` ; `year approve` fait la sauvegarde préalable et
  affiche l'avertissement de retard ; `year render … liasse` refuse une extension `.pdf`.
- `today` local : `time::OffsetDateTime::now_local()` avec repli UTC (helper unique
  `freeflow_cli::today()`, réutilisé par la fenêtre et le MCP).

### MCP

- `fiscal.close_year` / `fiscal.approve_year` passent `today` (paramètre facultatif, défaut
  local) ; `fiscal.approve_year` fait la sauvegarde préalable comme la CLI.

### Fenêtre

- `POST /cloture` : le bouton « clore » n'est rendu que si l'étape `close` du parcours est
  `todo` ; la route passe `today` et affiche le refus du cœur (bandeau) sinon.
- `POST /cloture/{id}/approve` : date max = aujourd'hui dans le `<input type="date">` **et**
  garde du cœur ; sauvegarde préalable ; avertissement de retard.
- Écran `cloture` : période par défaut = dernier exercice **écoulé** partout (barre, bilan,
  FEC, parcours) ; formulaire « clore » pré-rempli depuis `opens_on` du bilan d'ouverture ou la
  fin de l'exercice précédent (`+1 jour`, `+1 an −1 jour`) ; panneau de rapprochement :
  présélection du débit **le plus proche de la date de la dépense**.

### Tests et acceptation

- Cœur : clôture refusée à J−28 et acceptée à J+1 ; période de 10 ans refusée ; approbation
  future refusée ; approbation tardive acceptée avec `late_by_days` ; dépense avant `opens_on`
  refusée ; `init` sur nom nu ; overflow → erreur.
- CLI : snapshots `--help` ; snapshot texte de `company show` et `fiscal calendar` ; `grep -r
  '{:?}' crates/freeflow-cli/src/output.rs` vide.
- Fenêtre : `POST /cloture` prématuré → bandeau, pas de `freeflow:saved`.
- README : section « Clôturer seul » corrigée (`confirm <id>`, sauvegarde avant approbation).

---

## Lot 37 — Un débit qui n'est pas une charge : règlement des comptes de bilan, impôts et taxes, justesse

**Constats couverts** : 1, 13, 14, 15, et les mineurs « 110/119 simultanés », « libellés qui
fuient ». **Taille** : L. **Pourquoi maintenant** : c'est le constat qui rend faux tout bilan
repris d'un cabinet ; sans lui, les lots d'import (38, 40) importeraient des comptes que
l'application ne saurait pas solder.

### Modèle

Une transaction du relevé peut aujourd'hui être *un encaissement de facture* ou *le paiement
d'une dépense*. Elle peut désormais être aussi **le règlement d'un compte de bilan** : paiement
d'une dette reprise (401, 444, 4455, 431), remboursement ou apport en compte courant (455),
remboursement d'un crédit de TVA reçu (445670 au crédit), acompte d'IS versé (444 au débit),
dividendes payés (457), virement interne (580). Le lien reste porté par la **transaction**, comme
au lot 33 : migration `0017_bank_settlements` ajoute `bank_transactions.settlement_account TEXT`
et `settlement_label TEXT` (pas de table nouvelle : un règlement n'a pas d'identité propre, c'est
une lecture du débit). `BankTransaction::is_matched` couvre les trois cas ; `bank list` affiche
« réglé (401000 Fournisseurs) ».

- Domaine : `SettlementAccount` = `AccountCode` de classe 1 à 5, **hors 512** (un règlement
  contre la banque elle-même n'a pas de sens ; le virement interne passe par 580), avec le plan
  fixe enrichi (`ledger::accounts` : 401000, 421000, 431000, 444000, 445510, 445670, 455000,
  457000, 580000, 164000) et un libellé par défaut. Un compte hors plan fixe est accepté avec le
  libellé saisi (comme au bilan d'ouverture).
- Commandes : `SettleBankTransaction { transaction_id, account, label: Option<String> }`
  (`requires_confirmation`, même rail que `bank.reconcile`) et `UnsettleBankTransaction`.
  Gardes : transaction existante, non rapprochée (`AlreadyReconciled`), compte valide. Pas de
  `revision` (mêmes raisons que `ReconcileExpense`). `UnreconcileTransaction` reste distinct.
- Grand livre : une écriture `BQ` à la date du relevé, `compte / 512` pour un débit,
  `512 / compte` pour un crédit, pièce = référence de la transaction. Bilan : un 401 soldé
  disparaît de la case 166 ; un 444 débiteur tombe en « autres créances » par la règle de sens
  existante.
- `closing.rs`, étape `bank` : quand un débit non rapproché existe **et** que le bilan
  d'ouverture (ou l'exercice précédent) laisse un compte de tiers ouvert du même montant, le
  parcours propose le règlement (« ce débit de 600,00 € correspond au solde fournisseur repris »)
  avant la dépense. Nouvelle sous-vérification « comptes de tiers repris encore ouverts au
  dernier jour » (`warning`, avec la liste et les montants) : c'est le signal qui manquait à
  l'expert-comptable.

### Catégorie « impôts et taxes » et TVA déductible

- `ExpenseCategory::Taxes` (`taxes`, 635000 « Impôts, taxes et versements assimilés », case 244 du
  2033-B). Aide de champ : « CFE, CVAE, taxe sur les véhicules… pas l'IS ni la TVA, qui ne sont
  pas des charges ». `ExpenseCategory::ALL` passe à 10.
- `RecordExpense`/`UpdateExpense` : `vat_deductible ≤ amount − amount / (1 + taux)` calculé en
  `Money` entier (arrondi au centime supérieur pour la borne) ; taux `zero` → 0 exigé. Erreur
  `VatExceedsRate { max }` avec le maximum dans le message.

### FEC et grand livre

- `PieceRef` d'une dépense = `DEP-` + UUID complet (36 caractères, unique) ; avec justificatif,
  la même référence, le nom du fichier passant dans `EcritureLib`. Facture/avoir : inchangés
  (numéro séquentiel). Règlement : `BQ-` + UUID de la transaction.
- **Un libellé par compte** : le plan fixe prime toujours ; le libellé saisi au bilan
  d'ouverture ne sert qu'aux comptes hors plan. Test : chaque `CompteNum` du FEC a un seul
  `CompteLib`.
- `Ledger::closing_opening_lines` présente le report à nouveau **net** sur 110 ou 119 (jamais
  les deux), et le résultat en 120 ou 129 selon son signe.

### Façades

- CLI : `bank settle <ID> --account 401000 [--label …]`, `bank unsettle <ID>`, `bank list`
  colonne « affectation », `expense record --category taxes` ; `freeflow confirm` rejoue
  `SettleBankTransaction`.
- MCP : `bank.settle`, `bank.unsettle` (93-94 outils) ; description qui liste les comptes
  usuels.
- Fenêtre : dans le bloc « débits du relevé à rapprocher », chaque ligne offre « c'est une
  dépense » (existant) **ou** « c'est le règlement d'une dette ou d'un compte » → panneau avec
  un `<select>` des comptes usuels (libellé en français : « honoraires ou facture reprise au
  bilan (401) », « solde d'IS repris (444) », « compte courant d'associé (455) », « virement
  entre mes comptes (580) », « autre compte… ») ; la fiche d'une transaction réglée offre
  « défaire ».

### Tests et acceptation

- Scénario de l'expert-comptable rejoué sur base : 401 et 444 à zéro au 30/09/2026, banque
  6 343,12 €, résultat −1 226,90 €, 2033-A sans dette fantôme, FEC équilibré avec les deux
  écritures `BQ`.
- Proptest : le bilan reste équilibré avec des règlements arbitraires sur des comptes
  arbitraires de classes 1 à 5.
- TVA déductible : 150 € sur 600 € à 20 % refusé, 100 € accepté ; catégorie `taxes` en 635 et
  case 244.
- Les trois façades de bout en bout (agent → action en attente → `confirm`).

---

## Lot 38 — Import bancaire réel

**Constats couverts** : 4, 16 et le mineur « `--dry-run` n'annonce rien ». **Taille** : M.

### Cœur (`billing/import.rs`)

- `parse_bank_statement(bytes: &[u8], hint: Option<CsvDialect>) -> Result<ParsedStatement,
  ImportError>` prend des **octets** : décodage UTF-8 (BOM retiré), sinon `windows-1252` via
  `encoding_rs` (dépendance : petite, sans `unsafe` exposé, maintenue par Mozilla ; latin-1 « à
  la main » laisserait faux les caractères 0x80-0x9F que les banques françaises émettent).
- **Sniffing** : séparateur (`;`, `,`, tabulation — celui qui donne le même nombre de champs sur
  l'en-tête et les cinq premières lignes), guillemets, décimale (`,` ou `.`), séparateur de
  milliers (espace, espace insécable), formats de date (`AAAA-MM-JJ`, `JJ/MM/AAAA`,
  `JJ-MM-AAAA`, `JJ.MM.AAAA`), et **colonnes par nom** (dictionnaire FR/EN : date /
  date d'opération / date de valeur ; libellé / label / description / motif ; montant / amount ;
  débit / debit ; crédit / credit). Débit et crédit séparés → montant signé. Un fichier sans
  en-tête reconnu retombe sur `date;description;montant` (compatibilité).
- `ImportError` porte **le format attendu et un exemple** dans son message, et le numéro de
  ligne.
- OFX : `FITID` lu et conservé ; migration `0018_bank_fitid` : `bank_transactions.fitid TEXT`
  + index unique partiel. Dédoublonnage : par `fitid` s'il existe, sinon par (date, montant,
  libellé) comme avant.
- `ParsedStatement { transactions, dialect, skipped: Vec<(line, reason)> }` pour l'aperçu.
- Nouvelle commande `DeleteBankTransaction` (`bank rm`, non rapprochée seulement,
  `requires_confirmation`).
- Proptest : ne panique jamais sur des octets arbitraires (existant, étendu aux octets) ;
  fixtures réelles anonymisées : Qonto, Shine, Boursorama, Crédit Agricole, BNP, LCL, La Banque
  Postale (colonnes documentées par chaque banque), OFX 1.x et 2.x.

### Façades

- CLI : `bank import <FICHIER>` sans `--format` obligatoire (sniffé ; `--format csv|ofx` reste
  forçable), `--dry-run` imprime un aperçu (dialecte détecté, nombre de lignes, cinq premières,
  doublons ignorés, lignes sautées), `bank import --help` documente les formats acceptés,
  `bank rm <ID>`.
- MCP : `bank.import` prend le contenu en base64 ou un chemin (IO d'adaptateur), `bank.delete`.
- Fenêtre : bouton « importer un relevé » dans `depenses` et `facturation` (multipart, patron
  du lot 29) → panneau d'aperçu (dialecte, lignes, doublons) → « importer » ; erreurs de format
  en langage courant avec l'exemple attendu. Le panneau explique où exporter le relevé dans les
  banques courantes (une ligne par banque).

### Acceptation

- Les sept fixtures s'importent sans option ; l'export Qonto de l'audit donne `✓ 17`.
- Le même relevé en CSV puis en OFX ne crée aucun doublon quand le FITID est présent.
- Un CSV tronqué ou binaire donne un message avec le format attendu, jamais une panique ni un
  `500`.

---

## Lot 39 — Premier lancement guidé dans la fenêtre

**Constats couverts** : 3, 17, 19 et les mineurs « onboarding », « aide de la console »,
« lexique sans point d'entrée », « aides `--help` lacunaires ». **Taille** : L.

### Cœur

- `freeflow-core::setup` (requête) : `setup_status(conn, today) -> SetupStatus` avec, pour
  chaque prérequis, `Done | Missing | Incomplete { fields }` : profil (nom, SIREN, adresse,
  **date de clôture**, **régime de TVA**, capital, associé unique), bilan d'ouverture (ou choix
  explicite « société nouvelle », colonne `opening_balance.declared_new_company` — migration
  `0019_setup`), relevé importé, et `next_step: SetupStep` avec un texte. C'est cette requête que
  le tableau de bord, la palette, le parcours et `freeflow setup status` consomment — jamais une
  heuristique de façade.
- Profil : migration `0019_setup` ajoute `president_name`, `sole_shareholder_name`,
  `sole_shareholder_address`, `share_count` (nécessaires au PV du lot 41 et à la 2033-F ; posés
  ici pour n'ouvrir le profil qu'une fois). `SetCompanyProfile` reste rétro-compatible ; la
  date de clôture et le régime de TVA restent optionnels dans le cœur, mais `setup_status` les
  marque `Incomplete` et le calendrier le dit en clair (« régime de TVA non renseigné : je
  suppose mensuel »).
- `AttachReceipt { expense_id, receipt }` : commande distincte d'`UpdateExpense`, **non soumise
  à la garde « exercice clôturé »** (une pièce ne change ni montant ni date ; la facture du
  cabinet arrive après la clôture).
- Justificatifs : stockés dans `<coffre>.receipts/` (un dossier **par coffre**) et **chiffrés
  sous une clé dérivée de la clé maître** (HKDF-SHA256 domaine `receipts`, XChaCha20-Poly1305,
  nom de fichier = hash du contenu en clair, comme aujourd'hui). Lecture : déchiffrement dans
  un fichier temporaire `0600` ouvert par le visualiseur par défaut. Migration des pièces
  existantes au premier déverrouillage (chiffrement en place, idempotent). C'est la promesse
  « un seul fichier chiffré » du README, ramenée à « le coffre et ses pièces, chiffrés ».

### CLI

- `freeflow setup status` (texte : liste à cocher ; `--json`), `freeflow company set-profile`
  avec les quatre champs nouveaux et un `long_about` qui explique chaque régime de TVA en une
  phrase ; `expense attach <RÉF> <FICHIER>` (ex-`edit --receipt`, conservé comme alias).
- `--help` complétés : syntaxe des lignes de bilan sur `year opening set --line`, valeurs de
  `--vat-rate`, formats de `bank import`.

### MCP

- `setup.status`, `expense.attach_receipt` (chemin local, IO d'adaptateur, comme
  `expense.update` au lot 21 — un agent ne fait pas l'archivage lui-même).

### Fenêtre

- **Assistant de première configuration** `/setup` en quatre écrans, ouvert automatiquement
  après la création du coffre tant que `setup_status.next_step` n'est pas `Done`, ré-ouvrable
  depuis la palette (« configurer ma société ») : *Ma société* (formulaire complet, aide par
  champ : « la date de clôture est sur vos statuts et votre dernier bilan », régimes de TVA
  expliqués, associé unique / président) ; *D'où venez-vous ?* (société existante → « demandez à
  votre cabinet la balance de clôture » et lien vers l'import du lot 40 ou le formulaire ;
  société nouvelle → `declared_new_company`) ; *Votre banque* (import du lot 38) ; *Prochaine
  étape* (lien vers le parcours de l'exercice en cours).
- **Écran « Ma société »** (`/view/societe`, onglet) : même formulaire, modifiable ; plus aucun
  renvoi « console : company set-profile » (`views/cloture.rs` pointe vers `/societe`).
- **Bandeau « prochaine étape »** en tête du tableau de bord tant que `setup_status` n'est pas
  `Done` ; les indicateurs de prospection passent dessous.
- **Messages en langage courant** : `AppError::BusinessRule` est rendu sans le préfixe « règle
  métier violée » ; une table `views/errors.rs` associe aux erreurs typées du cœur (exercice
  clôturé, bilan figé, révision périmée, montant verrouillé…) une phrase et, quand c'est
  possible, un **bouton** (« supprimer le projet de clôture », « recharger »). Toute mention
  « en CLI : … » ou « freeflow … » dans `views/` est remplacée (test : un `grep` des vues ne
  trouve plus `freeflow `). Les erreurs d'extraction axum (`400` bruts) passent par un
  gestionnaire de rejet en français.
- Lexique : entrée de palette « lexique », lien `?` dans l'en-tête ; console : `aide`, `help` et
  ligne vide répondent avec les commandes utiles du parcours, placeholder `year checklist 2026`.
- Écran `facturation` vide : bouton « émettre une facture » (panneau) au lieu du renvoi CLI.
  Émettre une facture depuis la fenêtre est **déjà** hors périmètre du scénario (aucune facture)
  mais le renvoi CLI, lui, viole la règle de ce lot : au minimum le texte change, au mieux le
  panneau existe (à arbitrer à l'ouverture du lot selon le temps restant).

### Acceptation

- Test fenêtre de bout en bout : coffre neuf → assistant → profil → « société existante » →
  parcours affiché avec la bonne clôture, sans jamais toucher la console.
- `grep -rn "freeflow " crates/freeflow-web/src/views` vide (hors console).
- Pièce jointe après approbation acceptée ; `receipts/` absent, `<coffre>.receipts/` chiffré
  (test : le fichier ne contient pas le `%PDF` en clair).

---

## Lot 40 — Reprise depuis Tiime, Indy ou le cabinet

**Constats couverts** : 5 et les mineurs « dispense d'acomptes affirmée sans référence ».
**Taille** : M.

### Cœur (`opening_balance::import`)

- `OpeningBalanceImport::from_balance_csv(bytes, opens_on)` : balance générale (colonnes
  compte / libellé / débit / crédit [/ solde], sniffée avec le dialecte du lot 38) →
  `OpeningBalance` : comptes de classes 1 à 5 tels quels ; comptes 6 et 7 **agrégés en résultat**
  (7 − 6) posé en 120 ou 129, seulement s'il n'y a pas déjà de 120/129 (une balance avant
  affectation porte le résultat en 6/7, une balance après affectation en 110/120 : les deux cas
  sont dits dans l'aide). Les à-nouveaux d'une balance d'exercice N (colonnes « à-nouveaux »
  parfois présentes) sont ignorés au profit des soldes.
- `OpeningBalanceImport::from_fec(bytes, opens_on)` : FEC de l'exercice précédent (18 colonnes,
  format déjà connu de `fec.rs`) → soldes des classes 1 à 5 au dernier jour + résultat dérivé
  des classes 6/7. Le FEC de Tiime et d'Indy est exporté en `|` ou tabulation : les deux sont
  acceptés.
- Résultat de l'import = `ImportPreview { lines, dropped: Vec<(compte, motif)>, derived_result,
  warnings }`, jamais une écriture directe : le cœur **prévisualise**, l'adaptateur soumet
  ensuite `RecordOpeningBalance` (même rail de confirmation qu'aujourd'hui).
- Détection à l'import : un compte 28x/29x → avertissement « FreeFlow ne calcule pas les
  amortissements : le résultat sera surestimé de la dotation annuelle » (jusqu'au lot 42).
- Migration `0020_opening_references` : `opening_balance.prior_corporate_tax_cents` (IS de
  l'exercice précédent, base des acomptes d'IS) et `prior_vat_due_cents` (TVA due N−1, base des
  acomptes 3514). `fiscal.rs` s'en sert pour la dispense d'acomptes du premier exercice au lieu
  de l'affirmer ; sans valeur, le calendrier dit « base inconnue, vérifiez ».

### Façades

- CLI : `year opening import <FICHIER> --opens-on … [--format balance|fec]` avec aperçu
  (`--dry-run` : tableau des lignes retenues, écartées, résultat dérivé) ; `--prior-is`,
  `--prior-vat` sur `year opening set`.
- MCP : `fiscal.import_opening_balance` (contenu ou chemin), aperçu puis action en attente.
- Fenêtre : dans le panneau du bilan d'ouverture, « importer une balance ou un FEC » (multipart)
  → aperçu avec les lignes retenues **dans le textarea** (modifiables avant enregistrement), les
  lignes écartées à part, le résultat dérivé expliqué. Aide : « demandez à votre cabinet la
  *balance de clôture* au 30/09/2025 ; dans Tiime : Comptabilité → Exports → FEC ; dans Indy :
  Documents → Export FEC » (à vérifier sur les produits au moment du lot).
- Lexique : « balance », « débit et crédit (et pourquoi l'actif est au débit) », « à-nouveaux ».

### Acceptation

- Balance de cabinet de 14 comptes (avec 6/7) → bilan d'ouverture équilibré, résultat dérivé en
  120, capitaux propres reconstitués ; FEC Tiime-like → même bilan.
- La reprise du scénario de l'audit se fait **sans** saisir une ligne à la main.

---

## Lot 41 — Conformité des documents et des déclarations

**Constats couverts** : 9, 10, 11, 12 et les mineurs « IS non arrondi », « CA12 sans crédit
repris », « échéances hors calendrier », « DSN ». **Taille** : L. Comme aux lots 26, 27 et
32, **chaque règle est vérifiée à la source** (notices Cerfa au `pdftotext`, BOFIP, Code de
commerce) avant d'être codée, et la source est citée dans le doc-comment.

### Liasse (`freeflow-docs/src/liasse.rs`)

- 2033-B : prestations en **218**, impôts et taxes en **244** (catégorie `taxes`), rémunération
  en 250 et charges sociales en 252 (à partir de `director_gross` / `director_cost`), IS en
  **306** ; suppression des pseudo-cases « 2065 / IS » et « 2065 / NET » au profit des seules
  cases réelles du 2065 (C1 résultat fiscal, et le cadre distributions si dividendes).
- 2033-F (composition du capital) depuis les champs du lot 39 ; 2033-C réservé au lot 42.
- Export : un `README` dans le JSON (`_notice`) qui dit ce qu'est chaque case, déjà en partie
  présent — conservé.

### PV et décision d'affectation (`freeflow-docs/src/minutes.rs`, `appropriation.rs`)

- Identité de l'associé unique (nom, adresse, nombre d'actions) et du président ; signature.
- Mention de l'art. 223 quater CGI (montant des dépenses et charges non déductibles, « néant »
  par défaut ; champ `non_deductible_expenses_cents` sur `CloseFiscalYear`, `serde(default)`).
- Conventions réglementées (L227-10) : « aucune convention nouvelle » par défaut, texte
  modifiable ; mention de l'inscription au registre des décisions (L227-9) ; dispense de rapport
  de gestion (L232-1 IV) si les seuils sont respectés.
- Rédaction d'une **perte** : « affecte la perte de X au report à nouveau, qui passe de A à B » ;
  plus de « total distribuable » sur un exercice déficitaire.
- Option **L227-9 al. 3** : le parcours et le PV signalent que, si l'associé unique est
  président, le dépôt des comptes signés au greffe dans les six mois vaut approbation sans PV —
  proposé comme voie simple, la génération du PV restant disponible.

### Calendrier et parcours

- `FiscalDeadlineKind::Das2` : cumul des honoraires (`fees`) **par bénéficiaire et par année
  civile** ; migration `0021_expense_supplier` : `expenses.supplier TEXT` (facultatif ; le
  parcours avertit si des honoraires n'ont pas de bénéficiaire). Seuil 1 200 € TTC (art. 240
  CGI), échéance selon BOI-BIC-DECLA-30-70-20 pour un exercice décalé.
- `FiscalDeadlineKind::Dividends2777` : dès qu'une affectation approuvée porte des dividendes,
  échéance au 15 du mois suivant la mise en paiement (date d'approbation par défaut), montant
  = PFU 12,8 % + prélèvements sociaux 17,2 % ; texte qui dit que la société retient et verse.
- Nouvelle étape du parcours **`Vat`** (phase « déclarer ») : TVA collectée, déductible,
  crédit ou dette de l'exercise, déclarations dues sur la période (« 12 CA3 mensuelles, dont
  11 à néant ») ; le régime réel simplifié y montre acomptes et CA12 E. Rappel que la CA3
  « néant » se dépose même sans activité, et de la demande de remboursement de crédit (3519)
  possible au-delà de 150 € sur la CA12.
- `fiscal.rs` : pour un exercice décalé, la première CA3 après la suppression du RSI est celle
  du premier trimestre **de l'exercice ouvert après le 1er janvier 2027** (borne
  `earliest_period` calée sur la clôture réelle, testée sur 30/06, 30/09 et 31/03) ; la CA12
  intègre le crédit repris à l'ouverture (445670).
- `accounting::corporate_income_tax` arrondi à l'euro (art. 1657 CGI), avec mise à jour des
  chiffres attendus des tests existants (posés à la main).
- Échéances ajoutées à titre de rappel : relevé de solde 2572 même à zéro ; déclaration de
  confidentialité des comptes (L232-25) au dépôt ; registre des décisions à tenir.
- DSN : le montant affiché devient les cotisations estimées (coût − brut), pas le coût total, et
  l'aide de `--director-charge-ratio` précise « charges patronales + salariales / brut ».

### Acceptation

- Liasse de l'audit : 218 = 0 (aucun CA) puis 218 = CA 2027, 244 = 227 €, 306 = IS, 2033-F
  présent ; `pdftotext` du PV contient le nom de l'associé, « 223 quater », « registre des
  décisions ».
- CA3/CA12 pour clôture 30/09 sur 2026-2028 : aucune période déclarée deux fois (proptest sur
  les clôtures : l'union des périodes CA3 et CA12 est une partition).
- DAS2 : 600 € en octobre 2025 + 900 € en mars 2026 au même cabinet → **aucune** alerte (le
  cumul se fait par année civile : 600 € en 2025, 900 € en 2026, aucun ne dépasse 1 200 €) ;
  second cas posé à la main avec 1 300 € sur 2026 → alerte et échéance DAS2.

---

## Lot 42 — Amortissements minimaux et charges constatées d'avance

**Constat couvert** : 18. **Taille** : M. **Après tout le reste** : le scénario du brief n'a
pas d'immobilisation, mais un bilan de cabinet ordinaire en a une ; le lot 40 avertit déjà.

- Table `fixed_assets` (migration `0022`) : libellé, compte 2xx, date de mise en service, base,
  durée en mois, mode linéaire seul, amortissements cumulés repris à l'ouverture (28x). Créée
  automatiquement à l'import du lot 40 quand un couple 2xx/28x est présent (durée demandée).
- Dotation de l'exercice : écriture `OD` 681 / 28x au dernier jour, prorata temporis le premier
  et le dernier exercice ; `compute_result` la compte ; case 254 du 2033-B ; tableau 2033-C.
- Charge constatée d'avance reprise (486) : extournée au premier jour (`OD` 6xx / 486).
- Seuil d'immobilisation : une dépense `equipment` ≥ 500 € HT propose « immobiliser » au lieu de
  passer en charge (art. 39 A, tolérance 500 € HT).
- Façades : `asset list|add|rm`, `fiscal.assets`, panneau dans `depenses`.

---

## Transversal, à chaque lot

- **Scénario de preuve** : `closing_scenario.rs` est étendu au fil des lots (36 : clôture
  prématurée refusée ; 37 : dettes reprises soldées ; 38 : relevé Qonto ; 39 : pièce après
  clôture ; 40 : import de balance ; 41 : PV nominatif, DAS2, TVA) ; un scénario fenêtre
  (`freeflow-web/tests/first_launch.rs`) suit le débutant depuis `/setup`.
- **README** : la section « Clôturer seul son exercice, pas à pas » est réécrite avec les gestes
  de la fenêtre en premier et la CLI en équivalent ; « Limites connues » mise à jour ; l'audit
  est référencé (`docs/audits/`).
- **CLAUDE.md** : une entrée par lot dans l'historique, et un pointeur vers `docs/audits/`.
- **Snapshots** : `--help`, liste d'outils MCP, XML CII inchangé.

## Ordre, dépendances et ce qui peut se paralléliser

```
36 base saine ──► 37 règlement de comptes ──► 38 import bancaire ──► 40 reprise balance/FEC
                                    │                                       │
                                    └──► 39 premier lancement guidé ◄───────┘ (l'assistant
                                                     │                        branche 38 et 40)
                                                     └──► 41 conformité ──► 42 amortissements
```

- 37 dépend de 36 (dates ISO, sorties lisibles pour tester) ; 38 dépend de 37 (un relevé
  importé doit pouvoir régler un compte de bilan) ; 40 réutilise le sniffer de 38.
- 39 peut démarrer après 36 en parallèle de 37-38, mais son écran « Votre banque » et « D'où
  venez-vous ? » ne sont branchés qu'une fois 38 et 40 livrés.
- 41 dépend des champs de profil de 39 (identité) et de la catégorie `taxes` de 37.
- Sur cette machine, « parallèle » veut dire deux sessions qui ne compilent jamais en même temps
  (voir la mémoire « machine ancienne »).

## Ce que ce plan ne fait pas, dit tel quel

- Pas de suivi du **paiement** de l'IS, des acomptes ni de la créance de report en arrière
  (formulaire 2573) : le domaine n'a toujours pas de fait « impôt payé » autre que le règlement
  bancaire du lot 37, qui solde un 444 sans le rapprocher d'une échéance.
- Pas de rapprochement « un débit pour plusieurs dépenses » (relevé de carte mensuel).
- Pas de dépôt électronique (EDI-TDFC, guichet unique du greffe) : l'application produit les
  fichiers et dit où les déposer, elle ne se connecte à rien (promesse « aucune connexion
  sortante »).
- Pas de rapport de gestion ni de PV pour une SAS à plusieurs associés : périmètre SASU/EURL.
- Le chiffrement des justificatifs (lot 39) ne couvre pas le fichier temporaire ouvert par le
  visualiseur, qui vit le temps de la consultation.
