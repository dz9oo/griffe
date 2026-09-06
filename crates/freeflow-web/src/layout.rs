//! La coque commune à tous les écrans : chrome Atelier (lot 48), contenu, journal d'audit
//! replié, palette ⌘K. Trois pièces — Le jour, Les gens, La société — plus rien dans la
//! barre. La console et les anciens écrans restent joignables par ⌘K et leurs routes.

use maud::{DOCTYPE, Markup, html};

/// Les trois pièces de la chrome. Un écran ancien (relances, clôture…) appartient à l'une
/// d'elles pour le soulignement, ou à aucune (console).
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
            Self::Gens => "/gens",
            Self::Societe => "/societe",
        }
    }

    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Jour => "jour",
            Self::Gens => "gens",
            Self::Societe => "societe",
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Jour => "Le jour",
            Self::Gens => "Les gens",
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
}

impl ViewId {
    #[must_use]
    pub const fn path(self) -> &'static str {
        match self {
            Self::Jour => "/jour",
            Self::Gens => "/gens",
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
        }
    }

    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Jour => "jour",
            Self::Gens => "gens",
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
            Self::Gens => "Les gens",
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
            Self::Console => None,
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
    const PALETTE: [Self; 13] = [
        Self::Jour,
        Self::Gens,
        Self::Societe,
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
                      href="/gens/nouvelle"
                      hx-get="/gens/nouvelle" hx-target="#content" hx-push-url="true" hx-swap="innerHTML" {
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
                    button class="palette-item" type="button" data-label="lexique aide mots"
                      hx-get="/lexique" hx-target="#panel" hx-swap="innerHTML" {
                        "? lexique — les mots de la comptabilité"
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
pub fn page(active: ViewId, vault_label: &str, content: Markup) -> Markup {
    let piece = active.piece();
    html! {
        (DOCTYPE)
        html lang="fr" {
            head {
                meta charset="UTF-8";
                title { "FreeFlow — " (active.title()) }
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
                            "FreeFlow"
                        }
                        (nav(piece))
                    }
                    div class="body-grid audit-collapsed" id="body-grid" {
                        main class={ "content" @if active.wide() { " wide" } } id="content" {
                            (content)
                        }
                        (audit_rail())
                    }
                    footer class="foot" {
                        span class="lock" { span class="dot" {} (vault_label) }
                        span class="foot-actions" {
                            button class="quiet" type="button" id="audit-toggle" title="Afficher ou masquer le journal d'audit" { "Journal" }
                            button class="quiet" type="button" hx-get="/lexique" hx-target="#panel" hx-swap="innerHTML" title="Lexique : les mots de la comptabilité expliqués" { "Lexique" }
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

/// Coque nue pour `/unlock` et `/setup` : ni onglets, ni rail d'audit, ni palette ⌘K — et
/// surtout pas `app.js`, qui suppose leur présence (`#palette-overlay`/`#palette-input`) et
/// pilote le signal d'activité qui n'a pas de sens tant que le coffre n'est pas ouvert.
pub fn bare_page(title: &str, content: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="fr" {
            head {
                meta charset="UTF-8";
                title { "FreeFlow — " (title) }
                meta name="viewport" content="width=device-width, initial-scale=1.0";
                link rel="stylesheet" href="/assets/app.css";
            }
            body {
                div class="auth-shell" { (content) }
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
