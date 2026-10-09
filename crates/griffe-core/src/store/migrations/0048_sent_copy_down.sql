DROP INDEX IF EXISTS idx_contacts_one_correspondent;
ALTER TABLE contacts DROP COLUMN correspondent;

ALTER TABLE mail_account DROP COLUMN imap_username;
ALTER TABLE mail_account DROP COLUMN imap_port;
ALTER TABLE mail_account DROP COLUMN imap_host;

ALTER TABLE outbound_mail DROP COLUMN client_id;
ALTER TABLE outbound_mail DROP COLUMN copy_at;
ALTER TABLE outbound_mail DROP COLUMN sent_copy_error;
ALTER TABLE outbound_mail DROP COLUMN sent_copy;
