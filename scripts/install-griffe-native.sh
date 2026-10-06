#!/bin/sh
# Copie un ELF Griffe et Typst déjà fournis vers ~/.local et écrit le menu.
# Aucun réseau, pas de sudo, ne touche pas ~/.local/share/griffe/.
# Une archive est refusée si le .minisig voisin ne vérifie pas.
# Un répertoire local (déjà déballé) n'a pas de signature.
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
  echo "usage: install-griffe-native.sh CHEMIN.tar.xz|CHEMIN.tar.gz|REPERTOIRE" >&2
  exit 1
}

# racine d’archive : le premier griffe-desktop trouvé à profondeur 1 ou 2
find_payload() {
  base=$1
  if [ -f "$base/griffe-desktop" ]; then
    echo "$base"
    return
  fi
  found=$(find "$base" -mindepth 2 -maxdepth 2 -name griffe-desktop -type f 2>/dev/null | sed -n '1p' || true)
  if [ -n "$found" ]; then
    dirname "$found"
    return
  fi
  echo "griffe-desktop introuvable dans $base" >&2
  exit 1
}

is_elf() {
  # les 4 octets magiques, portable, sans `file`
  [ "$(od -An -N4 -tx1 "$1" | tr -d ' \n')" = "7f454c46" ]
}

[ $# -eq 1 ] || usage

src=$1

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

case "$src" in
  *.AppImage)
    echo "utiliser install-griffe.sh pour un AppImage" >&2
    exit 1
    ;;
esac

if [ ! -d "$src" ] && [ ! -f "$src" ]; then
  echo "fichier introuvable: $src" >&2
  exit 1
fi

if [ ! -d "$src" ]; then
  case "$src" in
    *.tar.xz|*.tar.gz)
      verify_signed_file "$src"
      ;;
  esac
fi

libdir="$HOME/.local/lib/griffe"
elf="$libdir/griffe-desktop"
typst_dest="$libdir/typst"
link="$HOME/.local/bin/griffe-desktop"
icon="$HOME/.local/share/icons/hicolor/256x256/apps/io.github.dz9oo.griffe.png"
desktop="$HOME/.local/share/applications/io.github.dz9oo.griffe.desktop"

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
fallback="$script_dir/../crates/griffe-desktop/icons/icon.png"

tmp=$(mktemp -d)
rollback=1
cleanup() {
  if [ "${rollback:-0}" -eq 1 ]; then
    rm -f "$elf" "$typst_dest" "$link" "$icon" "$desktop"
  fi
  rm -rf "$tmp"
}
trap cleanup EXIT

if [ -d "$src" ]; then
  payload=$(find_payload "$src")
else
  case "$src" in
    *.tar.xz)
      tar -xJf "$src" -C "$tmp"
      ;;
    *.tar.gz)
      tar -xzf "$src" -C "$tmp"
      ;;
    *)
      echo "attendu un .tar.xz, .tar.gz ou un répertoire" >&2
      usage
      ;;
  esac
  payload=$(find_payload "$tmp")
fi

if [ ! -f "$payload/typst" ]; then
  echo "typst introuvable dans $payload" >&2
  exit 1
fi

if ! is_elf "$payload/griffe-desktop" || ! is_elf "$payload/typst"; then
  echo "griffe-desktop et typst doivent être des ELF" >&2
  exit 1
fi

if [ ! -e /usr/lib/libwebkit2gtk-4.1.so.0 ] \
  && [ ! -e /usr/lib64/libwebkit2gtk-4.1.so.0 ] \
  && [ ! -e /usr/lib/x86_64-linux-gnu/libwebkit2gtk-4.1.so.0 ]; then
  echo "webkit2gtk-4.1 introuvable." >&2
  echo "Arch / Omarchy : pacman -S webkit2gtk-4.1" >&2
  echo "Debian / Ubuntu : apt install libwebkit2gtk-4.1-0" >&2
  exit 1
fi

install -D "$payload/griffe-desktop" "$elf"
chmod +x "$elf"
install -D "$payload/typst" "$typst_dest"
chmod +x "$typst_dest"

mkdir -p "$HOME/.local/bin"
ln -sfn "$libdir/griffe-desktop" "$HOME/.local/bin/griffe-desktop"

icon_src=

if [ -f "$payload/io.github.dz9oo.griffe.png" ]; then
  icon_src=$payload/io.github.dz9oo.griffe.png
fi
if [ -z "$icon_src" ]; then
  for f in "$payload"/*.png; do
    if [ -f "$f" ]; then
      icon_src=$f
      break
    fi
  done
fi
if [ -z "$icon_src" ]; then
  for f in "$payload/icons"/*.png; do
    if [ -f "$f" ]; then
      icon_src=$f
      break
    fi
  done
fi
if [ -z "$icon_src" ] && [ -f "$fallback" ]; then
  icon_src=$fallback
fi
if [ -z "$icon_src" ]; then
  echo "aucune icône PNG (archive vide et $fallback absent)" >&2
  exit 1
fi

install -D "$icon_src" "$icon"

mkdir -p "$HOME/.local/share/applications"
cat >"$desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Griffe
Exec=$elf
Icon=io.github.dz9oo.griffe
StartupWMClass=Griffe-desktop
Terminal=false
Categories=Office;Finance;
EOF

if [ -e "$HOME/.local/bin/griffe" ]; then
  rm -f "$HOME/.local/bin/griffe"
  echo "supprimé l'ancien lanceur AppImage ~/.local/bin/griffe" >&2
fi

rollback=0
printf '%s\n' "$elf" "$typst_dest" "$link" "$icon" "$desktop"
