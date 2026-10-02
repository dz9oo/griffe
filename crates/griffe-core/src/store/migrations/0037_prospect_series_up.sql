-- Une conversation déjà engagée finit la série sur laquelle elle est entrée.
-- Tant qu'aucune ligne n'existe, tout le monde lit la série vivante.
-- cycle_key vide : le cycle d'origine. Sinon l'identifiant du fait cycle_opened.
CREATE TABLE prospect_series (
    id TEXT PRIMARY KEY,
    opportunity_id TEXT NOT NULL,
    cycle_key TEXT NOT NULL,
    UNIQUE (opportunity_id, cycle_key)
);

CREATE TABLE prospect_series_steps (
    series_id TEXT NOT NULL,
    position INTEGER NOT NULL CHECK (position >= 0),
    phrase_key TEXT NOT NULL,
    label TEXT NOT NULL,
    offset_days INTEGER NOT NULL CHECK (offset_days >= 0),
    subject TEXT NOT NULL,
    body TEXT NOT NULL,
    PRIMARY KEY (series_id, position)
);

CREATE INDEX idx_prospect_series_opportunity
    ON prospect_series (opportunity_id, cycle_key);
