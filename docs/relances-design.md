# Relances (lot 47) — spec

Surface : **file d’une carte** (écran Relances, flux quotidien). Tableau « où j’en suis »
en second mode. Tableau de bord = briefing qui pointe ici.

Modèle : **journal de faits** + file dérivée. Pas d’instance de séquence. FreeFlow n’envoie
jamais le mail (`.eml` RFC 5322 + `xdg-open`).

## Faits

`DraftPrepared` · `MarkedSent` · `StepSkipped` · `Snoozed` · `DateSet` · `Retracted`

Envoyé et sauté avancent l’étape. Report et date à la main, non. Après la dernière étape,
une date est demandée (pas de recyclage silencieux).

## Deux horloges

- Prospect : écarts depuis le dernier geste (0 / 3 / 7 / 14). `next_action_at` est la
  projection écrite à chaque fait qui change l’échéance.
- Impayé : `due_on + J+n` (0 / 7 / 15 / 30). Un report n’est qu’une exception.

## Commandes

`SetFollowUpSender` · `PrepareFollowUp` (octets, pas de fichier) · `MarkFollowUpSent`
(confirmation si agent) · `SkipFollowUpStep` · `SnoozeFollowUp` · `SetFollowUpDate` ·
`RetractLastFollowUp`.

`today` vient de l’adaptateur. IO fichier = adaptateur (CLI / fenêtre).
