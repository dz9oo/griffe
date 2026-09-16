-- Lot 16 (« gestion des données : prospection & missions »). `0008` avait posé `revision` sur
-- `opportunities` et `missions` par anticipation, mais pas ce dont ce lot a besoin en plus :
--
-- `archived_at` sur les opportunités : un axe DISTINCT de `stage = won|lost`. Une opportunité
-- perdue reste dans l'historique de l'entonnoir de vente ; une opportunité archivée en sort des
-- listes actives, sans mentir sur son issue (ni gagnée, ni perdue — juste devenue sans objet).
-- Et une opportunité citée par un devis ne peut JAMAIS être supprimée (trigger d'immuabilité de
-- `0005` sur `quotes`), donc l'archivage est sa seule sortie possible d'une liste de travail.
ALTER TABLE opportunities ADD COLUMN archived_at TEXT;

-- `revision` sur les interactions : `UpdateInteraction`/`DeleteInteraction` ont exactement le
-- même besoin de garde-fou d'écriture concurrente que `UpdateContact`/`DeleteContact` — sans
-- elle, l'interaction serait la seule entité mutable adressable par un id propre sans le
-- garde-fou décrit dans le commentaire de `0008`.
ALTER TABLE interactions ADD COLUMN revision INTEGER NOT NULL DEFAULT 1;

-- Volontairement PAS de `revision` sur `milestones` : un jalon n'a pas d'identité externe
-- stable (id `INTEGER PRIMARY KEY` de substitution, référencé par rien ailleurs dans le schéma)
-- et n'est jamais édité seul — `UpdateMission` (lot 16) remplace toute la collection d'un coup,
-- dans la révision de la mission elle-même. Une révision par jalon serait un garde-fou que
-- personne ne pourrait jamais consulter, sur des lignes recréées à chaque enregistrement. Ne pas
-- « corriger » cette absence dans un lot futur sans relire ce commentaire.
