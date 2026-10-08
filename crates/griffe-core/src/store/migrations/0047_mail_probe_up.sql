-- Dernier essai de liaison. NULL : pas encore essayé. Jamais le mot de passe.
ALTER TABLE mail_account ADD COLUMN probe_ok INTEGER CHECK (probe_ok IN (0, 1));
ALTER TABLE mail_account ADD COLUMN probe_detail TEXT;
ALTER TABLE mail_account ADD COLUMN probe_at TEXT;
