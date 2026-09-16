//! Assets statiques, embarqués dans le binaire à la compilation — aucune requête réseau, aucun
//! CDN : `htmx` est vendorisé (`assets/htmx.min.js`, v2.0.4 telle que publiée par le projet),
//! les woff2 aussi (`assets/fonts/`, OFL, lot 48). Cohérent avec l'absence de toute connexion
//! sortante non explicite.

use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};

const APP_CSS: &str = include_str!("../assets/app.css");
const APP_JS: &str = include_str!("../assets/app.js");
const HTMX_JS: &str = include_str!("../assets/htmx.min.js");

const NEWSREADER_400: &[u8] = include_bytes!("../assets/fonts/newsreader-latin-400-normal.woff2");
const NEWSREADER_400I: &[u8] = include_bytes!("../assets/fonts/newsreader-latin-400-italic.woff2");
const NEWSREADER_500: &[u8] = include_bytes!("../assets/fonts/newsreader-latin-500-normal.woff2");
const SOURCE_SANS_400: &[u8] =
    include_bytes!("../assets/fonts/source-sans-3-latin-400-normal.woff2");
const SOURCE_SANS_400I: &[u8] =
    include_bytes!("../assets/fonts/source-sans-3-latin-400-italic.woff2");
const SOURCE_SANS_600: &[u8] =
    include_bytes!("../assets/fonts/source-sans-3-latin-600-normal.woff2");

pub async fn app_css() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/css; charset=utf-8")], APP_CSS)
}

pub async fn app_js() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        APP_JS,
    )
}

pub async fn htmx_js() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        HTMX_JS,
    )
}

/// Sert un woff2 vendorisé. Les noms sont une liste blanche : un chemin libre ouvrirait
/// `include_bytes!` à rien, mais évite aussi de répondre 200 sur une invention.
pub async fn font(axum::extract::Path(name): axum::extract::Path<String>) -> Response {
    let bytes: &[u8] = match name.as_str() {
        "newsreader-latin-400-normal.woff2" => NEWSREADER_400,
        "newsreader-latin-400-italic.woff2" => NEWSREADER_400I,
        "newsreader-latin-500-normal.woff2" => NEWSREADER_500,
        "source-sans-3-latin-400-normal.woff2" => SOURCE_SANS_400,
        "source-sans-3-latin-400-italic.woff2" => SOURCE_SANS_400I,
        "source-sans-3-latin-600-normal.woff2" => SOURCE_SANS_600,
        _ => {
            return (StatusCode::NOT_FOUND, "police introuvable").into_response();
        }
    };
    (
        [
            (header::CONTENT_TYPE, "font/woff2"),
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
        ],
        bytes,
    )
        .into_response()
}
