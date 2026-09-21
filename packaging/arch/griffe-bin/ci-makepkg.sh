#!/bin/sh
# Empacte griffe-bin dans le conteneur Arch (job bundle / griffe-bin).
# GRIFFE_BIN_PREPARE_ONLY=1 : réécrit les PKGBUILD, pas de pacman/makepkg.
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH= cd -- "$script_dir/../../.." && pwd)
pkgbuild_src="$script_dir/PKGBUILD"
packaging_license="$script_dir/LICENSE"
native_dir=${GRIFFE_NATIVE_DIR:-"$repo_root/native-tarball"}
arch_out=${GRIFFE_ARCH_OUT:-"$repo_root/arch-pkg"}
aur_out=${GRIFFE_AUR_OUT:-"$repo_root/aur-src"}
prepare_only=${GRIFFE_BIN_PREPARE_ONLY:-0}

die() { printf '%s\n' "$*" >&2; exit 1; }

[ -f "$pkgbuild_src" ] || die "PKGBUILD introuvable : $pkgbuild_src"
[ -d "$native_dir" ] || die "GRIFFE_NATIVE_DIR introuvable : $native_dir"

# Un seul tarball natif, où qu'actions/download-artifact l'ait posé.
tarball=$(find "$native_dir" -type f -name 'griffe-*-x86_64-linux.tar.xz' -print)
[ -n "$tarball" ] || die "aucun tarball natif dans $native_dir"
n=$(printf '%s\n' "$tarball" | grep -c .)
[ "$n" -eq 1 ] || die "attendu 1 tarball natif dans $native_dir (trouvé $n)"

pkgver=$(awk -F= '/^pkgver=/{print $2; exit}' "$pkgbuild_src")
[ -n "$pkgver" ] || die "pkgver vide"
ver=$pkgver

sha=$(sha256sum "$tarball" | awk '{print $1}')
[ "${#sha}" -eq 64 ] || die "sha256 inattendu"

read_tauri_ver() {
  python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["version"])' \
    "$repo_root/crates/griffe-desktop/tauri.conf.json"
}

if [ -n "${GRIFFE_WORK:-}" ]; then
  work=$GRIFFE_WORK
else
  work=$(mktemp -d)
fi
ci_dir="$work/ci"
mkdir -p "$ci_dir" "$aur_out" "$arch_out"

cp "$tarball" "$ci_dir/griffe-${ver}-x86_64-linux.tar.xz"

# Copie CI : fichier local + SHA réel (makepkg n'a pas d'URL Release encore).
sed -e 's|^source=.*|source=("griffe-${pkgver}-x86_64-linux.tar.xz")|' \
    -e "s|^sha256sums=.*|sha256sums=('$sha')|" \
    "$pkgbuild_src" > "$ci_dir/PKGBUILD"

# Copie AUR : URL GitHub conservée, même SHA.
sed -e "s|^sha256sums=.*|sha256sums=('$sha')|" \
    "$pkgbuild_src" > "$aur_out/PKGBUILD"
cp "$packaging_license" "$aur_out/LICENSE"

if [ "$prepare_only" = 1 ]; then
  tauri_ver=$(read_tauri_ver)
  [ "$pkgver" = "$tauri_ver" ] || die "pkgver PKGBUILD ($pkgver) ≠ tauri.conf ($tauri_ver)"
  exit 0
fi

# --- suite : uniquement dans le conteneur Arch ---
if [ "$(id -u)" -ne 0 ]; then
  die "le mode complet doit tourner en root dans le conteneur Arch"
fi

# Overlay Docker : CheckSpace se trompe souvent.
if [ -f /etc/pacman.conf ]; then
  sed -i 's/^CheckSpace/#CheckSpace/' /etc/pacman.conf || true
fi

pacman-key --init
pacman-key --populate archlinux
pacman -Syu --noconfirm
pacman -S --noconfirm namcap pacman-contrib sudo git python

