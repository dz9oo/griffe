ALTER TABLE invoices ADD COLUMN sequence INTEGER;
ALTER TABLE invoices ADD COLUMN credited_invoice_id TEXT REFERENCES invoices(id);

CREATE UNIQUE INDEX idx_invoices_sequence ON invoices(sequence);

-- Compteur de numérotation, un par exercice fiscal : FA-{fiscal_year}-{last_number}.
CREATE TABLE invoice_sequences (
    fiscal_year  INTEGER PRIMARY KEY,
    last_number  INTEGER NOT NULL
);

-- Lignes de relevé bancaire importées (CSV/OFX), rapprochées manuellement des factures.
CREATE TABLE bank_transactions (
    id                   TEXT PRIMARY KEY,
    occurred_on          TEXT NOT NULL,
    amount_cents         INTEGER NOT NULL,
    description          TEXT NOT NULL,
    matched_invoice_id   TEXT REFERENCES invoices(id)
);
CREATE UNIQUE INDEX idx_bank_transactions_dedup ON bank_transactions(occurred_on, amount_cents, description);

-- Une facture émise est immuable : la seule façon de l'annuler est un avoir (une nouvelle
-- facture à part entière), jamais une modification ou une suppression de la ligne existante.
CREATE TRIGGER trg_invoices_immutable_update
BEFORE UPDATE ON invoices
BEGIN
    SELECT RAISE(ABORT, 'les factures sont immuables : émettre un avoir plutôt que modifier');
END;

CREATE TRIGGER trg_invoices_immutable_delete
BEFORE DELETE ON invoices
BEGIN
    SELECT RAISE(ABORT, 'les factures sont immuables : émettre un avoir plutôt que supprimer');
END;

CREATE TRIGGER trg_invoice_lines_immutable_update
BEFORE UPDATE ON invoice_lines
BEGIN
    SELECT RAISE(ABORT, 'les lignes de facture sont immuables');
END;

CREATE TRIGGER trg_invoice_lines_immutable_delete
BEFORE DELETE ON invoice_lines
BEGIN
    SELECT RAISE(ABORT, 'les lignes de facture sont immuables');
END;
