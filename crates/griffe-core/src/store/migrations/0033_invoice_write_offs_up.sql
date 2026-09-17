CREATE TABLE invoice_write_offs (
    id TEXT PRIMARY KEY NOT NULL,
    invoice_id TEXT NOT NULL REFERENCES invoices(id),
    written_off_on TEXT NOT NULL,
    ht_cents INTEGER NOT NULL,
    vat_cents INTEGER NOT NULL,
    ttc_cents INTEGER NOT NULL,
    recovers_vat INTEGER NOT NULL CHECK (recovers_vat IN (0, 1)),
    retracted_on TEXT
);
CREATE UNIQUE INDEX invoice_write_offs_one_live
    ON invoice_write_offs(invoice_id) WHERE retracted_on IS NULL;
