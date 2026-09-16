-- Un prospect est une fiche `clients` (devis, facture et opportunité portent un `client_id`)
-- qui n'apparaît pas dans l'onglet Clients tant qu'aucun devis ni aucune facture ne la
-- référence. `CreateClient` laisse le défaut 0 : une fiche créée comme client reste listée
-- immédiatement, même sans pièce commerciale. `CreateProspect` pose 1.
ALTER TABLE clients ADD COLUMN is_prospect INTEGER NOT NULL DEFAULT 0 CHECK (is_prospect IN (0, 1));
