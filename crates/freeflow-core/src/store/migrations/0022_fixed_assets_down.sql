ALTER TABLE fiscal_years DROP COLUMN depreciation_cents;
DROP INDEX IF EXISTS fixed_assets_expense;
DROP TABLE IF EXISTS fixed_assets;
