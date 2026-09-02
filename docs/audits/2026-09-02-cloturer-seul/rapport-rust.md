# Rapport d'audit — relecteur « senior Rust » (fonctionnel, robustesse, cohérence des adaptateurs)

Dépôt : `/path/to/griffe` au commit `e58409f` (lot 35). Binaires `target/debug/{freeflow,freeflow-web-dev,freeflow-mcp}`.
Coffres de l'audit (dossier de travail) : `rust.db` (premier passage, état volontairement dégradé conservé
comme preuve), `rust2.db` (rejeu propre du scénario, script `replay.sh`), `rust3.db` (cas limites).
Relevés de test : `releves/`. Documents produits : `docs2/`. Fenêtre testée via `freeflow-web-dev`
sur `127.0.0.1:3423` (arrêté à la fin). Aucune modification du dépôt. Aucun `cargo`/`nix`/test lancé.

Date « du jour » du scénario : 2026-09-02 (c'est aussi la date réelle de la machine, ce qui compte :
plusieurs commandes ne prennent pas de `--today`).

## 1. Verdict

**Pas prêt pour ce scénario** (débutant guidé *dans* l'application, reprise depuis Tiime/Indy, relevé de
banque française). Le cœur de la chaîne de clôture est solide et cohérent (parcours, gardes, bilan,
documents, FEC, audit, sauvegarde) — mais l'utilisateur cible bute **avant** d'y arriver : aucun export
bancaire réel n'est importable (seul un CSV « maison » non documenté passe), la fenêtre n'a ni écran de
profil ni import de relevé, et plusieurs sorties « humaines » sont des dumps `Debug` Rust. S'y ajoutent
des trous de robustesse réels (coffre à moitié créé par `init`, clôture/approbation d'un exercice non
écoulé sans retour possible, TVA déductible non bornée, `PieceRef` FEC en collision).

## 2. Constats

Format : **sévérité — titre** ; où ; reproduction ; attendu ; preuve.

### BLOQUANT

**B1 — L'import de relevé n'accepte aucun export de banque française ; le format attendu n'est pas documenté.**
Où : `freeflow bank import` ; `crates/freeflow-core/src/billing/import.rs:26-73` ; `crates/freeflow-cli/src/invoice.rs:334` (`read_to_string`).
Repro (fichiers dans `releves/`) :
```
$ freeflow bank import --format csv releves/qonto.csv          # virgule, en-tête anglais (export Qonto)
✗ ligne 2 : attendu 3 champs, trouvé 1                        exit=4
$ freeflow bank import --format csv releves/qonto2.csv         # ; mais 5 colonnes
✗ ligne 2 : attendu 3 champs, trouvé 5                        exit=4
$ freeflow bank import --format csv releves/banque-fr-latin1.csv   # latin-1, Débit/Crédit séparés, JJ/MM/AAAA
✗ erreur inattendue : lecture de … impossible : stream did not contain valid UTF-8   exit=1
$ freeflow bank import --format csv releves/bom-jjmmaaaa.csv   # UTF-8 BOM, JJ/MM/AAAA
✗ ligne 2 : date invalide : the 'year' component could not be parsed   exit=4
$ freeflow bank import --format csv releves/milliers.csv       # « 1 200,00 » et « ; » dans le libellé
✗ ligne 2 : attendu 3 champs, trouvé 4
```
Seul `releves/app.csv` (`date;description;montant`, dates ISO) passe (`✓ 6`). `bank import --help` dit
seulement « Importe un relevé bancaire (CSV ou OFX) » : les colonnes attendues ne sont écrites nulle part
dans l'app. Le README (§ « Année 1 », étape 3) promet « Importe le relevé (CSV ou OFX exporté de ta
banque) ». Confirme le constat du premier relecteur (« trouvé 1 » = séparateur virgule).
Attendu : détection du séparateur (`;`/`,`), de la décimale, du format de date `JJ/MM/AAAA`, des colonnes
débit/crédit, de l'encodage (latin-1/CP1252, BOM), ou au minimum un message qui dit quel format produire.
Note : l'OFX 1.x SGML et un OFX 2 XML minimal passent (`releves/releve.ofx`, `releves/v2.ofx`, `--dry-run` OK).

