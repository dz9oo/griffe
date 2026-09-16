# Contribuer

## Setup

    direnv allow    # ou : nix develop
    just check      # fmt, clippy, tests, licences

Nix fournit Rust (voir `rust-toolchain.toml`), `cargo-nextest`, `typst`,
`sqlcipher`, GTK/WebKit (Tauri).

## Critère

`just check` vert. Pour un changement de schéma ou de crate visible par Nix :
`nix flake check` après `git add` (Nix ne voit que l’index).

## Patch

Une intention par PR. L’UI est en français. Le métier va dans `griffe-core`,
pas dans CLI/MCP/GUI. Lire `AGENTS.md` et `docs/architecture.md`.

Règle fiscale, document Cerfa, FEC, liasse : `docs/legal.md`. Source
primaire + test chiffré à la main. CODEOWNERS demandera une revue `@dz9oo`.

## Licence

En envoyant un patch, tu le places sous la même licence que le projet
(PolyForm Shield 1.0.0, voir `LICENSE`). Pas de CLA.

## Conduite

Voir `CODE_OF_CONDUCT.md`. Vulnérabilité coffre/crypto : `SECURITY.md`,
pas une issue publique.
