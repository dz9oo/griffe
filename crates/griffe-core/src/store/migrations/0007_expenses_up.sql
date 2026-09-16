-- Complète la table `expenses` (créée au lot 1, jamais utilisée jusqu'ici) : la TVA déductible
-- n'est pas forcément la TVA payée en intégralité (véhicules de tourisme, restauration
-- plafonnée, etc.) — un montant explicite, pas un simple pourcentage du taux facial.
ALTER TABLE expenses ADD COLUMN category TEXT NOT NULL DEFAULT 'other';
ALTER TABLE expenses ADD COLUMN vat_deductible_cents INTEGER NOT NULL DEFAULT 0;
ALTER TABLE expenses ADD COLUMN receipt_filename TEXT;
ALTER TABLE expenses ADD COLUMN created_at TEXT NOT NULL DEFAULT '1970-01-01T00:00:00Z';
