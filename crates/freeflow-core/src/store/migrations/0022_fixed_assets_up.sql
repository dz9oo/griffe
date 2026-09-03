-- Lot 42 : immobilisations et amortissements. Jusqu'ici tout équipement passait en charge et un
-- compte 28x repris au bilan d'ouverture restait figé (constat 18 de l'audit du 2 septembre
-- 2026). Une immobilisation est un fait durable : libellé, compte 2xx, mise en service, base
-- amortissable HT, durée en mois (linéaire seul), et pour une reprise de bilan de cabinet la
-- date à partir de laquelle l'application calcule les dotations (`depreciated_from`, la date du
-- bilan d'ouverture) avec le cumul déjà amorti (`prior_depreciation_cents`, le 28x repris). Les
-- dotations elles-mêmes ne sont pas stockées : dérivées par `domain::FixedAsset` à chaque
-- lecture (comme le grand livre), donc jamais désynchronisées.
--
-- `expense_id` : la dépense « immobilisée » (équipement au-delà de 500 € HT, BOI-BIC-CHG-20-30-10
-- § 20) — sa charge est remplacée par l'entrée à l'actif dans le grand livre. Une dépense au plus
-- par immobilisation (index unique partiel), et la suppression de la dépense est refusée par le
-- cœur tant que l'immobilisation existe (la FK sans cascade le garantit aussi).
CREATE TABLE fixed_assets (
    id TEXT PRIMARY KEY,
    label TEXT NOT NULL,
    account TEXT NOT NULL,                 -- compte d'immobilisation (domain::AccountCode, 2xx)
    acquired_on TEXT NOT NULL,             -- mise en service
    base_cents INTEGER NOT NULL CHECK (base_cents > 0),
    duration_months INTEGER NOT NULL CHECK (duration_months > 0),
    depreciated_from TEXT NOT NULL,        -- mise en service, ou date du bilan d'ouverture
    prior_depreciation_cents INTEGER NOT NULL DEFAULT 0 CHECK (prior_depreciation_cents >= 0),
    expense_id TEXT REFERENCES expenses(id),
    revision INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL
);
CREATE UNIQUE INDEX fixed_assets_expense ON fixed_assets(expense_id) WHERE expense_id IS NOT NULL;

-- La dotation aux amortissements figée au snapshot de clôture (ligne 254 du 2033-B) — comme
-- `expenses_cents`, le snapshot doit porter ce qu'il a compté. Défaut zéro : un exercice clos
-- avant ce lot n'en avait aucune.
ALTER TABLE fiscal_years ADD COLUMN depreciation_cents INTEGER NOT NULL DEFAULT 0;
