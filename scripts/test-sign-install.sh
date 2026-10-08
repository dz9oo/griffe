#!/usr/bin/env bash
# Preuve locale : signature minisign, refus sans écraser un binaire déjà là.
# HOME toujours un mktemp. Clé de test éphémère. Jamais le coffre réel.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
# shellcheck source=test-minisign-fixture.sh
. "$root/scripts/test-minisign-fixture.sh"
app_installer="$root/scripts/install-griffe.sh"
native_installer="$root/scripts/install-griffe-native.sh"
signer="$root/packaging/sign-release.sh"

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

require_minisign
[ -x "$signer" ] || fail "sign-release.sh manquant ou non exécutable"
grep -qx 'set -eu' "$signer" || fail "sign-release.sh : set -eu"
if grep -E '^[[:space:]]*set[[:space:]]+-x([[:space:]]|$)' "$signer"; then
  fail "sign-release.sh ne doit pas activer set -x"
fi

fn_body() {
  awk '
    $0 == "# --- verify-signed-file ---" { n++; next }
    n == 1 { print }
    n == 2 { exit }
  ' "$1"
}
app_fn=$(fn_body "$app_installer")
native_fn=$(fn_body "$native_installer")
[ -n "$app_fn" ] || fail "fonction de vérification absente de install-griffe.sh"
[ "$app_fn" = "$native_fn" ] || fail "les deux installeurs n'ont pas la même vérification"

extract_pub() {
  c=$(sed -n "s/^MINISIGN_PUB_COMMENT='\\(.*\\)'\$/\\1/p" "$1")
  k=$(sed -n "s/^MINISIGN_PUB_KEY='\\(.*\\)'\$/\\1/p" "$1")
  printf '%s\n%s\n' "$c" "$k"
}
[ "$(extract_pub "$app_installer")" = "$(extract_pub "$native_installer")" ] \
  || fail "les clés embarquées divergent"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

keys=$tmp/keys
make_test_keypair "$keys"
other=$tmp/other
make_test_keypair "$other"

sha_of() {
  dir=$1
  base=$2
  (cd "$dir" && sha256sum "$base" >"$base.sha256")
}

fill_stage() {
  stage=$1
  mkdir -p "$stage"
  printf 'app\n' >"$stage/Griffe_test.AppImage"
  printf 'tar\n' >"$stage/griffe-9.9.9-x86_64-linux.tar.xz"
  printf 'pkg\n' >"$stage/griffe-bin-9.9.9-1-x86_64.pkg.tar.zst"
  sha_of "$stage" "Griffe_test.AppImage"
  sha_of "$stage" "griffe-9.9.9-x86_64-linux.tar.xz"
  sha_of "$stage" "griffe-bin-9.9.9-1-x86_64.pkg.tar.zst"
}

# --- secret absent ---
stage=$tmp/stage-nosecret
fill_stage "$stage"
err=$tmp/nosecret.err
if MINISIGN_SECRET_KEY= "$signer" "$stage" "$keys/pub" >"$tmp/nosecret.out" 2>"$err"; then
  fail "signer sans secret doit échouer"
fi
grep -q 'MINISIGN_SECRET_KEY' "$err" || fail "le message doit nommer MINISIGN_SECRET_KEY"
if find "$stage" -name '*.minisig' | grep -q .; then
  fail "aucune signature ne doit être écrite sans secret"
fi

# --- trois fichiers ---
stage=$tmp/stage-ok
fill_stage "$stage"
MINISIGN_SECRET_KEY=$(cat "$keys/sec")
export MINISIGN_SECRET_KEY
"$signer" "$stage" "$keys/pub" >"$tmp/signed.out"
for base in Griffe_test.AppImage griffe-9.9.9-x86_64-linux.tar.xz griffe-bin-9.9.9-1-x86_64.pkg.tar.zst; do
  [ -f "$stage/$base.minisig" ] || fail "minisig manquant : $base"
  got=$(minisign -V -H -Q -m "$stage/$base" -x "$stage/$base.minisig" -p "$keys/pub")
  [ "$got" = "$base" ] || fail "commentaire : $base ($got)"
done

printf 'x' >>"$stage/Griffe_test.AppImage"
if minisign -V -H -Q -m "$stage/Griffe_test.AppImage" -x "$stage/Griffe_test.AppImage.minisig" -p "$keys/pub" \
  >/dev/null 2>&1; then
  fail "un octet modifié doit invalider la signature"
fi

# --- trop de fichiers du même type ---
stage=$tmp/stage-two
fill_stage "$stage"
cp "$stage/Griffe_test.AppImage" "$stage/Griffe_autre.AppImage"
sha_of "$stage" "Griffe_autre.AppImage"
if "$signer" "$stage" "$keys/pub" >"$tmp/two.out" 2>"$tmp/two.err"; then
  fail "deux AppImage : le signe doit échouer"
