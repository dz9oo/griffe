-- Coffre documentaire (lot 57) : index des pièces. Les octets vivent dans
-- `<coffre>.receipts/` (même primitive que les justificatifs, lot 39).
CREATE TABLE papers (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  origin TEXT NOT NULL,
  original_name TEXT NOT NULL,
  filename TEXT NOT NULL,
  content_hash TEXT NOT NULL,
  mime TEXT NOT NULL,
  byte_size INTEGER NOT NULL CHECK (byte_size >= 0),
  period INTEGER,
  issued_on TEXT,
  client_id TEXT REFERENCES clients(id),
  invoice_id TEXT REFERENCES invoices(id),
  expense_id TEXT REFERENCES expenses(id),
  fiscal_year_id TEXT REFERENCES fiscal_years(id),
  note TEXT,
  superseded_by TEXT REFERENCES papers(id) ON DELETE SET NULL,
  captured_at TEXT NOT NULL,
  idempotency_key TEXT
);
CREATE UNIQUE INDEX papers_idempotency ON papers(idempotency_key) WHERE idempotency_key IS NOT NULL;
CREATE INDEX papers_period_kind ON papers(period, kind);
CREATE UNIQUE INDEX papers_issued_invoice ON papers(invoice_id, kind)
  WHERE invoice_id IS NOT NULL AND superseded_by IS NULL AND origin = 'issued';
