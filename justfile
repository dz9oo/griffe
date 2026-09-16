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
test:
    cargo nextest run --workspace --no-tests=warn

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

# Traces perso / MIT projet / journal de lots dans le corpus public.
hygiene-check:
    ! git grep -n 'Nicolas Collier Conseil' -- ':!docs/superpowers' ':!justfile'
    ! git grep -n 'virements Collier' -- ':!docs/superpowers' ':!justfile'
    ! git grep -n '/home/nicolas/Work' -- ':!docs/superpowers' ':!justfile'
    ! git grep -n 'licence MIT\|license MIT' -- README.md site/index.html AGENTS.md CONTRIBUTING.md
    ! git grep -n '^license = "MIT"' -- Cargo.toml