**B2 — La fenêtre ne permet ni de renseigner le profil d'entreprise ni d'importer un relevé : les étapes 1 et 3 du pas-à-pas exigent la ligne de commande.**
Où : `crates/freeflow-web/src/lib.rs` (aucune route `company`), `views/cloture.rs:392-394`
(`ClosingStepKey::Profile` → lien vers `/view/console` « renseigner le profil (console : company set-profile) »).
Repro : `GET /cloture/checklist?period=2026` sur un coffre vide → l'étape « Profil d'entreprise » renvoie
vers la console, où il faut taper une commande à 12 options (`--siren`, `--fiscal-year-end 30/09`,
`--vat-regime real_simplified`, …). Idem pour le relevé : « bank import reste CLI/MCP » (dit dans le panneau).
Et la console rend `company show` en `Debug` (voir M3) : `Some(CompanyProfileWithVatFiling { profile: CompanyProfile { … siren: Siren([9, 0, 1, 2, 6, 5, 3, 2, 2]) …`.
Attendu pour « guidé DANS l'application, sans documentation externe » : un formulaire de profil et un
import de fichier dans la fenêtre.

### MAJEUR

**M1 — `freeflow init --db nom.db` (chemin sans répertoire) échoue et laisse un sidecar orphelin ; le coffre est ensuite ni créable ni ouvrable.**
Où : `crates/freeflow-core/src/store/kdf.rs:808-809` (`fs::rename` puis `sync_dir(path.parent())` — `Path::new("nom.db.kdf").parent()` vaut `Some("")`, `File::open("")` → ENOENT, après que le sidecar a été renommé en place).
Repro (dans le dossier de travail) :
```
$ freeflow --db rust.db --passphrase-file pass.txt init
✗ erreur de coffre : impossible d'accéder au fichier du coffre : No such file or directory (os error 2)   exit=3
$ ls rust.db*                       → rust.db.kdf (133 o), pas de rust.db
$ freeflow --db rust.db --passphrase-file pass.txt init
✗ erreur de coffre : un coffre existe déjà à rust.db   exit=3
$ freeflow --db rust.db --passphrase-file pass.txt company show
✗ aucun coffre à rust.db — lancez `freeflow init` pour en créer un   exit=7
$ freeflow --db rust.db vault status
coffre rust.db (sidecar v3) — verrouillé
```
`--db ./rust.db` ou un chemin absolu fonctionne. Attendu : `init` réussit avec un nom nu (ou refuse
*avant* d'écrire), et un échec ne laisse jamais un demi-coffre. C'est la toute première commande du scénario.

**M2 — On peut clore un exercice non écoulé, puis l'approuver à une date future (ou après le délai légal), sans aucun retour possible.**
Où : `fiscal_year.rs` (`CloseFiscalYear`/`ApproveFiscalYear`, aucune garde sur `today`) ; CLI `year close`, fenêtre `POST /cloture` et `POST /cloture/{id}/approve`.
Repro CLI (`rust.db`, 2026-09-02) :
```
$ freeflow year close --period 2027          → ✓ FiscalYearId(01a06376-d055-…)   (exercice 2026-10-01 → 2027-09-30, pas écoulé)
$ freeflow year approve 2026 --approved-on 2027-06-01   → ✓ 2   (six mois après le 30/09/2026 = 30/03/2027 : hors délai, accepté sans mot)
$ freeflow year close --starts-on 2025-10-01 --ends-on 2035-09-30   → ✓ (période de dix ans, rust3.db)
```
Repro fenêtre (`rust3.db`, exercice 2026 en projet) : `POST /cloture starts_on=2026-10-01&ends_on=2027-09-30` → `200` + `HX-Trigger: freeflow:saved` ; puis `POST /cloture/<id>/approve approved_on=2028-12-31` → `200` + `freeflow:saved` ; `year list` : `2027-09-30 … approuvé le 2028-12-31`, alors que 2026 est encore « projet » (un projet dont l'affectation peut encore être amendée sous un successeur approuvé immuable). Confirme le premier relecteur.
Le parcours, lui, dit « bloquant » sur « Exercice écoulé » — mais le README affirme qu'« une étape bloquante est un refus que le cœur opposerait à la clôture » : faux pour celle-ci.
Conséquence : un exercice approuvé est immuable (`year rm`/`amend` refusés, testé), il n'existe pas de
`unapprove`, et le seul filet est la sauvegarde automatique — **tous les 7 jours** (`freeflow-cli/src/lib.rs:144`
`AUTO_BACKUP_MAX_AGE = 7 j`) : dans `rust.db`, après 20 minutes de saisie, l'unique sauvegarde est celle
de l'`init`, vide. Attendu : refus (ou confirmation explicite) d'une clôture avant `ends_on`, refus d'une
date d'approbation > aujourd'hui, avertissement au-delà des six mois, et une sauvegarde forcée avant
`approve`.

**M3 — Une dizaine de sorties « humaines » sont des dumps `Debug` Rust (CLI et console de la fenêtre).**
Où : `crates/freeflow-cli/src/output.rs:28` (`format!("✓ {v:?}")`) et `:42` (`format!("{value:?}")`).
Preuves observées :
```
company show           → Some(CompanyProfileWithVatFiling { profile: CompanyProfile { name: "Nicolas Dev", … siren: Siren([9, 0, 1, 2, 6, 5, 3, 2, 2]), … share_capital: Some(Money(100000)) … })
company show (vide)    → None
company set-profile    → ✓ ()
fiscal calendar        → [Object {"amount_cents": Null, "due_on": String("2026-09-15"), "kind": String("is_acompte"), …
audit verify-chain     → Object {"status": String("intact")}
pending list           → [PendingAction { id: PendingActionId(…), command_json: "{\"starts_on\":[2025,274],…" }]
expense show           → ExpenseDetail { expense: Expense { … amount: Money(60000) … } }
bank unreconcile       → ✓ None        expense reconcile → ✓ ()
year opening set       → ✓ 1  puis  ✓ 2 (numéro de révision nu)
year close / confirm   → ✓ FiscalYearId(01a06379-7725-…)
```
Confirme le premier relecteur, et la liste est plus longue que `company show`/`fiscal calendar`. Les
formes `--json` sont, elles, propres.

**M4 — Contrat JSON incohérent pour les dates : `[année, jour ordinal]` dans plusieurs sorties, y compris la liasse remise au cabinet.**
Où : types `time::Date`/`OffsetDateTime` sérialisés sans `serde(with…)` : `BankTransaction`, `Expense`,
`PendingAction.command_json`, `liasse_export`.
Preuves : `bank list --json` → `"occurred_on": [2026, 181]` ; `expense show --json` → `"incurred_on": [2025, 344]`,
`"created_at": [2026, 245, 18, 48, 53, 668333219, 0, 0, 0]` ; `docs2/liasse.json` → `"period_start": [2025, 274], "period_end": [2026, 273]` ;
`pending list --json` → `command_json … "starts_on":[2025,274]`. Alors que `year show --json`,
`year checklist --json`, `fiscal calendar --json` donnent `"2026-09-30"`. Le serveur MCP partage ces types
(`expense.show`, `bank.list`, ressources) : un agent reçoit les deux formats selon l'outil.

**M5 — Dédoublonnage d'import fragile, `FITID` ignoré, et aucun moyen de supprimer une transaction importée.**
Où : `store/migrations/0004_billing_up.sql:20` (`UNIQUE(occurred_on, amount_cents, description)`), `billing/row.rs:245-255` (`INSERT OR IGNORE`), `import.rs:99-101` (`NAME` *ou* `MEMO`, jamais `FITID`) ; `freeflow bank --help` : `import|list|reconcile|unreconcile`, pas de `rm`.
Repro : import `app.csv` (`✓ 6`), ré-import (`✓ 0`, bien) ; puis `releve.ofx` des mêmes opérations (libellé `NAME` sans `MEMO`) → `✓ 2` : deux doublons `PRLV SEPA QONTO` / `VIR CABINET EXPERTISE` « à rapprocher » à jamais. Le parcours affiche ensuite « ! 2 non rapproché(s) sur 8 ». Attendu : dédup sur `FITID` en OFX, et un `bank rm`/`bank import --undo`.

**M6 — La TVA déductible n'est bornée que par le TTC, pas par la TVA contenue.**
Où : `crates/freeflow-core/src/expenses.rs:132-134` (`vat_deductible > amount` seulement).
Repro : `expense record --transaction <600 €> --label "Honoraires cabinet — mars" --category fees --vat-rate standard --vat-deductible 150` → `✓ ExpenseId(…)`. Sur 600 € TTC à 20 %, la TVA contenue est 100 €. La charge HT devient 450 €, le 445660 et la CA12 sont faux ; `--vat-deductible 700` est bien refusé (« ne peut pas dépasser le montant total »). Attendu : `vat_deductible ≤ amount × taux/(1+taux)` (le taux est saisi).

**M7 — FEC : `PieceRef` identique pour des dépenses différentes.**
Où : `ledger.rs` (pièce d'une dépense sans justificatif = `DEP-` + 8 premiers hex de l'UUID v7, qui sont un *horodatage*).
Preuve `docs2/901265322FEC20260930.txt` : cinq dépenses distinctes portent `DEP-01A06378` (toutes créées la même minute). Une dépense avec justificatif porte le nom de fichier complet (`a5321204…-facture-cabinet-dec.pdf`). Le FEC exige une référence de pièce identifiante (art. A. 47 A-1 LPF). Attendu : un numéro de pièce unique par dépense (ou l'UUID complet).

**M8 — Justificatifs : impossibles à joindre après clôture, et stockés en clair dans un dossier partagé entre coffres.**
Où : garde « exercice clôturé » sur `UpdateExpense` (`expenses.rs`) ; `freeflow-cli/src/expense.rs:107-134` (`<répertoire du coffre>/receipts/`).
Repro : après `year close --period 2026` (projet), `expense edit "Honoraires cabinet — mars" --receipt facture.pdf` → `✗ la dépense du 2026-03-12 tombe dans un exercice déjà clôturé …` (exit 4). Joindre une pièce ne change ni montant ni date : la refuser oblige à `year rm` puis re-clore ; après approbation, plus jamais. Or la facture du cabinet arrive typiquement après la clôture.
Et `ls receipts/` du dossier de travail montre les justificatifs des coffres `ec.db`/`freelance.db` des autres relecteurs à côté des miens, en clair (`-rw-------`) — le README promet « un seul fichier chiffré ».

**M9 — Un débit du relevé ne peut être qu'une charge : le scénario cible produit un double comptage, et les comptes repris à l'ouverture ne sont jamais soldés.**
Où : modèle `expenses`/`ledger.rs` (dépense rapprochée = 6xx/445660 contre 401 puis 401/512 ; rien d'autre n'explique un débit).
Repro (`rust2.db`) : bilan d'ouverture du cabinet avec `401000 Fournisseurs C 360,00` (honoraires du bilan 2025, facturés au 30/09, payés le 10/12/2025) et `444000 IS C 225,00`. Le débit `VIR CABINET EXPERTISE HONORAIRES −360` du 2025-12-10 ne peut être saisi que comme dépense `fees` → la charge est comptée deux fois (une fois par le cabinet en N−1 via 401, une fois ici en 622600), le 401 repris reste au bilan (`166 Fournisseurs 360,00 €` au 30/09/2026, balance : `401000 … 999,50 | 1 359,50 | solde C 360,00`), le résultat est minoré de 300 € HT. Même impasse pour le paiement du solde d'IS (444), de la TVA, de la CFE, d'un apport en compte courant, de dividendes : aucune catégorie « ce n'est pas une charge ». Le bilan « colle au relevé » (promesse du README) seulement tant que rien de tout cela ne passe en banque. Le `120000` repris (1 500 €) est réputé affecté en report (documenté), mais le compte reste tel quel dans la balance de clôture.
En plus : une dépense datée hors de tout exercice (`--incurred-on 2025-09-15`, avant le bilan d'ouverture) est acceptée (`✓`) et n'apparaît ni dans le parcours ni dans le bilan — disparition silencieuse d'une saisie erronée.

### MINEUR

**m1 — Doc vs binaire : `freeflow confirm <id>`** (README l.259/316, CLAUDE.md l.26) mais le binaire exige `--id` : `freeflow confirm 01a0…` → `error: unexpected argument … Usage: freeflow confirm [OPTIONS] --id <ID>` (exit 2).

**m2 — `fiscal calendar`, `fiscal deadlines`, `forecast show` exigent `--today`** (`error: the following required arguments were not provided: --today`) alors que `year checklist` le déduit — incohérent pour un débutant qui suit le pas-à-pas.

**m3 — `today` est pris en UTC** (`freeflow-cli/src/year.rs:787` `OffsetDateTime::now_utc().date()`, idem fenêtre/MCP) : à Paris le 1ᵉʳ octobre entre 00:00 et 02:00 le parcours dit encore « exercice en cours » ; les échéances « dépassée » basculent avec deux heures de retard.

**m4 — `closing.rs:403` `prior_chain(conn, exercise.start()).ok()`** avale l'erreur : si la chaîne est incohérente, la dotation minimale est simplement absente, sans explication.

**m5 — `--dry-run` n'annonce rien** : `bank import --dry-run` → `(dry-run) aucune écriture` sans le nombre de lignes lues ni les doublons ; `year close --dry-run` sans le résultat qui serait figé (CLAUDE.md : « montre ce qui serait fait »).

**m6 — Fenêtre : messages bruts d'axum en anglais** : `GET /cloture/checklist?period=abcd` → `400 Failed to deserialize query string: period: invalid digit found in string` ; `POST /depenses` non multipart → `400 Invalid boundary for multipart/form-data request` ; `?period=1900` rend un parcours « 1899-10-01 → 1900-09-30 ». (Aucun `500` observé : identifiants invalides/introuvables → texte lisible, bons points.)

**m7 — Débordement `Money` atteignable → panique** : `expense record --amount 60000000000000000` deux fois, puis `year checklist 2026` / `year balance 2026` → `thread 'main' panicked at crates/freeflow-core/src/domain/money.rs:315:14: attempt to add with overflow` (exit 101 ; fenêtre → `500` via `CatchPanicLayer`). Montants absurdes, mais `Money::parse_decimal` les accepte et le parcours devient inutilisable jusqu'à `expense rm`.

**m8 — `year render 2026 liasse --out x.pdf`** écrit du JSON dans un `.pdf` sans avertir ; `year render` écrase un fichier existant en silence (assumé côté CLI, mais aucune mention).

**m9 — Approbation hors délai non signalée** : approuvé le 2027-06-01 (délai : 2027-03-30), le parcours affiche `✓ Approbation des comptes (échéance 2027-06-01)` — l'échéance prend la valeur de la date d'approbation.

**m10 — Sans profil, le parcours suppose l'année civile** et affiche des échéances (solde d'IS 2027-05-15, AG 2027-06-30…) qui seront fausses pour une clôture au 30/09 ; « bloqué » sur le profil, mais les dates sont quand même montrées.

**m11 — Dashboard : `capacite_2026-09  -0%`** (zéro négatif).

**m12 — `year opening set` accepte une date décalée** (`--opens-on 2025-10-02` → `✓ 1`) et ne bloque qu'à la clôture — cohérent avec la doc, mais un avertissement immédiat éviterait de découvrir le problème un an plus tard.

### SUGGESTION

- **S1** Import : sniffer séparateur/décimale/date/encodage, colonnes débit/crédit, `FITID` ; `bank rm` ; aperçu en `--dry-run`.
- **S2** « Expliquer un débit » autrement qu'en charge : règlement d'une dette reprise (401/444/455), TVA/IS/CFE payés, compte courant, dividendes ; signaler les faits datés hors de tout exercice.
- **S3** Fenêtre : formulaire de profil, import de relevé (le multipart existe déjà pour les justificatifs).
- **S4** `approve` : refuser une date future, avertir au-delà de six mois, sauvegarde forcée avant ; `year unapprove` derrière confirmation humaine (ou au moins `backup create` automatique).
- **S5** Supprimer le repli `{:?}` d'`output.rs` (un rendu texte par type) ; `serde` ISO 8601 partout.
- **S6** Autoriser `--receipt` sur une dépense d'un exercice clos (la pièce n'entre pas dans le snapshot).

### Point 2 — MCP (en lecture, contrat)

Le binaire `freeflow-mcp` n'ouvre le coffre **que** via une session du trousseau OS (`main.rs:22-28` :
« lancez `freeflow unlock --remember` ») — interdit par le brief, donc non lancé. Vérifié dans
`crates/freeflow-mcp/src/tools/{company,fiscal,expenses}.rs` et `tests/mcp_integration.rs` :
- `company.set_profile` (`share_capital_cents`, `director_charge_ratio_bps`), `fiscal.set_opening_balance`
  (`lines: ["compte:libellé:D|C:montant"]`, `tax_losses_cents`), `expense.record` (`amount_cents`,
  `vat_deductible_cents`, `bank_transaction_id`), `fiscal.close_year` (`period` ou `starts_on`/`ends_on`,
  `legal_reserve_cents`, `dividends_cents`, `carry_back`, `dry_run`) : entrées en **centimes** là où la CLI
  prend des **euros** (`--legal-reserve 500`, `--share-capital 1000`) et un ratio en bps là où la CLI prend un
  pourcentage — cohérent à l'intérieur du MCP, documenté dans chaque schéma, mais à connaître.
- `fiscal.close_year` renvoie `{"status":"pending_confirmation","pending_action_id":…}` (test l.251), et
  la confirmation n'existe qu'en CLI/fenêtre (`pending.confirm` retiré, bon choix) — vérifié côté CLI :
  `--actor agent:audit year close --period 2026` → `pending_confirmation`, `confirm --id …` → `✓`,
  seconde confirmation → `✗ action en attente introuvable ou déjà résolue` (exit 5).
- Les sorties partagent les types de la CLI : `expense.show`/`bank.list`/ressources héritent du format
  de date `[année, jour]` (M4).

### Point 4 — Écart README/CLAUDE.md ↔ observé

| Promesse | Observé |
|---|---|
| « Importe le relevé (CSV ou OFX exporté de ta banque) » | Aucun CSV de banque réelle accepté (B1) |
| « guidé DANS l'application, sans documentation externe » | Profil et import de relevé impossibles dans la fenêtre (B2) ; format CSV non documenté |
| « un seul fichier chiffré sur ton disque » | `receipts/` en clair, partagé entre coffres (M8) |
| « une étape bloquante est un refus que le cœur opposerait à la clôture » | Vrai pour profil/bilan d'ouverture, faux pour « exercice écoulé » (M2) |
| `freeflow confirm <id>` | `--id` obligatoire (m1) |
| « sauvegarde automatique silencieuse … à chaque commande » | Une par 7 jours (`AUTO_BACKUP_MAX_AGE`) — exact mais trompeur comme filet avant `approve` |
| « le bilan colle au relevé » | Seulement si chaque débit est une charge (M9) |

## 3. Ce qui fonctionne bien

- **Parcours de clôture** : bascule correcte « en cours » → « prêt à clore » au 1er octobre (`--today`),
  seize étapes lisibles, commande exacte suggérée, bilan d'ouverture décalé bloqué exactement là où
  `year close` refuse (`✗ le bilan d'ouverture est daté du 2025-10-02 …`), lexique clair.
- **Gardes du cœur** : bilan déséquilibré, compte de charge, compte en double, montant négatif, période
  chevauchante, `HasSuccessor` (« corriger la chaîne se fait du plus récent au plus ancien »), dividendes >
  distribuable, AG avant clôture, dépense dans un exercice clos (create/edit/rm), bilan d'ouverture figé
  après clôture, rapprochement au montant exact, débit déjà rapproché, transaction déjà rapprochée —
  tous avec des messages en français utiles et des codes de sortie distincts (2/3/4/5/7).
- **Validation d'identité** : SIREN (Luhn) et clé de TVA vérifiés (c'est mon calcul qui était faux).
- **Chaîne comptable cohérente** : résultat −839,50 €, bilan 4 995,50 € des deux côtés, mêmes chiffres dans
  PV, affectation, synthèse, bilan PDF, liasse, FEC (13 écritures équilibrées, à-nouveaux `AN`,
  401 → 512 à la date du relevé) ; report à nouveau 3 900 → 3 060,50 ; déficit reportable 839,50 hérité par
  le parcours 2027 ; 29/02 géré.
