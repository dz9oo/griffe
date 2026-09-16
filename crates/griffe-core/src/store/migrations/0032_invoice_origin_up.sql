ALTER TABLE invoices ADD COLUMN origin TEXT NOT NULL DEFAULT 'issued'
    CHECK (origin IN ('issued', 'imported'));
