-- Les lettres reçues ne chargent pas les images distantes, sauf geste contraire.
ALTER TABLE mail_account ADD COLUMN block_remote INTEGER NOT NULL DEFAULT 1;