fi
grep -q 'attendu 1' "$tmp/two.err" || fail "message attendu 1 fichier"

# --- sha256 faux ---
stage=$tmp/stage-sha
fill_stage "$stage"
printf '0000  Griffe_test.AppImage\n' >"$stage/Griffe_test.AppImage.sha256"
if "$signer" "$stage" "$keys/pub" >"$tmp/sha.out" 2>"$tmp/sha.err"; then
  fail "sha256 faux : le signe doit échouer"
fi
grep -q 'sha256 invalide' "$tmp/sha.err" || fail "message sha256 invalide"

# --- clé publique qui n'est pas celle du secret ---
stage=$tmp/stage-wrongpub
fill_stage "$stage"
if "$signer" "$stage" "$other/pub" >"$tmp/wrongpub.out" 2>"$tmp/wrongpub.err"; then
  fail "clé publique différente du secret : le signe doit échouer"
fi

# --- AppImage : refus, binaire déjà là intact ---
icon=$root/crates/griffe-desktop/icons/icon.png
tree=$tmp/tree
mkdir -p "$tree/scripts" "$tree/crates/griffe-desktop/icons"
cp "$icon" "$tree/crates/griffe-desktop/icons/icon.png"
rewrite_installer "$app_installer" "$tree/scripts/install-griffe.sh" "$keys/pub"
signed_app=$tree/scripts/install-griffe.sh

plant_app() {
  export HOME=$1
  mkdir -p "$HOME/.local/bin"
  printf 'ANCIEN\n' >"$HOME/.local/bin/griffe"
  chmod +x "$HOME/.local/bin/griffe"
}

assert_ancien_app() {
  home=$1
  got=$(cat "$home/.local/bin/griffe")
  [ "$got" = "ANCIEN" ] || fail "le binaire déjà installé a été écrasé ($2)"
}

home=$tmp/home-app
plant_app "$home"
app=$tmp/Griffe_preuve.AppImage
printf 'nouveau\n' >"$app"
chmod +x "$app"
if "$signed_app" "$app" >"$tmp/nosig.out" 2>"$tmp/nosig.err"; then
  fail "AppImage sans .minisig doit être refusé"
fi
grep -q 'signature absente' "$tmp/nosig.err" || fail "message signature absente"
assert_ancien_app "$home" "sans minisig"

sign_artifact "$keys/sec" "$keys/pub" "$app"
printf 'x' >>"$app"
if "$signed_app" "$app" >"$tmp/tamper.out" 2>"$tmp/tamper.err"; then
  fail "AppImage modifié doit être refusé"
fi
grep -q 'signature refusée' "$tmp/tamper.err" || fail "message signature refusée"
assert_ancien_app "$home" "octet modifié"

printf 'nouveau\n' >"$app"
minisign -S -W -H -s "$keys/sec" -p "$keys/pub" -x "$app.minisig" -t "autre-nom" -m "$app" >/dev/null
if "$signed_app" "$app" >"$tmp/comment.out" 2>"$tmp/comment.err"; then
  fail "mauvais commentaire de confiance doit être refusé"
fi
grep -q 'commentaire de confiance' "$tmp/comment.err" || fail "message commentaire"
assert_ancien_app "$home" "mauvais commentaire"

sign_artifact "$other/sec" "$other/pub" "$app"
if "$signed_app" "$app" >"$tmp/otherkey.out" 2>"$tmp/otherkey.err"; then
  fail "signature d'une autre clé doit être refusée"
fi
grep -q 'signature refusée' "$tmp/otherkey.err" || fail "message autre clé"
assert_ancien_app "$home" "autre clé"

# clé du dépôt différente de la clé embarquée
mkdir -p "$tree/packaging"
cp "$other/pub" "$tree/packaging/minisign.pub"
sign_artifact "$keys/sec" "$keys/pub" "$app"
if "$signed_app" "$app" >"$tmp/diverge.out" 2>"$tmp/diverge.err"; then
  fail "clé voisine divergente doit être refusée"
fi
grep -q 'divergent' "$tmp/diverge.err" || fail "message de divergence"
assert_ancien_app "$home" "clé divergente"
rm -f "$tree/packaging/minisign.pub"

# installation qui réussit
sign_artifact "$keys/sec" "$keys/pub" "$app"
home_ok=$tmp/home-ok
export HOME=$home_ok
out=$("$signed_app" "$app")
[ -x "$HOME/.local/bin/griffe" ] || fail "installation signée : binaire absent"
cmp -s "$app" "$HOME/.local/bin/griffe" || fail "le binaire installé n'est pas l'AppImage signé"
printf '%s\n' "$out" | grep -qx "$HOME/.local/bin/griffe"

