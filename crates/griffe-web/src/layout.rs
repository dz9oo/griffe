//! La coque commune à tous les écrans : chrome Atelier (lot 48), contenu, journal d'audit
//! replié, palette ⌘K. Trois pièces — Le jour, Les affaires, La société — plus rien dans la
//! barre. La console, l'aide (`?`) et les anciens écrans restent joignables par ⌘K
//! et leurs routes.

use maud::{DOCTYPE, Markup, html};
use time::Date;

/// Les trois pièces de la chrome. Un écran ancien (relances, clôture…) appartient à l'une
/// d'elles pour le soulignement, ou à aucune (console, aide).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Piece {
    Jour,
    Gens,
    Societe,
}

impl Piece {
    const ALL: [Self; 3] = [Self::Jour, Self::Gens, Self::Societe];

    #[must_use]
    pub const fn path(self) -> &'static str {
        match self {
            Self::Jour => "/jour",
            Self::Gens => "/affaires",
            Self::Societe => "/societe",
        }
    }

    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Jour => "jour",
            Self::Gens => "affaires",
            Self::Societe => "societe",
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Jour => "Le jour",
            Self::Gens => "Les affaires",
            Self::Societe => "La société",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewId {
    Jour,
    Gens,
    Dashboard,
    Relances,
    Prospection,
    Devis,
    Missions,
    Facturation,
    Depenses,
    Clients,
    Societe,
    Cloture,
    Console,
    Aide,
    Boite,
}

impl ViewId {
    #[must_use]
    pub const fn path(self) -> &'static str {
        match self {
            Self::Jour => "/jour",
            Self::Gens => "/affaires",
            Self::Dashboard => "/view/dashboard",
            Self::Relances => "/view/relances",
            Self::Prospection => "/view/prospection",
            Self::Devis => "/view/devis",
            Self::Missions => "/view/missions",
            Self::Facturation => "/view/facturation",
            Self::Depenses => "/view/depenses",
            Self::Clients => "/view/clients",
            Self::Societe => "/societe",
            Self::Cloture => "/view/cloture",
            Self::Console => "/view/console",
            Self::Aide => "/aide",
            Self::Boite => "/boite",
        }
    }

    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Jour => "jour",
            Self::Gens => "affaires",
            Self::Dashboard => "dashboard",
            Self::Relances => "relances",
            Self::Prospection => "prospection",
            Self::Devis => "devis",
            Self::Missions => "missions",
            Self::Facturation => "facturation",
            Self::Depenses => "depenses",
            Self::Clients => "clients",
            Self::Societe => "societe",
            Self::Cloture => "cloture",
            Self::Console => "console",
            Self::Aide => "aide",
            Self::Boite => "boite",
        }
    }

    #[must_use]
    pub const fn title(self) -> &'static str {
        self.label()
    }

    /// Libellé français affiché dans le titre de page. Le [`Self::slug`] reste l'identifiant
    /// d'URL / `data-view`.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Jour => "Le jour",
            Self::Gens => "Les affaires",
            Self::Dashboard => "Le jour",
            Self::Relances => "Relances",
            Self::Prospection => "Prospection",
            Self::Devis => "Devis",
            Self::Missions => "Missions",
            Self::Facturation => "Facturation",
            Self::Depenses => "Dépenses",
            Self::Clients => "Clients",
            Self::Societe => "La société",
            Self::Cloture => "Clôture",
            Self::Console => "Console",
            Self::Aide => "Aide",
            Self::Boite => "Courriel",
        }
    }

    /// Pièce de la chrome à souligner, si cet écran en a une.
    #[must_use]
    pub const fn piece(self) -> Option<Piece> {
        match self {
            Self::Jour | Self::Dashboard | Self::Relances => Some(Piece::Jour),
            Self::Gens
            | Self::Prospection
            | Self::Devis
            | Self::Missions
            | Self::Facturation
            | Self::Clients => Some(Piece::Gens),
            Self::Societe | Self::Depenses | Self::Cloture => Some(Piece::Societe),
            Self::Console | Self::Aide | Self::Boite => None,
        }
    }

    /// La société (et ses chapitres encore servis par les anciens écrans) se lit en 960 px.
    #[must_use]
    pub const fn wide(self) -> bool {
        matches!(
            self,
            Self::Societe | Self::Depenses | Self::Cloture | Self::Console
        )
    }

    /// Palette ⌘K : les trois pièces d'abord, puis les écrans encore joignables hors nav.
    const PALETTE: [Self; 14] = [
        Self::Jour,
        Self::Gens,
        Self::Societe,
        Self::Boite,
        Self::Relances,
        Self::Prospection,
        Self::Devis,
        Self::Missions,
        Self::Facturation,
        Self::Depenses,
        Self::Clients,
        Self::Cloture,
        Self::Console,
        Self::Dashboard,
    ];
}

