-- Standard de la fiche, distinct de la personne qui répond.
ALTER TABLE clients ADD COLUMN switchboard_email TEXT;
ALTER TABLE clients ADD COLUMN switchboard_phone TEXT;

UPDATE clients
SET switchboard_email = (
        SELECT email FROM contacts
        WHERE contacts.client_id = clients.id AND correspondent = 1
    ),
    switchboard_phone = (
        SELECT phone FROM contacts
        WHERE contacts.client_id = clients.id AND correspondent = 1
    );

ALTER TABLE contacts ADD COLUMN given_name TEXT NOT NULL DEFAULT '';
ALTER TABLE contacts ADD COLUMN family_name TEXT NOT NULL DEFAULT '';
ALTER TABLE contacts ADD COLUMN civility TEXT NOT NULL DEFAULT ''
    CHECK (civility IN ('', 'monsieur', 'madame'));

-- Le courriel déjà connu devient le standard. La personne n'en a plus.
UPDATE contacts
SET email = NULL, phone = NULL
WHERE correspondent = 1;

-- Quelle adresse une lettre relit au départ.
ALTER TABLE outbound_mail ADD COLUMN recipient TEXT NOT NULL DEFAULT 'switchboard'
    CHECK (recipient IN ('switchboard', 'correspondent'));
