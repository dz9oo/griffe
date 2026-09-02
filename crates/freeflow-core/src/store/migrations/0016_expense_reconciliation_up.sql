-- Lot 33 : rapprochement bancaire des dépenses. Depuis le lot 5, une transaction importée ne
-- se rapproche que d'une facture (`matched_invoice_id`) : les débits du relevé — les dépenses
-- réellement décaissées — restaient à jamais « à rapprocher ». Une transaction rapprochée
-- d'une dépense porte l'id de celle-ci, exactement comme pour une facture ; les deux colonnes
-- sont exclusives (garde dans les commandes : SQLite n'accepte pas de CHECK multi-colonnes en
-- ALTER TABLE). Une dépense ne se rapproche qu'à un seul débit — index unique partiel.
ALTER TABLE bank_transactions ADD COLUMN matched_expense_id TEXT REFERENCES expenses(id);
CREATE UNIQUE INDEX idx_bank_transactions_matched_expense
    ON bank_transactions(matched_expense_id) WHERE matched_expense_id IS NOT NULL;
