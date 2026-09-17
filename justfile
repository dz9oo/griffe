default:
    @just --list

# Formate tout le workspace.
fmt:
    cargo fmt --all

# Vérifie le formatage sans le modifier (utilisé en CI).
fmt-check:
    cargo fmt --all -- --check

# Lint strict : aucun warning toléré.
lint:
    cargo clippy --workspace --all-targets --all-features -- -D warnings

# Dépendances déclarées mais jamais utilisées.
unused:
    cargo machete

# Suite de tests complète (unitaires, intégration, snapshots).
# --no-tests=warn : un workspace sans encore aucun test (lot 0) n'est pas un échec.
# GRIFFE_NO_OPEN / GRIFFE_TEST_KDF : aussi posés par le devShell Nix ; répétés ici
# pour qu'un `just test` hors direnv ne rouvre pas xdg-open ni ne paie Argon2 de prod.
# Suite de tests (nextest + KDF de test, sans xdg-open).
test:
    GRIFFE_NO_OPEN=1 GRIFFE_TEST_KDF=1 CARGO_INCREMENTAL=0 cargo nextest run --workspace --no-tests=warn

# Cache incrémental rustc (debug). Sans ça il grossit sans borne (dizaines de Gio).
clean-incremental:
    rm -rf target/debug/incremental

# Revue des licences et des CVE connues sur les dépendances.
audit:
    cargo deny check
    cargo audit

# Lance l'application desktop en mode développement.
run:
    cargo run -p griffe-desktop

# Tout ce qui doit passer avant de considérer un lot terminé.
check: fmt-check lint unused test audit

# Landing `site/` : présence des assets, copy Griffe, aucun CDN / sigle fiscal / CLI.
site-check:
    test -f site/index.html
    test -f site/lockup.svg
    test -f site/fonts/newsreader-latin-400-italic.woff2
    ! grep -E 'googleapis|fonts.gstatic|CA3|3514|2777' site/index.html site/app.css
    ! grep -F 'freeflow ' site/index.html
    grep -q 'Griffe' site/index.html
    grep -q 'La lettre du matin' site/index.html
    grep -q 'Le jour' site/index.html
    grep -q 'Les affaires' site/index.html
    grep -q 'La société' site/index.html
    ! grep -E 'licence MIT|license MIT' site/index.html
    grep -q 'PolyForm Shield' site/index.html
