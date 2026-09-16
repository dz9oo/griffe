CREATE TABLE interactions (
    id              TEXT PRIMARY KEY,
    opportunity_id  TEXT NOT NULL REFERENCES opportunities(id),
    kind            TEXT NOT NULL,
    note            TEXT NOT NULL,
    occurred_at     TEXT NOT NULL
);
CREATE INDEX idx_interactions_opportunity_id ON interactions(opportunity_id);
