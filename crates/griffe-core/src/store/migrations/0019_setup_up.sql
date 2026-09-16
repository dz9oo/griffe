-- Lot 39 : premier lancement guidé. Deux choses que le parcours de clôture et les documents
-- (PV nominatif du lot 41, 2033-F) réclament et que le profil ne portait pas : l'identité de
-- l'associé unique et du président, et le nombre d'actions. Toutes nullables : un coffre
-- existant reste valide, `setup_status` les signale simplement comme manquantes.
ALTER TABLE company_profile ADD COLUMN president_name TEXT;
ALTER TABLE company_profile ADD COLUMN sole_shareholder_name TEXT;
ALTER TABLE company_profile ADD COLUMN sole_shareholder_address TEXT;
ALTER TABLE company_profile ADD COLUMN share_count INTEGER;

-- Le choix explicite « société nouvelle » : sans bilan d'ouverture, l'assistant ne sait pas si
-- la reprise a été oubliée ou si la société vient de naître — ce n'est pas une donnée du bilan
-- (qui n'existe pas dans ce cas), d'où une ligne unique à part plutôt qu'une colonne de
-- `opening_balance` (dont `opens_on` est obligatoire). `CHECK (id = 1)` comme les autres
-- singletons.
CREATE TABLE setup (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    declared_new_company INTEGER NOT NULL DEFAULT 0,
    updated_at TEXT NOT NULL
);
