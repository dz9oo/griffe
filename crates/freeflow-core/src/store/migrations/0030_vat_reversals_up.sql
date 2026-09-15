-- TVA antérieurement déduite à reverser (case 15) sur une période CA3.
CREATE TABLE vat_reversals (
    period_key TEXT PRIMARY KEY,             -- AAAA-MM
    amount_cents INTEGER NOT NULL CHECK (amount_cents > 0),
    recorded_on TEXT NOT NULL                -- AAAA-MM-JJ
);
