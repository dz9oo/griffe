# Griffe

Lettre du matin pour un indépendant en SASU/EURL à l’IS : local-only, chiffré, sans serveur.

Vitrine : [https://dz9oo.github.io/freeflow/](https://dz9oo.github.io/freeflow/).
Captures : [`site/shots/`](site/shots/).

## Ce que ça fait

Prospection, devis, missions, facturation Factur-X, dépenses, clôture d’exercice,
coffre SQLCipher sur ta machine.

## Ce que ça ne fait pas

Pas d’avis fiscal, pas d’envoi de mail, pas de télétransmission, pas une
plateforme agréée. Griffe calcule et rappelle ; tu déposes ailleurs.

## Installation

```bash
nix develop
just check
cargo run -p griffe-desktop
```

## Architecture

Une seule couche applicative (`griffe-core::app`). CLI, MCP et fenêtre sont
trois adaptateurs équivalents : une règle métier n’y entre pas. Mutations via
`Command` (dry-run, idempotence, confirmation, audit chaîné) ; lectures via
`Query`. Aucun port TCP — Tauri exécute `griffe://` en mémoire contre le
routeur axum. Coffre SQLCipher, WAL multi-process, zéro connexion sortante.

Détail : [`docs/architecture.md`](docs/architecture.md). Invariants pour un
agent : [`AGENTS.md`](AGENTS.md).

## Licence

Le code est ouvert. Tu peux le lire, le modifier, t’en servir pour ta propre
activité. Tu ne peux pas vendre Griffe, le proposer en service hébergé, ni le
présenter comme un substitut, sans accord. Licence PolyForm Shield 1.0.0
([`LICENSE`](LICENSE), [`NOTICE`](NOTICE)).

« Griffe » désigne ce projet. Un fork se renomme.

## Contribuer

[`CONTRIBUTING.md`](CONTRIBUTING.md). Critère : `just check`.

## Sécurité

[`SECURITY.md`](SECURITY.md).

## Contact

[hello@nicolascollier.dev](mailto:hello@nicolascollier.dev)
