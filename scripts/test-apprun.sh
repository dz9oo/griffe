#!/bin/sh
# Contrôle statique de l'AppRun publié. Pas d'AppImage, pas de réseau.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
app="$root/packaging/appimage/AppRun"
wf="$root/.github/workflows/bundle.yml"

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

[ -f "$app" ] || fail "packaging/appimage/AppRun manquant"
[ -x "$app" ] || fail "AppRun n'est pas exécutable"
head -n 1 "$app" | grep -qx '#!/bin/bash' || fail "shebang bash"
bash -n "$app" || fail "syntaxe bash"
magic=$(od -An -N4 -tx1 "$app" | tr -d ' \n')
[ "$magic" != "7f454c46" ] || fail "AppRun ne doit pas être un ELF"
grep -q 'ARGV0' "$app" || fail "ARGV0"
grep -q 'unset ARGV0' "$app" || fail "unset ARGV0"
grep -q 'exec -a' "$app" || fail "exec -a"
grep -q -- '--appimage-' "$app" || fail "refus --appimage-"
grep -q 'cd "$here/usr"' "$app" || fail "cd vers usr"
grep -q 'LD_LIBRARY_PATH' "$app" || fail "LD_LIBRARY_PATH"
grep -q 'XDG_DATA_DIRS' "$app" || fail "XDG_DATA_DIRS"
if grep -q 'GST_PLUGIN_SYSTEM_PATH' "$app"; then
  fail "pas de GST_PLUGIN_SYSTEM_PATH"
fi
if grep -q 'APPIMAGE' "$app"; then
  fail "ne pas décider le mode avec APPIMAGE"
fi

grep -q 'packaging/appimage/AppRun' "$wf" || fail "bundle.yml ne copie pas AppRun"
grep -q 'appimagetool' "$wf" || fail "recompression absente"
# La recompression ne doit pas rester dans le seul if libwayland.
awk '
  /^          if find squashfs-root -name .libwayland-client/ { inif = 1 }
  inif && /appimagetool/ { inside = 1 }
  inif && /^          fi$/ { inif = 0 }
  END { exit inside ? 1 : 0 }
' "$wf" || fail "appimagetool est encore conditionné à libwayland"

echo "ok: scripts/test-apprun.sh"
