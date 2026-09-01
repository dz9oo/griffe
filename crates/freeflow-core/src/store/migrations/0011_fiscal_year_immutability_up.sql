-- Lot 20 : régime « contre-écriture » pour un exercice approuvé, au même titre que les factures
-- (0004) et les devis (0005). Tant que `approved_on` est NULL, l'exercice est un projet éditable
-- (révision optimiste) ; dès qu'une AG l'a approuvé, la ligne devient immuable au niveau SQL —
-- une altération directe en base échoue bruyamment au lieu de réécrire silencieusement un acte
-- juridique daté. Le trigger UPDATE laisse passer l'approbation elle-même (OLD.approved_on IS
-- NULL) : c'est la transition qui scelle la ligne.
CREATE TRIGGER trg_fiscal_years_immutable_update
BEFORE UPDATE ON fiscal_years
WHEN OLD.approved_on IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'un exercice approuvé est immuable : la décision d''AG fait foi');
END;

CREATE TRIGGER trg_fiscal_years_immutable_delete
BEFORE DELETE ON fiscal_years
WHEN OLD.approved_on IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'un exercice approuvé est immuable : la décision d''AG fait foi');
END;
