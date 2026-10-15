-- Le type et les phrases portent le même nom.
-- Un genre sans type devient un type. Un type sans genre reçoit les phrases
-- par défaut. Un dossier qui porte un genre reçoit ce type s'il manque.
-- Un dossier qui a des types et pas encore de genre prend les phrases du
-- premier type, par nom.

INSERT INTO work_kinds (id, name, tint, note, created_at)
SELECT lower(hex(randomblob(16))), g.name, 'encre', '', g.touched_at
  FROM prospect_genres AS g
 WHERE g.id = (
       SELECT g2.id
         FROM prospect_genres AS g2
        WHERE g2.name = g.name COLLATE NOCASE
        ORDER BY g2.name ASC, g2.id ASC
        LIMIT 1
   )
   AND NOT EXISTS (
       SELECT 1 FROM work_kinds AS k
        WHERE k.name = g.name COLLATE NOCASE
   );

INSERT INTO prospect_genres (id, name, touched_at)
SELECT lower(hex(randomblob(16))), k.name, k.created_at
  FROM work_kinds AS k
 WHERE k.id = (
       SELECT k2.id
         FROM work_kinds AS k2
        WHERE k2.name = k.name COLLATE NOCASE
        ORDER BY k2.name ASC, k2.id ASC
        LIMIT 1
   )
   AND NOT EXISTS (
       SELECT 1 FROM prospect_genres AS g
        WHERE g.name = k.name COLLATE NOCASE
   );

INSERT INTO prospect_genre_words (genre_id, phrase_key, subject, body)
SELECT g.id, p.id, p.subject, p.body
  FROM prospect_genres AS g
  JOIN prospect_phrases AS p
 WHERE NOT EXISTS (
     SELECT 1 FROM prospect_genre_words AS w
      WHERE w.genre_id = g.id AND w.phrase_key = p.id
 );

INSERT OR IGNORE INTO dossier_work_kinds (client_id, kind_id)
SELECT DISTINCT o.client_id, (
       SELECT k.id
         FROM work_kinds AS k
        WHERE k.name = g.name COLLATE NOCASE
        ORDER BY k.name ASC, k.id ASC
        LIMIT 1
   )
  FROM opportunities AS o
  JOIN prospect_genres AS g ON g.id = o.genre_id
 WHERE o.genre_id IS NOT NULL;

UPDATE opportunities
   SET genre_id = (
       SELECT g.id
         FROM dossier_work_kinds AS l
         JOIN work_kinds AS k ON k.id = l.kind_id
         JOIN prospect_genres AS g ON g.name = k.name COLLATE NOCASE
        WHERE l.client_id = opportunities.client_id
        ORDER BY k.name COLLATE NOCASE ASC, k.id ASC
        LIMIT 1
   )
 WHERE genre_id IS NULL
   AND EXISTS (
       SELECT 1 FROM dossier_work_kinds AS l
        WHERE l.client_id = opportunities.client_id
   );