/// Le mot Courriel, entre les pièces et la date. Le nombre arrive par `GET /boite/sceau`.
pub fn boite_link(count: u32, current: bool) -> Markup {
    html! {
        a class="boite-link" href="/boite"
          hx-get="/boite"
          hx-target="#content"
          hx-push-url="true"
          hx-swap="innerHTML"
          aria-current=[current.then_some("page")] {
            span class="env" aria-hidden="true" { (envelope()) }
            "Courriel"
            (boite_count(count))
        }
    }
}

/// Le compte seul. Le sondage remplace cet élément, pas le lien.
pub fn boite_count(count: u32) -> Markup {
    html! {
        @if count > 0 {
            span id="boite-count" class="n" { (count) }
        } @else {
            span id="boite-count" {}
        }
    }
}

fn envelope() -> Markup {
    html! {
        svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 12" width="14" height="12" fill="none" aria-hidden="true" {
            rect x="1" y="1" width="14" height="10" rx="1.2" stroke="currentColor" stroke-width="1.25" {}
            path d="M1 2.2 8 7.2 15 2.2" stroke="currentColor" stroke-width="1.25" stroke-linecap="round" stroke-linejoin="round" {}
        }
    }
}

fn piece_link(piece: Piece, active: Option<Piece>) -> Markup {
    let current = active == Some(piece);
    html! {
        a data-piece=(piece.slug())
          aria-current=[current.then_some("page")]
          href=(piece.path())
          hx-get=(piece.path())
          hx-target="#content"
          hx-push-url="true"
          hx-swap="innerHTML" {
            (piece.label())
        }
    }
}

fn nav(active: Option<Piece>) -> Markup {
    html! {
        nav aria-label="Pièces" {
            @for piece in Piece::ALL {
                (piece_link(piece, active))
            }
        }
    }
}

fn palette() -> Markup {
    html! {
        div class="palette-overlay" id="palette-overlay" {
            div class="palette" {
                input id="palette-input" type="text" placeholder="Aller à… (pièce, client, mission)" autocomplete="off";
                div class="palette-results" {
                    @for view in ViewId::PALETTE {
                        @if view != ViewId::Dashboard {
                            a class="palette-item" data-label=(format!("{} {}", view.label(), view.slug()))
                              href=(view.path())
                              hx-get=(view.path())
                              hx-target="#content"
                              hx-push-url="true"
                              hx-swap="innerHTML" {
                                (view.label())
                            }
                        }
                    }
                    button class="palette-item" type="button" data-label="nouveau client"
                      hx-get="/clients/new" hx-target="#panel" hx-swap="innerHTML" {
                        "+ nouveau client"
                    }
                    a class="palette-item" data-label="nouveau prospect nouvelle conversation"
                      href="/affaires/nouvelle"
                      hx-get="/affaires/nouvelle" hx-target="#content" hx-push-url="true" hx-swap="innerHTML" {
                        "+ nouvelle conversation"
                    }
                    button class="palette-item" type="button" data-label="nouvelle mission"
                      hx-get="/missions/new" hx-target="#panel" hx-swap="innerHTML" {
                        "+ nouvelle mission"
                    }
                    button class="palette-item" type="button" data-label="nouveau devis"
                      hx-get="/devis/new" hx-target="#panel" hx-swap="innerHTML" {
                        "+ nouveau devis"
                    }
                    button class="palette-item" type="button" data-label="nouvelle dépense"
                      hx-get="/depenses/new" hx-target="#panel" hx-swap="innerHTML" {
                        "+ nouvelle dépense"
                    }
                    button class="palette-item" type="button" data-label="clore un exercice"
                      hx-get="/cloture/new" hx-target="#panel" hx-swap="innerHTML" {
                        "+ clore un exercice"
                    }
                    a class="palette-item" data-label="configurer ma société premiers pas"
                      href="/premiers-pas" hx-get="/premiers-pas" hx-target="#content" hx-push-url="true" hx-swap="innerHTML" {
                        "→ configurer ma société (premiers pas)"
                    }
                    a class="palette-item" data-label="aide lexique ? atelier agent"
                      href="/aide" hx-get="/aide" hx-target="#content" hx-push-url="true" hx-swap="innerHTML" {
                        "? aide — l'atelier, les recettes"
                    }
                    @for page in crate::views::aide::guide_letters() {
                        @let href = format!("/aide/{}", page.slug);
                        @let label = format!("{} {}", page.kicker, page.title).to_lowercase();
                        a class="palette-item" data-label=(label)
                          href=(href)
                          hx-get=(href) hx-target="#content" hx-push-url="true" hx-swap="innerHTML" {
                            (page.title)
                        }
                    }
                }
            }
        }
    }
}

