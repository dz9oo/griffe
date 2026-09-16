-- Lot 41 : conformité des déclarations. Le bénéficiaire d'une dépense d'honoraires (DAS2 :
-- cumul par bénéficiaire et par année civile, seuil 2 400 € — art. 240 CGI, BOI-BIC-DECLA-30-70-20
-- § 140) et le montant des dépenses non déductibles à mentionner au PV d'approbation (art. 223
-- quater CGI), décidé à la clôture. Nullables / défaut zéro : un coffre existant reste valide.
ALTER TABLE expenses ADD COLUMN supplier TEXT;
ALTER TABLE fiscal_years ADD COLUMN non_deductible_expenses_cents INTEGER NOT NULL DEFAULT 0;
