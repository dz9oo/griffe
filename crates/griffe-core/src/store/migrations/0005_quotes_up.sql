ALTER TABLE quotes ADD COLUMN root_id TEXT;
ALTER TABLE quotes ADD COLUMN discount_json TEXT;
ALTER TABLE quotes ADD COLUMN terms TEXT;

-- Le contenu d'un devis est immuable dès sa création : réviser un devis crée une nouvelle
-- version (même root_id, version+1), jamais une modification en place. Seul `status` change,
-- via des transitions contrôlées par la couche applicative — ce trigger ne bloque qu'un UPDATE
-- qui toucherait explicitement une colonne de contenu.
CREATE TRIGGER trg_quotes_content_immutable
BEFORE UPDATE OF client_id, opportunity_id, root_id, version, valid_until, terms, discount_json, created_at ON quotes
BEGIN
    SELECT RAISE(ABORT, 'le contenu d''un devis est immuable : créez une nouvelle version pour le modifier');
END;

CREATE TRIGGER trg_quotes_immutable_delete
BEFORE DELETE ON quotes
BEGIN
    SELECT RAISE(ABORT, 'un devis ne peut pas être supprimé : laissez-le expirer ou déclinez-le');
END;

CREATE TRIGGER trg_quote_lines_immutable_update
BEFORE UPDATE ON quote_lines
BEGIN
    SELECT RAISE(ABORT, 'les lignes de devis sont immuables');
END;

CREATE TRIGGER trg_quote_lines_immutable_delete
BEFORE DELETE ON quote_lines
BEGIN
    SELECT RAISE(ABORT, 'les lignes de devis sont immuables');
END;
