# Fonctions partagées des preuves d'install. Sourcé, pas exécuté.
# shellcheck shell=bash

require_minisign() {
  command -v minisign >/dev/null 2>&1 || {
    echo "minisign manquant (devShell : paquet minisign)" >&2
    exit 1
  }
}

make_test_keypair() {
  pair_dir=$1
  mkdir -p "$pair_dir"
  minisign -G -W -f -p "$pair_dir/pub" -s "$pair_dir/sec" >/dev/null
}

rewrite_installer() {
  src=$1
  dest=$2
  pub=$3
  comment=$(head -n 1 "$pub")
  key=$(sed -n '2p' "$pub")
  sed \
    -e "s|^MINISIGN_PUB_COMMENT='.*'|MINISIGN_PUB_COMMENT='${comment}'|" \
    -e "s|^MINISIGN_PUB_KEY='.*'|MINISIGN_PUB_KEY='${key}'|" \
    "$src" >"$dest"
  chmod +x "$dest"
}

sign_artifact() {
  sec=$1
  pub=$2
  file=$3
  base=$(basename "$file")
  minisign -S -W -H -s "$sec" -p "$pub" -x "$file.minisig" -t "$base" -m "$file" >/dev/null
}
