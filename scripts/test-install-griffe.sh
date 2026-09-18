#!/usr/bin/env bash
# Preuve du geste d'install XDG. HOME toujours un mktemp — jamais le coffre réel.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
installer="$root/scripts/install-griffe.sh"
clone_icon="$root/crates/griffe-desktop/icons/icon.png"

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

assert_no_install() {
  if [ -e "$HOME/.local/bin/griffe" ]; then
    fail "ne doit pas écrire $HOME/.local/bin/griffe ($1)"
  fi
  if [ -e "$HOME/.local/share/applications/io.github.dz9oo.griffe.desktop" ]; then
    fail "ne doit pas écrire la fiche .desktop ($1)"
  fi
  if [ -e "$HOME/.local/share/icons/hicolor/256x256/apps/io.github.dz9oo.griffe.png" ]; then
    fail "ne doit pas écrire l'icône ($1)"
  fi
  if [ -e "$HOME/.local/share/freeflow" ]; then
    fail "ne doit pas toucher le coffre ($1)"
  fi
}

assert_three_paths() {
  bin="$HOME/.local/bin/griffe"
  icon="$HOME/.local/share/icons/hicolor/256x256/apps/io.github.dz9oo.griffe.png"
  desktop="$HOME/.local/share/applications/io.github.dz9oo.griffe.desktop"
  want=$(printf '%s\n' "$bin" "$icon" "$desktop")
  if [ "$1" != "$want" ]; then
    echo "stdout obtenu:" >&2
    printf '%s\n' "$1" >&2
    echo "stdout attendu:" >&2
    printf '%s\n' "$want" >&2
    fail "stdout doit être exactement les trois chemins"
  fi
  [ -x "$bin" ] || fail "binaire installé non exécutable"
  [ -f "$icon" ] || fail "icône absente"
  [ -f "$desktop" ] || fail "fiche .desktop absente"
  if [ -e "$HOME/.local/share/freeflow" ]; then
    fail "ne doit pas créer ~/.local/share/freeflow/"
  fi
  grep -qx 'Type=Application' "$desktop" || fail "Type= manquant"
  grep -qx 'Name=Griffe' "$desktop" || fail "Name=Griffe manquant"
  grep -Fqx "Exec=$bin" "$desktop" || fail "Exec= n'est pas le chemin absolu du binaire"
  if grep -Fq '$HOME' "$desktop"; then
    fail "la fiche contient un \$HOME littéral"
  fi
  grep -qx 'Icon=io.github.dz9oo.griffe' "$desktop" || fail "Icon= incorrect"
  grep -qx 'StartupWMClass=Griffe-desktop' "$desktop" || fail "StartupWMClass= incorrect"
  grep -qx 'Terminal=false' "$desktop" || fail "Terminal=false manquant"
  grep -qx 'Categories=Office;Finance;' "$desktop" || fail "Categories= incorrect"
}

write_tiny_png() {
  # PNG 1×1 distinct de crates/griffe-desktop/icons/icon.png
  base64 -d >"$1" <<'EOF'
iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC
EOF
}

write_extract_stub() {
  dest=$1
  png=$2
  cat >"$dest" <<EOF
#!/bin/sh
if [ "\$1" = "--appimage-extract" ]; then
  mkdir -p squashfs-root/usr/share/icons/hicolor/256x256/apps
  cp '$png' squashfs-root/usr/share/icons/hicolor/256x256/apps/io.github.dz9oo.griffe.png
  exit 0
fi
exit 0
EOF
  chmod +x "$dest"
}

if [ ! -f "$installer" ]; then
  echo "scripts/install-griffe.sh manquant" >&2
  exit 1
fi

# --- fichier absent ---
dir=$(mktemp -d)
dir2=
iso=
dir3=
trap 'rm -rf "$dir" "${dir2:-}" "${iso:-}" "${dir3:-}"' EXIT
export HOME="$dir"

if "$installer" /no/such.AppImage 2>/dev/null; then
  echo "devait échouer" >&2
  exit 1
fi
assert_no_install "fichier absent"