/// Le rail d'audit est vide de contenu initial ici : `GET /audit/recent` le peuple au
/// chargement puis toutes les 2 secondes (`hx-trigger="load, every 2s"`), remplacement complet
/// à chaque fois — voir [`crate::audit`] pour le choix du polling plutôt que SSE.
fn audit_rail() -> Markup {
    html! {
        div class="audit" {
            h3 { "Journal" span { "live" } }
            div id="audit-items"
                hx-get="/audit/recent"
                hx-trigger="load, every 2s"
                hx-swap="innerHTML" {
                div class="audit-empty" { "chargement…" }
            }
        }
    }
}

/// Page complète : utilisée pour un chargement direct (navigation, rechargement). Les
/// navigations suivantes, boostées par htmx, ne redemandent que le fragment dans `#content`.
///
/// La date du jour est dans la chrome, hors `#content` : un fragment ne la redessine pas.
pub fn page(active: ViewId, vault_label: &str, today: Date, content: Markup) -> Markup {
    let piece = active.piece();
    html! {
        (DOCTYPE)
        html lang="fr" {
            head {
                meta charset="UTF-8";
                title { "Griffe — " (active.title()) }
                meta name="viewport" content="width=device-width, initial-scale=1.0";
                link rel="stylesheet" href="/assets/app.css";
                script src="/assets/htmx.min.js" {}
            }
            body {
                div class="desk" {
                    header class="chrome" {
                        a class="mark" href=(Piece::Jour.path())
                          hx-get=(Piece::Jour.path())
                          hx-target="#content"
                          hx-push-url="true" {
                            (crate::brand::wordmark())
                        }
                        (nav(piece))
                        div class="chrome-end" {
                            span class="boite-slot" id="boite-slot"
                              hx-get="/boite/sceau"
                              hx-trigger="load, griffe:saved from:body, griffe:box from:body"
                              hx-target="#boite-count"
                              hx-swap="outerHTML" {
                                (boite_link(0, active == ViewId::Boite))
                            }
                            span class="today" { (crate::views::copy::letter_date(today)) }
                        }
                    }
                    div class="body-grid audit-collapsed" id="body-grid" {
                        main class={ "content" @if active.wide() { " wide" } } id="content" {
                            (content)
                        }
                        (audit_rail())
                    }
                    footer class="foot" {
                        span class="foot-status" {
                            span class="lock" { span class="dot" {} (vault_label) }
                            (app_version())
                        }
                        span class="foot-actions" {
                            button class="quiet" type="button" id="audit-toggle" title="Afficher ou masquer le journal d'audit" { "Journal" }
                            a class="quiet" id="aide-link" href="/aide"
                              hx-get="/aide" hx-target="#content" hx-push-url="true" hx-swap="innerHTML"
                              title="L'atelier, avec un agent, les recettes" { "Aide" }
                            button class="quiet danger-hover" type="button" hx-post="/lock" hx-swap="none" title="Verrouiller le coffre" { "Verrouiller" }
                            button class="palette-hint quiet" type="button" title="Palette de commandes" { "⌘K" }
                        }
                    }
                }
                (palette())
                div id="panel" {}
                script src="/assets/app.js" {}
            }
        }
    }
}

/// Numéro compilé dans ce binaire (`Cargo.toml` du workspace). Le même que `griffe --version`.
fn app_version() -> Markup {
    html! {
        span class="app-version" { "Griffe " (env!("CARGO_PKG_VERSION")) }
    }
}

/// Coque nue pour `/unlock` et `/setup` : ni onglets, ni rail d'audit, ni palette ⌘K — et
/// surtout pas `app.js`, qui suppose leur présence (`#palette-overlay`/`#palette-input`) et
/// pilote le signal d'activité qui n'a pas de sens tant que le coffre n'est pas ouvert.
pub fn bare_page(title: &str, content: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="fr" {
            head {
                meta charset="UTF-8";
                title { "Griffe — " (title) }
                meta name="viewport" content="width=device-width, initial-scale=1.0";
                link rel="stylesheet" href="/assets/app.css";
            }
            body {
                div class="auth-shell" {
                    div class="auth-column" {
                        (content)
                        (app_version())
                    }
                }
            }
        }
    }
}

pub fn view_head(active: ViewId, subtitle: &str) -> Markup {
    html! {
        div class="view-head" data-view=(active.slug()) {
            div {
                div class="view-title" { (active.label()) }
                div class="view-sub" { (subtitle) }
            }
        }
    }
}
