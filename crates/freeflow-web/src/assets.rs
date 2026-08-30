//! Assets statiques, embarqués dans le binaire à la compilation — aucune requête réseau, aucun
//! CDN : `htmx` est vendorisé (`assets/htmx.min.js`, v2.0.4 telle que publiée par le projet),
//! cohérent avec l'absence de toute connexion sortante non explicite.

use axum::http::header;
use axum::response::IntoResponse;

const APP_CSS: &str = include_str!("../assets/app.css");
const APP_JS: &str = include_str!("../assets/app.js");
const HTMX_JS: &str = include_str!("../assets/htmx.min.js");

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
