//! `POST /console/run` : exécute la ligne tapée dans la console via
//! [`freeflow_cli::run_capturing`] — le même parseur, la même sortie, qu'un terminal verrait,
//! simplement renvoyée en HTML au lieu d'être imprimée sur un stdout de process serveur.

use axum::Form;
use axum::response::Html;
use maud::html;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct ConsoleInput {
    line: String,
}

pub async fn run(Form(input): Form<ConsoleInput>) -> Html<String> {
    let trimmed = input.line.trim();
    if trimmed.is_empty() {
        return Html(String::new());
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
    let (output, exit_code) = freeflow_cli::run_capturing(args);

    Html(
        html! {
            div class="line" { span class="prompt" { "❯" } " " span class="cmd" { (trimmed) } }
            div class={ "out" @if exit_code != 0 { " err" } } { (output) }
        }
        .into_string(),
    )
}
