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

# Largeur du viewport. Un suffixe sépare la passe étroite (lisible sur un téléphone).
width=${GRIFFE_SHOT_WIDTH:-1200}
suffix=${GRIFFE_SHOT_SUFFIX:-}

shot() {
  local name=$1 path=$2 height=${3:-800}
  "$bin" --headless --disable-gpu --hide-scrollbars --no-first-run \
    --window-size="${width},${height}" \
    --virtual-time-budget=4000 \
    --screenshot="$out/${name}${suffix}.png" \
    "${base}${path}" >/dev/null
  echo "  ✓ ${name}${suffix}.png"
}

# Hauteurs : la phrase et les gestes entiers. Le mois peut être hors cadre.
# La lettre de TVA s'arrête avant le formulaire qui nomme le sigle.
if [[ "$width" -lt 700 ]]; then
  shot jour /jour 1280
  shot ecrire "/affaires/Camille%20Rivi%C3%A8re/ecrire" 1500
  shot releve /societe/releve 1280
  shot payer /societe/payer 1100
  shot impots /societe/impots/ca3/2026-08 980
else
  shot jour /jour 1040
  shot affaires /affaires 900
  shot societe /societe 1100
  shot camille "/affaires/Camille%20Rivi%C3%A8re" 900
  shot ecrire "/affaires/Camille%20Rivi%C3%A8re/ecrire" 980
  shot payer /societe/payer 860
  shot impots /societe/impots/ca3/2026-08 760
  shot releve /societe/releve 1040
fi

echo "✓ captures dans $out"