# script copié seul (pas de packaging/minisign.pub à côté)
alone_dir=$tmp/alone
mkdir -p "$alone_dir"
rewrite_installer "$app_installer" "$alone_dir/install-griffe.sh" "$keys/pub"
# le fallback d'icône part de scripts/../crates : on pose l'icône au bon endroit
mkdir -p "$tmp/crates/griffe-desktop/icons"
cp "$icon" "$tmp/crates/griffe-desktop/icons/icon.png"
# alone_dir est $tmp/alone, le fallback serait $tmp/crates — dirname/.. = $tmp. Oui.
home_alone=$tmp/home-alone
export HOME=$home_alone
"$alone_dir/install-griffe.sh" "$app" >/dev/null
cmp -s "$app" "$HOME/.local/bin/griffe" || fail "script copié seul : install refusée"

# --- tarball natif ---
rewrite_installer "$native_installer" "$tmp/install-native.sh" "$keys/pub"
signed_native=$tmp/install-native.sh
tarname=$tmp/griffe-9.9.9-x86_64-linux.tar.xz
printf 'archive\n' >"$tarname"

plant_native() {
  export HOME=$1
  mkdir -p "$HOME/.local/lib/griffe"
  printf 'ANCIEN\n' >"$HOME/.local/lib/griffe/griffe-desktop"
}

assert_ancien_native() {
  got=$(cat "$1/.local/lib/griffe/griffe-desktop")
  [ "$got" = "ANCIEN" ] || fail "l'ELF déjà installé a été écrasé ($2)"
}

home_nat=$tmp/home-nat
plant_native "$home_nat"
if "$signed_native" "$tarname" >"$tmp/tar-nosig.out" 2>"$tmp/tar-nosig.err"; then
  fail "tarball sans .minisig doit être refusé"
fi
grep -q 'signature absente' "$tmp/tar-nosig.err" || fail "tarball : message signature absente"
assert_ancien_native "$home_nat" "sans minisig"

sign_artifact "$keys/sec" "$keys/pub" "$tarname"
printf 'x' >>"$tarname"
if "$signed_native" "$tarname" >"$tmp/tar-tamper.out" 2>"$tmp/tar-tamper.err"; then
  fail "tarball modifié doit être refusé"
fi
grep -q 'signature refusée' "$tmp/tar-tamper.err" || fail "tarball : message signature refusée"
assert_ancien_native "$home_nat" "octet modifié"

# un répertoire local n'exige pas de .minisig
payload=$tmp/payload
mkdir -p "$payload"
src_true=/bin/true
[ -x "$src_true" ] || src_true=/usr/bin/true
[ -x "$src_true" ] || fail "pas de /bin/true"
install -D "$src_true" "$payload/griffe-desktop"
install -D "$src_true" "$payload/typst"
cp "$icon" "$payload/io.github.dz9oo.griffe.png"
home_dir=$tmp/home-dir
export HOME=$home_dir
webkit=0
if [ -e /usr/lib/libwebkit2gtk-4.1.so.0 ] \
  || [ -e /usr/lib64/libwebkit2gtk-4.1.so.0 ] \
  || [ -e /usr/lib/x86_64-linux-gnu/libwebkit2gtk-4.1.so.0 ]; then
  webkit=1
fi
if [ "$webkit" -eq 1 ]; then
  "$native_installer" "$payload" >/dev/null
  [ -x "$HOME/.local/lib/griffe/griffe-desktop" ] || fail "répertoire local : install refusée"
else
  if "$native_installer" "$payload" >"$tmp/dir-webkit.out" 2>"$tmp/dir-webkit.err"; then
    fail "sans webkit le répertoire doit échouer plus loin que la signature"
  fi
  if grep -q 'signature' "$tmp/dir-webkit.err"; then
    fail "un répertoire local ne doit pas exiger de signature"
  fi
  grep -q 'webkit2gtk-4.1' "$tmp/dir-webkit.err" || fail "échec webkit attendu"
fi

canon=$root/packaging/minisign.pub
if [ ! -f "$canon" ]; then
  fail "packaging/minisign.pub manquant — cérémonie dans packaging/SIGNATURES.md"
fi
if [ ! -f "$root/site/minisign.pub" ]; then
  fail "site/minisign.pub manquant"
fi
cmp -s "$canon" "$root/site/minisign.pub" || fail "site/minisign.pub diffère du dépôt"
[ "$(extract_pub "$app_installer")" = "$(cat "$canon")" ] \
  || fail "la clé embarquée n'est pas packaging/minisign.pub"

echo "ok: scripts/test-sign-install.sh"
