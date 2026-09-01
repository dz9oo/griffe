//! La coque commune à tous les écrans : barre de commandes (navigation), grille à deux
//! colonnes (contenu + rail d'audit persistant), palette ⌘K. Chaque écran ne fournit que le
//! contenu de `.content` — cette fonction assemble le reste, à l'identique de la maquette
//! Studio retenue.

use maud::{DOCTYPE, Markup, html};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewId {
    Dashboard,
    Prospection,
    Devis,
    Missions,
    Facturation,
    Depenses,
    Clients,
    Cloture,
    Console,
}

impl ViewId {
    #[must_use]
    pub const fn path(self) -> &'static str {
        match self {
            Self::Dashboard => "/view/dashboard",
            Self::Prospection => "/view/prospection",
            Self::Devis => "/view/devis",
            Self::Missions => "/view/missions",
            Self::Facturation => "/view/facturation",
            Self::Depenses => "/view/depenses",
            Self::Clients => "/view/clients",
            Self::Cloture => "/view/cloture",
            Self::Console => "/view/console",
        }
    }

    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Dashboard => "dashboard",
            Self::Prospection => "prospection",
            Self::Devis => "devis",
            Self::Missions => "missions",
            Self::Facturation => "facturation",
            Self::Depenses => "depenses",
            Self::Clients => "clients",
            Self::Cloture => "cloture",
            Self::Console => "console",
        }
    }

    #[must_use]
    pub const fn title(self) -> &'static str {
        self.slug()
    }

    // L'ordre suit le flux de travail : prospecter → deviser → réaliser → facturer → dépenser.
    const ALL: [Self; 9] = [
        Self::Dashboard,
        Self::Prospection,
        Self::Devis,
        Self::Missions,
        Self::Facturation,
        Self::Depenses,
        Self::Clients,
        Self::Cloture,
        Self::Console,
    ];
}

fn tabs(active: ViewId) -> Markup {
    html! {
        div class="tabs" id="tabs" {
            @for view in ViewId::ALL {
                a class={ "tab" @if view == active { " active" } }
                  data-view=(view.slug())
                  href=(view.path())
                  hx-get=(view.path())
                  hx-target="#content"
                  hx-push-url="true"
                  hx-swap="innerHTML" {
                    (view.slug())
                }
            }
        }
    }
}

fn palette() -> Markup {
    html! {
        div class="palette-overlay" id="palette-overlay" {
            div class="palette" {
                input id="palette-input" type="text" placeholder="aller à… (client, mission, facture)" autocomplete="off";
                div class="palette-results" {
                    @for view in ViewId::ALL {
                        a class="palette-item" data-label=(view.slug())
                          href=(view.path())
                          hx-get=(view.path())
                          hx-target="#content"
                          hx-push-url="true"
                          hx-swap="innerHTML" {
                            "→ " (view.slug())
                        }
                    }
                    // Actions, distinctes des écrans ci-dessus : ciblent `#panel`, jamais
                    // `#content`, et n'ont pas d'URL propre (pas de `hx-push-url`). À compléter
                    // au fil des lots suivants, un item par action de création.
                    button class="palette-item" type="button" data-label="nouveau client"
                      hx-get="/clients/new" hx-target="#panel" hx-swap="innerHTML" {
                        "+ nouveau client"
                    }
                    button class="palette-item" type="button" data-label="nouvelle opportunité"
                      hx-get="/prospection/new" hx-target="#panel" hx-swap="innerHTML" {
                        "+ nouvelle opportunité"
                    }
                    button class="palette-item" type="button" data-label="nouvelle mission"
                      hx-get="/missions/new" hx-target="#panel" hx-swap="innerHTML" {
                        "+ nouvelle mission"
                    }
                    button class="palette-item" type="button" data-label="nouvelle dépense"
                      hx-get="/depenses/new" hx-target="#panel" hx-swap="innerHTML" {
                        "+ nouvelle dépense"
                    }
                    button class="palette-item" type="button" data-label="clore un exercice"
                      hx-get="/cloture/new" hx-target="#panel" hx-swap="innerHTML" {
                        "+ clore un exercice"
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
            h3 { "journal d'audit" span { "live" } }
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
/// navigations suivantes, boostées par htmx, ne redemandent que [`view_fragment`].
pub fn page(active: ViewId, vault_label: &str, content: Markup) -> Markup {
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
                div class="shell" {
                    div class="cmdbar" {
                        a class="brand" href="/view/dashboard" hx-get="/view/dashboard" hx-target="#content" hx-push-url="true" {
                            span class="dot" {} "freeflow"
                        }
                        (tabs(active))
                        div class="cmdbar-right" {
                            div class="lock" { span class="dot" {} (vault_label) }
                            button class="lock-btn" hx-post="/lock" hx-swap="none" title="verrouiller le coffre" { "verrouiller" }
                            div class="palette-hint" { "⌘K palette de commandes" }
                        }
                    }
                    div class="body-grid" {
                        div class="content" id="content" { (content) }
                        (audit_rail())
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
        div class="view-head" {
            div {
                div class="view-title" { span class="prefix" { "~/" } (active.slug()) }
                div class="view-sub" { (subtitle) }
            }
        }
    }
}
