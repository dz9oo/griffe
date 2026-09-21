# Coffre mock Lumen Conseil

Société fictionnelle pour captures marketing. Aucune donnée réelle.

## Recette

Jamais `~/.local/share/freeflow/vault.db` (coffre ENGIT).

```bash
# 1. Semer le coffre (KDF de test, jobs 1)
scripts/lumen/seed.sh

# 2. Servir l'UI à la date du matin type
FREEFLOW_DB=$PWD/testdata/lumen/vault.db \
GRIFFE_TODAY=2026-09-05 \
FREEFLOW_WEB_ADDR=127.0.0.1:3417 \
  taskset -c 0,1 nice -n 10 env CARGO_BUILD_JOBS=1 \
  cargo run -p griffe-web --bin griffe-web-dev

# 3. Capturer
scripts/lumen/shots.sh testdata/lumen/shots
```

Passphrase : `scripts/lumen/passphrase` (`lumen-conseil`). Pas de `--remember`.

Date figée : **5 septembre 2026**. Le coffre n’est pas versionné (`*.db` / `*.kdf`).
