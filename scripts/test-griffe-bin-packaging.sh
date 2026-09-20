#!/usr/bin/env bash
# Preuve statique du gabarit griffe-bin + réécriture CI (pas de makepkg).
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
pkgbuild="$root/packaging/arch/griffe-bin/PKGBUILD"
packaging_license="$root/packaging/arch/griffe-bin/LICENSE"
ci_script="$root/packaging/arch/griffe-bin/ci-makepkg.sh"
fail() { echo "FAIL: $*" >&2; exit 1; }

[ -f "$pkgbuild" ] || fail "PKGBUILD manquant"

grep -qx 'pkgname=griffe-bin' "$pkgbuild" || fail "pkgname"
grep -qx 'pkgver=0.3.0' "$pkgbuild" || fail "pkgver doit matcher tauri 0.3.0"
grep -qx 'pkgrel=1' "$pkgbuild" || fail "pkgrel"
grep -qx "arch=('x86_64')" "$pkgbuild" || fail "arch"
grep -qx "depends=('webkit2gtk-4.1' 'gtk3' 'hicolor-icon-theme')" "$pkgbuild" \
  || fail "depends"
grep -qx 'provides=("griffe=${pkgver}")' "$pkgbuild" \
  || fail "provides doit être griffe=\${pkgver}"
grep -qx "conflicts=('griffe')" "$pkgbuild" || fail "conflicts"
if grep -q 'replaces=' "$pkgbuild"; then
  fail "pas de replaces"
fi
grep -qx "options=('!strip')" "$pkgbuild" || fail "!strip"
grep -q "sha256sums=('CHANGEME')" "$pkgbuild" || fail "gabarit : CHANGEME"
grep -q 'releases/download/v${pkgver}/griffe-${pkgver}-x86_64-linux.tar.xz' \
  "$pkgbuild" || fail "source= URL GitHub"
grep -q '/usr/lib/griffe/griffe-desktop' "$pkgbuild" || fail "libdir ELF"
grep -q '/usr/lib/griffe/typst' "$pkgbuild" || fail "libdir typst"
grep -q '/usr/bin/griffe-desktop' "$pkgbuild" || fail "bindir"
grep -q 'Exec=/usr/bin/griffe-desktop' "$pkgbuild" || fail "Exec= pacman"
grep -q 'io.github.dz9oo.griffe.desktop' "$pkgbuild" || fail "fiche"
grep -q 'hicolor/256x256/apps/io.github.dz9oo.griffe.png' "$pkgbuild" \
  || fail "icône"
if grep -nE '(^|[[:space:]])(curl|wget|sudo)([[:space:]]|$)' "$pkgbuild" \
  | grep -vq '^[[:space:]]*#'; then
  fail "PKGBUILD : pas de curl/wget/sudo (source= n'est pas un appel)"
fi
grep -q 'ne pas soumettre' "$pkgbuild" || fail "commentaire gabarit"

[ -f "$packaging_license" ] || fail "LICENSE 0BSD des sources de paquet"
grep -q 'Permission to use, copy, modify, and/or distribute this software' \
  "$packaging_license" || fail "texte 0BSD"
if grep -qi 'PolyForm' "$packaging_license"; then
  fail "LICENSE packaging ≠ PolyForm du logiciel"
fi

# .SRCINFO ne vit pas dans freeflow.git
if [ -f "$root/packaging/arch/griffe-bin/.SRCINFO" ]; then
  fail ".SRCINFO ne doit pas être commité"
fi

echo "OK PKGBUILD"

# --- réécriture CI / AUR, sans makepkg ---
[ -x "$ci_script" ] || fail "ci-makepkg.sh manquant ou non exécutable"
if grep -nE '(^|[[:space:]])(curl|wget)([[:space:]]|$)' "$ci_script" \
  | grep -vq '^[[:space:]]*#'; then
  fail "ci-makepkg.sh ne doit pas appeler curl/wget"
fi
grep -q 'set -eu' "$ci_script" || fail "set -eu"
if grep -qE 'cargo |tauri |nix develop' "$ci_script"; then
  fail "pas de compile dans ci-makepkg.sh"
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/native/griffe-0.3.0-x86_64-linux"
printf 'stub\n' > "$tmp/native/griffe-0.3.0-x86_64-linux/griffe-desktop"
tar -C "$tmp/native" -cJf "$tmp/native/griffe-0.3.0-x86_64-linux.tar.xz" \
  griffe-0.3.0-x86_64-linux