tauri_ver=$(read_tauri_ver)
[ "$pkgver" = "$tauri_ver" ] || die "pkgver PKGBUILD ($pkgver) ≠ tauri.conf ($tauri_ver)"

id builder >/dev/null 2>&1 || useradd -m builder
if ! grep -q '^builder ALL=' /etc/sudoers /etc/sudoers.d/* 2>/dev/null; then
  printf '%s\n' 'builder ALL=(ALL) NOPASSWD: ALL' > /etc/sudoers.d/builder
  chmod 440 /etc/sudoers.d/builder
fi

builder_home=$(getent passwd builder | awk -F: '{print $6}')
build_dir="$builder_home/build"
pkgdest="$builder_home/out"
rm -rf "$build_dir" "$pkgdest"
mkdir -p "$build_dir" "$pkgdest"
cp "$ci_dir/PKGBUILD" "$ci_dir/griffe-${ver}-x86_64-linux.tar.xz" "$build_dir/"
chown -R builder:builder "$build_dir" "$pkgdest" "$aur_out"

# makepkg refuse root.
( cd "$build_dir" && sudo -u builder env PKGDEST="$pkgdest" makepkg -s --noconfirm )

pkgfile=$(find "$pkgdest" -type f -name 'griffe-bin-*-x86_64.pkg.tar.zst' | awk 'END{print}')
[ -n "$pkgfile" ] && [ -f "$pkgfile" ] || die "paquet introuvable dans $pkgdest"
want_pkg="griffe-bin-${ver}-1-x86_64.pkg.tar.zst"
[ "$(basename "$pkgfile")" = "$want_pkg" ] \
  || die "nom de paquet $(basename "$pkgfile") ≠ $want_pkg"

namcap "$build_dir/PKGBUILD" | tee /tmp/namcap-pkgbuild.txt
namcap "$pkgfile" | tee /tmp/namcap-pkg.txt
if grep -q '^E:' /tmp/namcap-pkgbuild.txt /tmp/namcap-pkg.txt; then
  die "namcap a émis E:"
fi
printf '%s\n' '--- namcap W: (tri une fois, ne fait pas échouer) ---'
grep '^W:' /tmp/namcap-pkgbuild.txt /tmp/namcap-pkg.txt || true

pacman -U --noconfirm "$pkgfile"
[ -L /usr/bin/griffe-desktop ] || die "/usr/bin/griffe-desktop n'est pas un lien"
resolved=$(readlink -f /usr/bin/griffe-desktop)
[ "$resolved" = /usr/lib/griffe/griffe-desktop ] \
  || die "lien bindir → $resolved"
[ -x /usr/lib/griffe/griffe-desktop ] || die "ELF libdir manquant"
[ -x /usr/lib/griffe/typst ] || die "typst libdir manquant"

ldd /usr/lib/griffe/griffe-desktop | tee /tmp/ldd-pkg.txt
grep -q 'libwebkit2gtk-4.1.so.0' /tmp/ldd-pkg.txt || die "WebKit DT_NEEDED absent"
if grep -q /nix/store /tmp/ldd-pkg.txt; then
  die "l'ELF du paquet est lié au store Nix"
fi
if pacman -Qql griffe-bin | grep -E '\.so($|\.)'; then
  die "le paquet ne doit pas embarquer de .so"
fi
# Pas de lancement GUI.

( cd "$aur_out" && sudo -u builder makepkg --printsrcinfo > .SRCINFO )
[ -s "$aur_out/.SRCINFO" ] || die ".SRCINFO vide"
grep -q "pkgname = griffe-bin" "$aur_out/.SRCINFO" || die ".SRCINFO pkgname"
grep -q "$sha" "$aur_out/.SRCINFO" || die ".SRCINFO sans le SHA"

cp "$pkgfile" "$arch_out/$want_pkg"
( cd "$arch_out" && sha256sum "$want_pkg" | tee "$want_pkg.sha256" && sha256sum -c "$want_pkg.sha256" )
