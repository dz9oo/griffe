# Tests

Un test n’existe que s’il protège un invariant dont la violation coûte
de l’argent, de la conformité, ou des données.

## Doctrine

- **Pas de mock du SGBD.** Persistance = vraie base SQLCipher temporaire.
- **`proptest`** si l’espace d’entrée est grand (Money, calendrier,
  prévisionnel, parseurs CSV/OFX).
- **Concurrence réelle** (threads *et* process) sur la numérotation de facture.
- **Snapshots `insta`** = contrats externes (`--json`, `--help`, schémas MCP,
  XML CII). Une régression silencieuse y est coûteuse.
- **Montants à la main.** Le chiffre attendu est écrit dans le test *avant*
  l’assert (patron `crates/griffe-cli/tests/closing_scenario.rs`). Le test
  se plie au chiffre, jamais l’inverse.
- **Standards officiels.** XML CII validé `xmllint --schema` contre le XSD
  EN 16931 vendorisé. Limite : Schematron XPath 2.0 hors périmètre.

## Commande

`just check` (fmt, clippy `-D warnings`, machete, nextest, deny+audit).
`nix flake check` avant de considérer un chantier terminé.
