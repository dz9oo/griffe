-- Détail d'une estimation : des travaux, pas un devis immuable.
CREATE TABLE estimation_lines (
    id              INTEGER PRIMARY KEY,
    opportunity_id  TEXT NOT NULL REFERENCES opportunities(id),
    position        INTEGER NOT NULL,
    label           TEXT NOT NULL,
    amount_cents    INTEGER NOT NULL
);
CREATE INDEX idx_estimation_lines_opportunity
    ON estimation_lines(opportunity_id, position);
