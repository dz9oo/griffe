-- Écritures conservées. Une correction est une nouvelle écriture (extourne),
-- jamais une modification des montants. Les numéros FEC sont attribués à la
-- lecture, après tri : ils ne sont pas stockés.
--
-- Une seule écriture vivante par fait (source_kind, source_id). L'originale
-- extournée a reversed_at ; l'extourne a reversal_of. Les deux restent lisibles.

CREATE TABLE journal_entries (
    id TEXT PRIMARY KEY,
    journal TEXT NOT NULL,
    entry_date TEXT NOT NULL,
    piece_ref TEXT NOT NULL,
    piece_date TEXT NOT NULL,
    label TEXT NOT NULL,
    source_kind TEXT NOT NULL,
    source_id TEXT NOT NULL,
    reversal_of TEXT REFERENCES journal_entries(id),
    reversed_at TEXT,
    created_at TEXT NOT NULL
);

CREATE TABLE journal_lines (
    entry_id TEXT NOT NULL REFERENCES journal_entries(id),
    position INTEGER NOT NULL,
    account TEXT NOT NULL,
    account_label TEXT NOT NULL,
    amount_cents INTEGER NOT NULL,
    PRIMARY KEY (entry_id, position)
);

CREATE UNIQUE INDEX journal_entries_one_live
    ON journal_entries (source_kind, source_id)
    WHERE reversed_at IS NULL AND reversal_of IS NULL;
