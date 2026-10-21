-- En-têtes de fil, pour qu'une réponse parte avec la lettre qui l'a suscitée.
ALTER TABLE outbound_mail ADD COLUMN in_reply_to TEXT;
ALTER TABLE outbound_mail ADD COLUMN "references" TEXT;

-- Rattachement d'un message reçu. Le corps reste dans le miroir, pas ici.
CREATE TABLE mail_filing (
    message_id TEXT PRIMARY KEY,
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    opportunity_id TEXT REFERENCES opportunities(id) ON DELETE SET NULL,
    source TEXT NOT NULL CHECK (source IN ('auto', 'human'))
);
