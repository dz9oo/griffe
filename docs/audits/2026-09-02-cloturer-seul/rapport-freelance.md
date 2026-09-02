# Rapport d'audit — rôle « freelance débutant » (SASU à l'IS, zéro notion comptable)

Coffre : `freelance.db` (dossier de travail). Date simulée : 2 septembre 2026. Fenêtre parcourue via
`freeflow-web-dev` (port 3421) en HTTP/curl — le navigateur Chrome affichait une page d'erreur sur
`127.0.0.1:3421` et `localhost:3421` malgré un `200` en curl (deux essais, abandonné comme le brief
le prévoit) ; CLI en complément. Aucune modification du dépôt.

## 1. Verdict en 3 lignes

**Pas prêt pour ce scénario sans accompagnement.** Le cœur métier tient (bilan d'ouverture,
rapprochement, clôture, six documents corrects, parcours et lexique bien écrits), mais la fenêtre ne
permet pas de saisir le profil société (obligatoire) autrement qu'en tapant une ligne de commande
dans la console, **laisse clore et approuver un exercice non écoulé** (immuable ensuite, sans
retour possible), n'importe pas de relevé bancaire ni de données Tiime/Indy, ne dit rien de la TVA
dans le parcours de clôture, et plusieurs sorties CLI affichent du `Debug` Rust brut. Un vrai
débutant se perd dès l'écran d'accueil et peut figer un mauvais exercice en trois clics.

## 2. Constats

### BLOQUANT

**B1 — Le profil société ne se saisit pas dans la fenêtre : il faut taper une commande CLI dans la console.**
- Où : tous les écrans ; parcours (`/cloture/checklist`) → bouton « renseigner le profil (console : company set-profile) » → `crates/freeflow-web/src/views/cloture.rs:392-395` (`nav_link("/view/console", …)`). Aucune route `company` dans `crates/freeflow-web/src/lib.rs:34-227`.
- Reproduction : créer le coffre, ouvrir le dashboard. Le calendrier fiscal dit « profil d'entreprise non renseigné » sans dire où aller. Aller dans `cloture` → parcours → « bloquant Profil d'entreprise… Renseignez-le d'abord » → le bouton envoie sur la console dont le placeholder est `prospect pipeline --json`.
- Attendu : un formulaire « Ma société » (nom, SIREN, adresse, capital, **date de clôture**, **régime de TVA**) proposé dès le premier lancement.
- Preuve : il m'a fallu taper `company set-profile --help` dans la console, où l'aide mélange 8 champs métier et 12 options globales (`--db`, `--passphrase-file`, `--remember`, `--ttl`…), et deviner que `--fiscal-year-end 30/09` et `--vat-regime real_normal_monthly` existent alors qu'ils **ne sont pas obligatoires** : mon premier profil (champs requis seuls) a donné un parcours 2026 « du 2026-01-01 au 2026-12-31 » avec « date de clôture (année civile supposée) ; régime de TVA (mensuel supposé) ». Le choix entre `real_normal_monthly`/`real_normal_quarterly`/`real_simplified`/`franchise` n'est expliqué nulle part dans l'app.

