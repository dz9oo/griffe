-- Adresses que la relève copie. Liste vide : tout le compte.
-- Le compteur change quand la liste change. Le miroir s'aligne à la passe suivante.
ALTER TABLE mail_account ADD COLUMN kept_generation INTEGER NOT NULL DEFAULT 0;

CREATE TABLE mail_kept_address (
    address TEXT PRIMARY KEY COLLATE NOCASE,
    position INTEGER NOT NULL
);
