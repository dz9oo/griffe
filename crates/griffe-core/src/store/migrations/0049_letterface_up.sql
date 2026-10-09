-- Habit des lettres : couleur des liens, ligne de métier, site en bas.
ALTER TABLE mail_account ADD COLUMN link_ink TEXT NOT NULL DEFAULT 'vert';
ALTER TABLE mail_account ADD COLUMN metier TEXT NOT NULL DEFAULT '';
ALTER TABLE mail_account ADD COLUMN site TEXT NOT NULL DEFAULT '';