# --- suffixe autre que .AppImage : rien n'est écrit ---
fake_bin="$dir/not-an-appimage.bin"
printf 'x' >"$fake_bin"
chmod +x "$fake_bin"
if "$installer" "$fake_bin" 2>/dev/null; then
  fail "devait refuser un fichier sans suffixe .AppImage"
fi
assert_no_install "mauvais suffixe"

# --- .AppImage factice : icône du clone, pas d'extraction ---
fake_app="$dir/Griffe_test.AppImage"
printf 'pas un AppImage\n' >"$fake_app"
chmod +x "$fake_app"
out=$("$installer" "$fake_app")
assert_three_paths "$out"
out=$("$installer" "$fake_app")
assert_three_paths "$out"
cmp -s "$HOME/.local/share/icons/hicolor/256x256/apps/io.github.dz9oo.griffe.png" "$clone_icon" \
  || fail "sans squashfs, l'icône doit venir du clone"

# --- stub --appimage-extract : png extrait, pas celui du clone ---
dir2=$(mktemp -d)
export HOME="$dir2"
tiny="$dir2/tiny.png"
write_tiny_png "$tiny"
stub="$dir2/Griffe_stub.AppImage"
write_extract_stub "$stub" "$tiny"
out=$("$installer" "$stub")
assert_three_paths "$out"
cmp -s "$HOME/.local/share/icons/hicolor/256x256/apps/io.github.dz9oo.griffe.png" "$tiny" \
  || fail "l'icône extraite de l'AppImage doit primer sur le clone"
cmp -s "$HOME/.local/share/icons/hicolor/256x256/apps/io.github.dz9oo.griffe.png" "$clone_icon" \
  && fail "l'icône extraite ne doit pas être celle du clone"

# --- script isolé du clone, sans png : échec et rollback ---
iso=$(mktemp -d)
mkdir -p "$iso/bin" "$iso/home"
cp "$installer" "$iso/bin/install-griffe.sh"
chmod +x "$iso/bin/install-griffe.sh"
export HOME="$iso/home"
lonely="$iso/Griffe.AppImage"
printf 'x' >"$lonely"
chmod +x "$lonely"
if "$iso/bin/install-griffe.sh" "$lonely" 2>/dev/null; then
  fail "sans png ni clone, l'install doit échouer"
fi
if [ -e "$HOME/.local/bin/griffe" ]; then
  fail "rollback : ~/.local/bin/griffe doit disparaître"
fi
if [ -e "$HOME/.local/share/icons/hicolor/256x256/apps/io.github.dz9oo.griffe.png" ]; then
  fail "rollback : icône ne doit pas rester"
fi
if [ -e "$HOME/.local/share/applications/io.github.dz9oo.griffe.desktop" ]; then
  fail "rollback : .desktop ne doit pas rester"
fi
if [ -e "$HOME/.local/share/freeflow" ]; then
  fail "rollback : ne doit pas toucher le coffre"
fi

# --- pas de réseau / sudo dans l'installeur ---
if grep -E '^[[:space:]]*(curl|wget|sudo)[[:space:]]' "$installer"; then
  fail "l'installeur ne doit pas appeler curl, wget ou sudo"
fi
head -n 1 "$installer" | grep -qx '#!/bin/sh' || fail "shebang POSIX #!/bin/sh"
grep -qx 'set -eu' "$installer" || fail "set -eu manquant"

# --- intégration optionnelle : vrai AppImage, jamais téléchargé ici ---
if [ -n "${GRIFFE_APPIMAGE:-}" ]; then
  [ -f "$GRIFFE_APPIMAGE" ] || fail "GRIFFE_APPIMAGE n'est pas un fichier"
  dir3=$(mktemp -d)
  export HOME="$dir3"
  out=$("$installer" "$GRIFFE_APPIMAGE")
  assert_three_paths "$out"
  echo "ok: intégration GRIFFE_APPIMAGE"
else
  echo "skip: intégration AppImage (GRIFFE_APPIMAGE non défini)"
fi

echo "ok: scripts/test-install-griffe.sh"