- **Audit, sauvegarde, idempotence** : `verify-chain` intact partout ; `backup create --out`/`restore --from/--to`
  fonctionnent ; ré-import identique → `✓ 0` ; `--dry-run` n'écrit jamais (justificatif compris).
- **Fenêtre** : aucun `500`, formulaires re-rendus avec erreurs de champ, documents téléchargeables,
  console = même chemin que la CLI (`backup restore` bien refusé : « lancez-la depuis un terminal »).
- Résolution de référence par libellé avec liste des candidats en cas d'ambiguïté.

## 4. Recommandations priorisées

1. **Import bancaire réel** (B1, M5) : sniffing de format + encodage + `FITID` + `bank rm`. Sans ça, l'étape 3 du pas-à-pas est morte pour l'utilisateur cible.
2. **`init` avec un nom nu** (M1) : corriger `sync_dir("")` et rendre la création atomique (ou nettoyer le sidecar à l'échec).
3. **Clôture/approbation** (M2) : garde `ends_on ≤ today`, `approved_on ≤ today`, avertissement six mois, sauvegarde forcée avant `approve`, chemin de retour.
4. **Fenêtre** (B2) : écran profil + import de relevé.
5. **Sorties** (M3, M4) : plus de `{:?}` en mode texte ; dates ISO dans tous les JSON (liasse en premier).
6. **Justesse comptable** (M6, M7, M9) : borne de TVA déductible par le taux ; `PieceRef` unique ; « débit qui n'est pas une charge » et alerte sur les faits hors exercice.
7. **Justificatif après clôture** (M8) et chiffrement/isolement de `receipts/`.
8. Mineurs : `confirm <id>`, `--today` par défaut, `today` en heure locale, messages bruts de la fenêtre, overflow `Money`.
