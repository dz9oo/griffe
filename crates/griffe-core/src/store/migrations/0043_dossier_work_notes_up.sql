-- Le récit des travaux appartient au dossier, pas à l'étape de la relance.
-- Pas de clé étrangère : la commande tient la cohérence.
CREATE TABLE dossier_work_notes (
    client_id  TEXT PRIMARY KEY,
    body       TEXT NOT NULL,
    revision   INTEGER NOT NULL,
    updated_at TEXT NOT NULL
);
