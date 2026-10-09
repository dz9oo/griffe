-- Un seul correspondant par fiche. Le tri du nom ne le désigne plus.
ALTER TABLE contacts ADD COLUMN correspondent INTEGER NOT NULL DEFAULT 0
    CHECK (correspondent IN (0, 1));

-- Ce que la fiche montrait déjà : le premier nom, puis l'ordre d'insertion.
UPDATE contacts
SET correspondent = 1
WHERE rowid IN (
    SELECT chosen.rowid
    FROM contacts AS chosen
    WHERE chosen.rowid = (
        SELECT inner_row.rowid
        FROM contacts AS inner_row
        WHERE inner_row.client_id = chosen.client_id
        ORDER BY inner_row.name COLLATE NOCASE, inner_row.rowid
        LIMIT 1
    )
);

CREATE UNIQUE INDEX idx_contacts_one_correspondent
    ON contacts (client_id)
    WHERE correspondent = 1;

-- Hôte des copies pour un serveur autre qu'iCloud. Vide : pas de copie.
-- iCloud connaît le sien sans ces colonnes. imap_username est l'identifiant
-- qui a ouvert la session, pas un secret.
ALTER TABLE mail_account ADD COLUMN imap_host TEXT;
ALTER TABLE mail_account ADD COLUMN imap_port INTEGER;
ALTER TABLE mail_account ADD COLUMN imap_username TEXT;

-- La copie dans Envoyés. sent_copy : null, saved, failed, skipped.
-- copy_at fige la date de la lettre. client_id relie la relance à la fiche.
ALTER TABLE outbound_mail ADD COLUMN sent_copy TEXT;
ALTER TABLE outbound_mail ADD COLUMN sent_copy_error TEXT;
ALTER TABLE outbound_mail ADD COLUMN copy_at TEXT;
ALTER TABLE outbound_mail ADD COLUMN client_id TEXT;

UPDATE outbound_mail
SET client_id = (
    SELECT client_id FROM opportunities
    WHERE opportunities.id = outbound_mail.follow_subject_id
)
WHERE follow_subject = 'opportunity'
  AND follow_subject_id IS NOT NULL
  AND client_id IS NULL;

UPDATE outbound_mail
SET client_id = (
    SELECT client_id FROM invoices
    WHERE invoices.id = outbound_mail.follow_subject_id
)
WHERE follow_subject = 'invoice'
  AND follow_subject_id IS NOT NULL
  AND client_id IS NULL;
