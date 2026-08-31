ALTER TABLE payments DROP COLUMN voided_at;

ALTER TABLE missions DROP COLUMN archived_at;
ALTER TABLE clients DROP COLUMN archived_at;

ALTER TABLE expenses DROP COLUMN revision;
ALTER TABLE time_entries DROP COLUMN revision;
ALTER TABLE missions DROP COLUMN revision;
ALTER TABLE opportunities DROP COLUMN revision;
ALTER TABLE contacts DROP COLUMN revision;
ALTER TABLE clients DROP COLUMN revision;
