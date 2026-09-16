UPDATE setup SET created_on = NULL;
-- SQLite ne retire pas la colonne ; le down se contente de vider.
