#!/usr/bin/env bash
# Sème le coffre mock Lumen Conseil. Jamais le coffre ENGIT.
set -euo pipefail
root=$(git rev-parse --show-toplevel)
cd "$root"
out=${GRIFFE_LUMEN_OUT:-"$root/testdata/lumen/vault.db"}
case "$out" in
  *'/.local/share/freeflow/vault.db')
    echo "✗ refuse d'écrire le coffre par défaut (ENGIT)" >&2
    exit 1
    ;;
esac
mkdir -p "$(dirname "$out")"
rm -f "$out" "$out.kdf" "${out}.kdf"
export GRIFFE_TEST_KDF=1 GRIFFE_NO_OPEN=1 CARGO_INCREMENTAL=0
export GRIFFE_LUMEN_OUT="$out"
export GRIFFE_LUMEN_PASSPHRASE_FILE="$root/scripts/lumen/passphrase"
taskset -c 0,1 nice -n 10 env CARGO_BUILD_JOBS=1 \
  cargo nextest run -p griffe-cli --test lumen_demo --test-threads 1 --build-jobs 1
echo "✓ coffre Lumen : $out"
