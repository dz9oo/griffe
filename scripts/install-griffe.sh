#!/bin/sh
# Copie un AppImage Griffe déjà téléchargé vers ~/.local et écrit le menu.
# Aucun réseau, pas de sudo, ne touche pas ~/.local/share/freeflow/.
set -eu

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
