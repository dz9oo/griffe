-- La formule qui ferme les lettres. NULL : Bien à vous, le nom, la société.
ALTER TABLE mail_account ADD COLUMN signature TEXT;

-- Seulement la fin exacte des phrases semées et des mots d'un genre.
-- Une série déjà copiée sur une conversation n'est pas réécrite.

UPDATE prospect_phrases
SET body = substr(
        body,
        1,
        length(body) - length('Bien à vous,' || char(10) || '{{moi}}' || char(10) || '{{societe}}' || char(10))
    ) || '{{signature}}' || char(10)
WHERE body LIKE '%' || ('Bien à vous,' || char(10) || '{{moi}}' || char(10) || '{{societe}}' || char(10));

UPDATE prospect_phrases
SET body = substr(
        body,
        1,
        length(body) - length('Bien à vous,' || char(10) || '{{moi}}' || char(10) || '{{societe}}')
    ) || '{{signature}}'
WHERE body LIKE '%' || ('Bien à vous,' || char(10) || '{{moi}}' || char(10) || '{{societe}}');

UPDATE prospect_phrases
SET body = substr(
        body,
        1,
        length(body) - length('Bien à vous,' || char(10) || '{{moi}}' || char(10))
    ) || '{{signature}}' || char(10)
WHERE body LIKE '%' || ('Bien à vous,' || char(10) || '{{moi}}' || char(10));

UPDATE prospect_phrases
SET body = substr(
        body,
        1,
        length(body) - length('Bien à vous,' || char(10) || '{{moi}}')
    ) || '{{signature}}'
WHERE body LIKE '%' || ('Bien à vous,' || char(10) || '{{moi}}');

UPDATE prospect_genre_words
SET body = substr(
        body,
        1,
        length(body) - length('Bien à vous,' || char(10) || '{{moi}}' || char(10) || '{{societe}}' || char(10))
    ) || '{{signature}}' || char(10)
WHERE body LIKE '%' || ('Bien à vous,' || char(10) || '{{moi}}' || char(10) || '{{societe}}' || char(10));

UPDATE prospect_genre_words
SET body = substr(
        body,
        1,
        length(body) - length('Bien à vous,' || char(10) || '{{moi}}' || char(10) || '{{societe}}')
    ) || '{{signature}}'
WHERE body LIKE '%' || ('Bien à vous,' || char(10) || '{{moi}}' || char(10) || '{{societe}}');

UPDATE prospect_genre_words
SET body = substr(
        body,
        1,
        length(body) - length('Bien à vous,' || char(10) || '{{moi}}' || char(10))
    ) || '{{signature}}' || char(10)
WHERE body LIKE '%' || ('Bien à vous,' || char(10) || '{{moi}}' || char(10));

UPDATE prospect_genre_words
SET body = substr(
        body,
        1,
        length(body) - length('Bien à vous,' || char(10) || '{{moi}}')
    ) || '{{signature}}'
WHERE body LIKE '%' || ('Bien à vous,' || char(10) || '{{moi}}');
