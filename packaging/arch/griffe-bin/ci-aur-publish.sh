#!/bin/sh
# Pousse l'arbre AUR (PKGBUILD + .SRCINFO + LICENSE 0BSD). Jamais le .pkg.tar.zst.
set -eu

die() { printf '%s\n' "$*" >&2; exit 1; }

[ -n "${AUR_SSH_PRIVATE_KEY:-}" ] || die "secret AUR_SSH_PRIVATE_KEY manquant"

aur_src=${GRIFFE_AUR_SRC:-aur-src}
[ -f "$aur_src/PKGBUILD" ] || die "PKGBUILD AUR manquant dans $aur_src"
[ -f "$aur_src/.SRCINFO" ] || die ".SRCINFO manquant dans $aur_src"
[ -f "$aur_src/LICENSE" ] || die "LICENSE 0BSD manquant dans $aur_src"

umask 077
mkdir -p "$HOME/.ssh"
printf '%s\n' "$AUR_SSH_PRIVATE_KEY" > "$HOME/.ssh/id_ed25519"
chmod 600 "$HOME/.ssh/id_ed25519"
ssh-keyscan aur.archlinux.org >> "$HOME/.ssh/known_hosts"

work=$(mktemp -d)
GIT_SSH_COMMAND='ssh -i '"$HOME/.ssh/id_ed25519"' -o IdentitiesOnly=yes' \
  git clone ssh://aur@aur.archlinux.org/griffe-bin.git "$work/griffe-bin"

cp "$aur_src/PKGBUILD" "$aur_src/.SRCINFO" "$aur_src/LICENSE" "$work/griffe-bin/"
# Interdit d'embarquer le binaire pacman.
rm -f "$work/griffe-bin/"*.pkg.tar.* "$work/griffe-bin/"*.tar.xz

cd "$work/griffe-bin"
git config user.name "${AUR_GIT_NAME:-dz9oo}"
git config user.email "${AUR_GIT_EMAIL:-dz9oo@users.noreply.github.com}"
git add PKGBUILD .SRCINFO LICENSE
git status --porcelain | grep -q . || { printf '%s\n' 'rien à pousser'; exit 0; }
ver=$(awk -F= '/^pkgver=/{print $2; exit}' PKGBUILD)
rel=$(awk -F= '/^pkgrel=/{print $2; exit}' PKGBUILD)
git commit -m "griffe-bin ${ver}-${rel}"
git push origin master
