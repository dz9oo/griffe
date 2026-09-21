# AGENTS.md — Griffe

Application desktop de gestion pour un indépendant en SASU à l’IS
(prospection, missions, devis, facturation Factur-X, dépenses, obligations
fiscales indicatives, prévisionnel de trésorerie). Local-only, chiffré, sans
serveur.

Tu es un agent qui patch un logiciel d’argent et de conformité. Une règle
fausse coûte plus cher qu’un test en trop. Lis aussi `docs/legal.md`,
`docs/testing.md`, `docs/design.md`, `CONTRIBUTING.md`.

## Thèse Studio

Il n’y a **qu’une seule couche applicative**, `griffe-core::app`.
`griffe-cli`, `griffe-mcp` et `griffe-desktop`+`griffe-web` sont trois
adaptateurs strictement équivalents. Une règle métier (calcul, validation,
transition d’état) écrite dans un adaptateur est mal placée : elle va dans
`griffe-core`.

Chaque mutation passe par un type `griffe_core::app::Command`, exécutée via
`Executor::execute` : dry-run, idempotence, confirmation humaine si
`requires_confirmation` et acteur `Agent`, journal d’audit chaîné par hash.

Chaque lecture est une `Query` : fonction libre sur `&Connection`, hors
Executor.

Preuve : la console de la fenêtre appelle `griffe_cli::run_capturing` /
`run_capturing_with_vault`. Pas d’interpréteur maison.

## Pas de serveur, pas de socket, pas de connexion sortante

Aucun port TCP. Tauri enregistre `griffe://` ; le handler exécute le
`axum::Router` **en mémoire** (`tower::Service::oneshot`). GUI, CLI et MCP
ouvrent le même fichier SQLCipher (WAL). Le rail d’audit de la GUI poll
`PRAGMA data_version` (pas de SSE : le transport URI ne streame pas).

Les relances produisent un `.eml` ouvert dans le client mail. Rien n’est
envoyé. Pas de Google Fonts, pas de CDN.

## Crates

```
crates/
  griffe-core/      domaine + persistance + app — LE cœur
    src/domain/     types sans IO (Money, VatRate, Siren, …)
    src/store/      SQLCipher, migrations, sauvegarde
    src/app/        Command/Query, Executor, audit, PendingAction
    src/*.rs        modules métier (Command + Query + tests sur vraie base)
  griffe-cli/       clap → core, exposé en bibliothèque
  griffe-mcp/       rmcp stdio → core (outils = commandes, ressources = requêtes)
  griffe-web/       axum + Maud + htmx ; pas de JS de framework
  griffe-desktop/   Tauri v2 ; griffe:// → Router
  griffe-invoice/   Typst → PDF/A-3b + XML CII EN 16931
  griffe-docs/      Typst : PV, affectation, synthèse, 2033-A, liasse JSON
```

`domain` : aucune IO. `Money` = `i64` centimes. Ids = newtypes.

## Invariants (ne pas casser en silence)

- **Money sans flottant.** Toute arithmétique via `domain::Money`.
- **Facture émise immuable.** Ni update ni delete : un avoir. Numérotation
  `FA-{YYYY}-{NNNN}` allouée dans la même transaction `IMMEDIATE` que
  l’insertion (concurrence inter-process, pas seulement inter-thread).
- **Audit chaîné.** `griffe audit verify-chain` détecte une altération SQL.
- **`Command::apply` ne touche que `&Connection`.** Hash de justificatif,
  copie de fichier, Typst : dans l’adaptateur, *avant* la Command.
- **Pas de passphrase en clair** (disque, env, historique). Coffre v3 :
  clé maître aléatoire, sidecar `.kdf`, Argon2id. Sources : TTY masqué,
  `--passphrase-file`, `--passphrase-command`, `--passphrase-stdin`.
  Types `Passphrase` / `VaultKey` zeroizants, pas de `Debug` utile.
- **`griffe-core` est clippy pedantic.** Un `expect`/`panic!` porte `# Panics`.
- **Sauvegarde auto** après ouverture du coffre (CLI `dispatch`), sauf
  `backup restore` qui s’exécute *avant* d’ouvrir le coffre cassé.

## Exception session (seule faille assumée à Studio)

`init` / `unlock` / `lock` / `vault status` : CLI+MCP partagent le trousseau
OS ; la fenêtre tient une session en mémoire. La console refuse
`init`/`unlock`/`lock`/`passphrase change`/`backup restore` (mauvais objet
de session, ou mode d’échec qui exige un vrai terminal). Elle exécute
`run_capturing_with_vault(..., VaultAccess::Borrowed { store, db_path })`.

## Nix

`direnv allow` puis `nix develop`. Le shell pose `GRIFFE_TEST_KDF=1`,
`GRIFFE_NO_OPEN=1` et `CARGO_INCREMENTAL=0` : un `cargo nextest` nu
n'a plus à les répéter. `just check` = fmt-check, clippy `-D warnings` (hors fenêtre), machete,
nextest, deny+audit, puis clippy Tauri/`griffe-desktop`. `nix flake check`
avant de dire « terminé ».
Une invocation `nix develop -c …` longue peut être signalée « killed »
alors que le process continue : vérifier `ps -p`.

## Tests

Lire `docs/testing.md`. Pas de mock SGBD. Un test existe s’il protège un
invariant d’argent, de conformité, ou de données.

## Fiscal

Lire `docs/legal.md`. Tu ne « corriges » pas une règle de TVA, d’IS, de
liasse ou de FEC sur ta culture générale. Source primaire (BOFIP, CGI,
C. com., notice Cerfa) + montant attendu posé à la main dans le test.
CODEOWNERS (`* @dz9oo`, plus les chemins fiscaux) exige une revue humaine.

## UI Atelier

Lire `docs/design.md` et `docs/atelier-design.md`. Lettre du matin, trois
pièces (Le jour / Les affaires / La société). Pas de dashboard à tuiles,
pas de sigle (CA3, 3514, 2777) dans la lettre. Pas de `hx-on--*` (CSP
`script-src 'self'`, htmx ferait `new Function`). Succès mutation :
`200` vide + `HX-Trigger: griffe:saved` (pas `204`).

## Ce que tu ne fais pas

- Commit, force-push, publication, filter-repo, sans demande explicite.
- Pousser sur `master` : une PR, revue `@dz9oo`. Bypass seulement via l’UI
  GitHub, sur demande explicite.
- Inviter un collaborateur Write ou Admin.
- Inventer une licence, un calcul fiscal, ou un écran « module ».
- Réintroduire `FREEFLOW_PASSPHRASE`, un import Google Fonts, un socket.
- Écrire du métier dans `griffe-cli` / `griffe-mcp` / `griffe-web`.
