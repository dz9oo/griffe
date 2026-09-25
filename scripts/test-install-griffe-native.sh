#!/usr/bin/env bash
# Preuve du geste d'install XDG natif. HOME toujours un mktemp — jamais le coffre réel.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
installer="$root/scripts/install-griffe-native.sh"
clone_icon="$root/crates/griffe-desktop/icons/icon.png"

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

assert_no_install() {
  if [ -e "$HOME/.local/lib/griffe/griffe-desktop" ] \
    || [ -e "$HOME/.local/lib/griffe/typst" ] \
    || [ -e "$HOME/.local/bin/griffe-desktop" ] \
    || [ -e "$HOME/.local/share/applications/io.github.dz9oo.griffe.desktop" ] \
    || [ -e "$HOME/.local/share/icons/hicolor/256x256/apps/io.github.dz9oo.griffe.png" ]; then
    fail "ne doit pas écrire une install ($1)"
  fi
  if [ -e "$HOME/.local/share/freeflow" ] || [ -e "$HOME/.local/share/griffe" ]; then
    fail "ne doit pas toucher le coffre ($1)"
  fi
}

assert_five_paths() {
  elf="$HOME/.local/lib/griffe/griffe-desktop"
  typst="$HOME/.local/lib/griffe/typst"
  link="$HOME/.local/bin/griffe-desktop"
  icon="$HOME/.local/share/icons/hicolor/256x256/apps/io.github.dz9oo.griffe.png"
  desktop="$HOME/.local/share/applications/io.github.dz9oo.griffe.desktop"
  want=$(printf '%s\n' "$elf" "$typst" "$link" "$icon" "$desktop")
  if [ "$1" != "$want" ]; then
    echo "stdout obtenu:" >&2
    printf '%s\n' "$1" >&2
    echo "stdout attendu:" >&2
    printf '%s\n' "$want" >&2
    fail "stdout doit être exactement les cinq chemins"
  fi
  [ -x "$elf" ] || fail "ELF non exécutable"
  [ -x "$typst" ] || fail "typst non exécutable"
  [ -L "$link" ] || fail "le bindir doit être un lien"
  target=$(readlink -f "$link")
  [ "$target" = "$elf" ] || fail "le lien ne pointe pas vers le libdir"
  [ -f "$icon" ] || fail "icône absente"
  [ -f "$desktop" ] || fail "fiche .desktop absente"
  if [ -e "$HOME/.local/share/freeflow" ] || [ -e "$HOME/.local/share/griffe" ]; then
    fail "ne doit pas créer le répertoire du coffre"
  fi
  grep -qx 'Type=Application' "$desktop" || fail "Type= manquant"
  grep -qx 'Name=Griffe' "$desktop" || fail "Name=Griffe manquant"
  grep -Fqx "Exec=$elf" "$desktop" || fail "Exec= n'est pas l'ELF du libdir"
  if grep -Fq '$HOME' "$desktop"; then
    fail "la fiche contient un \$HOME littéral"
  fi
  grep -qx 'Icon=io.github.dz9oo.griffe' "$desktop" || fail "Icon= incorrect"
  grep -qx 'StartupWMClass=Griffe-desktop' "$desktop" || fail "StartupWMClass= incorrect"
  grep -qx 'Terminal=false' "$desktop" || fail "Terminal=false manquant"
  grep -qx 'Categories=Office;Finance;' "$desktop" || fail "Categories= incorrect"
}

elf_stub() {
  # Vrai ELF minimal : /bin/true est dynamiquement lié, magie \x7fELF.
  dest=$1
  src=/bin/true
  [ -x "$src" ] || src=/usr/bin/true
  [ -x "$src" ] || fail "pas de /bin/true pour le stub ELF"
  install -D "$src" "$dest"
}

webkit_present() {
  [ -e /usr/lib/libwebkit2gtk-4.1.so.0 ] \
    || [ -e /usr/lib64/libwebkit2gtk-4.1.so.0 ] \
    || [ -e /usr/lib/x86_64-linux-gnu/libwebkit2gtk-4.1.so.0 ]
}

make_payload_dir() {
  payload=$1
  mkdir -p "$payload"
  elf_stub "$payload/griffe-desktop"
  elf_stub "$payload/typst"
  write_tiny_png "$payload/io.github.dz9oo.griffe.png"
}

write_tiny_png() {
  base64 -d >"$1" <<'EOF'
iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGP4z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC
EOF
}

