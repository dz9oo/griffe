//! La coque commune à tous les écrans : barre de commandes, contenu, journal d'audit
//! repliable, palette ⌘K. Lot 46 : libellés français, nav primaire + « Plus », thème clair
//! par défaut. Chaque écran ne fournit que le contenu de `.content`.

use maud::{DOCTYPE, Markup, html};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewId {
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
            Self::Dashboard => "/view/dashboard",
            Self::Relances => "/view/relances",
            Self::Prospection => "/view/prospection",
            Self::Devis => "/view/devis",
            Self::Missions => "/view/missions",
            Self::Facturation => "/view/facturation",
            Self::Depenses => "/view/depenses",
            Self::Clients => "/view/clients",
            Self::Societe => "/view/societe",
            Self::Cloture => "/view/cloture",
            Self::Console => "/view/console",
        }
    }

    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
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

    /// Libellé français affiché dans la barre et le titre de page (lot 46).
    /// Le [`Self::slug`] reste l'identifiant d'URL / `data-view`.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Dashboard => "Tableau de bord",
            Self::Relances => "Relances",
            Self::Prospection => "Prospection",
            Self::Devis => "Devis",
            Self::Missions => "Missions",
            Self::Facturation => "Facturation",
            Self::Depenses => "Dépenses",
            Self::Clients => "Clients",
            Self::Societe => "Société",
            Self::Cloture => "Clôture",
            Self::Console => "Console",
        }
    }

    /// Flux quotidien : prospecter → deviser → réaliser → dépenser, plus les fiches clients.
    const PRIMARY: [Self; 6] = [
        Self::Relances,
        Self::Prospection,
        Self::Devis,
        Self::Missions,
        Self::Depenses,
        Self::Clients,
    ];

    /// Moins fréquent, derrière « Plus » — y compris Facturation (émission encore CLI/PA).
    const MORE: [Self; 4] = [
        Self::Facturation,
        Self::Societe,
        Self::Cloture,
        Self::Console,
    ];

    const ALL: [Self; 11] = [
        Self::Dashboard,
        Self::Relances,
        Self::Prospection,
        Self::Devis,
        Self::Missions,
        Self::Facturation,
        Self::Depenses,
        Self::Clients,
        Self::Societe,
        Self::Cloture,
        Self::Console,
    ];
}

fn tab_link(view: ViewId, active: ViewId) -> Markup {
    html! {
        a class={ "tab" @if view == active { " active" } }
          data-view=(view.slug())
          href=(view.path())
          hx-get=(view.path())
          hx-target="#content"
          hx-push-url="true"
          hx-swap="innerHTML" {
            (view.label())
        }
    }
}

fn tabs(active: ViewId) -> Markup {
    let more_active = ViewId::MORE.contains(&active);
    html! {
        div class="tabs" id="tabs" {
            @for view in ViewId::PRIMARY {
                (tab_link(view, active))
            }
            details class={ "nav-more" @if more_active { " has-active" } } id="nav-more" {
                summary class="tab" { "Plus" }
                div class="nav-more-menu" {
                    @for view in ViewId::MORE {
                        (tab_link(view, active))
                    }
                }
            }
        }
    }
}

fn palette() -> Markup {
    html! {
        div class="palette-overlay" id="palette-overlay" {
            div class="palette" {
                input id="palette-input" type="text" placeholder="Aller à… (écran, client, mission)" autocomplete="off";
                div class="palette-results" {
                    @for view in ViewId::ALL {
                        a class="palette-item" data-label=(format!("{} {}", view.label(), view.slug()))
                          href=(view.path())
                          hx-get=(view.path())
                          hx-target="#content"
                          hx-push-url="true"
                          hx-swap="innerHTML" {
                            (view.label())
                        }
                    }
                    // Actions, distinctes des écrans ci-dessus : ciblent `#panel`, jamais
                    // `#content`, et n'ont pas d'URL propre (pas de `hx-push-url`). À compléter
                    // au fil des lots suivants, un item par action de création.
                    button class="palette-item" type="button" data-label="nouveau client"
                      hx-get="/clients/new" hx-target="#panel" hx-swap="innerHTML" {
                        "+ nouveau client"
                    }
                    button class="palette-item" type="button" data-label="nouveau prospect"
                      hx-get="/prospection/new" hx-target="#panel" hx-swap="innerHTML" {
                        "+ nouveau prospect"
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
                        a class={ "brand" @if active == ViewId::Dashboard { " active" } }
                          href="/view/dashboard"
                          hx-get="/view/dashboard"
                          hx-target="#content"
                          hx-push-url="true" {
                            span class="dot" {} "FreeFlow"
                        }
                        (tabs(active))
                        div class="cmdbar-right" {
                            div class="lock" { span class="dot" {} (vault_label) }
                            button class="lock-btn" type="button" id="theme-toggle" title="Thème clair ou sombre" { "Sombre" }
                            button class="lock-btn" type="button" id="audit-toggle" title="Afficher ou masquer le journal d'audit" { "Journal" }
                            button class="lock-btn" hx-get="/lexique" hx-target="#panel" hx-swap="innerHTML" title="Lexique : les mots de la comptabilité expliqués" { "?" }
                            button class="lock-btn danger-hover" hx-post="/lock" hx-swap="none" title="Verrouiller le coffre" { "Verrouiller" }
                            div class="palette-hint" { "⌘K" }
                        }
                    }
                    div class="body-grid audit-collapsed" id="body-grid" {
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
        div class="view-head" data-view=(active.slug()) {
            div {
                div class="view-title" { (active.label()) }
                div class="view-sub" { (subtitle) }
            }
        }
    }
}
