-- Prestation intracommunautaire autoliquidée. Défaut 0 : les dépenses déjà là
-- restent des paiements domestiques.
ALTER TABLE expenses ADD COLUMN reverse_charge INTEGER NOT NULL DEFAULT 0;
