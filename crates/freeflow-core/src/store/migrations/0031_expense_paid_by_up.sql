-- Lot 63 : qui a payé la dépense (société / associé). Défaut company = comportement historique
-- (crédit 512 si non rapprochée).
ALTER TABLE expenses ADD COLUMN paid_by TEXT NOT NULL DEFAULT 'company';