**B2 — La fenêtre laisse clore un exercice non écoulé, puis l'approuver à une date future ; l'exercice devient immuable.**
- Où : `POST /cloture` (`crates/freeflow-web/src/cloture.rs`, handler `create`, aucune comparaison à la date du jour) ; `CloseFiscalYear::apply` (`crates/freeflow-core/src/fiscal_year.rs:458-476`) ne vérifie que l'ordre des dates et le chevauchement ; `ApproveFiscalYear` (`fiscal_year.rs:644`) refuse seulement `approved_on < ends_on`.
- Reproduction (le 2026-09-02) : `cloture` → « + clore un exercice » → 2025-10-01 → 2026-09-30 → « Clore l'exercice » → **`200` + `HX-Trigger: freeflow:saved`, exercice créé « projet »** alors que le parcours, un panneau plus tôt, disait « bloquant : Exercice écoulé — le clore aujourd'hui figerait un résultat incomplet ». Puis « approuver » : la date par défaut (2026-09-02) est refusée (« la date d'AG ne peut pas précéder la clôture (2026-09-30) »), mais **2026-12-15 est acceptée** → « approuvé le 2026-12-15 », immuable.
- Conséquences observées : `year rm 2026` → « ✗ l'exercice … est déjà approuvé : la décision d'AG fait foi, il est immuable » ; toute dépense de septembre 2026 est refusée (« la dépense du 2026-09-05 tombe dans un exercice déjà clôturé… supprimez d'abord l'exercice s'il n'est qu'un projet (freeflow year rm) ») ; le PV généré est daté « Fait le 15 décembre 2026 ». Le coffre est définitivement faux pour l'exercice 2026 (frais bancaires de juillet-septembre exclus).
- Aggravant : après la clôture prématurée, le parcours affiche « ✓ Exercice écoulé — Exercice du 2025-10-01 au 2026-09-30 écoulé » le 2 septembre (`closing.rs:509-520` : `facts.record.is_some() || today > end`).
- Attendu : refus (ou double confirmation explicite) de clore avant `ends_on`, refus d'une date d'approbation postérieure à aujourd'hui, et un bouton « clore » grisé tant que le parcours est bloquant.

**B3 — Aucune reprise possible des données d'un autre logiciel (Tiime, Indy, cabinet).**
- Où : `freeflow fec import` → « error: unrecognized subcommand 'import' — tip: a similar subcommand exists: 'export' » ; `year opening set --lines-file freelance-tiime-balance.csv` → « ligne de bilan invalide : Compte;Libellé;Débit;Crédit;Solde (attendu `compte:libellé:D|C:montant`…) » ; aucun `grep -i "tiime\|indy\|import.*fec"` dans `crates/`.
- Attendu : importer une balance (CSV) ou un FEC de l'ancien logiciel pour construire le bilan d'ouverture, et au minimum un texte qui dise « demandez au cabinet la *balance de clôture* (pas le bilan) et recopiez les comptes de classes 1 à 5 ».
- Preuve : le bilan PDF que remet un cabinet est présenté par rubriques (« Disponibilités », « Capitaux propres ») sans numéros de compte ; la seule aide de l'app (« Reprenez la balance de clôture de l'expert-comptable ») suppose de savoir qu'une balance existe et de la demander.

### MAJEUR

**M1 — Sorties CLI/console en `Debug` Rust brut.**
- Où : `crates/freeflow-cli/src/company.rs:137-141` et `fiscal.rs:44-50` appellent `format_value(&…, json)`, dont le mode texte est littéralement `format!("{value:?}")` (`crates/freeflow-cli/src/output.rs:38-43`).
- Preuve : `company show` → `Some(CompanyProfileWithVatFiling { profile: CompanyProfile { name: "Nicolas Dev", … siren: Siren([9, 0, 1, 2, 6, 5, 3, 2, 2]), … share_capital: Some(Money(100000)) … } })` et, sans profil, `None`. `fiscal calendar --today 2026-02-01` → `[Object {"amount_cents": Number(-12000), "due_on": String("2026-02-24"), "kind": String("ca3"), …`. `company set-profile` réussi → `✓ ()`. Identique dans la console de la fenêtre (même parseur).
- Attendu : rendu tabulaire comme `year show`/`bank list`, qui sont propres.

**M2 — Le parcours de clôture ignore totalement la TVA.**
- Où : `closing.rs`, phase « Déclarer et déposer » (documents, solde d'IS, liasse, greffe) — aucune étape `vat`.
- Preuve : sur l'exercice, 120 € de TVA déductible (honoraires) et 0 € collectée ; le calendrier du dashboard n'affiche que la prochaine CA3 (« TVA de 2026-08 : 0,00 € »). Le crédit de 120 € n'apparaît que si l'on consulte `fiscal calendar --today 2026-02-01` (« amount_cents: -12000 »). Rien ne dit au débutant qu'une CA3 mensuelle « néant » est due chaque mois même sans CA, ni comment récupérer/reporter le crédit, ni que le bilan garde « Autres créances 120 € » parce que la TVA n'est pas liquidée (dit seulement dans la note de bas de page du PDF).
- Attendu : une étape « TVA de l'exercice » (collectée, déductible, crédit/dette, déclarations manquantes) dans le parcours, avec renvoi impots.gouv.fr.

**M3 — Pas d'import de relevé bancaire dans la fenêtre, et le format CSV attendu n'est documenté nulle part.**
- Où : aucune route d'import (`lib.rs`), `bank import --help` n'indique pas le format ; le format `date;description;montant` n'existe que dans le doc-comment de `crates/freeflow-core/src/billing/import.rs:26-29`.
- Preuve : export Qonto (`Date de l'opération,Libellé,Montant (TTC),Devise,Catégorie`) → « ✗ ligne 2 : attendu 3 champs, trouvé 1 » (exit 4), sans indiquer le format attendu. Le parcours dit « Importez le relevé (bank import) ». Dans la fenêtre, `bank import --format csv freelance-releve-juillet.csv` fonctionne **depuis la console**, avec un chemin relatif au cwd du serveur — inconnaissable depuis l'app desktop — mais rien ne l'indique.
- Attendu : bouton « importer un relevé » (sélecteur de fichier) dans `depenses`/`facturation`, détection de colonnes ou mapping, message d'erreur avec le format attendu.

**M4 — Pas de catégorie « impôts et taxes » : la CFE tombe en charges externes.**
- Où : `ExpenseCategory` (9 valeurs, `depenses/new`), `ledger::charge_account`.
- Preuve : débit « PRLV IMPOTS CFE 250 € » saisi en « autre » (seul choix plausible) → balance : `658000 Charges diverses de gestion courante 250 €` → liasse `2033-B case 242 Autres charges externes 89500` (au lieu de la case 244 impôts et taxes, compte 635). Le résultat est juste, la ventilation de la liasse ne l'est pas, et le débutant ne peut pas le savoir.

**M5 — Texte trompeur « réserve légale cumulée 0,00 € » dans le parcours de l'exercice suivant.**
- Où : `closing.rs:618-621` (`previous.legal_reserve` = dotation de l'exercice précédent, pas le cumul de la chaîne).
- Preuve : bilan d'ouverture avec `106100 Réserve légale C 100`, bilan 2026 case 126 = 100 € ; `year checklist 2027` → « Exercice précédent … approuvé — hérité : report à nouveau 3 005,00 €, **réserve légale cumulée 0,00 €** ». Le calcul du minimum (`closing.rs:404-408`, `chain.reserve`) semble lui utiliser la chaîne ; seul le texte est faux, mais c'est exactement le texte qu'un débutant lit pour se rassurer.

**M6 — Les messages d'erreur de la fenêtre renvoient à des commandes CLI et parlent en jargon.**
- Preuve : « règle métier violée : la dépense du 2026-01-15 tombe dans un exercice déjà clôturé… supprimez d'abord l'exercice s'il n'est qu'un projet (freeflow year rm) » ; fiche dépense : « ajoutez-en un via « modifier », ou en CLI : freeflow expense edit 01a06372-… --receipt <fichier> » ; `/cloture/balance` sans profil : « règle métier violée : aucun profil d'entreprise défini : le grand livre en dépend (company set-profile) ». Le préfixe « règle métier violée » est systématique.

### MINEUR

**m1 — Aucun onboarding après la création du coffre.** Le dashboard montre `pipeline_pondere`, `capacite_2026-09 -0% sous-remplissage`, `rentabilite_client` — des KPI de prospection sans rapport avec « je veux saisir ma société et clôturer ». Rien ne liste ce qui manque (profil, clôture, bilan d'ouverture, TVA). La palette ⌘K (`aller à… (client, mission, facture)`) n'offre ni aide ni « configurer ma société ». La console répond à `aide` par « error: unrecognized subcommand 'aide' » (anglais) et à une ligne vide par rien.

**m2 — Valeurs par défaut de l'écran `cloture` incohérentes le 2 septembre.** Barre : « parcours de l'exercice clos en **2025** » vs « bilan **2026** » / « FEC **2026** » ; formulaire « + clore un exercice » pré-rempli **2024-10-01 → 2025-09-30** (exercice antérieur au bilan d'ouverture, qui échouerait sur `OpeningBalanceMismatch`). Le débutant qui prépare le 30/09 doit corriger trois champs.

**m3 — Le panneau « rapprocher d'un débit » présélectionne le débit le plus récent, pas le plus proche.** Dépense du 2026-03-05 → `<option selected>` = 2026-07-05 (il existait un 2026-03-05 de même montant). Un clic distrait rapproche le mauvais mois.

**m4 — `fiscal calendar` et `fiscal deadlines` exigent `--today` sans défaut en CLI** (« error: the following required arguments were not provided: --today »), alors que le dashboard le calcule seul.

**m5 — Liasse JSON : dates sérialisées en `[2025, 274]` / `[2026, 273]`** (année, jour ordinal) — illisible pour un cabinet ou un partenaire EDI. Les libellés des cases sont par ailleurs très bons.

**m6 — Aides `--help` lacunaires** : `year close --starts-on/--ends-on` sans description ; `expense record --vat-rate` sans liste de valeurs ; `year render <PERIOD>` sans description ; `bank import <FILE>` sans description.

**m7 — Le justificatif attaché est stocké dans `receipts/` à côté du coffre**, partagé par tous les coffres du même dossier (observé : les pièces des autres coffres de l'audit y sont mêlées). Sans conséquence fonctionnelle, mais surprenant.

### SUGGESTION

**s1 —** Le lexique est excellent mais n'est accessible que depuis le panneau « parcours » (volet `<details>`) et `year glossary` ; il gagnerait un point d'entrée global (palette, `?`).
**s2 —** Le message « Aucun IS dû (résultat fiscal nul ou déficitaire) : pas de solde à verser » devrait ajouter « mais le relevé 2572 reste à télédéclarer à zéro » si c'est le cas (à vérifier par un fiscaliste).
**s3 —** L'écran `facturation` vide dit « émettez-en une depuis la CLI : freeflow invoice emit » — même symptôme que B1 côté facturation.

## 3. Ce qui fonctionne bien

- Création du coffre : écran clair, avertissement « aucune récupération » explicite, confirmation de passphrase vérifiée (« les deux saisies ne correspondent pas »), écran de déverrouillage propre (« passphrase incorrecte, ou fichier de coffre corrompu »).
- Bilan d'ouverture dans la fenêtre : aide de champ précise, trois erreurs testées et bien expliquées (« bilan déséquilibré : total débit 900,00 €, total crédit 1 000,00 € », « le compte 622600 n'est pas un compte de bilan (classes 1 à 5) », « ligne de bilan invalide : … (attendu `compte:libellé:D|C:montant`, ex. …) »), récapitulatif « Capital repris / Réserve légale reprise / Report à nouveau repris 3 900 € » (120 réputé affecté, comme annoncé), badge « modifiable ».
- Dépenses depuis le relevé : bloc « débit du relevé à rapprocher », formulaire pré-rempli (libellé, montant verrouillé, date), justificatif archivé par hash, garde « la TVA déductible ne peut pas dépasser le montant total ». Réimport d'un même CSV dédoublonné (« ✓ 0 »).
- Parcours de clôture : lisible, statuts explicites, montants chiffrés (« Déficit fiscal de 895,00 € : reportable en avant… »), échéances d'une clôture décalée correctes (approbation 30/03/2027, solde IS 15/01/2027, liasse 30/12/2026, greffe dans le mois suivant l'AG, 2 mois par voie électronique), commande exacte proposée en CLI, boutons branchés dans la fenêtre. Le lexique (18 entrées) est vraiment écrit pour un débutant.
- Documents : PV, décision d'affectation, compte de résultat, bilan 2033-A + balance (équilibré, 4 105 € = 4 105 €), liasse, FEC (`901265322FEC20260930.txt`, journaux AN/AC/BQ, 401 pour les dépenses rapprochées) — tous téléchargeables depuis la fiche, tous avec la mention « modèle indicatif à faire relire ».
- Gardes sur l'immuabilité : suppression et modification refusées après approbation ; rapprochement bancaire encore possible après clôture (comme documenté).

## 4. Recommandations priorisées

1. **Garde-fous de clôture (B2)** : refuser `ends_on > aujourd'hui` et `approved_on > aujourd'hui` dans le cœur (ou exiger une confirmation dédiée), griser « clore » quand le parcours est bloquant, corriger `period_ended_step`.
2. **Écran « Ma société » (B1)** + assistant de première configuration (voir §6).
3. **Import de relevé dans la fenêtre (M3)** avec mapping de colonnes et message d'erreur qui donne le format ; documenter le format dans `bank import --help`.
4. **Étape TVA dans le parcours (M2)** et catégorie « impôts et taxes » (M4).
5. **Rendu texte des commandes (M1)** : `company show`, `fiscal calendar`, `set-profile`.
6. **Import de balance/FEC (B3)** pour le bilan d'ouverture — même un simple CSV `compte;libellé;débit;crédit`.
7. Corriger le texte « réserve légale cumulée » (M5), les défauts de l'écran `cloture` (m2), la présélection du rapprochement (m3), `--today` par défaut (m4), dates ISO dans la liasse (m5).

## 5. Chronologie honnête

1. **Création du coffre** (fenêtre) : compris immédiatement. 0 lecture externe.
2. **Premier écran** : dashboard de prospection, aucun « que faire ensuite ». J'ai cliqué tous les onglets ; « cloture » avait un bouton « parcours » — c'est lui qui m'a dit qu'il manquait le profil. **Perdu n° 1** : le bouton m'envoie sur une console vide. J'ai tapé `help`, puis `company set-profile --help`, j'ai saisi les champs obligatoires seulement → parcours en année civile. J'ai dû **relire l'aide** pour trouver `--fiscal-year-end` et `--vat-regime` ; pour le régime, j'ai choisi `real_normal_monthly` au hasard raisonnable (première lecture du README nécessaire pour comprendre que « mensuel supposé » pointait vers ça). **README : 1.**
3. **Bilan d'ouverture** : le formulaire explique la syntaxe ; mais je n'aurais pas su, avec le PDF du cabinet, quels numéros de compte écrire (« Disponibilités » ≠ 512000). J'ai lu le README (« Recopie le dernier bilan du cabinet… 101000/110000/512000 »). **README : 2.**
4. **Relevé bancaire** : rien dans la fenêtre. Le parcours dit `bank import`. Mon export Qonto est refusé sans explication du format ; j'ai **lu le code** (`billing/import.rs`) pour connaître `date;description;montant`. **Code : 1.** J'ai réécrit le CSV à la main.
5. **Dépenses** : très bien guidé par le bloc « débits à rapprocher ». Hésitation sur la CFE (aucune catégorie « impôts ») et sur la TVA des honoraires (le champ « TVA déductible » à 0 par défaut aurait pu me faire oublier les 120 €).
6. **Clôture** : le parcours disait « bloquant : exercice écoulé » ; j'ai quand même cliqué « + clore » pour voir → accepté. J'ai supprimé le projet, refait les dépenses, puis reclos (le bouton l'accepte toujours) et approuvé au 15/12/2026 → immuable. Un débutant pressé aurait fait exactement ça. **Perdu n° 2** (et définitivement).
7. **Documents** : six téléchargements depuis la fiche, tout lisible. Mais « et maintenant ? » : le parcours nomme EDI-TDFC, 2572, greffe, sans lien ni explication de « partenaire EDI » ; j'ai lu la section « Clôturer seul » du README pour comprendre qu'il faut passer par un cabinet ou un partenaire EDI. **README : 3.**
8. **TVA** : jamais mentionnée dans le parcours ; j'ai découvert le crédit de 120 € en CLI avec `fiscal calendar --today 2026-02-01`, dont la sortie est illisible (`Object {…}`).
9. **CLI** : `freeflow --help` et `year --help` compréhensibles ; `year checklist 2026` excellent ; `year glossary` excellent ; `company show`/`fiscal calendar` illisibles.

Bilan : **3 lectures du README, 1 lecture du code**, 2 moments où j'étais réellement perdu, 1 erreur irréversible commise sans avertissement.

## 6. Propositions d'aide intégrée (par impact décroissant)

1. **Assistant de première configuration** (après création du coffre, ré-ouvrable depuis la palette) : 4 écrans — *Ma société* (formulaire avec aide par champ : « date de clôture : sur vos statuts ou votre dernier bilan », régime de TVA expliqué en une phrase par option), *D'où venez-vous ?* (« société existante → importez ou saisissez le bilan d'ouverture » / « société nouvelle »), *Votre banque* (import du relevé, mapping de colonnes), *Prochaine étape* (lien vers le parcours de l'exercice en cours).
2. **Écran « Ma société »** persistant (onglet ou palette) avec les mêmes aides ; supprimer tout renvoi « console : company set-profile » de la fenêtre.
3. **Import du bilan d'ouverture** : coller/importer une balance CSV ou un FEC ; à défaut, un texte « demandez à votre cabinet la *balance de clôture au 30/09/2025* ; recopiez seulement les comptes qui commencent par 1, 2, 3, 4 ou 5 » et un tableau (compte / libellé / débit / crédit) à la place du textarea.
4. **Import de relevé dans la fenêtre** + format documenté dans l'erreur et dans `--help` + presets Qonto/Shine/banques classiques.
5. **Étape TVA** dans le parcours, avec « à déclarer chaque mois même à zéro (CA3 néant) », crédit en cours, lien impots.gouv.fr.
6. **Bandeau « prochaine étape »** sur le dashboard tant que profil/bilan d'ouverture/relevé manquent (les KPI de prospection en dessous).
7. **Messages d'erreur en langage courant** : remplacer « règle métier violée » par une phrase et un bouton (« Cet exercice est déjà clos en projet — le supprimer ? »), jamais une commande CLI dans la fenêtre.
8. **Aides de champ manquantes** : « TVA déductible : en général montant × taux/(1+taux), laissez 0 si la facture ne mentionne pas de TVA » ; catégorie « impôts et taxes (CFE, TVS…) » ; sur « approuver » : « la date doit être celle où vous signez réellement le PV ».
9. **Catégorie/étape « et après ? »** dans la fiche d'un exercice approuvé : trois cartes datées (liasse : par qui, comment ; solde IS : où ; greffe : URL du guichet unique), reprenant le texte du README « Clôturer seul » qui existe déjà mais que l'app ne montre pas.
10. **Console** : réponse à `aide`/`help`/ligne vide en français avec les 5 commandes utiles, et placeholder `year checklist 2026`.