if [ ! -f "$installer" ]; then
  echo "scripts/install-griffe-native.sh manquant" >&2
  exit 1
fi

dir=$(mktemp -d)
dir2=
dir3=
trap 'rm -rf "$dir" "${dir2:-}" "${dir3:-}"' EXIT
export HOME="$dir"

# --- fichier absent ---
if "$installer" /no/such.tar.xz 2>/dev/null; then
  fail "devait échouer sur un tarball absent"
fi
assert_no_install "fichier absent"

# --- suffixe AppImage : rien n'est écrit ---
fake_app="$dir/Griffe_test.AppImage"
printf 'x' >"$fake_app"
chmod +x "$fake_app"
if "$installer" "$fake_app" 2>/dev/null; then
  fail "devait refuser un .AppImage"
fi
assert_no_install "suffixe AppImage"

# --- non-ELF ---
bad="$dir/payload"
mkdir -p "$bad"
printf 'pas un elf\n' >"$bad/griffe-desktop"
printf 'pas un elf\n' >"$bad/typst"
chmod +x "$bad/griffe-desktop" "$bad/typst"
write_tiny_png "$bad/io.github.dz9oo.griffe.png"
if "$installer" "$bad" 2>/dev/null; then
  fail "devait refuser un non-ELF"
fi
assert_no_install "non-ELF"

# --- pas de réseau / sudo ---
if grep -E '^[[:space:]]*(curl|wget|sudo|pacman|apt)[[:space:]]' "$installer"; then
  fail "l'installeur ne doit pas appeler curl, wget, sudo, pacman ou apt"
fi
head -n 1 "$installer" | grep -qx '#!/bin/sh' || fail "shebang POSIX #!/bin/sh"
grep -qx 'set -eu' "$installer" || fail "set -eu manquant"

if webkit_present; then
  payload="$dir/ok"
  make_payload_dir "$payload"
  out=$("$installer" "$payload")
  assert_five_paths "$out"
  cmp -s "$HOME/.local/share/icons/hicolor/256x256/apps/io.github.dz9oo.griffe.png" \
    "$payload/io.github.dz9oo.griffe.png" \
    || fail "l'icône du payload doit primer"

  # rejouer = mêmes chemins
  out=$("$installer" "$payload")
  assert_five_paths "$out"

  # ancien AppImage : supprimé
  printf 'old\n' >"$HOME/.local/bin/griffe"
  chmod +x "$HOME/.local/bin/griffe"
  out=$("$installer" "$payload")
  assert_five_paths "$out"
  if [ -e "$HOME/.local/bin/griffe" ]; then
    fail "l'ancien ~/.local/bin/griffe AppImage doit disparaître"
  fi

  # tarball .tar.xz avec racine versionnée
  dir2=$(mktemp -d)
  export HOME="$dir2"
  staged="$dir/griffe-0.3.2-x86_64-linux"
  make_payload_dir "$staged"
  tar -C "$(dirname "$staged")" -cJf "$dir/griffe-0.3.2-x86_64-linux.tar.xz" "$(basename "$staged")"
  out=$("$installer" "$dir/griffe-0.3.2-x86_64-linux.tar.xz")
  assert_five_paths "$out"

  # rollback : icône absente
  dir3=$(mktemp -d)
  export HOME="$dir3"
  lonely="$dir3/payload"
  mkdir -p "$lonely"
  elf_stub "$lonely/griffe-desktop"
  elf_stub "$lonely/typst"
  iso=$(mktemp -d)
  cp "$installer" "$iso/install-griffe-native.sh"
  chmod +x "$iso/install-griffe-native.sh"
  if "$iso/install-griffe-native.sh" "$lonely" 2>/dev/null; then
    fail "sans png ni clone, l'install doit échouer"
  fi
  if [ -e "$HOME/.local/lib/griffe/griffe-desktop" ]; then
    fail "rollback : ELF ne doit pas rester"
  fi
  if [ -e "$HOME/.local/share/freeflow" ] || [ -e "$HOME/.local/share/griffe" ]; then
    fail "rollback : ne doit pas toucher le coffre"
  fi
else
  payload="$dir/ok"
  make_payload_dir "$payload"
  if "$installer" "$payload" 2>/dev/null; then
    fail "sans webkit2gtk-4.1 l'install doit échouer"
  fi
  assert_no_install "webkit absent"
  echo "skip: chemin heureux (webkit2gtk-4.1 absent sur cet hôte)"
fi

echo "ok: scripts/test-install-griffe-native.sh"
