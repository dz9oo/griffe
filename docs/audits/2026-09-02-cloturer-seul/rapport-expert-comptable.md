# Rapport d'audit — rôle expert-comptable (SASU à l'IS, clôture décalée au 30/09)

Coffre rejoué : `ec.db` (dossier de travail), scénario du brief exécuté intégralement par la CLI
(`target/debug/freeflow`), documents produits dans `docs/`, copies de coffre `naif.db` et `test27.db`
pour les contre-épreuves. Date machine : 2 septembre 2026. Binaires du commit `e58409f` (lot 35).
Aucune modification du dépôt.

Jeu de données rejoué (toutes les valeurs attendues ont été posées à la main avant vérification) :

- Profil : Nova Dev, SASU, SIREN 889112348 (Luhn OK), TVA FR16889112348, Nantes (44), capital
  1 000 €, clôture 30/09, **réel simplifié de TVA**, président non rémunéré.
- Bilan d'ouverture au 01/10/2025 « remis par le cabinet » (8 comptes) : 101000 C 1 000 ; 106100
  C 100 (réserve légale déjà au plafond) ; 110000 C 6 350 ; 401000 C 600 (facture du cabinet de
  septembre 2025, payée en octobre) ; 444000 C 1 200 (solde d'IS 2025, payé le 15/01/2026) ;
  455000 C 500 (compte courant d'associé) ; 445670 D 210 (crédit de TVA) ; 512000 D 9 540.
- Relevé bancaire 01/10/2025 → 30/09/2026 (17 lignes, CSV) : 12 × −12,50 frais de compte ;
  −600 (cabinet, dette reprise) ; −227 CFE 2025 ; −1 200 (solde IS 2025, dette reprise) ;
  −900 honoraires 2026 (750 HT + 150 TVA) ; −119,88 OVH (99,90 HT + 19,98 TVA). Aucune facture
  émise. Solde bancaire réel au 30/09/2026 : **6 343,12 €**.
- Attendu à la main : charges de l'exercice 750 + 150 + 99,90 + 227 = **1 226,90 €** ; résultat
  −1 226,90 ; IS 0 ; déficit reportable 1 226,90 ; report à nouveau après affectation
  6 350 − 1 226,90 = **5 123,10 €** ; TVA déductible 169,98 ; 401 et 444 soldés à zéro ; banque
  6 343,12.

## 1. Verdict

**Pas prêt pour ce scénario.** L'arithmétique du cœur est juste (résultat, déficit, report à
nouveau, plafond de réserve légale, FEC équilibré), mais dès que le bilan du cabinet contient une
dette ou une créance réglée pendant l'exercice — le cas normal : honoraires de septembre payés en
octobre, solde d'IS de janvier, TVA — FreeFlow n'offre **aucun** moyen de la solder : le bilan de
clôture, le 2033-A et le FEC sont faux (banque surestimée de 1 800 €, dettes fantômes), et le
parcours guidé pousse le débutant vers une saisie en dépense qui fausse le résultat de 1 700 €.
S'y ajoutent des erreurs de conformité (case 210 du 2033-B au lieu de 218, PV sans associé
identifié ni mention de l'art. 223 quater CGI, DAS2 et 2777 absentes, CA3 en doublon de la CA12 E
en 2027) et une clôture acceptée avant la fin de l'exercice.

## 2. Constats

### BLOQUANT

#### B1 — Une dette ou une créance reprise au bilan d'ouverture ne peut jamais être soldée : bilan de clôture, 2033-A et FEC faux

- **Où** : `freeflow-core/src/ledger.rs` (aucune écriture possible hors 401 d'une dépense
  rapprochée), `expenses::rapprochable_debit` (un débit du relevé ne se rapproche que d'une
  dépense), `closing.rs:776` (étape « rapprochement bancaire »).
- **Reproduction** : bilan d'ouverture ci-dessus, `bank import`, dépenses créées depuis le relevé
  pour les 15 lignes qui en sont, puis `year balance 2026` et `year checklist 2026 --today
  2026-10-02`.
