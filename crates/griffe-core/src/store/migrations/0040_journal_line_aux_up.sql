-- Compte auxiliaire d'une ligne conservée (client sous 411). Les écritures
-- déjà posées (paiements de dépenses) n'en ont pas : les deux colonnes restent
-- nulles, et une ligne sans les deux est lue sans auxiliaire.

ALTER TABLE journal_lines ADD COLUMN aux_number TEXT;
ALTER TABLE journal_lines ADD COLUMN aux_label TEXT;
