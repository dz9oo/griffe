-- Demande de versement d'un crédit de TVA (case 26) sur une période CA3.
CREATE TABLE vat_refunds (
    period_key TEXT PRIMARY KEY,             -- AAAA-MM
    amount_cents INTEGER NOT NULL CHECK (amount_cents > 0),
    requested_on TEXT NOT NULL               -- AAAA-MM-JJ
);
