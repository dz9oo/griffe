-- Lien stable entre une origine externe (page Notion, note, carte) et la fiche ou la piste.
-- L'empreinte est celle du dernier document appliqué sans garde. Une garde laisse l'empreinte
-- précédente : le même fichier est réévalué, il ne crée pas une seconde piste.
CREATE TABLE exchange_links (
    origin TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('fiche', 'piste')),
    entity_id TEXT NOT NULL,
    applied_at TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    next_action_on TEXT,
    PRIMARY KEY (origin, kind)
);

CREATE UNIQUE INDEX idx_exchange_links_entity ON exchange_links (kind, entity_id);
