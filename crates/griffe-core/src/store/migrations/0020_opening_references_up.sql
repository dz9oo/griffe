-- Lot 40 : deux références de l'exercice précédent, hors bilan, que le calendrier fiscal
-- affirmait jusqu'ici sans les connaître : l'IS de l'exercice précédent (base des acomptes d'IS,
-- dispense sous 3 000 €) et la TVA due de l'exercice précédent (base des acomptes 3514 du réel
-- simplifié). NULL = inconnu : le calendrier le dit alors en clair au lieu d'affirmer une
-- dispense.
ALTER TABLE opening_balance ADD COLUMN prior_corporate_tax_cents INTEGER;
ALTER TABLE opening_balance ADD COLUMN prior_vat_due_cents INTEGER;
