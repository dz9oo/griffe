#!/bin/sh
# Signe les trois binaires d'une release (AppImage, tarball, paquet pacman).
# MINISIGN_SECRET_KEY : contenu d'une clé secrète minisign déjà sans phrase
# de passe. minisign lit une phrase sur un TTY si la clé est chiffrée, et -W
# ne saute pas cette demande au moment de signer.
# Ne pas lancer ce script avec set -x : la clé ne doit pas aller dans les logs.
set -eu

die() { printf '%s\n' "$*" >&2; exit 1; }

usage() { die "usage: sign-release.sh RÉPERTOIRE [CLÉ.pub]"; }

[ $# -ge 1 ] && [ $# -le 2 ] || usage

dir=$1
script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
pub=${2:-$script_dir/minisign.pub}

[ -d "$dir" ] || die "répertoire introuvable : $dir"
[ -f "$pub" ] || die "clé publique introuvable : $pub"
[ -n "${MINISIGN_SECRET_KEY:-}" ] || die "secret MINISIGN_SECRET_KEY manquant"
command -v minisign >/dev/null 2>&1 || die "minisign introuvable"
command -v sha256sum >/dev/null 2>&1 || die "sha256sum introuvable"

ver=$(minisign -v 2>&1 | awk 'NR==1 { print $2; exit }')
major=$(printf '%s' "$ver" | cut -d. -f1)
minor=$(printf '%s' "$ver" | cut -d. -f2)
case "$major$minor" in
  *[!0-9]*) die "version minisign illisible : $ver" ;;
esac
if [ "$major" -eq 0 ] && [ "$minor" -lt 11 ]; then
  die "minisign $ver trop ancien : 0.11 requis pour le pré-hachage"
fi

keyfile=
cleanup() {
  if [ -n "${keyfile:-}" ]; then
    rm -f "$keyfile"
  fi
}
trap cleanup EXIT

umask 077
keyfile=$(mktemp)
printf '%s\n' "$MINISIGN_SECRET_KEY" >"$keyfile"

header=$(head -n 1 "$keyfile")
case "$header" in
  "untrusted comment:"*) ;;
  *) die "MINISIGN_SECRET_KEY n'a pas l'en-tête d'une clé minisign" ;;
esac
payload=$(sed -n '2p' "$keyfile")
[ -n "$payload" ] || die "MINISIGN_SECRET_KEY incomplète"

one_file() {
  pat=$1
  found=$(find "$dir" -type f -name "$pat" | sort)
  if [ -z "$found" ]; then
    die "aucun fichier $pat dans $dir"
  fi
  n=$(printf '%s\n' "$found" | grep -c .)
  [ "$n" -eq 1 ] || die "attendu 1 fichier $pat, trouvé $n"
  printf '%s\n' "$found"
}

sign_one() {
  file=$1
  base=$(basename "$file")
  file_dir=$(CDPATH= cd -- "$(dirname -- "$file")" && pwd)
  [ -f "$file.sha256" ] || die "sha256 absent pour $base"
  if ! (cd "$file_dir" && sha256sum -c "$base.sha256" >/dev/null); then
    die "sha256 invalide pour $base"
  fi
  # -H sur -S ne change pas le mode : depuis 0.11 le pré-hachage est le défaut.
  # -p refuse tout de suite si cette clé secrète n'est pas celle du dépôt.
  minisign -S -W -H -s "$keyfile" -p "$pub" \
    -x "$file.minisig" -t "$base" -m "$file"
  got=$(minisign -V -H -Q -m "$file" -x "$file.minisig" -p "$pub") \
    || die "vérification échouée : $base"
  [ "$got" = "$base" ] || die "commentaire de confiance inattendu pour $base"
  printf '%s\n' "$file.minisig"
}

sign_one "$(one_file '*.AppImage')"
sign_one "$(one_file 'griffe-*-x86_64-linux.tar.xz')"
sign_one "$(one_file 'griffe-bin-*-x86_64.pkg.tar.zst')"