want_sha=$(sha256sum "$tmp/native/griffe-0.3.0-x86_64-linux.tar.xz" | awk '{print $1}')
[ "${#want_sha}" -eq 64 ] || fail "sha de test"

export GRIFFE_BIN_PREPARE_ONLY=1
export GRIFFE_NATIVE_DIR="$tmp/native"
export GRIFFE_AUR_OUT="$tmp/aur-src"
export GRIFFE_ARCH_OUT="$tmp/arch-pkg"
export GRIFFE_WORK="$tmp/work"
if ! bash "$ci_script"; then
  fail "PREPARE_ONLY doit réussir sans makepkg"
fi

ci_pb="$tmp/work/ci/PKGBUILD"
aur_pb="$tmp/aur-src/PKGBUILD"
[ -f "$ci_pb" ] || fail "PKGBUILD CI non écrit"
[ -f "$aur_pb" ] || fail "PKGBUILD AUR non écrit"
grep -q 'source=("griffe-${pkgver}-x86_64-linux.tar.xz")' "$ci_pb" \
  || fail "CI source= doit être le nom de fichier"
if grep -q 'releases/download' "$ci_pb"; then
  fail "CI source= ne doit pas être l'URL GitHub"
fi
grep -q "sha256sums=('$want_sha')" "$ci_pb" || fail "CI sha256sums réel"
grep -q 'releases/download/v${pkgver}/griffe-${pkgver}-x86_64-linux.tar.xz' \
  "$aur_pb" || fail "AUR source= URL GitHub"
grep -q "sha256sums=('$want_sha')" "$aur_pb" || fail "AUR même SHA"
if grep -q CHANGEME "$ci_pb" "$aur_pb"; then
  fail "plus de CHANGEME sur les copies"
fi
grep -q "sha256sums=('CHANGEME')" "$pkgbuild" \
  || fail "le gabarit commité garde CHANGEME"
test -f "$tmp/work/ci/griffe-0.3.0-x86_64-linux.tar.xz" \
  || fail "tarball copié à côté du PKGBUILD CI"
# package() identique
pkg_fn() { awk '/^package\(\)/,/^}/' "$1"; }
[ "$(pkg_fn "$pkgbuild")" = "$(pkg_fn "$ci_pb")" ] || fail "package() CI divergé"
[ "$(pkg_fn "$pkgbuild")" = "$(pkg_fn "$aur_pb")" ] || fail "package() AUR divergé"

echo "OK rewrite"

wf="$root/.github/workflows/bundle.yml"
[ -f "$wf" ] || fail "bundle.yml manquant"
grep -q 'griffe-linux-native' "$wf" || fail "artefact natif 68"
awk '/^  griffe-bin:/{found=1} END{exit found?0:1}' "$wf" \
  || fail "job griffe-bin manquant"
grep -q 'needs: appimage' "$wf" || fail "needs: appimage"
grep -q 'archlinux:base-devel@sha256:305558d2bce0b33170f7f7e4ee690633df4b1e0bdfd8194fe45a6545172319ce' \
  "$wf" || fail "digest épinglé"
grep -q 'timeout-minutes: 20' "$wf" || fail "timeout 20"
grep -q -- '--privileged' "$wf" || fail "privileged (pacman-key)"
grep -q 'name: griffe-linux-arch' "$wf" || fail "artefact griffe-linux-arch"
grep -q 'name: griffe-aur-src' "$wf" || fail "artefact griffe-aur-src"
grep -q 'packaging/arch/griffe-bin/ci-makepkg.sh' "$wf" || fail "appel du script"
if grep -nE 'uses: .*makepkg|uses: .*namcap' "$wf"; then
  fail "pas d'action Marketplace makepkg/namcap"
fi
if grep -n 'DeterminateSystems\|nix develop' "$wf" | grep -q griffe-bin; then
  fail "pas de Nix dans griffe-bin"
fi
# le job appimage continue de publier AppImage + tar.xz
grep -q 'name: griffe-linux-appimage' "$wf" || fail "artefact AppImage"
grep -Fq 'target/release/bundle/native/*.tar.xz' "$wf" \
  || fail "Release tar.xz appimage inchangée"

echo "OK yaml-griffe-bin"

