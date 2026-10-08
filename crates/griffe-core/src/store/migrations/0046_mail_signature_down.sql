UPDATE prospect_phrases
SET body = substr(body, 1, length(body) - length('{{signature}}' || char(10)))
        || 'Bien à vous,' || char(10) || '{{moi}}' || char(10) || '{{societe}}' || char(10)
WHERE body LIKE '%' || ('{{signature}}' || char(10));

UPDATE prospect_phrases
SET body = substr(body, 1, length(body) - length('{{signature}}'))
        || 'Bien à vous,' || char(10) || '{{moi}}' || char(10) || '{{societe}}'
WHERE body LIKE '%' || '{{signature}}';

UPDATE prospect_genre_words
SET body = substr(body, 1, length(body) - length('{{signature}}' || char(10)))
        || 'Bien à vous,' || char(10) || '{{moi}}' || char(10) || '{{societe}}' || char(10)
WHERE body LIKE '%' || ('{{signature}}' || char(10));

UPDATE prospect_genre_words
SET body = substr(body, 1, length(body) - length('{{signature}}'))
        || 'Bien à vous,' || char(10) || '{{moi}}' || char(10) || '{{societe}}'
WHERE body LIKE '%' || '{{signature}}';

ALTER TABLE mail_account DROP COLUMN signature;
