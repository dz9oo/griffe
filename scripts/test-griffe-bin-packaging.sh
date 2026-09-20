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
