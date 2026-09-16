//! `POST /console/run` : exécute la ligne tapée dans la console via
//! [`griffe_cli::run_capturing_with_vault`] avec [`griffe_cli::VaultAccess::Borrowed`] — le
//! coffre déjà ouvert par la fenêtre, jamais une seconde connexion. C'est ce qui permet à la
//! console de continuer à fonctionner maintenant que la mise en cache dans le trousseau OS est
//! opt-in : `Store::open_cached` n'a plus rien à retrouver par défaut. `Borrowed` interdit aussi
//! structurellement tout prompt TTY sur ce chemin, et la CLI refuse elle-même `init`/`unlock`/
//! `lock`/`passphrase change`/`backup restore` (la session de la fenêtre et celle du trousseau OS
//! ne sont pas le même objet, et un ré-chiffrement complet du coffre est un geste qui mérite un
//! vrai terminal, fenêtre fermée — voir `CLAUDE.md`).

use axum::Form;
use axum::extract::State;
use axum::response::Html;
use griffe_cli::VaultAccess;
use maud::html;
use serde::Deserialize;

use crate::state::AppState;

/// Pas de `Debug` : la ligne tapée est un texte libre qui pourrait, par accident, contenir
/// quelque chose que l'utilisateur ne voudrait pas voir apparaître dans un futur `{:?}`.
#[derive(Deserialize)]
pub struct ConsoleInput {
    line: String,
}

pub async fn run(State(state): State<AppState>, Form(input): Form<ConsoleInput>) -> Html<String> {
    let trimmed = input.line.trim();
    // Lot 39 : `aide`, `help`, `?` et la ligne vide répondent avec les commandes utiles du
    // parcours plutôt que par le silence.
    if trimmed.is_empty() || matches!(trimmed, "aide" | "help" | "?") {
        return Html(crate::views::console::help(trimmed).into_string());
    }

    let tokens = match shell_words::split(trimmed) {
        Ok(tokens) => tokens,
        Err(e) => {
            return Html(
                html! {
                    div class="line" { span class="prompt" { "❯" } " " span class="cmd" { (trimmed) } }
                    div class="out err" { "guillemets non fermés : " (e.to_string()) }
                }
                .into_string(),
            );
        }
    };

    let args = std::iter::once("freeflow".to_string()).chain(tokens);
    let db_path = state.db_path().to_path_buf();
    let outcome = state
        .with_store_mut(|store| {
            griffe_cli::run_capturing_with_vault(
                args,
                VaultAccess::Borrowed {
                    store,
                    db_path: &db_path,
                },
            )
        })
        .await;

    let (output, exit_code) = outcome.unwrap_or_else(|| {
        (
            "✗ coffre verrouillé — rechargez la page pour le déverrouiller".to_string(),
            2,
        )
    });

    Html(
        html! {
            div class="line" { span class="prompt" { "❯" } " " span class="cmd" { (trimmed) } }
            div class={ "out" @if exit_code != 0 { " err" } } { (output) }
        }
        .into_string(),
    )
}
