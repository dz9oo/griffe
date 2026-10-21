-- Une réponse humaine retient la prochaine relance de ce dossier.
ALTER TABLE opportunities ADD COLUMN reply_hold INTEGER NOT NULL DEFAULT 0
    CHECK (reply_hold IN (0, 1));

-- Adresse qui ne reçoit pas, ou qui a été confirmée. On ne l'efface pas.
CREATE TABLE address_fault (
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    address TEXT NOT NULL COLLATE NOCASE,
    status TEXT NOT NULL CHECK (status IN ('failed', 'probable', 'confirmed')),
    message_id TEXT,
    PRIMARY KEY (client_id, address)
);
