DROP TRIGGER trg_invoice_lines_immutable_delete;
DROP TRIGGER trg_invoice_lines_immutable_update;
DROP TRIGGER trg_invoices_immutable_delete;
DROP TRIGGER trg_invoices_immutable_update;
DROP TABLE bank_transactions;
DROP TABLE invoice_sequences;
DROP INDEX idx_invoices_sequence;
ALTER TABLE invoices DROP COLUMN credited_invoice_id;
ALTER TABLE invoices DROP COLUMN sequence;
