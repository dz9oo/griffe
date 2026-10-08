-- Libellé de ligne (EcritureLib). Une OD de liquidation CA3 porte plusieurs
-- libellés sur la même pièce. Null : le FEC reprend le libellé de l'écriture.

ALTER TABLE journal_lines ADD COLUMN ecriture_lib TEXT;
