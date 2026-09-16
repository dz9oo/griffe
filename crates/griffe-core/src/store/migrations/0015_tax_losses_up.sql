-- Lot 32 : déficit fiscal reportable en avant (art. 209 I CGI) et report en arrière
-- (art. 220 quinquies CGI). Jusqu'ici, un exercice déficitaire ne portait qu'un report à nouveau
-- comptable : son déficit n'était jamais réimputé sur l'IS des exercices suivants, et l'option
-- de report en arrière n'existait pas.
--
-- Trois faits par exercice clos, figés dans le snapshot comme le reste (jamais recalculés) :
-- les déficits antérieurs imputés sur le bénéfice de l'exercice (ligne 360 du 2033-B), le
-- déficit de l'exercice reporté en arrière (ligne 356) et la créance d'IS qui en naît
-- (2039-SD, comptabilisée 444 contre 699). Le *stock* de déficits reportables n'est pas
-- stocké : il se dérive de la chaîne (déficit de chaque exercice − reporté en arrière − imputé
-- ensuite), donc les exercices clos avant ce lot — où rien n'était imputé — gardent
-- exactement la lecture qu'ils avaient : un déficit intégralement reportable.
ALTER TABLE fiscal_years ADD COLUMN losses_imputed_cents INTEGER NOT NULL DEFAULT 0;
ALTER TABLE fiscal_years ADD COLUMN carried_back_cents INTEGER NOT NULL DEFAULT 0;
ALTER TABLE fiscal_years ADD COLUMN carry_back_credit_cents INTEGER NOT NULL DEFAULT 0;

-- Le bilan d'ouverture (0014) reprend les capitaux propres ; il reprend aussi, hors bilan, les
-- déficits fiscaux antérieurs encore reportables (case 870 du dernier 2033-D déposé) — le
-- maillon zéro de la chaîne ci-dessus pour une société préexistante.
ALTER TABLE opening_balance ADD COLUMN tax_losses_cents INTEGER NOT NULL DEFAULT 0;
