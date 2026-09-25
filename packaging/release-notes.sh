#!/bin/sh
# Extrait la section utilisateur de CHANGELOG.md pour une version (0.3.2 ou v0.3.2).
# Échoue si la section manque ou est vide : un tag ne part pas sans notes.
set -eu

die() { printf '%s\n' "$*" >&2; exit 1; }

[ $# -eq 1 ] || die "usage: release-notes.sh <version>"
ver=${1#v}
case $ver in
  *[!0-9.]*) die "version illisible : $ver" ;;
  '') die "version vide" ;;
esac

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
file=$root/CHANGELOG.md
[ -f "$file" ] || die "CHANGELOG.md introuvable"

notes=$(awk -v ver="$ver" '
  function heading(line,    rest) {
    if (substr(line, 1, 3) != "## ") return ""
    rest = substr(line, 4)
    sub(/[[:space:]].*$/, "", rest)
    return rest
  }
  heading($0) == ver { on = 1; next }
  on && substr($0, 1, 3) == "## " { exit }
  on { print }
' "$file")

trimmed=$(printf '%s' "$notes" | tr -d '[:space:]')
[ -n "$trimmed" ] || die "pas de notes pour $ver dans CHANGELOG.md"
printf '%s\n' "$notes"
