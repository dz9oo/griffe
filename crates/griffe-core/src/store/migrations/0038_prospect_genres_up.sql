-- Le genre ne change que les mots. L'ordre, le nombre de moments et les écarts
-- restent ceux de la série, les mêmes pour tout le monde.
-- Pas de clé étrangère : une fiche peut rester sans genre, et retirer un genre
-- vide les dossiers qui le portaient.
CREATE TABLE prospect_genres (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    touched_at TEXT NOT NULL
);

CREATE TABLE prospect_genre_words (
    genre_id TEXT NOT NULL,
    phrase_key TEXT NOT NULL,
    subject TEXT NOT NULL,
    body TEXT NOT NULL,
    PRIMARY KEY (genre_id, phrase_key)
);

ALTER TABLE opportunities ADD COLUMN genre_id TEXT;
