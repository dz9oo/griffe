-- Crédit de TVA à reporter : case 27 de la dernière CA3 déjà déposée auprès de
-- l'administration, pour nourrir la case 25 de la suivante. Indépendant du bilan
-- d'ouverture (mauvaise date, et le bilan se fige dès qu'un exercice existe).
CREATE TABLE vat_carry_in (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    after_period TEXT NOT NULL,              -- AAAA-MM de la dernière CA3 déjà déposée
    credit_cents INTEGER NOT NULL CHECK (credit_cents >= 0),
    source TEXT,
    revision INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL
);
