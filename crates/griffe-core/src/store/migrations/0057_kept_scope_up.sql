-- Ce que la relève a le droit de lire. pending : rien. listed : les adresses. all : tout le compte.
-- Une liste déjà écrite devient listed. Une liste vide reste pending : le premier lancement n'ouvre pas le compte.
ALTER TABLE mail_account ADD COLUMN kept_scope TEXT NOT NULL DEFAULT 'pending';

UPDATE mail_account
SET kept_scope = 'listed'
WHERE EXISTS (SELECT 1 FROM mail_kept_address);