- **Attendu** : pouvoir affecter le débit de −600 € au compte 401 et celui de −1 200 € au compte
  444 (règlement d'une dette existante, sans charge) ; banque 6 343,12 € ; 401 = 0 ; 444 = 0.
- **Observé** :
  ```
  512000  Banque   9 540,00 €  1 396,88 €  8 143,12 €
  401000  Fournisseurs (honoraires cabinet, facture 09/2025)  1 396,88 €  1 996,88 €   600,00 €
  444000  État - IS à payer (solde IS 2025)                                1 200,00 €  1 200,00 €
  ```
  Le 2033-A porte 084 Disponibilités 8 143,12 € (réel : 6 343,12), 166 Fournisseurs 600 €, 172
  Autres dettes 1 700 €. Le parcours ne signale qu'un « ! 2 non rapproché(s) sur 17 … ↳ bank
  reconcile / expense reconcile » : la seule action proposée est fausse.
- **Contre-épreuve (coffre `naif.db`)** : en suivant ce conseil (`expense record --transaction
  <id>` sur les deux débits), le parcours passe au vert (« ✓ 17 mouvements … tous rapprochés »)
  avec un résultat de **−2 926,90 €** au lieu de −1 226,90 (le solde d'IS devient une charge
  déductible en 658000, les honoraires 2025 sont comptés deux fois), et 401/444 restent à 600 et
  1 200 au passif. Aucun des deux chemins ne donne un bilan juste.
- **Portée** : toute SASU reprise d'un cabinet a au 30/09 des honoraires à payer, une TVA ou un IS
  à régler, souvent un compte courant. Sans écriture de règlement de dette/créance (ou un
  rapprochement « débit → compte de bilan »), le scénario du brief ne peut pas produire un bilan
  déposable. Même trou pour un remboursement de compte courant (455), un remboursement de crédit de
  TVA reçu (crédit non rapprochable à une facture), un acompte d'IS versé, une CFE réglée
  (aujourd'hui saisie en 658 « charges diverses » faute de catégorie « impôts et taxes »).

### MAJEUR

#### M1 — `year close` accepte de clore un exercice pas encore écoulé (le cœur n'a pas la garde que le parcours annonce)

- **Où** : `freeflow-core/src/fiscal_year.rs` (`CloseFiscalYear::apply` — validations période
  non chevauchante, affectation ≤ distribuable, plafond de réserve ; aucune comparaison avec la
  date du jour) ; le parcours (`closing.rs`, étape `period_ended`) est seul à bloquer.
- **Reproduction** : à la date machine 2026-09-02, `freeflow year checklist 2026` → « ✗ Exercice
  écoulé (échéance 2026-09-30) : le clore aujourd'hui figerait un résultat incomplet » ; puis
  `freeflow year close --period 2026` → `✓ FiscalYearId(…)` (exit 0). Idem dans la fenêtre
  (panneau « clore un exercice », aucun avertissement de date).
- **Attendu** : refus (ou au minimum un `--force` explicite) tant que `ends_on > today`, la
  commande prenant `today` de l'adaptateur comme `checklist`. Le snapshot figé le 2 septembre ne
  contiendrait pas les frais de septembre.

#### M2 — Liasse : le chiffre d'affaires est placé en case 210 du 2033-B (ventes de marchandises, compte 707) au lieu de 218 (production vendue – services, compte 706) ; autres cases approximatives

- **Où** : `crates/freeflow-docs/src/liasse.rs:185-203`.
- **Preuve** : `docs/liasse-2026.json` : `{"form":"2033-B","case":"210","label":"Chiffre
  d'affaires — prestations de services (HT)"}`. Vérifié à la source : sur le 2033-B-SD, la
  ligne 210 est « Ventes de marchandises » et la 218 « Production vendue – services »
  (notice et guides de remplissage du formulaire, cf. sources en fin de rapport). Pour une SASU
  de développement, le CA doit aller en 218.
- Également : la CFE (et toute taxe) tombe en 242 « autres charges externes » via la catégorie
  `other` → 658000, alors que le 2033-B a une ligne 244 « impôts, taxes et versements
  assimilés » (compte 63) ; la rémunération du dirigeant « coût employeur » est mise en 250
  (rémunérations du personnel) sans ventiler les charges sociales en 252 ; l'IS n'est pas porté
  en 306 ; les entrées « 2065 / IS » et « 2065 / NET » ne correspondent à aucune case du 2065.
  Les tableaux 2033-C (immobilisations, dès qu'il y en a une au bilan d'ouverture — cf. M7) et
  2033-F (composition du capital, obligatoire : l'associé unique détient 100 %) sont absents.
  Les dates `period_start`/`period_end` sont sérialisées `[2025, 274]` (année + jour ordinal) —
  illisibles pour l'expert-comptable à qui ce JSON est destiné (voir m1).

#### M3 — Le PV de décisions de l'associé unique n'identifie pas l'associé et omet des mentions obligatoires

- **Où** : `crates/freeflow-docs/src/minutes.rs` ; `company_profile` n'a aucun champ pour
  l'associé unique.
- **Preuve** (`pdftotext docs/minutes-2026.pdf`) : « Le 15 décembre 2026, l'associé unique … a
  pris les décisions suivantes … Fait le 15 décembre 2026. L'associé unique » — aucun nom, aucune
  adresse, aucun nombre d'actions ; le président n'est pas nommé non plus (le quitus lui est donné
  anonymement).
- **Manque, obligatoire ou attendu par tout greffe/cabinet** :
  1. identité de l'associé unique (et du président s'il est distinct), qualité, signature ;
  2. **art. 223 quater CGI** : approbation du montant global des dépenses et charges non
     déductibles visées à l'art. 39-4 (dépenses somptuaires) et de l'impôt correspondant —
     mention obligatoire de toute décision d'approbation dans une société à l'IS, même « néant » ;
  3. conventions réglementées (art. L227-10 al. 4 C. com. : quand l'associé unique est le
     président, mention au registre des décisions ; la décision indique usuellement qu'aucune
     convention n'a été conclue) ;
  4. mention de l'inscription au **registre des décisions de l'associé unique** (L227-9 al. 4)
     — le registre n'est cité nulle part dans l'app ;
  5. rappel de la dispense de rapport de gestion (L232-1 IV) pour une SASU dont l'associé unique
     personne physique est président, sous les seuils de la petite entreprise ;
  6. formulation de l'affectation d'une **perte** : le PV dit « affecter le résultat … Dotation
     0 €, Dividendes 0 €, Report à nouveau 5 123,10 € » et la décision d'affectation affiche un
     « Total distribuable 5 123,10 € » sur un exercice déficitaire ; la rédaction attendue est
     « affecte la perte de 1 226,90 € au compte report à nouveau, qui passe de 6 350,00 € à
     5 123,10 € ».
- Non mentionnée non plus, l'option de l'art. L227-9 al. 3 : quand l'associé unique personne
  physique est lui-même président, le **dépôt au greffe des comptes signés dans les six mois vaut
  approbation** sans PV — la voie la plus simple pour la cible du brief.

#### M4 — Obligations déclaratives absentes : DAS2 et prélèvements sur dividendes (2777 / PFU)

- **Où** : `freeflow-core/src/fiscal.rs` (`FiscalDeadlineKind` : `ca3`, `vat_acompte`, `ca12`,
  `is_acompte`, `is_solde`, `liasse`, `approval_meeting`, `accounts_filing`, `cfe`, `dsn`) ;
  `grep -rni 'das2\|2777\|PFU'` sur `crates/` : aucune occurrence.
- **DAS2** (art. 240 CGI) : toute société qui verse à un même bénéficiaire plus de 1 200 € TTC
  d'honoraires dans l'année civile doit les déclarer (DAS2-T en ligne, dépôt début mai N+1 ; pour un
  exercice décalé, l'administration admet le délai de la déclaration de résultats — à vérifier au
  BOFIP BOI-BIC-DECLA-30-70-20). Le cœur possède désormais la catégorie `fees` (622600) : il a tout
  pour cumuler par bénéficiaire et par année civile, mais ne le fait pas. Dans le scénario, le
  cabinet a reçu 600 € (oct. 2025) + 900 € (mars 2026) : 1 500 € entre octobre 2025 et mars 2026,
  répartis sur deux années civiles — précisément le genre de calcul qu'un débutant ne fera pas.
- **Dividendes** : l'affectation modélise des dividendes (`--dividends`, écriture 457) mais rien
  ne rappelle que la société doit **retenir à la source** le PFU (12,8 % + 17,2 % de prélèvements
  sociaux) et déposer/payer la **2777** avant le 15 du mois suivant la mise en paiement, ni que
  le montant brut doit figurer sur la 2065 bis (distributions). Contre-épreuve `test27.db` :
  `year close --period 2027 --dividends 3000` puis `fiscal calendar --today 2027-10-05` →
  aucune échéance liée aux dividendes.

#### M5 — Calendrier de TVA : CA3 trimestrielle en doublon de la CA12 E pour un exercice décalé à cheval sur la suppression du réel simplifié

- **Où** : `freeflow-core/src/fiscal.rs` (`VatFilingScheme::for_exercise`,
  `next_ca3_filing_from(…, earliest_period)`).
- **Reproduction** (`test27.db`, RSI, clôture 30/09, exercice 2027 clos) :
  `fiscal calendar --today 2027-10-05 --json` →
  ```
  2027-10-25 ca3  | CA3 trimestrielle, TVA de 2027-07 à 2027-09 ; … régime simplifié supprimé …
  2027-12-31 ca12 | CA12 : TVA due au titre de l'exercice 2026-10-01 – 2027-09-30 (2 000,00 €) … dernière CA12
  ```
- **Attendu** : le trimestre juillet–septembre 2027 appartient à l'exercice 01/10/2026 →
  30/09/2027, encore au réel simplifié (rescrit BOI-RES-TVA-000253 cité par l'app elle-même) et
  déclaré par la CA12 E du 31/12/2027. La première CA3 trimestrielle est celle du premier
  trimestre de l'exercice ouvert le 01/10/2027, soit octobre–décembre 2027, déposée en janvier
  2028. Le lot 27 a testé cette borne pour une clôture au 31/12 seulement ; pour une clôture
  décalée, elle est fausse — l'utilisateur déclarerait deux fois la même TVA.

#### M6 — `freeflow init --db ec.db` (chemin relatif sans répertoire) échoue et laisse un sidecar orphelin qui bloque le nom

- **Où** : `freeflow-core/src/store/kdf.rs:809-818` — `write_atomic` appelle
  `sync_dir(path.parent())` ; pour `ec.db`, `parent()` renvoie `Some("")` et `File::open("")`
  échoue en ENOENT **après** le `rename` du sidecar.
- **Reproduction** : `freeflow --db ec.db --passphrase-file pass.txt init` →
  `✗ erreur de coffre : impossible d'accéder au fichier du coffre : No such file or directory
  (os error 2)` (exit 3), `ls` → `ec.db.kdf` présent, `ec.db` absent ; nouvel `init` →
  `✗ un coffre existe déjà à ec.db`. `--db ./ec.db` et un chemin absolu fonctionnent.
- **Attendu** : succès ; c'est la toute première commande du parcours du brief, et le README
  documente `--db` sans préciser cette contrainte.

#### M7 — Immobilisations, amortissements, régularisations : hors périmètre documenté, mais présents dans le bilan de quasi tout développeur

- **Preuve** (`naif.db`, bilan d'ouverture avec 218300 D 1 500 / 281830 C 900 / 486000 D 120) :
  la reprise est acceptée et bien ventilée (028 brut 1 500, amort. 900, net 600 ; 092 CCA
  120), mais **aucune dotation** de l'exercice n'est calculée ni proposée, la charge constatée
  d'avance n'est jamais extournée : le résultat est surestimé, la case 254 du 2033-B reste vide,
  le 2033-C n'existe pas. Le README le dit (« il ne sait pas amortir un ordinateur ») — mais un
  bilan de cabinet pour un développeur contient un ordinateur dans la plupart des cas, et rien
  dans l'app ne détecte la présence d'un compte 28x au bilan d'ouverture pour prévenir
  l'utilisateur que son résultat sera faux.

#### M8 — FEC : `PieceRef` non unique et libellés de compte incohérents

- **Où** : `freeflow-core/src/ledger.rs:552` (`format!("DEP-{}", short_id(expense.id))`, 8
  premiers caractères d'un UUIDv7 = horodatage à la seconde près) et règle « premier libellé
  vu » du compte.
- **Preuve** (`docs/889112348FEC20260930.txt`) : 8 dépenses distinctes portent la même pièce
  `DEP-01A06372`, 7 autres `DEP-01A06371` ; le compte 401000 est libellé « Fournisseurs
  (honoraires cabinet, facture 09/2025) » dans l'écriture AN et « Fournisseurs » ailleurs. Une
  dépense avec justificatif reçoit en `PieceRef` le nom de fichier complet (hash de 64 hex +
  nom, > 80 caractères).
- **Attendu** : une référence de pièce unique et courte par dépense (numéro séquentiel de
  pièce, ou l'UUID complet), un libellé unique par `CompteNum` (le contrôle « Test Compta Demat »
  de la DGFiP signale les libellés multiples). Le reste du format est conforme : 18 colonnes,
  `|`, AAAAMMJJ, virgule décimale, écritures équilibrées, numérotation continue par journal, à-
  nouveaux `AN`, nommage `<SIREN>FEC<AAAAMMJJ>.txt` — vérifié par script.

#### M9 — Sorties « humaines » de la CLI en `Debug` Rust brut sur les commandes de la clôture

- **Où** : `freeflow-cli/src/output.rs:38-44` (`format_value` → `format!("{value:?}")` hors
  `--json`), utilisé par `company show`, `company set-profile` (`✓ ()`), `fiscal calendar`,
  `fiscal deadlines`, `audit verify-chain` (`Object {"status": String("intact")}`),
  `expense show`, et tous les retours `✓ ExpenseId(…)` / `✓ FiscalYearId(…)` / `✓ 2`.
- **Preuve** : `freeflow fiscal calendar --today 2026-09-02` →
  `[Object {"amount_cents": Null, "due_on": String("2026-09-15"), "kind": String("is_acompte"),
  "note": String("dispense d'acompte …")}, …]` ; `company show` → `Some(CompanyProfileWithVatFiling
  { profile: CompanyProfile { … siren: Siren([8, 8, 9, 1, 1, 2, 3, 4, 8]) … } })`.
- Pour un « vrai débutant » à qui le README dit de piloter la clôture par la CLI, le calendrier
  fiscal — la commande qui liste ses obligations — est illisible. (Le tableau de bord de la
  fenêtre, lui, le rend correctement.)

### MINEUR

- **m1 — Dates en jour ordinal dans plusieurs JSON** : `bank list --json` → `"occurred_on":
  [2026, 248]` ; liasse → `"period_start": [2025, 274]`. Le reste de la CLI est en ISO 8601.
- **m2 — Parcours 2027 : « réserve légale cumulée 0,00 € »** alors que 100 € sont repris au
  bilan d'ouverture (`year checklist 2027`, étape `previous_year`). Le cœur, lui, compte bien
  la reprise : `year close --period 2027 --legal-reserve 50` → « la réserve légale cumulée
  (150,00 €) dépasserait 10 % du capital social » ; `minimum_legal_reserve_cents = 0` sur un
  bénéfice de 8 684,04 € (correct, plafond atteint). Texte trompeur seulement.
- **m3 — IS non arrondi à l'euro** (art. 1657 CGI) : `test27.db`, résultat fiscal 8 773,10 → IS
  affiché 1 315,96 € (15 % exact) au lieu de 1 316 €.
- **m4 — CA12 sans le crédit de TVA repris** : l'ouverture porte 445670 D 210 € ; la CA12 E
  chiffre « −169,98 € » (TVA de l'exercice seule) ; le crédit à reporter/rembourser est 379,98 €.
  Aucune mention de la demande de remboursement de crédit (3519) possible dès 150 € sur la CA12.
- **m5 — Dispense d'acompte d'IS affirmée** (« IS de référence < 3 000 € ») alors que le
  premier exercice n'a aucun IS de référence connu (le bilan d'ouverture ne le reprend pas) ;
  idem pour les acomptes de TVA 3514 de décembre 2025 / juillet 2026, dus au titre de l'exercice
  2025 que l'app ne connaît pas.
- **m6 — Libellés de compte qui fuient** : le libellé saisi au bilan d'ouverture devient le
  libellé permanent du compte (« État - IS à payer (solde IS 2025) » sur le 444000 en 2028,
  « Fournisseurs (honoraires cabinet, facture 09/2025) » sur tout le 401).
- **m7 — À-nouveaux 2027 avec 110000 C 6 350 et 119000 D 1 226,90 simultanés** (puis 110000
  C 12 034,04 et 119000 D 1 226,90 en 2028) : arithmétiquement juste, mais un report à nouveau
  se présente soldé sur un seul compte.
- **m8 — CFE sans catégorie** : saisie en `other` → 658000 « charges diverses de gestion
  courante » ; attendue en 635110 (ligne 244 du 2033-B). Manque une catégorie « impôts et
  taxes ».
- **m9 — Échéances hors calendrier** : relevé de solde 2572 à télédéclarer même à zéro ;
  déclaration de confidentialité des comptes (micro-entreprise, art. L232-25) au dépôt ; coût du
  dépôt ; registre des décisions.
- **m10 — `invoice emit --client` exige un UUID nu** (« invalid character: found `l` ») alors que
  tous les autres verbes résolvent par nom ; `--lines` est un JSON avec `unit_price` en
  centimes et `vat_rate: "Standard"`, non documenté dans `--help`.
- **m11 — `fiscal deadlines` exige `--today`** avec un message clap en anglais ; `--help` de
  `year opening set --line` ne donne pas la syntaxe (`compte:libellé:D|C:montant`), qu'il faut
  aller chercher dans le parcours ou le README.
- **m12 — Montant « DSN »** avec un président à 2 000 € brut et ratio 45 % : `2 900,00 €`
  libellé « cotisations sociales mensuelles du dirigeant (estimation) » — c'est le coût total,
  pas les cotisations ; la sémantique de `--director-charge-ratio` (« charges/net ») est ambiguë.

### SUGGESTION

- Catégoriser le débit d'un relevé vers un **compte de bilan** (401, 444, 445, 455, 164, 512
  virement interne) — c'est le geste manquant de B1, et la fenêtre a déjà le bloc « débits du
  relevé à rapprocher » pour l'accueillir.
- Importer la **balance** ou le **FEC** du logiciel précédent (voir § 5).
- Tracer « document produit » et « déclaration déposée » pour que le parcours puisse passer les
  étapes « documents », « liasse », « dépôt » à `done`.

## 3. Ce qui fonctionne bien

- Le calcul du cœur est juste et rejouable : résultat −1 226,90 €, IS 0, déficit reportable
  1 226,90 €, report à nouveau 5 123,10 €, TVA déductible 169,98 €, bilan dérivé équilibré ; en
  2027 (copie) : déficit imputé, résultat fiscal 8 773,10, IS 15 %, RAN 10 807,14 €, plafond de
  réserve légale respecté avec la réserve reprise, chaînage des à-nouveaux (106100 = 100 €,
  457 = 3 000 €, 444 = 2 515,96 €).
- Bilan d'ouverture : syntaxe simple, équilibre vérifié, capitaux propres reconstitués, 120/129
  et 28x/486 correctement ventilés au 2033-A (cases 028/092/169), figé dès qu'un exercice est
  clos ; `year opening rm` et `expense edit` refusés après clôture, `audit verify-chain` intact.
- FEC formellement conforme (18 colonnes, journaux AN/AC/BQ, équilibre par écriture, numérotation
  continue, nommage réglementaire) et écritures 401/512 datées du relevé quand la dépense est
  rapprochée.
- Le parcours de clôture (CLI et fenêtre) est la bonne idée : phases, statuts, commande suivante
  à taper, échéances datées d'une clôture décalée exactes (approbation 30/03/2027 ; liasse 3 mois
  → 30/12/2026 ; solde d'IS 15/01/2027 ; dépôt au greffe 1 mois après l'AG, 2 mois par voie
  électronique ; CA12 E 31/12/2026 ; CA3 le 24 pour une SAS hors Paris, report au jour ouvré) ;
  le lexique est bien écrit pour la cible.
- La grille CA3, la dispense d'acomptes 3514 sous 1 000 €, la dispense d'acomptes d'IS sous
  3 000 €, la fin du RSI au 01/01/2027 sont correctement modélisées (hors M5).

## 4. Recommandations priorisées

1. **Règlement de dettes/créances de bilan depuis le relevé** (B1) : une commande
   `bank settle <transaction> --account 401000|444000|455000|…` (et son panneau) qui écrit
   compte de bilan contre 512 sans passer par une dépense ; le parcours doit la proposer quand un
   compte de tiers du bilan d'ouverture est encore ouvert. Sans cela, aucune reprise de cabinet ne
   donne un bilan juste.
2. **Garde « exercice écoulé » dans `CloseFiscalYear`** (M1), `today` fourni par l'adaptateur.
3. **Corriger la liasse** (M2) : 218 pour les prestations, 244 pour les taxes (nouvelle catégorie
   « impôts et taxes »), 250/252 ventilés, 306 IS, retirer les pseudo-cases 2065, ajouter 2033-F
   et 2033-C, dates ISO.
4. **PV et décision d'affectation** (M3) : champ « associé unique / président » au profil,
   mention 223 quater CGI, conventions réglementées, registre des décisions, rédaction propre
   d'une perte, et proposer la voie L227-9 al. 3.
5. **DAS2 et 2777** (M4) : cumul des honoraires par bénéficiaire et année civile avec alerte à
   1 200 € ; échéance 2777 et retenue PFU dès qu'un dividende est affecté.
6. **CA3/CA12 pour clôture décalée** (M5) et `init` sur chemin relatif (M6).
7. **Sorties humaines lisibles** sur toute la CLI (M9) et dates ISO partout (m1).
8. **Amortissements minimaux** (M7) : au moins détecter un compte 28x au bilan d'ouverture et
   avertir ; à terme une dotation linéaire saisie ou reprise du cabinet.
9. **Reprise Tiime/Indy** (§ 5) : importer la balance (CSV) ou le FEC de l'ancien logiciel pour
   générer le bilan d'ouverture et les déficits reportables.

## 5. Reprise depuis Tiime / Indy : est-ce réaliste ?

Ce que l'utilisateur possède réellement au 30/09/2025 : la liasse (2065 + 2033-A/B/C/D) et les
comptes annuels en PDF remis par le cabinet, souvent une balance générale et un grand livre (PDF
ou CSV), et — pour Tiime comme pour Indy — un **export FEC** de l'exercice clos (les deux outils
le proposent). Il n'a en général **pas** de « bilan après affectation » : la balance du cabinet
porte le résultat en 120/129, et l'affectation est dans le PV de l'AG précédente.

Ce que FreeFlow sait en faire aujourd'hui :

- **Aucun import** : ni balance CSV, ni FEC, ni liasse. Le seul chemin est de recopier à la main
  chaque compte de bilan en `compte:libellé:D|C:montant` (`--line`, `--lines-file` ou le
  textarea de la fenêtre). Le README annonce « il n'y en a souvent que trois ou quatre » ; une
  balance réelle de SASU en a huit à quinze (banque, clients, TVA déductible/collectée/crédit,
  fournisseurs, IS, CCA, immobilisations et amortissements, CCA/FNP, capital, réserves, RAN,
  résultat). C'est faisable pour un débutant motivé, mais rien dans l'app ne lui dit **où
  trouver** ces comptes (dernière page de la balance, ou 2033-A) ni comment convertir « actif /
  passif » en « débit / crédit » — un point que le lexique devrait couvrir.
- Le 120/129 « réputé affecté en report à nouveau » est documenté et juste dans le cas courant ;
  si l'AG précédente a distribué des dividendes, l'utilisateur doit le savoir et corriger
  lui-même le 110 — non guidé.
- Les déficits reportables (`--tax-losses`, case 870 du 2033-D) sont bien pris en charge et bien
  expliqués (formulaire et README).
- Ne sont pas repris, faute de champ : l'IS de l'exercice précédent (base des acomptes), la TVA
  due de l'exercice précédent (base des acomptes 3514), les honoraires déjà versés dans l'année
  civile (DAS2), le tableau d'amortissement des immobilisations (M7).
- Une fois le bilan saisi, le problème B1 survient immédiatement : les 401/444/445/455 repris
  ne pourront jamais être soldés.

Conclusion : la reprise est **réaliste sur le papier pour un bilan sans immobilisation ni dette**,
et **irréaliste pour un bilan de cabinet ordinaire** tant que B1 n'est pas traité. Un import de
balance CSV (colonnes compte / libellé / débit / crédit, ce que Tiime, Indy et tout cabinet
exportent) ou du FEC de l'exercice précédent (dont on prend les soldes des classes 1 à 5 au
dernier jour) supprimerait la recopie et la plupart des erreurs de saisie.

## Annexes — commandes rejouées (extraits) et sources

```
freeflow --db $PWD/ec.db --passphrase-file pass.txt init
… company set-profile --name "Nova Dev" --legal-form SASU --siren 889112348 --vat-number FR16889112348 \
    --street "12 rue de la Fosse" --postal-code 44000 --city Nantes --country FR --share-capital 1000 \
    --rcs-city Nantes --fiscal-year-end 30/09 --vat-regime real_simplified
… year opening set --opens-on 2025-10-01 --source "Bilan au 30/09/2025 - Cabinet Expertise SARL" --lines-file bilan-ouverture.txt
… bank import --format csv releve-2026.csv            # ✓ 17
… expense record --transaction <id> --label … --category bank_charges|fees|software|other …
… year checklist 2026 --today 2026-10-02              # PRÊT À CLORE, 2 avertissements
… year balance 2026 ; year close --period 2026 ; year approve 2026 --approved-on 2026-12-15
… year render 2026 minutes|appropriation|synthesis|balance-sheet|liasse --out docs/… ; fec export 2026 --out docs/
… fiscal calendar --today 2026-10-02 ; year glossary ; audit verify-chain
```

Sources consultées pour les points de droit : notice/guides du 2033-B-SD (ligne 210 « ventes
de marchandises », 218 « production vendue – services » — cerfaliassefiscale.com, indy.fr,
compta-online.com) ; C. com. art. L227-9 (approbation dans les six mois, dépôt valant approbation
quand l'associé unique est président, registre des décisions), L227-10 (conventions), L232-10
(réserve légale), L232-23 (dépôt au greffe, un mois / deux mois électronique), L232-25
(confidentialité) ; CGI art. 223 (délai de la déclaration de résultats), 223 quater (mention
des dépenses non déductibles), 240 (DAS2), 1657 (arrondi), 1668 (acomptes d'IS), 117 quater /
2777 (prélèvements sur dividendes) ; BOI-RES-TVA-000253 et art. 38 LF 2025 (fin du RSI, cités
par l'app).
