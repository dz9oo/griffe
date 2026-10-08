-- Le type dit le métier du dossier. Le genre ne change que les phrases.
-- Pas de clé étrangère : la commande tient l'unicité du nom et retire les liens.
CREATE TABLE work_kinds (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE dossier_work_kinds (
    client_id TEXT NOT NULL,
    kind_id TEXT NOT NULL,
    PRIMARY KEY (client_id, kind_id)
);
