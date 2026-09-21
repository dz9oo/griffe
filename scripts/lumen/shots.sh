#!/usr/bin/env bash
# Photos de l'app réelle (griffe-web-dev) sur le coffre Lumen.
# Prérequis : seed.sh déjà joué ; web-dev écoute FREEFLOW_WEB_ADDR.
set -euo pipefail
root=$(git rev-parse --show-toplevel)
addr=${FREEFLOW_WEB_ADDR:-127.0.0.1:3417}
base="http://${addr}"
out=${1:-"$root/testdata/lumen/shots"}
passfile=${GRIFFE_LUMEN_PASSPHRASE_FILE:-"$root/scripts/lumen/passphrase"}
pass=$(tr -d '\n' <"$passfile")
mkdir -p "$out"

bin=$(command -v chromium || command -v google-chrome-stable || command -v google-chrome || command -v chromium-browser || true)
if [[ -z "$bin" ]]; then
  echo "✗ chromium introuvable" >&2
  exit 1
fi

echo "attente de $base …"
ok=0
for _ in $(seq 1 60); do
  if curl -sf -o /dev/null "$base/unlock"; then
    ok=1
    break
  fi
  sleep 1
done
if [[ "$ok" -ne 1 ]]; then
  echo "✗ $base ne répond pas — lancez griffe-web-dev" >&2
  exit 1
fi

# La session est en mémoire côté serveur : un POST déverrouille toutes les GETs suivantes.
curl -sf -o /dev/null -X POST "$base/unlock" \
  -H 'Content-Type: application/x-www-form-urlencoded' \
  --data-urlencode "passphrase=${pass}"

shot() {
  local name=$1 path=$2 height=${3:-800}
  "$bin" --headless --disable-gpu --hide-scrollbars --no-first-run \
    --window-size=1200,"$height" \
    --virtual-time-budget=4000 \
    --screenshot="$out/${name}.png" \
    "${base}${path}" >/dev/null
  echo "  ✓ ${name}.png"
}

shot jour /jour 800
shot affaires /affaires 900
shot societe /societe 1100
shot camille "/affaires/Camille%20Rivi%C3%A8re" 900
shot payer /societe/payer 900
shot impots /societe/impots 900
shot releve /societe/releve 800

echo "✓ captures dans $out"
