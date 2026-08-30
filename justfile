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
    cargo run -p freeflow-desktop

# Tout ce qui doit passer avant de considérer un lot terminé.
check: fmt-check lint unused test audit
