-- Périodicité de la relève. L'heure et l'échec vivent dans le miroir :
-- un passage toutes les minutes ne doit pas allonger le journal d'audit.
ALTER TABLE mail_account ADD COLUMN releve_every_secs INTEGER NOT NULL DEFAULT 180;
