# Architecture — Studio

Griffe n’a **qu’une couche applicative**. CLI, serveur MCP et fenêtre desktop
sont trois façades du même cœur. Cette page dit le *pourquoi* pour un humain.
Les invariants que le code ne doit pas casser en silence sont dans
[`AGENTS.md`](../AGENTS.md).

## Thèse

Tout calcul, toute validation, toute transition d’état vit dans
`griffe-core::app`. Si on s’apprête à l’écrire dans `griffe-cli`,
`griffe-mcp` ou `griffe-web`, elle est mal placée.

Les trois adaptateurs sont **strictement équivalents**. Taper une commande
dans la console de la fenêtre et la taper dans un terminal empruntent le
même chemin : `griffe-cli` expose son parseur `clap` en bibliothèque
(`run_capturing` / `run_capturing_with_vault`), et `griffe-web` l’appelle
tel quel. Pas d’interpréteur maison.

C’est volontaire. Un indépendant pilote Griffe depuis un terminal, depuis
un agent (MCP, stdio), ou depuis la lettre du matin. Les trois voient le
même métier, les mêmes gardes, le même journal d’audit.

## Command et Query

Une **mutation** est un type qui implémente `griffe_core::app::Command`,
exécutée par `Executor::execute`. L’exécuteur gère uniformément :

- le **dry-run** — n’écrit rien, montre ce qui serait fait ;
- l’**idempotence** — deux exécutions avec la même clé n’ont qu’un effet ;
- la **confirmation** — une commande sensible déclenchée par un acteur
  `Agent` crée une `PendingAction` ; seul un humain (`griffe confirm`)
  la valide ;
- l’**audit** append-only, chaîné par hash.

Une **lecture** est une `Query` : fonction libre sur `&Connection`, hors
Executor. Pas de dry-run, pas d’audit, pas de confirmation.

`Command::apply` ne touche que `&Connection`. Hasher un justificatif,
copier un fichier, appeler Typst : dans l’adaptateur, *avant* la commande.
Le cœur reste testable sans disque annexé.

## Pourquoi zéro socket (`griffe://`)

Aucun port TCP n’est ouvert, y compris pendant que la GUI tourne. Tauri
enregistre le protocole `griffe://`. Le handler convertit la requête
webview en `http::Request` et l’exécute **en mémoire** contre le
`axum::Router` de `griffe-web` (`tower::Service::oneshot`).

Zéro socket → zéro surface CSRF / DNS-rebinding depuis un autre process
ou un onglet de navigateur. La fenêtre n’est pas un serveur local
déguisé.

Aucune connexion sortante. Les relances produisent un brouillon `.eml`
ouvert dans le client mail. Pas de Google Fonts, pas de CDN.

## WAL multi-process

Pas de démon. La GUI, la CLI et le MCP ouvrent chacun le fichier
SQLCipher. SQLite en WAL gère la concurrence inter-process.

Le rail d’audit live de la GUI n’est pas un pub/sub : `griffe-web`
interroge `PRAGMA data_version` à intervalle court et pousse une mise
à jour dès qu’un autre process a écrit — polling htmx, pas SSE (le
transport URI ne streame pas : un `UriSchemeResponder` Tauri v2 n’a
qu’un buffer unique).

## Exception session

`init` / `unlock` / `lock` / `vault status` divergent entre adaptateurs :
CLI et MCP partagent une session dans le trousseau OS ; la fenêtre en
tient une distincte en mémoire. Verrouiller l’une ne referme pas l’autre.

Ce n’est pas une règle métier (calcul, validation, transition d’état),
donc ça ne viole pas Studio — mais c’est **la seule** zone où
« CLI / MCP / GUI strictement équivalents » ne s’applique pas.

La console de la fenêtre refuse `init` / `unlock` / `lock` /
`passphrase change` / `backup restore` : mauvais objet de session, ou
mode d’échec qui exige un vrai terminal. Elle exécute
`run_capturing_with_vault(..., VaultAccess::Borrowed { store, db_path })`
sur le `Store` déjà ouvert, jamais une seconde connexion, jamais le
prompt TTY.

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

Le reste (facture immuable, coffre v3, Nix, tests, fiscal, UI Atelier) :
[`AGENTS.md`](../AGENTS.md).
