-- Journal des dépôts de démarches d'État (lot 56) : un fait humain « c'est déposé »,
-- pas une preuve DGFiP. L'identité est (kind, period_key) — la période déclarée, pas
-- la date limite, qui peut glisser au jour ouvré.
CREATE TABLE duty_filings (
    id          TEXT PRIMARY KEY,
    kind        TEXT NOT NULL,
    period_key  TEXT NOT NULL,
    due_on      TEXT NOT NULL,
    filed_on    TEXT NOT NULL,
    UNIQUE (kind, period_key)
);
CREATE INDEX idx_duty_filings_kind ON duty_filings(kind, period_key);
