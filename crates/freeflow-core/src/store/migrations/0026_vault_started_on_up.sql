-- Date à laquelle ce coffre a commencé à être utilisé. Les échéances antérieures ne sont pas
-- des retards dans FreeFlow : c'est un rattrapage (déjà déposé chez l'État, ou à dire).
-- Backfill : premier événement d'audit, sinon laissé NULL (la query retombe sur `today`).
ALTER TABLE setup ADD COLUMN created_on TEXT;
INSERT INTO setup (id, declared_new_company, updated_at, created_on)
SELECT 1, 0, datetime('now'), NULL
WHERE NOT EXISTS (SELECT 1 FROM setup WHERE id = 1);
UPDATE setup
SET created_on = (SELECT substr(min(occurred_at), 1, 10) FROM audit_log)
WHERE created_on IS NULL
  AND EXISTS (SELECT 1 FROM audit_log);
