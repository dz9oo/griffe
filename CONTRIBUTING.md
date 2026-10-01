# Contribuer

Griffe est en **alpha** (Linux, SASU à l’IS). Attends-toi à des aspérités.
N’ouvre pas une issue avec un coffre, un SIREN réel, une passphrase ou un
relevé de production.

## Setup

    direnv allow    # ou : nix develop
    just check      # fmt, clippy, tests, licences

Nix fournit Rust (voir `rust-toolchain.toml`), `cargo-nextest`, `typst`,
`sqlcipher`, GTK/WebKit (Tauri).

## Critère

`just check` vert **en local** avant de pousser : la CI GitHub rejoue exactement
cette recette (pas de trousseau OS, `GRIFFE_NO_OPEN=1`, `GRIFFE_TEST_KDF=1`).
Un push rouge brûle des minutes Actions après 10+ min de compilation.

Pour un changement de schéma ou de crate visible par Nix :
`nix flake check` après `git add` (Nix ne voit que l’index).

## Dépôt

`master` est protégée. Une PR, une revue `@dz9oo`, CI `check` et `desktop`
vertes. Les tags `v*` aussi : création, suppression et force-push réservés
à `@dz9oo`.

Couper un tag `vX.Y.Z` exige une section `## X.Y.Z` dans `CHANGELOG.md`.
Chaque version publiée en a une. On y écrit ce qui change pour la personne
qui s'en sert, dans ses mots : ce qu'elle voit, ce qu'elle peut faire.
Pas de jargon de développement, pas de nom de crate, de test ou de commit.
Le corps de la release GitHub et le journal du paquet pacman
(`pacman -Qc griffe-bin`) reprennent cette section, via
`packaging/release-notes.sh`. Le script échoue si elle manque ou si elle
est vide.

Les contributeurs externes travaillent depuis un **fork**. Un coup de
main sur les issues se fait en rôle Triage. Write et Admin restent à
`@dz9oo`, qui merge.

Tu peux pousser autant de correctifs que besoin sur la branche de la PR.
Chaque nouveau push retire les revues déjà données : le code à jour est
relu avant le merge.

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
