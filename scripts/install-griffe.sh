#!/bin/sh
# Copie un AppImage Griffe déjà téléchargé vers ~/.local et écrit le menu.
# Aucun réseau, pas de sudo, ne touche pas ~/.local/share/griffe/.
# Refuse le fichier si le .minisig voisin ne vérifie pas.
set -eu

# Clé publique minisign, copie exacte des deux lignes de packaging/minisign.pub.
# Un script copié seul vérifie avec ces lignes. Dans un clone, le fichier du
# dépôt doit être identique.
MINISIGN_PUB_COMMENT='untrusted comment: minisign public key BB297F887583C599'
MINISIGN_PUB_KEY='RWSZxYN1iH8pu9EsvAA07kQZZ8hfSS0QI4Sk5kzpk7Iyw/aIPmoEYsrx'

# --- verify-signed-file ---
verify_signed_file() {
  sig_file=$1
  sig_path=$sig_file.minisig
  if [ ! -f "$sig_path" ]; then
    printf '%s\n' "signature absente : $sig_path" >&2
    printf '%s\n' "rien n'est installé" >&2
    exit 1
  fi
  if ! command -v minisign >/dev/null 2>&1; then
    printf '%s\n' "minisign manquant : installe le paquet minisign, puis relance" >&2
    exit 1
  fi
  sig_pub=$(mktemp)
  printf '%s\n%s\n' "$MINISIGN_PUB_COMMENT" "$MINISIGN_PUB_KEY" >"$sig_pub"
  sig_script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
  sig_repo=$sig_script_dir/../packaging/minisign.pub
  if [ -f "$sig_repo" ]; then
    if ! cmp -s "$sig_pub" "$sig_repo"; then
      rm -f "$sig_pub"
      printf '%s\n' "la clé packaging/minisign.pub et la clé embarquée dans le script divergent" >&2
      exit 1
    fi
  fi
  sig_err=$(mktemp)
  if ! sig_comment=$(minisign -V -H -Q -m "$sig_file" -x "$sig_path" -p "$sig_pub" 2>"$sig_err"); then
    printf '%s\n' "signature refusée pour $(basename "$sig_file")" >&2
    cat "$sig_err" >&2
    rm -f "$sig_pub" "$sig_err"
    exit 1
  fi
  rm -f "$sig_pub" "$sig_err"
  sig_base=$(basename "$sig_file")
  if [ "$sig_comment" != "$sig_base" ]; then
    printf '%s\n' "commentaire de confiance inattendu pour $sig_base" >&2
    exit 1
  fi
}
# --- verify-signed-file ---

usage() {
  echo "usage: install-griffe.sh CHEMIN.AppImage" >&2
  exit 1
}

[ $# -eq 1 ] || usage

src=$1

case "$src" in
  *.AppImage) ;;
  *)
    echo "attendu un fichier .AppImage" >&2
    usage
    ;;
esac

if [ ! -f "$src" ]; then
  echo "fichier introuvable: $src" >&2
  exit 1
fi

if [ -z "${HOME:-}" ]; then
  echo "HOME n'est pas défini" >&2
  exit 1
fi

case "$HOME" in
  /*) ;;
  *)
    echo "HOME doit être un chemin absolu" >&2
    exit 1
    ;;
esac

verify_signed_file "$src"

if [ ! -x "$src" ]; then
  chmod +x "$src"
fi

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
fallback="$script_dir/../crates/griffe-desktop/icons/icon.png"

bin="$HOME/.local/bin/griffe"
icon="$HOME/.local/share/icons/hicolor/256x256/apps/io.github.dz9oo.griffe.png"
desktop="$HOME/.local/share/applications/io.github.dz9oo.griffe.desktop"

tmp=$(mktemp -d)
rollback=1
cleanup() {
  if [ "${rollback:-0}" -eq 1 ]; then
    rm -f "$bin" "$icon" "$desktop"
  fi
  rm -rf "$tmp"
}
trap cleanup EXIT

install -D "$src" "$bin"
chmod +x "$bin"

icon_src=

if (cd "$tmp" && "$bin" --appimage-extract >/dev/null 2>&1); then
  for f in "$tmp/squashfs-root/usr/share/icons/hicolor/256x256/apps/"*.png; do
    if [ -f "$f" ]; then
      icon_src=$f
      break
    fi
  done
  if [ -z "$icon_src" ] && [ -d "$tmp/squashfs-root/usr/share/icons" ]; then
    f=$(find "$tmp/squashfs-root/usr/share/icons" -type f -name '*.png' 2>/dev/null | sed -n '1p' || true)
    if [ -n "$f" ] && [ -f "$f" ]; then
      icon_src=$f
    fi
  fi
fi

if [ -z "$icon_src" ] && [ -f "$fallback" ]; then
  icon_src=$fallback
fi

if [ -z "$icon_src" ]; then
  echo "aucune icône PNG (extraction AppImage vide et $fallback absent)" >&2
  exit 1
fi

install -D "$icon_src" "$icon"

mkdir -p "$HOME/.local/share/applications"
cat >"$desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Griffe
Exec=$bin
Icon=io.github.dz9oo.griffe
StartupWMClass=Griffe-desktop
Terminal=false
Categories=Office;Finance;
EOF

rollback=0
printf '%s\n' "$bin" "$icon" "$desktop"
