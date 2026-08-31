-- Socle du « copilote fiscal & social » (lot 17) : donner au domaine la notion d'exercice
-- comptable, de régime de TVA et de rémunération du dirigeant — les fondations que le calcul du
-- résultat, le calendrier chiffré, les documents de clôture et le prévisionnel consomment ensuite.
--
-- Toutes les colonnes ajoutées à `company_profile` sont nullables : un coffre existant reste
-- valide sans elles (le profil restait lisible sans exercice ni régime déclaré), et l'absence
-- vaut « non configuré » — le calendrier retombe alors sur son comportement indicatif antérieur.

-- Date de clôture d'exercice récurrente (mois + jour, typiquement 31/12), régime de TVA déclaré,
-- et rémunération du président (assimilé salarié) avec son ratio de charges paramétrable.
ALTER TABLE company_profile ADD COLUMN fiscal_year_end_month INTEGER;      -- 1..=12
ALTER TABLE company_profile ADD COLUMN fiscal_year_end_day INTEGER;        -- 1..=31
ALTER TABLE company_profile ADD COLUMN vat_regime TEXT;                    -- voir domain::VatRegime
ALTER TABLE company_profile ADD COLUMN director_monthly_gross_cents INTEGER; -- NULL = non rémunéré
ALTER TABLE company_profile ADD COLUMN director_charge_ratio_bps INTEGER;  -- ex. 8000 = 80 % du net

-- Exercices clôturés : la DÉCISION d'affectation du résultat est un acte juridique daté (AG
-- d'approbation) qui devient source de vérité et se cumule d'un exercice sur l'autre (report à
-- nouveau, réserve légale). Le calcul du résultat, lui, reste une requête pure recalculée à la
-- volée ; ce qu'on fige ici, c'est le snapshot au moment de la clôture + l'affectation validée.
CREATE TABLE fiscal_years (
    id TEXT PRIMARY KEY,
    starts_on TEXT NOT NULL,
    ends_on TEXT NOT NULL,
    -- Snapshot indicatif figé à la clôture (jamais recalculé après coup) :
    revenue_ht_cents INTEGER NOT NULL,
    expenses_cents INTEGER NOT NULL,
    director_remuneration_cents INTEGER NOT NULL,
    result_before_tax_cents INTEGER NOT NULL,
    corporate_tax_cents INTEGER NOT NULL,
    net_result_cents INTEGER NOT NULL,
    -- Affectation (décision d'AG) :
    legal_reserve_cents INTEGER NOT NULL DEFAULT 0,
    dividends_cents INTEGER NOT NULL DEFAULT 0,
    retained_earnings_cents INTEGER NOT NULL,   -- report à nouveau cumulé (peut être négatif)
    approved_on TEXT,                            -- date de l'AG (NULL tant que projet non approuvé)
    revision INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    UNIQUE (starts_on, ends_on)
);
