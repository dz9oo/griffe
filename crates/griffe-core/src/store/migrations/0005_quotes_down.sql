DROP TRIGGER trg_quote_lines_immutable_delete;
DROP TRIGGER trg_quote_lines_immutable_update;
DROP TRIGGER trg_quotes_immutable_delete;
DROP TRIGGER trg_quotes_content_immutable;
ALTER TABLE quotes DROP COLUMN terms;
ALTER TABLE quotes DROP COLUMN discount_json;
ALTER TABLE quotes DROP COLUMN root_id;
