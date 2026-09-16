# Design — Atelier

Source d’orientation : [`atelier-design.md`](./atelier-design.md).
Maquette : [`mockups/atelier-app.html`](./mockups/atelier-app.html).

Griffe est une **lettre du matin**, pas un dashboard. Trois pièces :
**Le jour**, **Les affaires**, **La société**.

## Tokens

| Token | Valeur | Rôle |
|---|---|---|
| `--paper` | `#f1ebe0` | fond |
| `--paper-2` | `#e8e0d2` | fond secondaire |
| `--ink` | `#1c1814` | texte |
| `--ink-2` | `#6a635a` | secondaire |
| `--seal` | `#9f3218` | accent |
| `--seal-soft` | `#f0d8cf` | accent faible |

Polices vendorisées (OFL) : Newsreader (titres, nom), Source Sans 3 (UI).
Aucune Google Font. Thème sombre retiré de la chrome.

## Français de la lettre

Dans l’UI quotidienne : « TVA du trimestre », « acompte de TVA », jamais
CA3 / 3514 / 2777. Les sigles existent dans le cœur et dans « Sur le site ».

## Interdit

Menu d’un item par module, tuiles KPI, Inter, teal SaaS, `hx-on--*`,
un écran « Factures » comme base de documents (le dossier d’une personne
porte les papiers).

Succès d’une mutation htmx : `200` à corps vide + `HX-Trigger: griffe:saved`.
