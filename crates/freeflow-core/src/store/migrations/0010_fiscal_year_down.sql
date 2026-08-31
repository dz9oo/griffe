-- Annule 0010 : supprime la table des exercices clôturés et les colonnes ajoutées au profil.
DROP TABLE fiscal_years;

ALTER TABLE company_profile DROP COLUMN fiscal_year_end_month;
ALTER TABLE company_profile DROP COLUMN fiscal_year_end_day;
ALTER TABLE company_profile DROP COLUMN vat_regime;
ALTER TABLE company_profile DROP COLUMN director_monthly_gross_cents;
ALTER TABLE company_profile DROP COLUMN director_charge_ratio_bps;
